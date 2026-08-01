//! The stateless provider half of the generic ACP driver.

use std::sync::Arc;

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    Agent, ModelAliases, ModelCatalog, OpenIntent, Provider, ProviderError, ProviderId,
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

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        if self.models.aliases.is_empty() {
            return Ok(ModelCatalog::unknown(
                "this ACP provider does not declare model aliases",
            ));
        }
        Ok(ModelCatalog::Aliases {
            aliases: self.models.aliases.clone(),
            why: "these aliases come from the provider manifest".into(),
        })
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
