//! The stateless provider half of the generic ACP driver.

use std::sync::Arc;

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    Agent, ModelAliases, ModelCatalog, NativeSessionCatalogue, NativeSessionQuery, OpenIntent,
    Provider, ProviderCapabilities, ProviderCapability, ProviderCapabilitySource, ProviderError,
    ProviderId,
};

use crate::acp::agent::AcpAgent;

/// A provider declared as `kind = "acp"`.
#[derive(Debug)]
pub struct AcpProvider {
    id: ProviderId,
    program: Program,
    contained_by: Arc<Containment>,
    models: ModelAliases,
    transport_argv: Vec<Box<str>>,
}

impl AcpProvider {
    /// Build a provider without starting its executable.
    #[must_use]
    pub const fn new(
        id: ProviderId,
        program: Program,
        contained_by: Arc<Containment>,
        models: ModelAliases,
        transport_argv: Vec<Box<str>>,
    ) -> Self {
        Self {
            id,
            program,
            contained_by,
            models,
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
        if self.models.aliases.is_empty() {
            // Unsupported, not unknown. Stable ACP v1 has no method that enumerates models at all, so there is
            // nothing here that failed or might answer later. Reporting this as unknown made an absent surface
            // indistinguishable from a discovery that broke, which is the one thing a surface must never blur.
            return Ok(ModelCatalog::unsupported(
                "the Agent Client Protocol has no model enumeration method, and this provider declares no aliases",
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
                aliases: vec!["fast".into(), "deep".into()],
            },
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
