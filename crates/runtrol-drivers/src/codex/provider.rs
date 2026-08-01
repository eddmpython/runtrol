//! The provider half: stateless, built once, opens sessions onto one shared daemon.
//!
//! # Why this one holds something and the other does not
//!
//! The CLI that runs a process per session needs no shared state at all. This one is a daemon, so the first
//! session to open starts it and every session after joins it. What is held is a **weak** handle: the sessions
//! own the connection, and when the last of them goes away the process stops on its own. A strong handle here
//! would keep an `app-server` running with nobody attached, which is exactly the stray process the containment
//! design exists to prevent.
//!
//! Starting is serialized by the same lock that reads the handle, so two sessions opening at once produce one
//! daemon rather than two.

use std::collections::BTreeSet;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    Agent, MAX_MODEL_CHOICES, MAX_REASONING_CHOICES, ModelCatalog, ModelChoice, OpenIntent,
    Provider, ProviderError, ProviderId, ReasoningChoice,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::codex::agent::CodexAgent;
use crate::codex::conn::Connection;

/// What runtrol calls itself in the handshake.
///
/// The provider records it against the conversation. A name is the honest thing to send: a client that hid
/// behind the CLI's own name would make an operator's own thread list unable to say what started a session.
pub const CLIENT_NAME: &str = "runtrol";

/// Pagination and field bounds for an untrusted model catalogue.
const MAX_MODEL_PAGES: usize = 32;
const PAGE_SIZE: u32 = 100;
const MAX_MODEL_ID_BYTES: usize = 256;
const MAX_DISPLAY_NAME_BYTES: usize = 512;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
const MAX_REASONING_ID_BYTES: usize = 64;
const MAX_CURSOR_BYTES: usize = 1024;

/// Parameters for the provider's model discovery request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelListParams<'cursor> {
    cursor: Option<&'cursor str>,
    include_hidden: bool,
    limit: u32,
}

/// One page exactly as the provider reports it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelPage {
    data: Vec<ListedModel>,
    next_cursor: Option<Box<str>>,
}

/// The fields runtrol needs to offer a model choice and send the chosen value back unchanged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedModel {
    model: Box<str>,
    display_name: Box<str>,
    description: Box<str>,
    hidden: bool,
    is_default: bool,
    supported_reasoning_efforts: Vec<ListedReasoning>,
}

/// One reasoning option in the provider's model response.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedReasoning {
    reasoning_effort: Box<str>,
    description: Box<str>,
}

/// The driver for the CLI whose sessions share one daemon.
pub struct CodexProvider {
    /// Which provider this is, as its manifest declares it.
    id: ProviderId,
    /// The program to run, already resolved with its launchers unwrapped.
    program: Program,
    /// The containment every child joins.
    contained_by: Arc<Containment>,
    /// The daemon, for as long as any session is on it.
    ///
    /// Weak on purpose. See the module notes.
    shared: Mutex<Weak<Connection>>,
}

impl core::fmt::Debug for CodexProvider {
    /// Prints what identifies the driver. The connection is deliberately absent: whether one exists is a fact
    /// about the sessions that are open, not about the driver.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CodexProvider")
            .field("id", &self.id.as_str())
            .field("program", &self.program.path().as_str())
            .finish_non_exhaustive()
    }
}

impl CodexProvider {
    /// Build the driver.
    ///
    /// Starts nothing. A provider is a way to open sessions, not a session, and the daemon does not exist
    /// until somebody opens one.
    #[must_use]
    pub fn new(id: ProviderId, program: Program, contained_by: Arc<Containment>) -> Self {
        Self {
            id,
            program,
            contained_by,
            shared: Mutex::new(Weak::new()),
        }
    }

    /// The program this driver will run.
    #[must_use]
    pub const fn program(&self) -> &Program {
        &self.program
    }

