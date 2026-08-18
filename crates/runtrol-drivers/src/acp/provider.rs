//! The stateless provider half of the generic ACP driver.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program, capture};
use runtrol_provider::{
    Agent, MAX_MODEL_CHOICES, ModelAliases, ModelCatalog, ModelChoice, NativeSessionCatalogue,
    NativeSessionQuery, OpenIntent, Provider, ProviderCapabilities, ProviderCapability,
    ProviderCapabilitySource, ProviderError, ProviderId, SessionCatalogue,
};

use crate::acp::agent::AcpAgent;

/// A provider declared as `kind = "acp"`.
#[derive(Debug)]
pub struct AcpProvider {
    id: ProviderId,
    program: Program,
    contained_by: Arc<Containment>,
    models: ModelAliases,
    sessions: SessionCatalogue,
    transport_argv: Vec<Box<str>>,
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
        sessions: SessionCatalogue,
        transport_argv: Vec<Box<str>>,
    ) -> Self {
        Self {
            id,
            program,
            contained_by,
            models,
            sessions,
            transport_argv,
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
        }
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
            SessionCatalogue::default(),
            vec!["serve".into(), "--stdio".into()],
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
            SessionCatalogue::default(),
            Vec::new(),
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
