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
use crate::claude::roster::ClaudeRoster;
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
    /// The CLI's own record of its running processes, asked whenever the panel wants to know which
    /// conversations have a model answering.
    roster: ClaudeRoster,
    /// Bound flags confirmed by this installed CLI's own parser.
    available_flags: BTreeSet<Box<str>>,
    /// Optional bound flags not confirmed by the parser and what their absence means.
    unavailable_flags: BTreeMap<Box<str>, &'static str>,
    /// The CLI's own status command, from the manifest's `[account]`; none means the surface is unpublished.
    account_status: Option<Vec<Box<str>>>,
    /// The arguments that open this CLI's headless control channel, from the manifest's `[account] usage`.
    ///
    /// Empty means this installation publishes no way to ask for limits outside a turn, and the report then
    /// carries no windows rather than a guess.
    account_usage: Vec<Box<str>>,
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
        account: Option<runtrol_provider::AccountSpec>,
        available_flags: BTreeSet<Box<str>>,
        unavailable_flags: BTreeMap<Box<str>, &'static str>,
    ) -> Self {
        Self {
            id,
            program,
            contained_by,
            models: ClaudeModels::from_environment(models),
            store: ClaudeStore::from_environment(),
            roster: ClaudeRoster::from_environment(),
            available_flags,
            unavailable_flags,
            account_usage: account
                .as_ref()
                .map(|account| account.usage.clone())
                .unwrap_or_default(),
            account_status: account.map(|account| account.status),
        }
    }

    /// The program this driver will run.
    #[must_use]
    pub const fn program(&self) -> &Program {
        &self.program
    }

    /// Ask this CLI where the account stands, over its own headless control channel.
    ///
    /// One request line in, the pipe closed straight after, one answer line out. Closing the pipe is what
    /// makes this finish: the CLI ends a session with no message rather than waiting for a prompt that
    /// never comes, so the same call that asks also cleans up. Measured 2026-08-26 on 2.1.246, the answer
    /// lands about six seconds in and the process closes about six after that.
    ///
    /// `Ok(None)` when the manifest declares no such channel, which is nothing to explain to anybody.
    /// `Err` carries a sentence a strip can show, because "asked and could not read it" is a different
    /// thing to tell somebody than "this service publishes no limits".
    async fn read_usage(&self) -> Result<Option<super::limits::UsageAnswer>, String> {
        if self.account_usage.is_empty() {
            return Ok(None);
        }
        let args: Vec<String> = self.account_usage.iter().map(ToString::to_string).collect();
        let mut request = serde_json::to_vec(&serde_json::json!({
            "type": "control_request",
            "request_id": USAGE_REQUEST_ID,
            "request": { "subtype": "get_usage" },
        }))
        .map_err(|error| format!("the usage request could not be written: {error}"))?;
        request.push(b'\n');
        let output = runtrol_childproc::capture_with_input(
            &self.program,
            &args,
            &request,
            ACCOUNT_USAGE_DEADLINE,
            &self.contained_by,
        )
        .await
        .map_err(|error| format!("the usage channel did not answer: {error}"))?;
        if output.truncated {
            // The answer was cut mid-line, so the tail of it is not JSON any more. Said as truncation rather
            // than as "no answer", because the two want different things looked at.
            return Err("the usage answer was longer than this build reads".to_owned());
        }
        usage_answer(&String::from_utf8_lossy(&output.stdout)).map(Some)
    }
}

/// The identifier runtrol puts on its one usage request, so the answer to it is the one that is read.
///
/// Fixed rather than generated: one request travels this channel, the process is this driver's own and
/// lives for one exchange, and a counter would only make the same line harder to recognise in a log.
const USAGE_REQUEST_ID: &str = "runtrol-usage";

