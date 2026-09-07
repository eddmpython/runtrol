//! Real terminal adapter admission, not a second implementation of its priority comparison.

use super::*;

async fn peer(fixture: &Fixture, identity: u8, priority: bool) -> TerminalView {
    let key = IntegrationKey::from_bytes([identity; 16]);
    let mut row = fixture.row.clone();
    if priority {
        row.scopes
            .push(AppScope::SessionInputPriority.to_string().into());
    }
    fixture
        .composed
        .integration_authority
        .publish_committed(key, row.clone())
        .unwrap();
    let authority = AuthorizedIntegration {
        key,
        grant: crate::runtime_auth::grant(crate::runtime_auth::integration_id(key), &row).unwrap(),
        roots: row.roots,
    };
    fixture
        .composed
        .runtime_terminals
        .attach(
            &fixture.composed,
            authority,
            &TerminalAttachParams {
                terminal_id: fixture.view.opened.terminal.terminal_id.clone(),
            },
        )
        .await
        .unwrap()
}

async fn acquire(
    view: &TerminalView,
    composed: &Composed,
) -> Result<TerminalControlLease, TerminalRuntimeFailure> {
    let proof = Arc::clone(view.root_proof());
    tokio::task::spawn_blocking(move || proof.checked_now())
        .await
        .unwrap()
        .unwrap();
    composed
        .runtime_terminals
        .acquire_view(
            composed,
            view,
            &TerminalAcquireControlParams {
                request_id: MutationRequestId::now(),
                terminal_id: view.opened.terminal.terminal_id.clone(),
                expected_terminal_generation: view.hosted.generation,
                only_if_free: false,
            },
        )
        .await
}

#[tokio::test]
async fn current_grant_rank_blocks_only_lower_to_higher_transfer() {
    let fixture = Fixture::new().await;
    let low = peer(&fixture, 6, false).await;
    let high = peer(&fixture, 7, true).await;
    let other_high = peer(&fixture, 8, true).await;
    let first = acquire(&fixture.view, &fixture.composed).await.unwrap();
    let second = acquire(&low, &fixture.composed)
        .await
        .expect("ordinary peers can explicitly transfer");
    assert_ne!(first.lease_id, second.lease_id);
    let elevated = acquire(&high, &fixture.composed)
        .await
        .expect("the approved interactive surface takes control");
    assert_eq!(
        acquire(&low, &fixture.composed).await.unwrap_err().kind,
        RuntimeErrorKind::ControlConflict
    );
    let state = fixture.composed.runtime_terminals.state.lock().await;
    assert_eq!(
        state.leases.get(&high.hosted.id).unwrap().lease_id,
        elevated.lease_id
    );
    drop(state);
    let next = acquire(&other_high, &fixture.composed)
        .await
        .expect("equal interactive priority preserves handoff");
    assert!(next.lease_generation > elevated.lease_generation);
    drop((low, high, other_high));
    fixture.close().await;
}