    /// The daemon, starting it if no session is on one.
    ///
    /// # Errors
    ///
    /// Whatever [`Connection::start`] returns.
    async fn connection(&self) -> Result<Arc<Connection>, ProviderError> {
        // Held across the start on purpose. It is what makes two sessions opening at once produce one daemon,
        // and it is only ever contended while a session is being opened.
        let mut shared = self.shared.lock().await;
        if let Some(live) = shared.upgrade() {
            return Ok(live);
        }

        let started = Arc::new(
            Connection::start(
                self.id,
                &self.program,
                &self.contained_by,
                CLIENT_NAME,
                env!("CARGO_PKG_VERSION"),
            )
            .await?,
        );
        *shared = Arc::downgrade(&started);
        Ok(started)
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        let conn = self.connection().await?;
        let mut cursor: Option<Box<str>> = None;
        let mut seen_cursors = BTreeSet::new();
        let mut seen_models = BTreeSet::new();
        let mut models = Vec::new();

        for _ in 0..MAX_MODEL_PAGES {
            let answer = conn
                .call(
                    "model/list",
                    &ModelListParams {
                        cursor: cursor.as_deref(),
                        include_hidden: false,
                        limit: PAGE_SIZE,
                    },
                    "listing models",
                )
                .await?;
            let page: ModelPage =
                serde_json::from_slice(&answer).map_err(|error| ProviderError::Protocol {
                    provider: self.id,
                    doing: "listing models",
                    detail: error.to_string(),
                })?;

            for listed in page.data {
                if listed.hidden || !seen_models.insert(listed.model.clone()) {
                    continue;
                }
                if models.len() == MAX_MODEL_CHOICES {
                    return Err(catalogue_too_large(self.id, "model choices"));
                }
                models.push(read_choice(self.id, listed)?);
            }

            let Some(next) = page.next_cursor else {
                return Ok(ModelCatalog::Known { models });
            };
            bounded(self.id, "pagination cursor", &next, MAX_CURSOR_BYTES)?;
            if !seen_cursors.insert(next.clone()) {
                return Err(ProviderError::Protocol {
                    provider: self.id,
                    doing: "listing models",
                    detail: "the provider repeated a pagination cursor".to_owned(),
                });
            }
            cursor = Some(next);
        }

        Err(catalogue_too_large(self.id, "pages"))
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        let conn = self.connection().await?;
        let agent = CodexAgent::start(conn, self.id, &intent).await?;
        Ok(Box::new(agent))
    }
}