/// The answer to the one control request, out of everything the channel said.
///
/// The channel also carries the session's own frames, so the answer is found by its request identifier
/// rather than by position. A refusal is read as a refusal: this CLI answers `subtype: "error"` with its
/// own sentence when the request is not supported, and reporting that as "no limits" would put a wrong
/// cause on the row.
fn usage_answer(stdout: &str) -> Result<super::limits::UsageAnswer, String> {
    use serde::Deserialize;

    /// One line of the control channel, in the shape that matters here.
    #[derive(Deserialize)]
    struct ControlLine {
        #[serde(rename = "type")]
        kind: Option<String>,
        #[serde(default)]
        response: Option<ControlBody>,
    }

    /// The body of a control response.
    #[derive(Deserialize)]
    struct ControlBody {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        response: Option<super::limits::UsageAnswer>,
    }

    for line in stdout.lines() {
        let Ok(read) = serde_json::from_str::<ControlLine>(line) else {
            // Not a control line at all. The channel carries the session's own frames too, and one it
            // cannot read is not this exchange's answer; the loop keeps looking for the one that is.
            continue;
        };
        if read.kind.as_deref() != Some("control_response") {
            continue;
        }
        let Some(body) = read.response else { continue };
        if body.request_id.as_deref() != Some(USAGE_REQUEST_ID) {
            continue;
        }
        return match (body.subtype.as_deref(), body.response) {
            (Some("success"), Some(answer)) => Ok(answer),
            (Some("success"), None) => Err("the usage answer arrived without a body".to_owned()),
            _ => Err(body.error.unwrap_or_else(|| {
                "the CLI refused the usage request without saying why".to_owned()
            })),
        };
    }
    Err("the usage channel closed without answering".to_owned())
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
            // (the catalogue above); deleting is that same contract carried to its end. The driver permanently
            // removes the complete measured artifact set and verifies absence under the delete scope the Runtime
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

    async fn active_native_sessions(
        &self,
    ) -> Result<Vec<runtrol_provider::NativeSessionId>, ProviderError> {
        let roster = self.roster.clone();
        let provider = self.id;
        // Small file reads and one existence check per record: blocking work, kept off the reactor so a slow
        // disk cannot stall every other provider's answer.
        tokio::task::spawn_blocking(move || roster.running(provider))
            .await
            .map_err(|join| ProviderError::Protocol {
                provider,
                doing: "reading which of this CLI's conversations have a model answering",
                detail: join.to_string(),
            })?
    }

    async fn native_process_activity(
        &self,
    ) -> Result<runtrol_provider::NativeProcessActivity, ProviderError> {
        let roster = self.roster.clone();
        let provider = self.id;
        // One small roster scan names both process existence and the busy subset. Keeping this off the reactor
        // makes the 250 ms observation clock independent of filesystem latency.
        tokio::task::spawn_blocking(move || roster.activity(provider))
            .await
            .map_err(|join| ProviderError::Protocol {
                provider,
                doing: "reading which conversations this CLI's live processes own",
                detail: join.to_string(),
            })?
    }

    async fn delete_native_session(
        &self,
        deletion: NativeSessionDeletion,
    ) -> Result<(), ProviderError> {
        const DOING: &str = "deleting a stored conversation";
        let store = self.store.clone();
        let roster = self.roster.clone();
        let provider = self.id;
        let native = deletion.native.as_str().to_owned();
        // Bounded history rewrite and filesystem removal: blocking work, kept off the reactor so a slow disk
        // cannot stall every other provider's answer.
        //
        // Three different answers, kept apart because a person acts on them differently. A live process
        // still owning the conversation is the CLI refusing, and the sentence says what to do about it. A
        // disk that will not cooperate is an operating system failure. Only a worker that never answered is
        // a shape Runtrol cannot read. Folding all three into the last one sent "answered in a shape Runtrol
        // cannot read" to a person whose conversation was merely still open (measured 2026-08-29).
        tokio::task::spawn_blocking(move || {
            if roster.owns_live(provider, &native)? {
                return Err(ProviderError::NativeRefused {
                    provider,
                    doing: DOING,
                    detail: "the CLI still has this conversation open; stop its live process before deleting it"
                        .to_owned(),
                });
            }
            store.delete(&native).map_err(|source| ProviderError::Io {
                provider,
                doing: DOING,
                source,
            })
        })
        .await
        .map_err(|join| ProviderError::Protocol {
            provider,
            doing: DOING,
            detail: join.to_string(),
        })?
    }

    /// Two questions this CLI answers about the account, neither of them a turn.
    ///
    /// `claude auth status --json` says who is signed in (measured 2.1.242: `loggedIn`, `authMethod`,
    /// `subscriptionType`). A `get_usage` control request on its headless stream channel says where the
    /// account stands against every window it has (measured 2.1.246). The second one is only asked of an
    /// account that is signed in, because a signed-out CLI has no limits to state and asking anyway would
    /// spend six seconds learning that.
    ///
    /// The limits question failing does not fail this answer. Sign-in state is a separate fact and still
    /// true, so the report carries it along with why the windows are missing; a strip that loses both
    /// would say "no usage published" about a service that publishes it perfectly well.
    async fn account(&self) -> Result<runtrol_provider::AccountReport, ProviderError> {
        // The command is the manifest's `[account] status`, so the surface this driver reads is declared
        // beside every other reachable fact about the CLI rather than spelled here a second time.
        let args: Vec<String> = match &self.account_status {
            Some(status) if !status.is_empty() => status.iter().map(ToString::to_string).collect(),
            _ => {
                return Ok(runtrol_provider::AccountReport::unpublished(
                    "this provider's manifest declares no account status command",
                ));
            }
        };
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
        let mut report =
            account_report(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
                ProviderError::Protocol {
                    provider: self.id,
                    doing: "asking the CLI who is signed in",
                    detail: "the status answer carried no readable loggedIn field".to_owned(),
                }
            })?;
        if matches!(report.status, runtrol_provider::AccountStatus::SignedIn) {
            match self.read_usage().await {
                Ok(Some(answer)) => {
                    if answer.rate_limits_available {
                        report.limits = Some(runtrol_provider::AccountLimits::new(
                            answer.windows(),
                            answer.reached(),
                        ));
                    } else {
                        // The CLI's own word for "plan limits do not apply here": an API key, Bedrock,
                        // Vertex, or a sign-in without the scope that reads them. Saying so beats an empty
                        // bar, and it is not a failure to retry.
                        report.limits_absent = Some(runtrol_provider::LimitsAbsent::Unmetered {
                            why: "this sign-in has no plan limits".into(),
                        });
                    }
                    if report.plan.is_none() {
                        report.plan = answer.plan();
                    }
                }
                // The manifest declares no channel for this installation, so nothing was asked and there is
                // nothing to explain: the absent windows are the absent declaration.
                Ok(None) => {}
                Err(why) => {
                    report.limits_absent =
                        Some(runtrol_provider::LimitsAbsent::Unread { why: why.into() });
                }
            }
        }
        Ok(report)
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
        )
        .await?;
        Ok(Box::new(agent))
    }
}

/// How long the status command may take. It reads a local file and prints; a signed-out CLI answers as fast.
const ACCOUNT_STATUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the limits question may take.
///
/// Longer than the sign-in one because it is a different kind of work: the CLI opens its own machine
/// channel, asks its vendor for the account's position and then closes down, which measured about twelve
/// seconds end to end on 2.1.246. The bound is well past that so a slow network reports a limit instead of
/// a timeout, and bounded at all so a child that hangs cannot hold up the round that asks every service.
const ACCOUNT_USAGE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

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
        limits_absent: None,
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
            None,
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
            None,
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
            None,
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
            None,
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