#[tokio::test]
async fn an_initial_open_cannot_bypass_a_higher_holder() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let lease = acquire(&high, &fixture.composed).await.unwrap();
    let proof = Arc::clone(fixture.view.root_proof());
    tokio::task::spawn_blocking(move || proof.checked_now())
        .await
        .unwrap()
        .unwrap();
    let initial = fixture
        .composed
        .runtime_terminals
        .initial_lease(
            &fixture.composed,
            &fixture.view.authority,
            &fixture.view.pinned_root,
            &fixture.view.hosted,
        )
        .await
        .expect("read-only attachment remains possible");
    assert!(initial.is_none());
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&high.hosted.id)
            .unwrap()
            .lease_id,
        lease.lease_id
    );
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn removing_priority_changes_the_live_holder_without_a_cached_rank() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let lease = acquire(&high, &fixture.composed).await.unwrap();
    assert_eq!(
        acquire(&fixture.view, &fixture.composed)
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::ControlConflict
    );
    let mut changed = fixture
        .composed
        .integration_authority
        .row(high.authority.key)
        .unwrap()
        .as_ref()
        .clone();
    changed
        .scopes
        .retain(|scope| scope.as_ref() != AppScope::SessionInputPriority.as_str());
    changed.grant_generation += 1;
    fixture
        .composed
        .integration_authority
        .publish_committed(high.authority.key, changed)
        .unwrap();
    let after = acquire(&fixture.view, &fixture.composed)
        .await
        .expect("removed priority cannot protect an existing lease");
    assert!(after.lease_generation > lease.lease_generation);
    assert!(
        fixture
            .composed
            .runtime_terminals
            .renew_view(&fixture.composed, &high, &renewal(&lease))
            .await
            .is_err()
    );
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn a_queued_lower_write_does_not_cross_an_interactive_takeover() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let low_lease = acquire(&fixture.view, &fixture.composed).await.unwrap();
    let operation = fixture.view.hosted.terminal.operation().await.unwrap();
    let params = TerminalWriteParams {
        request_id: MutationRequestId::now(),
        terminal_id: low_lease.terminal_id.clone(),
        lease_id: low_lease.lease_id.clone(),
        lease_generation: low_lease.lease_generation,
        bytes_base64: Base64::encode_string(b"owned-priority-fixture"),
    };
    let mut pending = Box::pin(fixture.composed.runtime_terminals.write_view(
        &fixture.composed,
        &fixture.view,
        &params,
    ));
    poll_fn(|context| {
        assert!(
            pending.as_mut().poll(context).is_pending(),
            "the real terminal operation lane holds the old write"
        );
        Poll::Ready(())
    })
    .await;
    acquire(&high, &fixture.composed).await.unwrap();
    drop(operation);
    assert_eq!(
        pending.await.unwrap_err().kind,
        RuntimeErrorKind::ControlConflict
    );
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn local_owner_and_public_views_share_one_holder_and_resize_cannot_reclaim_it() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let local = fixture
        .composed
        .runtime_terminals
        .local_control(&fixture.composed, &fixture.view.hosted)
        .await
        .unwrap();
    assert_eq!(
        acquire(&fixture.view, &fixture.composed)
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::ControlConflict
    );
    let public = acquire(&high, &fixture.composed).await.unwrap();
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .resize_local(&fixture.composed, &local, 90, 25)
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::ControlConflict
    );
    fixture
        .composed
        .runtime_terminals
        .release_local(&fixture.composed, &local)
        .await;
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&high.hosted.id)
            .unwrap()
            .lease_id,
        public.lease_id
    );
    fixture
        .composed
        .runtime_terminals
        .write_local(&fixture.composed, &local, b"")
        .await
        .unwrap();
    assert_eq!(
        acquire(&fixture.view, &fixture.composed)
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::ControlConflict
    );
    fixture
        .composed
        .runtime_terminals
        .release_local(&fixture.composed, &local)
        .await;
    acquire(&fixture.view, &fixture.composed)
        .await
        .expect("exact local detach releases its own authority");
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn a_revoked_or_no_longer_visible_holder_cannot_keep_its_priority() {
    for removal in ["revoked", "input", "root"] {
        let fixture = Fixture::new().await;
        let high = peer(&fixture, 7, true).await;
        let before = acquire(&high, &fixture.composed).await.unwrap();
        if removal == "revoked" {
            fixture
                .composed
                .integration_authority
                .publish_revocation(
                    high.authority.key,
                    runtrol_store::IntegrationRevocation {
                        key_generation: 1,
                        grant_generation: 2,
                        revoked_at: WallMs::now(),
                        order: 1,
                    },
                )
                .unwrap();
        } else {
            let mut changed = fixture
                .composed
                .integration_authority
                .row(high.authority.key)
                .unwrap()
                .as_ref()
                .clone();
            changed.grant_generation += 1;
            if removal == "input" {
                changed
                    .scopes
                    .retain(|scope| scope.as_ref() != AppScope::SessionInputWrite.as_str());
            } else {
                changed.roots.clear();
            }
            fixture
                .composed
                .integration_authority
                .publish_committed(high.authority.key, changed)
                .unwrap();
        }
        let after = acquire(&fixture.view, &fixture.composed)
            .await
            .expect("a removed grant cannot reserve control");
        assert!(after.lease_generation > before.lease_generation);
        drop(high);
        fixture.close().await;
    }
}