/// Convert one provider-owned entry after enforcing the discovery memory contract.
fn read_choice(provider: ProviderId, listed: ListedModel) -> Result<ModelChoice, ProviderError> {
    bounded(
        provider,
        "model identifier",
        &listed.model,
        MAX_MODEL_ID_BYTES,
    )?;
    bounded(
        provider,
        "model display name",
        &listed.display_name,
        MAX_DISPLAY_NAME_BYTES,
    )?;
    bounded(
        provider,
        "model description",
        &listed.description,
        MAX_DESCRIPTION_BYTES,
    )?;
    if listed.supported_reasoning_efforts.len() > MAX_REASONING_CHOICES {
        return Err(catalogue_too_large(provider, "reasoning choices"));
    }

    let reasoning_efforts = listed
        .supported_reasoning_efforts
        .into_iter()
        .map(|effort| {
            bounded(
                provider,
                "reasoning identifier",
                &effort.reasoning_effort,
                MAX_REASONING_ID_BYTES,
            )?;
            bounded(
                provider,
                "reasoning description",
                &effort.description,
                MAX_DESCRIPTION_BYTES,
            )?;
            Ok(ReasoningChoice {
                id: effort.reasoning_effort,
                description: effort.description,
            })
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;

    Ok(ModelChoice {
        id: listed.model,
        display_name: listed.display_name,
        description: listed.description,
        is_default: listed.is_default,
        reasoning_efforts,
    })
}

/// Refuse a field that would evade the catalogue's item bounds through one oversized string.
fn bounded(
    provider: ProviderId,
    what: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), ProviderError> {
    if value.len() <= limit {
        return Ok(());
    }
    Err(ProviderError::Protocol {
        provider,
        doing: "listing models",
        detail: format!("the provider returned an oversized {what}"),
    })
}

/// Refuse an item count that exceeds the discovery contract.
fn catalogue_too_large(provider: ProviderId, what: &'static str) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing: "listing models",
        detail: format!("the provider returned too many {what}"),
    }
}

#[cfg(test)]
mod tests {
    use runtrol_childproc::resolve;

    use super::*;

    fn a_provider_id() -> ProviderId {
        ProviderId::parse("codex").expect("the test's own id must be valid")
    }

    /// The test binary, which is by definition installed: it is running this test.
    fn a_resolved_program() -> Program {
        let exe = std::env::current_exe().expect("a test binary has a path");
        let exe = exe.to_str().expect("the test binary's path is UTF-8");
        resolve(exe).expect("the test binary resolves")
    }

    fn a_driver() -> CodexProvider {
        CodexProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
        )
    }

    fn listed_model() -> ListedModel {
        ListedModel {
            model: "runtime-choice".into(),
            display_name: "Runtime Choice".into(),
            description: "reported by the provider".into(),
            hidden: false,
            is_default: true,
            supported_reasoning_efforts: vec![ListedReasoning {
                reasoning_effort: "balanced".into(),
                description: "the provider's balanced setting".into(),
            }],
        }
    }

    #[test]
    fn building_a_driver_starts_nothing() {
        // A build assembles every provider at boot. If constructing one started a daemon, a fresh start would
        // put a process launch in front of the operator's first list, for a provider they may never use.
        let driver = a_driver();
        assert_eq!(driver.id().as_str(), "codex");
        assert!(driver.program().path().as_std_path().exists());
    }

    #[tokio::test]
    async fn no_daemon_exists_until_a_session_asks_for_one() {
        let driver = a_driver();
        assert!(
            driver.shared.lock().await.upgrade().is_none(),
            "a driver that had already started one would be holding a process nobody asked for"
        );
    }

    #[test]
    fn a_discovered_choice_keeps_the_value_the_provider_accepts() {
        let choice = read_choice(a_provider_id(), listed_model()).expect("bounded");
        assert_eq!(&*choice.id, "runtime-choice");
        assert_eq!(&*choice.display_name, "Runtime Choice");
        assert!(choice.is_default);
        assert!(
            choice
                .reasoning_efforts
                .first()
                .is_some_and(|effort| &*effort.id == "balanced")
        );
    }

    #[test]
    fn an_oversized_discovery_field_is_refused() {
        let mut listed = listed_model();
        listed.model = "x".repeat(MAX_MODEL_ID_BYTES + 1).into();
        let error = read_choice(a_provider_id(), listed).expect_err("over the contract");
        assert!(error.to_string().contains("oversized model identifier"));
    }

    #[test]
    fn the_current_page_shape_is_read_without_owning_unneeded_fields() {
        let page: ModelPage = serde_json::from_str(
            r#"{"data":[{"id":"opaque-row-id","model":"runtime-choice","displayName":"Runtime Choice","description":"reported now","hidden":false,"isDefault":true,"defaultReasoningEffort":"balanced","supportedReasoningEfforts":[{"reasoningEffort":"balanced","description":"provider setting"}]}],"nextCursor":null}"#,
        )
        .expect("the provider's generated schema shape");
        assert_eq!(page.data.len(), 1);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn a_driver_can_be_held_without_naming_its_type() {
        // What the kind table hands back. The kernel holds one of these without a line that mentions which
        // CLI it is, which is what makes adding a provider not touch the kernel.
        let held: Box<dyn Provider> = Box::new(a_driver());
        assert_eq!(held.id().as_str(), "codex");
    }

    #[test]
    fn the_driver_does_not_print_a_connection_it_may_not_have() {
        // The realistic leak is a debug format written during an investigation. Whether a daemon exists is a
        // fact about open sessions, and printing a handle here would invite reading one out of the driver.
        let printed = format!("{:?}", a_driver());
        assert!(printed.contains("codex"), "{printed}");
        assert!(!printed.contains("shared"), "{printed}");
    }

    #[tokio::test]
    async fn opening_a_session_against_a_program_that_is_not_the_cli_fails_by_name() {
        // The test binary is a real program that is not this CLI. What is checked is that a failure to open
        // arrives as a named error rather than as a panic or a hang, because that error becomes session state.
        use runtrol_provider::{AbsPath, Disposition, SessionId};

        let driver = a_driver();
        let intent = OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::from_os(&std::env::temp_dir()).expect("the temporary directory"),
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        };

        match driver.open(intent).await {
            Err(error) => {
                let said = error.to_string();
                assert!(
                    said.contains("codex"),
                    "an error has to name its provider: {said}"
                );
            }
            Ok(_) => panic!("a program that is not the CLI must not open a session"),
        }
    }
}
