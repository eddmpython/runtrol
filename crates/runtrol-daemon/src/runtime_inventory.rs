//! Public inventory adapters over the registry and the single managed-session catalogue.

use runtrol_core::{BinFacts, ProbeCache, SessionManager, locate};
use runtrol_provider::AbsPath;
use runtrol_runtime_protocol::{
    InstallationObservation, InstallationState, ManagedSessionList, ProviderDescriptor, ProviderId,
    ProviderList, RuntimeSessionId, SessionDescriptor,
};
use runtrol_security::ProjectRootIdentity;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;

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
    descriptor: SessionDescriptor,
    workspace: Box<str>,
}

/// One immutable snapshot published by the session owner.
pub(crate) struct RuntimeSessionCatalogue {
    sessions: Vec<RuntimeSessionRecord>,
    unreadable: usize,
    available: bool,
}

/// Build the fast provider inventory without starting any provider process.
pub(crate) fn providers(composed: &Composed) -> ProviderList {
    let cache = ProbeCache::open(composed.home.paths().probe_cache());
    ProviderList {
        providers: composed
            .registry
            .all()
            .map(|provider| ProviderDescriptor {
                provider_id: ProviderId::new(provider.id().as_str()),
                display_name: provider.manifest.display_name.to_string(),
                installation: installation(provider, &cache),
            })
            .collect(),
    }
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
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new(session.session.to_string()),
                    provider_id: ProviderId::new(session.provider.as_str()),
                    lifecycle: session.lifecycle.public(session.hot),
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
                    .any(|root| workspace.is_under(root))
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
        if !roots.iter().any(|root| workspace.is_under(root)) {
            return Err(RuntimeInventoryFailure::SessionNotFound);
        }
        Ok(session.session)
    }
}

fn approved_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AbsPath>, RuntimeInventoryFailure> {
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
        roots.push(current);
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
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new("session_fixture"),
                    provider_id: ProviderId::new("provider_fixture"),
                    lifecycle: LifecycleState::Cold,
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
