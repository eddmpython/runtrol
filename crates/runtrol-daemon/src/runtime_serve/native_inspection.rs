//! Filesystem authorization within the native catalogue's existing admission bound.

use tokio::sync::OwnedSemaphorePermit;

use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_inventory::{AuthorizedRoot, RuntimeInventoryFailure, authorized_roots};

pub(super) fn roots(
    authority: &AuthorizedIntegration,
    requested: Option<&str>,
) -> Result<(Option<AuthorizedRoot>, Vec<AuthorizedRoot>), RuntimeInventoryFailure> {
    let approved = authorized_roots(authority)?;
    let selected = requested
        .map(|requested| {
            approved
                .iter()
                .find(|root| root.path.as_str() == requested)
                .cloned()
                .ok_or(RuntimeInventoryFailure::RootAuthorityChanged)
        })
        .transpose()?;
    Ok((selected, approved))
}

pub(super) async fn inspect<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    inspect: impl FnOnce() -> Result<T, RuntimeInventoryFailure> + Send + 'static,
) -> Result<(OwnedSemaphorePermit, T), RuntimeInventoryFailure> {
    // A cancelled request cannot interrupt filesystem work. Keep its existing listing slot in the worker,
    // then return it for the next phase, so neither cancellation nor phase changes admit extra scans.
    tokio::task::spawn_blocking(move || inspect().map(|result| (permit, result)))
        .await
        .map_err(|_| RuntimeInventoryFailure::Unavailable)?
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Semaphore;

    use super::*;

    #[tokio::test]
    async fn a_cancelled_catalogue_retains_admission_until_its_filesystem_work_ends() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("listing admitted");
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let inspection = tokio::spawn(inspect(permit, move || {
            started.send(()).expect("test awaits filesystem inspection");
            blocked
                .recv()
                .expect("test releases the stalled filesystem");
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .expect("filesystem work leaves the request executor responsive")
            .expect("inspection started");
        inspection.abort();
        assert!(
            inspection
                .await
                .expect_err("request cancelled")
                .is_cancelled()
        );
        assert!(Arc::clone(&slots).try_acquire_owned().is_err());
        release.send(()).expect("filesystem worker remains alive");
        let replacement = tokio::time::timeout(Duration::from_secs(5), slots.acquire_owned())
            .await
            .expect("completion releases admission")
            .expect("the listing semaphore remains open");
        let (retained, answer) = inspect(replacement, || Ok(42_u8))
            .await
            .expect("next phase");
        assert_eq!(answer, 42);
        drop(retained);
    }
}
