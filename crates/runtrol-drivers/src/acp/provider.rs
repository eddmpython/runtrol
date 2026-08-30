//! The stateless provider half of the generic ACP driver.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program, capture};
use runtrol_provider::{
    Agent, MAX_MODEL_CHOICES, ModelAliases, ModelCatalog, ModelChoice, NativeSessionCatalogue,
    NativeSessionDeletion, NativeSessionQuery, OpenIntent, Provider, ProviderCapabilities,
    ProviderCapability, ProviderCapabilitySource, ProviderError, ProviderId, StoreSpec,
};

use crate::acp::agent::AcpAgent;

/// A provider declared as `kind = "acp"`.
#[derive(Debug)]
pub struct AcpProvider {
    id: ProviderId,
    program: Program,
    contained_by: Arc<Containment>,
    models: ModelAliases,
    sessions: StoreSpec,
    transport_argv: Vec<Box<str>>,
    /// What this agent's manifest declares about reading its account, when it declares anything.
    ///
    /// The standard protocol has no account surface, so every agent that publishes one does it through its
    /// own extension. This driver holds the declaration and never a vendor's name.
    account: Option<runtrol_provider::AccountSpec>,
}

/// How long the CLI's own model listing may take before the answer is treated as absent.
///
/// Measured on the one CLI that has this command: it answers in well under a second because it reads a
/// catalogue it already holds. The budget is generous against that and short enough that a hung child cannot
/// make opening a conversation feel broken.
const MODEL_LIST_DEADLINE: Duration = Duration::from_secs(10);

impl AcpProvider {
    /// Ask the CLI's own command for its current models.
    ///
    /// One identifier per line, which is the shape the CLI that has this command actually prints. Nothing is
    /// parsed out of a line beyond trimming it: an identifier is the provider's value to send back, and
    /// splitting it into parts would be Runtrol deciding what a model name means.
    ///
    /// A failure is reported as unknown rather than unsupported. The command exists, so this is a discovery
    /// that did not answer, and blurring that into "this CLI has no models" would tell an operator to stop
    /// looking for something that is there.
    async fn enumerate_models(&self) -> ModelCatalog {
        let arguments: Vec<String> = self.models.list.iter().map(ToString::to_string).collect();
        let Ok(output) = capture(
            &self.program,
            &arguments,
            MODEL_LIST_DEADLINE,
            &self.contained_by,
        )
        .await
        else {
            return ModelCatalog::unknown(
                "this coding service's own model listing command could not be run",
            );
        };
        if output.truncated {
            return ModelCatalog::unknown(
                "this coding service's model listing was longer than the bounded read",
            );
        }
        let mut models: Vec<ModelChoice> = Vec::new();
        for line in output.text().lines() {
            let id = line.trim();
            // Blank lines and anything a terminal wrote for a person rather than for a caller. A decorated
            // line is not an identifier, and sending one back as a model would fail at session start.
            if id.is_empty() || id.contains(char::is_whitespace) || id.contains('\u{1b}') {
                continue;
            }
            if models.iter().any(|choice| &*choice.id == id) {
                continue;
            }
            models.push(ModelChoice {
                id: id.into(),
                display_name: id.into(),
                description: Box::default(),
                // The command prints a list, not a default. Marking one would be an invention.
                is_default: false,
                reasoning_efforts: Vec::new(),
            });
            if models.len() >= MAX_MODEL_CHOICES {
                break;
            }
        }
        if models.is_empty() {
            return ModelCatalog::unknown(
                "this coding service's model listing command printed nothing Runtrol could use",
            );
        }
        ModelCatalog::Known { models }
    }

    /// Build a provider without starting its executable.
    #[must_use]
    pub const fn new(
        id: ProviderId,
        program: Program,
        contained_by: Arc<Containment>,
        models: ModelAliases,
        sessions: StoreSpec,
        transport_argv: Vec<Box<str>>,
        account: Option<runtrol_provider::AccountSpec>,
    ) -> Self {
        Self {
            id,
            program,
            contained_by,
            models,
            sessions,
            transport_argv,
            account,
        }
    }

    /// Arguments supplied by the provider manifest.
    pub fn transport_argv(&self) -> impl Iterator<Item = &str> {
        self.transport_argv.iter().map(AsRef::as_ref)
    }
}

