//! Public inventory adapters over the registry and the single managed-session catalogue.

use runtrol_core::{BinFacts, ProbeCache, SessionManager, locate};
use runtrol_provider::{AbsPath, NativeSessionId, ProviderId as CoreProviderId};
use runtrol_runtime_protocol::{
    InstallationObservation, InstallationState, ManagedSessionList, ProviderDescriptor,
    ProviderHelp, ProviderId, ProviderList, RuntimeSessionId, SessionDescriptor,
};
use runtrol_security::ProjectRootIdentity;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_control::public_waiting;

/// Safe reason a public session snapshot could not be authorized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeInventoryFailure {
    /// The durable source catalogue could not be read.
    Unavailable,
    /// At least one granted root no longer names the approved filesystem object.
    RootAuthorityChanged,
    /// The session is absent or outside this integration's approved roots.
    SessionNotFound,
}

/// One public session plus the canonicalization input required for grant filtering.
pub(crate) struct RuntimeSessionRecord {
    session: runtrol_provider::SessionId,
    provider: CoreProviderId,
    native: Option<Box<str>>,
    descriptor: SessionDescriptor,
    workspace: Box<str>,
}

/// One immutable snapshot published by the session owner.
pub(crate) struct RuntimeSessionCatalogue {
    sessions: Vec<RuntimeSessionRecord>,
    unreadable: usize,
    available: bool,
}

/// Project the supervisor's account gauges into the public usage list.
///
/// Structured fields only. The verbatim payload never reaches this list: it rides the session event stream under
/// session-output authority, and this list answers under provider authority.
pub(crate) fn provider_usage(
    gauges: &[runtrol_core::ProviderGauge],
) -> runtrol_runtime_protocol::ProviderUsageList {
    let window = |window: runtrol_provider::Window| runtrol_runtime_protocol::ProviderUsageWindow {
        used_percent: window.used_percent,
        resets_at_ms: window.resets_at.map(runtrol_provider::WallMs::as_millis),
        window_minutes: window.window_minutes,
    };
    runtrol_runtime_protocol::ProviderUsageList {
        providers: gauges
            .iter()
            .map(|gauge| runtrol_runtime_protocol::ProviderUsageGauge {
                provider_id: ProviderId::new(gauge.provider.as_str()),
                reached: gauge.reached,
                primary: gauge.primary.map(window),
                secondary: gauge.secondary.map(window),
                at_ms: gauge.at.as_millis(),
            })
            .collect(),
    }
}

/// Build the fast provider inventory without starting any provider process.
pub(crate) fn providers(composed: &Composed) -> ProviderList {
    let cache = ProbeCache::open(composed.home.paths().probe_cache());
    let registered: Vec<&runtrol_core::registry::Provider> = composed.registry.all().collect();
    // Resolved concurrently because each entry walks the operator's search path for its own executable and
    // stats what it finds, and that cost is per provider rather than shared. Done in sequence it meant every
    // supported CLI added delay to the moment the window becomes usable, which is the one wait a person feels
    // on every single launch. Threads rather than tasks because this is blocking filesystem work and the
    // function is called from places that are not async.
    let observations: Vec<InstallationObservation> = std::thread::scope(|scope| {
        let resolving: Vec<_> = registered
            .iter()
            .map(|provider| scope.spawn(|| installation(provider, &cache)))
            .collect();
        resolving
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| InstallationObservation {
                    // A panic while resolving one provider must not lose the other three. Reported as
                    // unavailable rather than missing: something went wrong here, and claiming the CLI is
                    // absent would send the operator to install what they may already have.
                    state: InstallationState::Unavailable,
                    version: None,
                    why: Some("resolving this provider's executable did not complete".to_owned()),
                })
            })
            .collect()
    });
    ProviderList {
        providers: registered
            .into_iter()
            .zip(observations)
            .map(|(provider, installation)| ProviderDescriptor {
                provider_id: ProviderId::new(provider.id().as_str()),
                display_name: provider.manifest.display_name.to_string(),
                icon: provider.manifest.icon.as_ref().map(ToString::to_string),
                installation,
                help: help(provider),
            })
            .collect(),
    }
}