#[tokio::test]
async fn an_unavailable_holder_authority_is_not_mistaken_for_a_free_lease() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let before = acquire(&high, &fixture.composed).await.unwrap();
    fixture
        .composed
        .draining
        .store(true, std::sync::atomic::Ordering::Release);
    let failure = fixture
        .composed
        .runtime_terminals
        .local_control(&fixture.composed, &fixture.view.hosted)
        .await;
    assert_eq!(
        failure
            .err()
            .expect("the successor authority has not arrived")
            .kind,
        RuntimeErrorKind::RuntimeUnavailable
    );
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&high.hosted.id)
            .unwrap()
            .lease_id,
        before.lease_id
    );
    fixture
        .composed
        .draining
        .store(false, std::sync::atomic::Ordering::Release);
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn a_queued_acquire_cannot_use_priority_removed_before_admission() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let before = acquire(&fixture.view, &fixture.composed).await.unwrap();
    let proof = Arc::clone(high.root_proof());
    tokio::task::spawn_blocking(move || proof.checked_now())
        .await
        .unwrap()
        .unwrap();
    let state = fixture.composed.runtime_terminals.state.lock().await;
    let params = TerminalAcquireControlParams {
        request_id: MutationRequestId::now(),
        terminal_id: high.opened.terminal.terminal_id.clone(),
        expected_terminal_generation: high.hosted.generation,
        only_if_free: false,
    };
    let mut pending = Box::pin(fixture.composed.runtime_terminals.acquire_view(
        &fixture.composed,
        &high,
        &params,
    ));
    poll_fn(|context| {
        assert!(pending.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let mut changed = fixture.row.clone();
    changed.grant_generation += 1;
    fixture
        .composed
        .integration_authority
        .publish_committed(high.authority.key, changed)
        .unwrap();
    drop(state);
    assert_eq!(
        pending.await.unwrap_err().kind,
        RuntimeErrorKind::Unauthenticated
    );
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&high.hosted.id)
            .unwrap()
            .lease_id,
        before.lease_id
    );
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn a_draining_generation_uses_the_successors_narrowed_holder_priority() {
    let fixture = Fixture::new().await;
    let high = peer(&fixture, 7, true).await;
    let before = acquire(&high, &fixture.composed).await.unwrap();
    fixture
        .composed
        .generation_authority
        .freeze(&fixture.composed.integration_authority);
    fixture
        .composed
        .draining
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        acquire(&fixture.view, &fixture.composed)
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::ControlConflict
    );
    let mut snapshot = fixture.composed.integration_authority.generation_snapshot();
    let changed = snapshot
        .iter_mut()
        .find(|row| row.integration_key == high.authority.key.to_bytes())
        .unwrap();
    changed
        .scopes
        .retain(|scope| scope.as_ref() != AppScope::SessionInputPriority.as_str());
    changed.grant_generation += 1;
    fixture
        .composed
        .generation_authority
        .apply("owned-priority-successor", &snapshot)
        .unwrap();
    let after = acquire(&fixture.view, &fixture.composed)
        .await
        .expect("the successor's scope removal applies to the old live terminal");
    assert!(after.lease_generation > before.lease_generation);
    fixture
        .composed
        .draining
        .store(false, std::sync::atomic::Ordering::Release);
    drop(high);
    fixture.close().await;
}