#[async_trait]
impl Provider for AcpProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        let protocol = || ProviderCapability::available(ProviderCapabilitySource::OfficialProtocol);
        ProviderCapabilities {
            fresh_session: protocol(),
            resume: ProviderCapability::unknown(
                "the ACP agent announces resume support during initialization",
            ),
            structured_events: protocol(),
            interrupt: protocol(),
            approvals: protocol(),
            cooling: ProviderCapability::available(ProviderCapabilitySource::DriverContract),
            native_session_catalogue: ProviderCapability::unknown(
                "the ACP agent announces session catalogue support during initialization",
            ),
            // The vendor extension `session/set_model` exists only where the session announced a
            // model set; an agent without it answers method-not-found, loudly.
            set_model: ProviderCapability::unknown(
                "the ACP agent announces switchable models per session",
            ),
            // Measured on grok 1.0.x: session/set_reasoning_effort is method-not-found, set_model
            // ignores an extra effort field, and no session announces a config option for it.
            set_reasoning_effort: ProviderCapability::unsupported(
                "no ACP surface announces a mid-session effort switch",
            ),
            // The protocol has no delete (measured on grok 1.0.5: `session/delete` is method-not-found
            // and the handshake announces list, resume and close only), so the act exists exactly where
            // the CLI publishes its own command for it (cline `history delete`), declared in the manifest.
            native_session_delete: if self.sessions.delete.is_empty() {
                ProviderCapability::unsupported(
                    "this coding service publishes no command or protocol method for deleting a stored conversation",
                )
            } else {
                ProviderCapability::available(ProviderCapabilitySource::OfficialCli)
            },
            native_session_archive: ProviderCapability::unsupported(
                "the Agent Client Protocol publishes no conversation archive method",
            ),
        }
    }

    /// Which conversations a live process of this agent has open, from the store the agent keeps itself.
    ///
    /// Answers with nothing unless this agent's manifest declares where that evidence is (`store.live`). An
    /// agent that declares none is not guessed about: its conversations are listed as stored, which is what
    /// they are as far as anything here can prove.
    async fn native_process_activity(
        &self,
    ) -> Result<runtrol_provider::NativeProcessActivity, ProviderError> {
        let Some(spec) = self.sessions.live.clone() else {
            return Ok(crate::acp::live::nothing());
        };
        let Ok(home) = crate::operator::operator_home(&mut |name| std::env::var_os(name)) else {
            return Ok(crate::acp::live::nothing());
        };
        // Walking a store and asking the filesystem who holds a file are both blocking work, kept off the
        // reactor so a slow disk cannot stall every other provider's answer.
        let answered = tokio::task::spawn_blocking(move || {
            crate::acp::live::activity(&home, &spec, runtrol_childproc::holder_of)
        })
        .await;
        match answered {
            Ok(activity) => Ok(activity),
            Err(join) => Err(ProviderError::Protocol {
                provider: self.id,
                doing: "reading which conversations this agent has open",
                detail: join.to_string(),
            }),
        }
    }

    async fn delete_native_session(
        &self,
        deletion: NativeSessionDeletion,
    ) -> Result<(), ProviderError> {
        if self.sessions.delete.is_empty() {
            return Err(ProviderError::Unsupported {
                provider: self.id,
                what: "deleting a provider-native conversation".to_owned(),
                why: "this coding service publishes no command or protocol method for deleting a stored conversation",
            });
        }
        crate::acp::history::delete(
            self.id,
            &self.program,
            &self.sessions.delete,
            &deletion,
            &self.contained_by,
        )
        .await
    }

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        // The protocol cannot answer this, but the CLI behind it may have a command of its own that can. Asking
        // that command is still asking the provider: the identifiers come from the installed CLI at the moment
        // of asking, so nothing here goes stale the way a catalogue written into a manifest would.
        if !self.models.list.is_empty() {
            return Ok(self.enumerate_models().await);
        }
        if self.models.aliases.is_empty() {
            // Unsupported, not unknown. Stable ACP v1 has no method that enumerates models at all, so there is
            // nothing here that failed or might answer later. Reporting this as unknown made an absent surface
            // indistinguishable from a discovery that broke, which is the one thing a surface must never blur.
            return Ok(ModelCatalog::unsupported(
                "the Agent Client Protocol has no model enumeration method, this provider declares no listing \
                 command of its own, and it declares no aliases",
            ));
        }
        Ok(ModelCatalog::Aliases {
            aliases: self.models.aliases.clone(),
            reasoning_efforts: Vec::new(),
            why: "these aliases come from the provider manifest".into(),
        })
    }

    /// Both paths this driver takes answer without a folder.
    ///
    /// Measured 2026-08-20: the ACP v1 `ListSessionsRequest` has no required field and documents
    /// `cwd` as a filter (grok 1.0.5 answered 30 sessions across 30 folders, opencode 1.2.27
    /// answered 36 across 36, and supplying an unrelated folder returned none); a manifest-declared
    /// CLI listing has no folder argument at all (`cline history --help`). Either way every row
    /// carries its own `cwd`.
    fn enumerates_machine(&self) -> bool {
        true
    }

    /// Where the operator's account with this agent stands, from what its manifest declares.
    ///
    /// The standard protocol publishes no account surface, so an agent that has one publishes it as its own
    /// extension. The manifest names that extension's method and where its answer keeps each fact, and
    /// [`crate::acp::account`] walks it. An agent whose manifest declares none says so rather than having
    /// this driver guess at a method name.
    async fn account(&self) -> Result<runtrol_provider::AccountReport, ProviderError> {
        let Some(spec) = self.account.as_ref() else {
            return Ok(runtrol_provider::AccountReport::unpublished(
                "this provider's manifest declares no account surface",
            ));
        };
        let Some(identity) = spec.identity.as_ref() else {
            return Ok(runtrol_provider::AccountReport::unpublished(
                "this provider's manifest declares no protocol method that answers about the account",
            ));
        };
        crate::acp::account::read(
            self.id,
            &self.program,
            &self.transport_argv,
            identity,
            &spec.windows,
            spec.unmetered.as_ref(),
            &self.contained_by,
        )
        .await
    }

    async fn native_sessions(
        &self,
        query: NativeSessionQuery,
    ) -> Result<NativeSessionCatalogue, ProviderError> {
        // A CLI that lists its own conversations is asked directly. The protocol answering "method not found" says
        // nothing about the command line, and reading the protocol's silence as the CLI's is how this driver came
        // to report an absent surface for a provider that had one all along.
        if !self.sessions.list.is_empty() {
            return crate::acp::history::list(
                self.id,
                &self.program,
                &self.sessions.list,
                self.sessions.limit_flag.as_deref(),
                &query,
                &self.contained_by,
            )
            .await;
        }
        crate::acp::catalogue::list(
            self.id,
            &self.program,
            &self.transport_argv,
            query,
            &self.contained_by,
        )
        .await
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        let agent = AcpAgent::start(
            self.id,
            &self.program,
            &self.transport_argv,
            &intent,
            &self.contained_by,
        )
        .await?;
        Ok(Box::new(agent))
    }
}

