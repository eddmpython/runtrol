//! The provider half: stateless, built once, opens sessions.
//!
//! Holds a resolved program and the process containment, and nothing else. Constructing one starts nothing and asks
//! nothing, which is what lets a build assemble every provider at boot without putting a second of process starts
//! in front of the operator's first list.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    Agent, ModelAliases, ModelCatalog, OpenIntent, Provider, ProviderCapabilities,
    ProviderCapability, ProviderCapabilitySource, ProviderError, ProviderId,
};

use crate::claude::agent::ClaudeAgent;
use crate::claude::models::{ClaudeModels, discover_reasoning_efforts};

/// The driver for the CLI that runs one process per session.
#[derive(Debug)]
pub struct ClaudeProvider {
    /// Which provider this is, as its manifest declares it.
    id: ProviderId,
    /// The program to run, already resolved with its launchers unwrapped.
    ///
    /// Resolved once rather than per session. Measured: unwrapping the launcher a package manager installs saves a
    /// process and roughly 80 ms, and the operator pays that on every session start otherwise.
    program: Program,
    /// The containment every child joins.
    ///
    /// Shared, because there is one per process and holding it is what holds the guarantee.
    contained_by: Arc<Containment>,
    /// Stable aliases plus exact options read from provider-owned state on each discovery request.
    models: ClaudeModels,
    /// Bound flags confirmed by this installed CLI's own parser.
    available_flags: BTreeSet<Box<str>>,
    /// Optional bound flags not confirmed by the parser and what their absence means.
    unavailable_flags: BTreeMap<Box<str>, &'static str>,
}

impl ClaudeProvider {
    /// Build the driver.
    ///
    /// Starts nothing. A provider is a way to open sessions, not a session.
    #[must_use]
    pub fn new(
        id: ProviderId,
        program: Program,
        contained_by: Arc<Containment>,
        models: ModelAliases,
        available_flags: BTreeSet<Box<str>>,
        unavailable_flags: BTreeMap<Box<str>, &'static str>,
    ) -> Self {
        Self {
            id,
            program,
            contained_by,
            models: ClaudeModels::from_environment(models),
            available_flags,
            unavailable_flags,
        }
    }

    /// The program this driver will run.
    #[must_use]
    pub const fn program(&self) -> &Program {
        &self.program
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        let cli = || ProviderCapability::available(ProviderCapabilitySource::OfficialCli);
        let resume = if self.available_flags.contains("--resume") {
            cli()
        } else {
            ProviderCapability::unsupported(
                self.unavailable_flags
                    .get("--resume")
                    .copied()
                    .unwrap_or("the installed CLI did not confirm its resume flag"),
            )
        };
        ProviderCapabilities {
            fresh_session: cli(),
            resume,
            structured_events: cli(),
            interrupt: ProviderCapability::available(ProviderCapabilitySource::DriverContract),
            approvals: cli(),
            cooling: ProviderCapability::available(ProviderCapabilitySource::DriverContract),
            native_session_catalogue: ProviderCapability::unsupported(
                "this driver has no registered official native session catalogue",
            ),
        }
    }

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        let found = self.models.discover();
        let reasoning_efforts = if self.available_flags.contains("--effort") {
            discover_reasoning_efforts(&self.program, &self.contained_by).await
        } else {
            Vec::new()
        };
        Ok(ModelCatalog::Partial {
            aliases: found.aliases,
            models: found.models,
            reasoning_efforts,
            why: found.why,
        })
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        // Returns as soon as the process exists and its streams are bound. Whether the provider has anything to say
        // yet is answered by events, not by this returning: the startup frame arrives on the stream like everything
        // else, and waiting for it here would make opening a session depend on a frame that a broken CLI never
        // sends.
        let agent = ClaudeAgent::start(
            self.id,
            &self.program,
            &intent,
            &self.contained_by,
            &self.available_flags,
            &self.unavailable_flags,
        )?;
        Ok(Box::new(agent))
    }
}

#[cfg(test)]
mod tests {
    use runtrol_childproc::resolve;
    use runtrol_provider::{AbsPath, Disposition, SessionId};

    use super::*;

    fn a_provider_id() -> ProviderId {
        ProviderId::parse("claude").expect("the test's own id must be valid")
    }

    /// The build tool, which is by definition installed: it is running this test.
    fn a_resolved_program() -> Program {
        let exe = std::env::current_exe().expect("a test binary has a path");
        let exe = exe.to_str().expect("the test binary's path is UTF-8");
        resolve(exe).expect("the test binary resolves")
    }

    fn all_flags() -> BTreeSet<Box<str>> {
        crate::claude::FLAGS
            .iter()
            .map(|flag| Box::<str>::from(flag.flag))
            .collect()
    }

    #[test]
    fn building_a_driver_starts_nothing() {
        // A build assembles every provider at boot. If constructing one started a process, a fresh start would put
        // a second of nothing in front of the operator's first list.
        let driver = ClaudeProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
            ModelAliases::default(),
            all_flags(),
            BTreeMap::new(),
        );
        assert_eq!(driver.id().as_str(), "claude");
        assert!(driver.program().path().as_std_path().exists());
    }

    #[test]
    fn a_driver_can_be_held_without_naming_its_type() {
        // What the kind table hands back. The kernel holds one of these without a line that mentions which CLI it
        // is, which is what makes adding a provider not touch the kernel.
        let held: Box<dyn Provider> = Box::new(ClaudeProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
            ModelAliases::default(),
            all_flags(),
            BTreeMap::new(),
        ));
        assert_eq!(held.id().as_str(), "claude");
    }

    #[tokio::test]
    async fn model_aliases_are_reported_without_starting_the_cli() {
        let driver = ClaudeProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
            ModelAliases {
                aliases: vec!["fast".into(), "deep".into()],
            },
            all_flags(),
            BTreeMap::new(),
        );
        match driver.models().await.expect("aliases need no process") {
            ModelCatalog::Partial {
                aliases,
                models: _,
                why,
                ..
            } => {
                assert_eq!(
                    aliases,
                    vec![Box::<str>::from("fast"), Box::<str>::from("deep")]
                );
                assert!(why.contains("does not enumerate"), "{why}");
            }
            other => panic!("expected a partial catalogue, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn opening_a_session_against_a_program_that_is_not_the_cli_fails_by_name() {
        // The test binary is a real program that is not this CLI. What is being checked is that a failure to open
        // arrives as a named error rather than as a panic or a hang, because that error becomes session state.
        let driver = ClaudeProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
            ModelAliases::default(),
            all_flags(),
            BTreeMap::new(),
        );
        let intent = OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::from_os(&std::env::temp_dir()).expect("the temporary directory"),
            disposition: Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        };

        // Starting succeeds (it is a real executable) and the session then says something runtrol cannot read, or it
        // ends. Either is a value; neither is a panic.
        let mut agent = driver.open(intent).await.expect("a real program starts");
        match agent.next().await {
            None | Some(Err(_)) => {}
            Some(Ok(produced)) => panic!("a program that is not the CLI produced {produced:?}"),
        }
        agent
            .close(runtrol_provider::CloseMode::Kill)
            .await
            .expect("stopping it works");
    }
}
