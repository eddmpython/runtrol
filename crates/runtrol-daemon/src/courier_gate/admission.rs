//! Authenticate process ancestry away from the single-thread Runtime's input lane.

use std::sync::Arc;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_childproc::{Containment, ProcessTree};
use runtrol_courier::PROTOCOL_VERSION;
use runtrol_courier::env::TOKEN_BYTES;
use runtrol_courier::wire::Hello;
use runtrol_provider::ProcessIdentity;
use tokio::sync::OwnedSemaphorePermit;

use super::{Admitted, CourierGate, Denied};

#[cfg(test)]
#[path = "tests/admission.rs"]
mod tests;

impl CourierGate {
    /// Admit a hello only when its token, containment and exact process ancestry agree.
    /// The captured activation remains provisional until command admission rechecks it.
    pub(crate) async fn admit(
        &self,
        containment: &Arc<Containment>,
        peer: Option<ProcessIdentity>,
        hello: &Hello,
        greeting: OwnedSemaphorePermit,
    ) -> Result<Admitted, Denied> {
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(Denied::Version(hello.protocol_version));
        }
        let peer = peer.ok_or(Denied::NoPeer)?;
        let (expected, root, admitted) = {
            let state = self.state.lock().await;
            let registered = state
                .sessions
                .get(&hello.session)
                .ok_or(Denied::UnknownSession)?;
            (
                registered.token.clone(),
                registered.root,
                registered.admission(hello.session),
            )
        };
        let root = root.ok_or(Denied::RootUnbound)?;
        let offered =
            Base64UrlUnpadded::decode_vec(&hello.token).map_err(|_malformed| Denied::Token)?;
        if !same_token(&expected, &offered) {
            return Err(Denied::Token);
        }
        let containment = Arc::clone(containment);
        inspect_peer(greeting, move || {
            match containment.contains(root, peer) {
                Ok(true) => {}
                Ok(false) => return Err(Denied::OutsideContainment),
                Err(error) => return Err(Denied::Unanswered(error.to_string())),
            }
            let tree =
                ProcessTree::capture().map_err(|error| Denied::Unanswered(error.to_string()))?;
            if !tree.contains_identity(root, peer) {
                return Err(Denied::OutsideTree);
            }
            Ok(())
        })
        .await?;
        Ok(admitted)
    }
}

async fn inspect_peer(
    greeting: OwnedSemaphorePermit,
    inspect: impl FnOnce() -> Result<(), Denied> + Send + 'static,
) -> Result<(), Denied> {
    tokio::task::spawn_blocking(move || {
        // Cancellation cannot free another greeting while this kernel inspection still owns work.
        let _greeting = greeting;
        inspect()
    })
    .await
    .map_err(|error| Denied::Unanswered(format!("process inspection failed: {error}")))?
}

/// Equal or not, in time that does not depend on where the two first differ.
fn same_token(expected: &[u8; TOKEN_BYTES], offered: &[u8]) -> bool {
    if offered.len() != TOKEN_BYTES {
        return false;
    }
    let mut difference = 0_u8;
    for (mine, theirs) in expected.iter().zip(offered) {
        difference |= mine ^ theirs;
    }
    difference == 0
}