#[cfg(test)]
mod tests {
    use runtrol_childproc::resolve;

    use super::*;

    fn program() -> Program {
        let executable = std::env::current_exe().expect("the test executable exists");
        let executable = executable
            .to_str()
            .expect("the test executable path is UTF-8");
        resolve(executable).expect("the running program resolves")
    }

    #[test]
    fn construction_starts_nothing_and_keeps_manifest_arguments() {
        let provider = AcpProvider::new(
            ProviderId::parse("example-acp").expect("valid provider id"),
            program(),
            Arc::new(Containment::without_any()),
            ModelAliases::default(),
            StoreSpec::default(),
            vec!["serve".into(), "--stdio".into()],
            None,
        );
        assert_eq!(provider.id().as_str(), "example-acp");
        assert_eq!(
            provider.transport_argv().collect::<Vec<_>>(),
            vec!["serve", "--stdio"]
        );
    }

    #[tokio::test]
    async fn manifest_aliases_need_no_provider_process() {
        let provider = AcpProvider::new(
            ProviderId::parse("example-acp").expect("valid provider id"),
            program(),
            Arc::new(Containment::without_any()),
            ModelAliases {
                list: Vec::new(),
                aliases: vec!["fast".into(), "deep".into()],
            },
            StoreSpec::default(),
            Vec::new(),
            None,
        );
        let ModelCatalog::Aliases { aliases, .. } =
            provider.models().await.expect("manifest aliases are local")
        else {
            panic!("expected manifest aliases");
        };
        assert_eq!(
            aliases,
            vec![Box::<str>::from("fast"), Box::<str>::from("deep")]
        );
    }
}
