//! View-owned filesystem guards, shared only across an identical authority binding.

use std::sync::{Arc, Weak};
use std::time::Instant;

use runtrol_store::IntegrationRootRow;

use super::root_proof::{
    ROOT_CHECK_DEADLINE, RootCheck, RootCheckFailure, SharedRootProof, run_root_check_until,
};
use super::{
    AbsPath, AuthorizedIntegration, HostedTerminal, TerminalRuntimeFailure, root_authority_failure,
};

#[derive(Clone, PartialEq, Eq)]
struct RootBinding {
    integration: [u8; 16],
    key_generation: u64,
    grant_generation: u64,
    approved: IntegrationRootRow,
    worker: Option<crate::isolated_workspace::WorktreeBinding>,
    // Unix revalidates all grant roots instead of a pinned Windows handle. Keep that exact boundary.
    #[cfg(not(windows))]
    roots: Vec<IntegrationRootRow>,
}

impl RootBinding {
    fn for_view(
        authority: &AuthorizedIntegration,
        hosted: &HostedTerminal,
    ) -> Result<Self, TerminalRuntimeFailure> {
        let approved = authority
            .roots
            .iter()
            .find(|root| {
                AbsPath::new(&root.path).is_ok_and(|path| hosted.project_root().is_under(&path))
            })
            .cloned()
            .ok_or_else(root_authority_failure)?;
        Ok(Self {
            integration: authority.key.to_bytes(),
            key_generation: authority.grant.key_generation,
            grant_generation: authority.grant.grant_generation,
            approved,
            worker: hosted.worktree().cloned(),
            #[cfg(not(windows))]
            roots: authority.roots.clone(),
        })
    }
}

/// Weak entries are pruned on every admission. They retain no guard, proof worker, or departed view.
#[derive(Default)]
pub(super) struct RootProofPool {
    entries: tokio::sync::Mutex<Vec<(RootBinding, Weak<PinLane>)>>,
}

type PinLane = tokio::sync::Mutex<Option<Arc<PinnedRoot>>>;

pub(super) struct RootLease {
    root: Arc<PinnedRoot>,
    _lane: Arc<PinLane>,
}

impl std::ops::Deref for RootLease {
    type Target = PinnedRoot;

    fn deref(&self) -> &Self::Target {
        &self.root
    }
}

pub(super) struct PinnedRoot {
    binding: RootBinding,
    pub(super) proof: Arc<SharedRootProof>,
    #[cfg(all(test, windows))]
    pub(super) guard: Arc<tokio::sync::Mutex<TerminalRootGuards>>,
}

impl PinnedRoot {
    pub(super) fn matches_grant(&self, authority: &AuthorizedIntegration) -> bool {
        self.binding.integration == authority.key.to_bytes()
            && self.binding.key_generation == authority.grant.key_generation
            && self.binding.grant_generation == authority.grant.grant_generation
    }
}

impl RootProofPool {
    pub(super) async fn pin(
        &self,
        authority: &AuthorizedIntegration,
        hosted: &HostedTerminal,
        permits: Arc<tokio::sync::Semaphore>,
    ) -> Result<RootLease, TerminalRuntimeFailure> {
        let binding = RootBinding::for_view(authority, hosted)?;
        let deadline = Instant::now() + ROOT_CHECK_DEADLINE;
        let lane = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.lane(&binding),
        )
        .await
        .map_err(|_| root_authority_failure())?;
        let held = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Arc::clone(&lane).lock_owned(),
        )
        .await
        .map_err(|_| root_authority_failure())?;
        if Instant::now() > deadline {
            return Err(root_authority_failure());
        }
        if let Some(root) = held.as_ref() {
            match root.proof.fresh() {
                Ok(completed_at) if completed_at <= deadline => {
                    return Ok(RootLease {
                        root: Arc::clone(root),
                        _lane: lane,
                    });
                }
                Ok(_) => return Err(root_authority_failure()),
                Err(RootCheckFailure::Stale) => {
                    root.proof
                        .ensure_fresh_until(deadline)
                        .await
                        .map_err(|_| root_authority_failure())?;
                    return Ok(RootLease {
                        root: Arc::clone(root),
                        _lane: lane,
                    });
                }
                // ok: Failed cached authority is not reused. The bounded fresh pin below must succeed or return its error.
                Err(_) => {}
            }
        }
        let authority = authority.clone();
        let worker_permits = Arc::clone(&permits);
        let checked = run_root_check_until(permits, deadline, move || {
            let mut held = held;
            let root = Arc::new(pin_root(binding, &authority, worker_permits)?);
            // A late pin cannot publish reusable authority after its caller has already timed out.
            if Instant::now() > deadline {
                return Err(root_authority_failure());
            }
            *held = Some(Arc::clone(&root));
            Ok(root)
        })
        .await
        .and_then(RootCheck::fresh)
        .map_err(|_| root_authority_failure())?;
        Ok(RootLease {
            root: checked.value?,
            _lane: lane,
        })
    }

    async fn lane(&self, binding: &RootBinding) -> Arc<PinLane> {
        let mut entries = self.entries.lock().await;
        entries.retain(|(_, lane)| lane.strong_count() != 0);
        if let Some(lane) = entries
            .iter()
            .find_map(|(key, lane)| (key == binding).then(|| lane.upgrade()).flatten())
        {
            return lane;
        }
        let lane = Arc::new(tokio::sync::Mutex::new(None));
        entries.push((binding.clone(), Arc::downgrade(&lane)));
        lane
    }
}

