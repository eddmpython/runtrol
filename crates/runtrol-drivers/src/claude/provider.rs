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
    Agent, Disposition, ModelAliases, ModelCatalog, NativeSessionCatalogue, NativeSessionDeletion,
    NativeSessionQuery, OpenIntent, Provider, ProviderCapabilities, ProviderCapability,
    ProviderCapabilitySource, ProviderError, ProviderId,
};

use crate::claude::agent::ClaudeAgent;
use crate::claude::models::{ClaudeModels, discover_reasoning_efforts};
use crate::claude::store::ClaudeStore;

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
    /// The conversations this CLI has stored, named from its own store on each listing request.
    store: ClaudeStore,
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
            store: ClaudeStore::from_environment(),
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
            // This CLI publishes no command or protocol method for the conversations it has stored (measured
            // 2.1.238: `claude agents` is a roster of running processes, `claude project` only purges, the
            // resume picker is a terminal). The driver names them from the CLI's own store instead, the one
            // place the CLI resumes them from, reading only identity, folder and the CLI's own title. That is a
            // driver contract, not a CLI surface, and the capability says which.
            native_session_catalogue: ProviderCapability::available(
                ProviderCapabilitySource::DriverContract,
            ),
            // The control channel's set_model succeeds mid-session (measured 2.1.x).
            set_model: cli(),
            // Measured 2.1.235: the control channel refuses set_effort and set_reasoning_effort.
            // The effort is an open-time flag on this CLI, so a new choice applies from the next
            // session, and this observation is what lets a surface say so before the attempt.
            set_reasoning_effort: ProviderCapability::unsupported(
                "the installed CLI refuses a mid-session effort switch; the effort is an open-time \
                 flag, so a new choice applies from the next session",
            ),
            // The CLI publishes no delete command, but the driver already reads this store to name conversations
            // (the catalogue above); deleting is that same contract carried to its end. The conversation is
            // moved out of the store, reversibly (into `runtrol-deleted`), under the delete scope the Runtime
            // grants only from the machine. Said available so the surface offers the act that now exists.
            native_session_delete: ProviderCapability::available(
                ProviderCapabilitySource::DriverContract,
            ),
            native_session_archive: ProviderCapability::unsupported(
                "Claude Code publishes no command or protocol method for archiving its stored conversations",
            ),
        }
    }

    /// The store holds every folder's conversations side by side, so a query without a folder is
    /// answered with all of them, each row naming its own folder.
    fn enumerates_machine(&self) -> bool {
        true
    }

    async fn native_sessions(
        &self,
        query: NativeSessionQuery,
    ) -> Result<NativeSessionCatalogue, ProviderError> {
        // Whether a listed session can be reopened is the resume flag's answer, and the flag was probed against
        // the installed CLI rather than assumed. Runtime only offers to open a row it was told is resumable, so
        // guessing high would put a row on screen that fails on click and guessing low would hide every one.
        let resumable = self.available_flags.contains("--resume");
        let store = self.store.clone();
        let provider = self.id;
        // Directory listing and bounded file reads: blocking work, kept off the reactor so a slow disk cannot
        // stall every other provider's answer.
        tokio::task::spawn_blocking(move || store.list(provider, resumable, &query))
            .await
            .map_err(|join| ProviderError::Protocol {
                provider,
                doing: "reading the conversations this CLI has stored",
                detail: join.to_string(),
            })?
    }

    async fn delete_native_session(
        &self,
        deletion: NativeSessionDeletion,
    ) -> Result<(), ProviderError> {
        let store = self.store.clone();
        let provider = self.id;
        let native = deletion.native.as_str().to_owned();
        // A directory move on disk: blocking work, kept off the reactor so a slow disk cannot stall every other
        // provider's answer.
        tokio::task::spawn_blocking(move || store.delete(&native))
            .await
            .map_err(|join| ProviderError::Protocol {
                provider,
                doing: "deleting a stored conversation",
                detail: join.to_string(),
            })?
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing: "deleting a stored conversation",
                detail: error.to_string(),
            })
    }

    /// `claude auth status --json`: this CLI's own answer to "who is signed in", measured on 2.1.242
    /// (`loggedIn`, `authMethod`, `subscriptionType`). Limits are not asked here: this CLI reports them
    /// only on a turn, and that report fills the same gauge.
    async fn account(&self) -> Result<runtrol_provider::AccountReport, ProviderError> {
        let args = ["auth".to_owned(), "status".to_owned(), "--json".to_owned()];
        let output = runtrol_childproc::capture(
            &self.program,
            &args,
            ACCOUNT_STATUS_DEADLINE,
            &self.contained_by,
        )
        .await
        .map_err(|error| ProviderError::Protocol {
            provider: self.id,
            doing: "asking the CLI who is signed in",
            detail: error.to_string(),
        })?;
        account_report(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
            ProviderError::Protocol {
                provider: self.id,
                doing: "asking the CLI who is signed in",
                detail: "the status answer carried no readable loggedIn field".to_owned(),
            }
        })
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
        // A resume shows the stored conversation first, read from the CLI's own store (its stream-json mode
        // prints no history; its terminal mode draws it). Read off the reactor, before the process starts,
        // so the page receives the tail right behind the attachment.
        let replay = match &intent.disposition {
            Disposition::Resume { native } => {
                let store = self.store.clone();
                let native = native.to_string();
                let provider = self.id;
                Some(
                    tokio::task::spawn_blocking(move || store.recent_records(&native))
                        .await
                        .map_err(|join| ProviderError::Protocol {
                            provider,
                            doing: "reading the stored conversation back",
                            detail: join.to_string(),
                        })?,
                )
            }
            _ => None,
        };
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
            replay,
        )?;
        Ok(Box::new(agent))
    }
}

/// How long the status command may take. It reads a local file and prints; a signed-out CLI answers as fast.
const ACCOUNT_STATUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// `auth status --json`, the fields this build reads.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
}

/// The report, or nothing when the answer is not this CLI's status object.
fn account_report(answer: &str) -> Option<runtrol_provider::AccountReport> {
    use runtrol_provider::{AccountReport, AccountStatus, account_token};
    // Anything that is not this CLI's status object (help text, a stack trace) is "no report", which the
    // caller names as a protocol error with the answer's shape; the parse error itself adds nothing.
    let Ok(status) = serde_json::from_str::<AuthStatus>(answer.trim()) else {
        return None;
    };
    Some(AccountReport {
        status: if status.logged_in {
            AccountStatus::SignedIn
        } else {
            AccountStatus::SignedOut
        },
        plan: account_token(status.subscription_type.as_deref()),
        method: account_token(status.auth_method.as_deref()),
        limits: None,
        tokens_today: None,
    })
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
                list: Vec::new(),
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

    #[test]
    fn the_status_answer_reads_as_a_report_and_anything_else_as_nothing() {
        let signed_in = account_report(
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"x@y","subscriptionType":"max"}"#,
        )
        .expect("a status object");
        assert!(matches!(
            signed_in.status,
            runtrol_provider::AccountStatus::SignedIn
        ));
        assert_eq!(signed_in.plan.as_deref(), Some("max"));
        assert_eq!(signed_in.method.as_deref(), Some("claude.ai"));
        let signed_out = account_report(r#"{"loggedIn":false}"#).expect("a status object");
        assert!(matches!(
            signed_out.status,
            runtrol_provider::AccountStatus::SignedOut
        ));
        assert!(signed_out.plan.is_none());
        assert!(account_report("Usage: claude auth status").is_none());
    }
}