/// This service's own help commands, assembled into lines a person can read and run.
///
/// Uses the first declared binary name rather than a resolved path. Three reasons, and the third is the
/// deciding one: it is what an operator types in their own terminal, it needs no quoting on any platform,
/// and the install line is wanted exactly when nothing resolved at all, so depending on resolution would
/// withhold the one command that is still useful when the CLI is absent.
///
/// Returns `None` rather than an empty structure so that a client shows nothing instead of an action that
/// leads nowhere.
fn help(provider: &runtrol_core::registry::Provider) -> Option<ProviderHelp> {
    let declared = &provider.manifest.help;
    let command = provider.manifest.bin.names.first()?;
    let line = |arguments: &[Box<str>]| {
        (!arguments.is_empty()).then(|| {
            let mut text = command.to_string();
            for argument in arguments {
                text.push(' ');
                text.push_str(argument);
            }
            text
        })
    };
    let assembled = ProviderHelp {
        sign_in: line(&declared.sign_in),
        diagnose: line(&declared.diagnose),
        install: declared.install.as_ref().map(ToString::to_string),
    };
    (!assembled.is_empty()).then_some(assembled)
}

fn installation(
    provider: &runtrol_core::registry::Provider,
    cache: &ProbeCache,
) -> InstallationObservation {
    if !provider.is_usable() {
        return InstallationObservation {
            state: InstallationState::Unavailable,
            version: None,
            why: Some("this Runtime build has no driver for the declared provider kind".to_owned()),
        };
    }
    let Ok(program) = locate(&provider.manifest) else {
        return InstallationObservation {
            state: InstallationState::Missing,
            version: None,
            why: Some("no registered executable candidate is installed".to_owned()),
        };
    };
    let Ok(facts) = BinFacts::of_program(&program) else {
        return InstallationObservation {
            state: InstallationState::Unavailable,
            version: None,
            why: Some("the installed executable identity could not be verified".to_owned()),
        };
    };
    match cache.get(provider.id(), &facts) {
        Some(entry) => InstallationObservation {
            state: InstallationState::Usable,
            version: Some(entry.version.clone()),
            why: None,
        },
        None => InstallationObservation {
            state: InstallationState::Unavailable,
            version: None,
            why: Some("the installed executable has not completed a verified probe".to_owned()),
        },
    }
}

/// Read the one session owner into an immutable public projection.
pub(crate) fn sessions(
    composed: &Composed,
    sessions: &SessionManager,
) -> Result<RuntimeSessionCatalogue, runtrol_store::StoreError> {
    let catalogue = crate::session_catalogue::read(composed, sessions)?;
    Ok(RuntimeSessionCatalogue {
        sessions: catalogue
            .sessions
            .into_iter()
            .map(|session| RuntimeSessionRecord {
                session: session.session,
                provider: session.provider,
                native: session.native.clone(),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new(session.session.to_string()),
                    provider_id: ProviderId::new(session.provider.as_str()),
                    native_session_id: session.native.as_deref().map(str::to_owned),
                    workspace: session.workspace.to_string(),
                    hot: session.hot,
                    lifecycle: session.lifecycle.public(session.hot),
                    looks_stuck: session.looks_stuck,
                    waiting_on: session.waiting.map(public_waiting),
                    session_generation: session.generation,
                    label: session.label.map(Into::into),
                },
                workspace: session.workspace,
            })
            .collect(),
        unreadable: catalogue.warnings.len(),
        available: true,
    })
}

impl RuntimeSessionCatalogue {
    /// Publish an explicit unavailable snapshot after a durable catalogue read fails.
    pub(crate) const fn unavailable() -> Self {
        Self {
            sessions: Vec::new(),
            unreadable: 0,
            available: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn one_for_tests(
        provider: CoreProviderId,
        native: &str,
        workspace: &AbsPath,
    ) -> Self {
        let session = runtrol_provider::SessionId::now();
        Self {
            sessions: vec![RuntimeSessionRecord {
                session,
                provider,
                native: Some(native.into()),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new(session.to_string()),
                    provider_id: ProviderId::new(provider.as_str()),
                    native_session_id: Some(native.to_owned()),
                    workspace: workspace.to_string(),
                    hot: false,
                    lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
                    looks_stuck: false,
                    waiting_on: None,
                    session_generation: 0,
                    label: None,
                },
                workspace: workspace.as_str().into(),
            }],
            unreadable: 0,
            available: true,
        }
    }

    /// Filter against canonical current paths before any descriptor leaves Runtime.
    pub(crate) fn authorized(
        &self,
        authority: &AuthorizedIntegration,
    ) -> Result<ManagedSessionList, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let roots = approved_roots(authority)?;
        let sessions = self
            .sessions
            .iter()
            .filter_map(|session| {
                let Ok(workspace) = AbsPath::canonicalize(&session.workspace) else {
                    return None;
                };
                roots
                    .iter()
                    .any(|root| workspace.is_under(&root.path))
                    .then(|| session.descriptor.clone())
            })
            .collect();
        let warnings = if self.unreadable == 0 {
            Vec::new()
        } else {
            vec![format!(
                "{} stored session rows were unreadable and omitted",
                self.unreadable
            )]
        };
        Ok(ManagedSessionList { sessions, warnings })
    }

