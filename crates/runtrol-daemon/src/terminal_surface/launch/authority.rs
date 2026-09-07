//! The request's existing authority follows preparation to the final execution boundary.

use std::sync::Arc;

use runtrol_provider::AbsPath;
use runtrol_runtime_protocol::AppScope;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::terminal_surface::TerminalOpenError;

pub(crate) enum LaunchAuthority {
    /// The authenticated owner-local broker explicitly supplied the invocation.
    TrustedLocal,
    /// Worker and retained-resume tickets carry their original project and exact authority together.
    Worktree,
    /// A public ordinary open or official attachment retains the exact accepted grant generation.
    Integration {
        authority: Arc<AuthorizedIntegration>,
        scope: AppScope,
    },
}

impl LaunchAuthority {
    pub(crate) fn integration(authority: &AuthorizedIntegration, scope: AppScope) -> Self {
        Self::Integration {
            authority: Arc::new(authority.clone()),
            scope,
        }
    }

    pub(super) fn validate(
        &self,
        composed: &Composed,
        cwd: &AbsPath,
    ) -> Result<(), TerminalOpenError> {
        let Self::Integration { authority, scope } = self else {
            return Ok(());
        };
        let current = crate::runtime_serve::refresh_current(composed, authority)
            .map_err(|failure| TerminalOpenError::Provider(failure.message.to_owned()))?;
        if current.grant.key_generation != authority.grant.key_generation
            || current.grant.grant_generation != authority.grant.grant_generation
            || !current.grant.scopes.contains(scope)
        {
            return Err(TerminalOpenError::Provider(
                "the terminal open belongs to an earlier or insufficient approval".to_owned(),
            ));
        }
        let roots = crate::runtime_inventory::authorized_roots(&current).map_err(|_| {
            TerminalOpenError::Provider("the approved project root changed identity".to_owned())
        })?;
        if !roots.iter().any(|root| cwd.is_under(&root.path)) {
            return Err(TerminalOpenError::Provider(
                "the terminal workspace is outside its current approved roots".to_owned(),
            ));
        }
        Ok(())
    }
}