#[tokio::test]
async fn local_typing_invalidates_queued_automation_without_reclaiming_a_later_interactive_holder()
{
    for later_interactive in [false, true] {
        let fixture = Fixture::new().await;
        let high = peer(&fixture, 7, true).await;
        let local = fixture
            .composed
            .runtime_terminals
            .local_control(&fixture.composed, &fixture.view.hosted)
            .await
            .unwrap();
        fixture
            .composed
            .runtime_terminals
            .release_local(&fixture.composed, &local)
            .await;
        let lease = acquire(&fixture.view, &fixture.composed).await.unwrap();
        let operation = fixture.view.hosted.terminal.operation().await.unwrap();
        let params = TerminalWriteParams {
            request_id: MutationRequestId::now(),
            terminal_id: lease.terminal_id.clone(),
            lease_id: lease.lease_id,
            lease_generation: lease.lease_generation,
            bytes_base64: Base64::encode_string(b"owned-queued-automation"),
        };
        let mut automation = Box::pin(fixture.composed.runtime_terminals.write_view(
            &fixture.composed,
            &fixture.view,
            &params,
        ));
        poll_fn(|context| {
            assert!(automation.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        let mut typing = Box::pin(fixture.composed.runtime_terminals.write_local(
            &fixture.composed,
            &local,
            b"",
        ));
        poll_fn(|context| {
            assert!(typing.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(
            fixture
                .composed
                .runtime_terminals
                .state
                .lock()
                .await
                .leases
                .get(&high.hosted.id)
                .unwrap()
                .owner,
            LeaseOwner::Local(local.owner)
        );
        if later_interactive {
            acquire(&high, &fixture.composed).await.unwrap();
        }
        drop(operation);
        let typed = tokio::time::timeout(Duration::from_millis(50), typing)
            .await
            .expect("local input advances without repolling retired automation");
        assert_eq!(
            automation.await.unwrap_err().kind,
            RuntimeErrorKind::ControlConflict
        );
        if later_interactive {
            assert_eq!(typed.unwrap_err().kind, RuntimeErrorKind::ControlConflict);
            assert_eq!(
                fixture
                    .composed
                    .runtime_terminals
                    .state
                    .lock()
                    .await
                    .leases
                    .get(&high.hosted.id)
                    .unwrap()
                    .owner,
                LeaseOwner::Integration(high.authority.key)
            );
        } else {
            typed.expect("local input follows the invalidated automation write");
        }
        fixture
            .composed
            .runtime_terminals
            .release_local(&fixture.composed, &local)
            .await;
        drop(high);
        fixture.close().await;
    }
}

#[tokio::test]
async fn a_queued_resize_or_stop_rechecks_the_grant_after_its_terminal_wait() {
    for resize in [false, true] {
        let fixture = Fixture::new().await;
        let lease = acquire(&fixture.view, &fixture.composed).await.unwrap();
        let operation = fixture.view.hosted.terminal.operation().await.unwrap();
        let mut pending = Box::pin(async {
            if resize {
                fixture
                    .composed
                    .runtime_terminals
                    .resize(
                        &fixture.composed,
                        &fixture.view.authority,
                        &TerminalResizeParams {
                            request_id: MutationRequestId::now(),
                            terminal_id: lease.terminal_id.clone(),
                            lease_id: lease.lease_id.clone(),
                            lease_generation: lease.lease_generation,
                            geometry: TerminalGeometry {
                                columns: 90,
                                rows: 25,
                            },
                        },
                    )
                    .await
            } else {
                fixture
                    .composed
                    .runtime_terminals
                    .stop(
                        &fixture.composed,
                        &fixture.view.authority,
                        &TerminalStopParams {
                            request_id: MutationRequestId::now(),
                            terminal_id: lease.terminal_id.clone(),
                            lease_id: lease.lease_id.clone(),
                            lease_generation: lease.lease_generation,
                        },
                    )
                    .await
            }
        });
        poll_fn(|context| {
            assert!(pending.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        fixture
            .composed
            .integration_authority
            .publish_revocation(
                fixture.view.authority.key,
                runtrol_store::IntegrationRevocation {
                    key_generation: 1,
                    grant_generation: 2,
                    revoked_at: WallMs::now(),
                    order: 1,
                },
            )
            .unwrap();
        drop(operation);
        assert_eq!(
            pending.await.unwrap_err().kind,
            RuntimeErrorKind::IntegrationRevoked
        );
        assert_eq!(fixture.view.hosted.terminal.size().cols, 80);
        assert!(fixture.view.hosted.terminal.exit().is_none());
        fixture.close().await;
    }
}