    /// Resolve one authorized public session identity without revealing sessions outside the grant.
    pub(crate) fn authorized_session(
        &self,
        authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<runtrol_provider::SessionId, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let roots = approved_roots(authority)?;
        let session = self
            .sessions
            .iter()
            .find(|session| session.descriptor.session_id.as_str() == requested.as_str())
            .ok_or(RuntimeInventoryFailure::SessionNotFound)?;
        let workspace = AbsPath::canonicalize(&session.workspace)
            .map_err(|_| RuntimeInventoryFailure::SessionNotFound)?;
        if !roots.iter().any(|root| workspace.is_under(&root.path)) {
            return Err(RuntimeInventoryFailure::SessionNotFound);
        }
        Ok(session.session)
    }

    /// Read one authorized public descriptor without revealing sessions outside the grant.
    pub(crate) fn authorized_descriptor(
        &self,
        authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<SessionDescriptor, RuntimeInventoryFailure> {
        self.authorized_managed_session(authority, requested)
            .map(|session| session.descriptor)
    }

    /// Resolve the provider pointer and exact current workspace needed to heat one managed session.
    pub(crate) fn authorized_managed_session(
        &self,
        authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<AuthorizedManagedSession, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let roots = approved_roots(authority)?;
        let session = self
            .sessions
            .iter()
            .find(|session| session.descriptor.session_id.as_str() == requested.as_str())
            .ok_or(RuntimeInventoryFailure::SessionNotFound)?;
        let workspace = AbsPath::canonicalize(&session.workspace)
            .map_err(|_| RuntimeInventoryFailure::SessionNotFound)?;
        if !roots.iter().any(|root| workspace.is_under(&root.path)) {
            return Err(RuntimeInventoryFailure::SessionNotFound);
        }
        Ok(AuthorizedManagedSession {
            session: session.session,
            provider: session.provider,
            native: session.native.clone(),
            descriptor: session.descriptor.clone(),
            workspace,
        })
    }

    /// Find an authorized managed pointer by the only safe native merge key.
    pub(crate) fn managed_as(
        &self,
        authority: &AuthorizedIntegration,
        provider: CoreProviderId,
        native: &NativeSessionId,
    ) -> Result<Option<RuntimeSessionId>, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let roots = approved_roots(authority)?;
        Ok(self.sessions.iter().find_map(|session| {
            if session.provider != provider || session.native.as_deref() != Some(native.as_str()) {
                return None;
            }
            let Ok(workspace) = AbsPath::canonicalize(&session.workspace) else {
                return None;
            };
            roots
                .iter()
                .any(|root| workspace.is_under(&root.path))
                .then(|| session.descriptor.session_id.clone())
        }))
    }
}

/// One exact currently valid approved root and its filesystem identity.
pub(crate) struct AuthorizedRoot {
    pub(crate) path: AbsPath,
    pub(crate) identity: [u8; 24],
}

/// One current canonical workspace proven to remain below a locally approved root.
pub(crate) struct AuthorizedWorkspace {
    pub(crate) path: AbsPath,
}

/// One managed session resolved without disclosing anything outside the caller's roots.
pub(crate) struct AuthorizedManagedSession {
    pub(crate) session: runtrol_provider::SessionId,
    pub(crate) provider: CoreProviderId,
    pub(crate) native: Option<Box<str>>,
    pub(crate) descriptor: SessionDescriptor,
    pub(crate) workspace: AbsPath,
}