#[cfg(windows)]
pub(super) struct TerminalRootGuards {
    approved: runtrol_security::ProjectRootGuard,
    worker: Option<[runtrol_security::ProjectRootGuard; 2]>,
}

#[cfg(windows)]
impl TerminalRootGuards {
    pub(super) fn valid(&mut self) -> bool {
        self.approved.validate().is_ok()
            && self
                .worker
                .as_mut()
                .is_none_or(|guards| guards.iter_mut().all(|guard| guard.validate().is_ok()))
    }
}

#[cfg(windows)]
fn pin_root(
    binding: RootBinding,
    _authority: &AuthorizedIntegration,
    permits: Arc<tokio::sync::Semaphore>,
) -> Result<PinnedRoot, TerminalRuntimeFailure> {
    use runtrol_security::{ProjectRootGuard, ProjectRootIdentity};

    let path = AbsPath::new(&binding.approved.path).map_err(|_| root_authority_failure())?;
    let approved = ProjectRootGuard::acquire(
        &path,
        ProjectRootIdentity::from_bytes(binding.approved.identity),
    )
    .map_err(|_| root_authority_failure())?;
    let worker = binding
        .worker
        .as_ref()
        .map(|worker| {
            let project = ProjectRootGuard::acquire(
                worker.project.root(),
                ProjectRootIdentity::from_bytes(worker.project.root_identity()),
            )
            .map_err(|_| root_authority_failure())?;
            let workspace = ProjectRootGuard::acquire(
                &worker.workspace,
                ProjectRootIdentity::from_bytes(worker.workspace_identity),
            )
            .map_err(|_| root_authority_failure())?;
            Ok::<_, TerminalRuntimeFailure>([project, workspace])
        })
        .transpose()?;
    let completed_at = Instant::now();
    let guard = Arc::new(tokio::sync::Mutex::new(TerminalRootGuards {
        approved,
        worker,
    }));
    let checked_guard = Arc::clone(&guard);
    let proof = SharedRootProof::new(permits, completed_at, move || {
        checked_guard.blocking_lock().valid()
    });
    Ok(PinnedRoot {
        binding,
        proof,
        #[cfg(test)]
        guard,
    })
}

#[cfg(not(windows))]
fn pin_root(
    binding: RootBinding,
    authority: &AuthorizedIntegration,
    permits: Arc<tokio::sync::Semaphore>,
) -> Result<PinnedRoot, TerminalRuntimeFailure> {
    super::current_roots(authority)?;
    if let Some(worker) = &binding.worker {
        worker.verify().map_err(|_| root_authority_failure())?;
    }
    let completed_at = Instant::now();
    let authority = authority.clone();
    let worker = binding.worker.clone();
    let proof = SharedRootProof::new(permits, completed_at, move || {
        super::current_roots(&authority).is_ok()
            && worker.as_ref().is_none_or(|worker| worker.verify().is_ok())
    });
    Ok(PinnedRoot { binding, proof })
}

#[cfg(test)]
mod tests {
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    use std::time::Duration;

    use super::*;
    use crate::runtime_terminal::view_control_tests::Fixture;

    #[tokio::test]
    async fn a_queued_pool_lookup_cannot_restart_the_absolute_check_deadline() {
        let fixture = Fixture::new().await;
        let pool = Arc::clone(&fixture.composed.runtime_terminals.roots);
        let proof = fixture.view.root_proof();
        let calls = proof.test_calls();
        let held = pool.entries.lock().await;
        let mut pinning = Box::pin(pool.pin(
            &fixture.view.authority,
            &fixture.view.hosted,
            proof.permits(),
        ));
        poll_fn(|context| {
            assert!(pinning.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        // Both the lock and timeout will be ready at the next poll. An old fresh cache entry must
        // not let that scheduling delay start a new 400ms budget or approve the expired admission.
        std::thread::sleep(ROOT_CHECK_DEADLINE + Duration::from_millis(50));
        drop(held);
        let refusal = pinning.await.err().map(|failure| failure.kind);
        assert_eq!(
            proof.test_calls(),
            calls,
            "the queued lookup starts no OS check"
        );
        fixture.close().await;
        assert_eq!(
            refusal,
            Some(runtrol_runtime_protocol::RuntimeErrorKind::RootDenied)
        );
    }
}