/// Resolve one caller-selected root only when it still names the locally approved object.
pub(crate) fn authorized_root(
    authority: &AuthorizedIntegration,
    requested: &str,
) -> Result<AuthorizedRoot, RuntimeInventoryFailure> {
    approved_roots(authority)?
        .into_iter()
        .find(|root| root.path.as_str() == requested)
        .ok_or(RuntimeInventoryFailure::RootAuthorityChanged)
}

/// Revalidate every approved root before provider-supplied paths are filtered.
pub(crate) fn authorized_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AuthorizedRoot>, RuntimeInventoryFailure> {
    approved_roots(authority)
}

/// Resolve any exact current workspace under the integration's current approved roots.
pub(crate) fn authorized_workspace(
    authority: &AuthorizedIntegration,
    requested: &str,
) -> Result<AuthorizedWorkspace, RuntimeInventoryFailure> {
    let workspace = AbsPath::canonicalize(requested)
        .map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
    let roots = approved_roots(authority)?;
    if !roots.iter().any(|root| workspace.is_under(&root.path)) {
        return Err(RuntimeInventoryFailure::RootAuthorityChanged);
    }
    Ok(AuthorizedWorkspace { path: workspace })
}

fn approved_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AuthorizedRoot>, RuntimeInventoryFailure> {
    let mut roots = Vec::with_capacity(authority.roots.len());
    for root in &authority.roots {
        let approved =
            AbsPath::new(&root.path).map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
        let current = AbsPath::canonicalize(&root.path)
            .map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
        let identity = ProjectRootIdentity::read(&current)
            .map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
        if current != approved || identity.to_bytes() != root.identity {
            return Err(RuntimeInventoryFailure::RootAuthorityChanged);
        }
        roots.push(AuthorizedRoot {
            path: current,
            identity: root.identity,
        });
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use runtrol_runtime_protocol::{AppScope, IntegrationGrant, IntegrationId, LifecycleState};
    use runtrol_store::{IntegrationKey, IntegrationRootRow};

    use super::*;

    #[test]
    fn replaced_root_invalidates_public_session_authority() {
        let base = std::env::temp_dir().join(format!(
            "runtrol-runtime-root-replacement-{}",
            std::process::id()
        ));
        drop(std::fs::remove_dir_all(&base));
        let project_path = base.join("project");
        std::fs::create_dir_all(&project_path).expect("create project root");
        let project = AbsPath::canonicalize(project_path.to_str().expect("UTF-8 path"))
            .expect("canonical project root");
        let identity = ProjectRootIdentity::read(&project).expect("read root identity");
        let authority = AuthorizedIntegration {
            key: IntegrationKey::from_bytes([4; 16]),
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("int_fixture"),
                scopes: vec![AppScope::SessionList],
                roots: vec![project.to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
            roots: vec![IntegrationRootRow {
                path: project.as_str().into(),
                identity: identity.to_bytes(),
            }],
        };
        let catalogue = RuntimeSessionCatalogue {
            sessions: vec![RuntimeSessionRecord {
                session: runtrol_provider::SessionId::now(),
                provider: runtrol_provider::ProviderId::parse("provider-fixture")
                    .expect("valid provider"),
                native: Some("native_fixture".into()),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new("session_fixture"),
                    provider_id: ProviderId::new("provider_fixture"),
                    native_session_id: Some("native_fixture".to_owned()),
                    workspace: project.to_string(),
                    hot: false,
                    lifecycle: LifecycleState::Cold,
                    looks_stuck: false,
                    waiting_on: None,
                    session_generation: 0,
                    label: None,
                },
                workspace: project.as_str().into(),
            }],
            unreadable: 0,
            available: true,
        };
        assert_eq!(
            catalogue
                .authorized(&authority)
                .expect("original root remains authorized")
                .sessions
                .len(),
            1
        );

        let retired = base.join("retired");
        std::fs::rename(&project_path, &retired).expect("retire approved directory");
        std::fs::create_dir(&project_path).expect("replace directory at same path");
        assert!(matches!(
            catalogue.authorized(&authority),
            Err(RuntimeInventoryFailure::RootAuthorityChanged)
        ));
        std::fs::remove_dir_all(&base).expect("clean root replacement fixture");
    }
}
