use super::*;
use crate::courier_gate::tests::commands::admission;

async fn lead(gate: &CourierGate) -> TerminalId {
    let terminal = TerminalId::now();
    gate.launch(gate.mint(terminal).expect("mint"), || {
        Ok::<_, ()>(((), Some(here())))
    })
    .await
    .expect("register");
    gate.set_dialogue(terminal, true).await.expect("activate");
    terminal
}

#[tokio::test]
async fn pending_and_live_workers_share_one_per_lead_limit_and_global_capacity() {
    let gate = gate();
    let lead = lead(&gate).await;
    let admitted = admission(&gate, session_id(lead)).await;
    let (first, _) = gate
        .reserve_spawn(admitted, here(), 1, None, 1000)
        .await
        .expect("first");
    let (second, _) = gate
        .reserve_spawn(admitted, here(), 1, None, 1000)
        .await
        .expect("second");
    assert!(
        gate.reserve_spawn(admitted, here(), 1, None, 1000)
            .await
            .is_err()
    );
    gate.launch_worker(
        gate.mint(first.worker.terminal).expect("mint"),
        &first,
        || Ok(()),
        || Ok::<_, String>(((), None)),
        str::to_owned,
    )
    .await
    .expect("launch");
    assert!(
        gate.cancel_spawn(&first).await.is_none(),
        "client cancellation cannot retire a live child"
    );
    assert!(
        gate.reserve_spawn(admitted, here(), 2, None, 1000)
            .await
            .is_err()
    );
    assert!(gate.cancel_spawn(&second).await.is_some());
    assert!(
        gate.reserve_spawn(
            admitted,
            here(),
            crate::terminal_surface::MAX_HOSTED_TERMINALS,
            None,
            1000
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn activation_changes_before_launch_refuse_without_creating_a_process() {
    let gate = gate();
    let lead = lead(&gate).await;
    let admitted = admission(&gate, session_id(lead)).await;
    let (ticket, _) = gate
        .reserve_spawn(admitted, here(), 1, None, 1000)
        .await
        .expect("reserved");
    gate.set_dialogue(lead, false).await.expect("disable");
    gate.set_dialogue(lead, true).await.expect("new activation");
    let started = std::cell::Cell::new(false);
    assert!(
        gate.launch_worker(
            gate.mint(ticket.worker.terminal).expect("mint"),
            &ticket,
            || Ok(()),
            || {
                started.set(true);
                Ok::<_, String>(((), None))
            },
            str::to_owned
        )
        .await
        .is_err()
    );
    assert!(!started.get());
    assert!(gate.cancel_spawn(&ticket).await.is_some());
    assert_eq!(gate.pending_spawns().await, 0);
}

#[tokio::test]
async fn a_live_worker_cannot_spawn_and_survives_its_leads_exit() {
    let gate = gate();
    let lead = lead(&gate).await;
    let admitted = admission(&gate, session_id(lead)).await;
    let (ticket, _) = gate
        .reserve_spawn(admitted, here(), 1, None, 1000)
        .await
        .expect("reserved");
    gate.launch_worker(
        gate.mint(ticket.worker.terminal).expect("mint"),
        &ticket,
        || Ok(()),
        || Ok::<_, String>(((), None)),
        str::to_owned,
    )
    .await
    .expect("launched");
    gate.set_dialogue(ticket.worker.terminal, true)
        .await
        .expect("worker activation");
    let worker = admission(&gate, session_id(ticket.worker.terminal)).await;
    assert!(
        gate.reserve_spawn(worker, here(), 2, None, 1000)
            .await
            .is_err()
    );
    gate.forget(lead).await;
    assert!(gate.dialogue_enabled(ticket.worker.terminal).await);
    assert!(gate.cancel_spawn(&ticket).await.is_none());
    gate.forget(ticket.worker.terminal).await;
    assert!(
        gate.ended_worker(ticket.worker.terminal)
            .await
            .expect("supervisor observed exit")
            .is_some()
    );
    assert!(gate.state.lock().await.workers.is_empty());
}

#[tokio::test]
async fn initial_bytes_remain_in_the_existing_mailbox_until_visible_activation() {
    let gate = gate();
    let lead = lead(&gate).await;
    let admitted = admission(&gate, session_id(lead)).await;
    let body =
        runtrol_courier::BoundedUtf8::new("한국어 initial task".to_owned(), 100).expect("fits");
    let bytes = body.len();
    let (ticket, initial) = gate
        .reserve_spawn(admitted, here(), 1, Some(body), 1000)
        .await
        .expect("reserved");
    assert!(initial.is_some());
    assert_eq!(gate.state.lock().await.courier.charged_bytes(), bytes);
    assert!(!gate.dialogue_enabled(ticket.worker.terminal).await);
    gate.launch_worker(
        gate.mint(ticket.worker.terminal).expect("mint"),
        &ticket,
        || Ok(()),
        || Ok::<_, String>(((), None)),
        str::to_owned,
    )
    .await
    .expect("launched");
    assert!(!gate.dialogue_enabled(ticket.worker.terminal).await);
    gate.set_dialogue(ticket.worker.terminal, true)
        .await
        .expect("activate");
    let mut state = gate.state.lock().await;
    let mail = state
        .courier
        .receive(
            session_id(ticket.worker.terminal),
            crate::courier_gate::commands::now(),
        )
        .expect("initial delivery");
    assert_eq!(
        Some(mail.message_id),
        initial.map(|receipt| receipt.message_id)
    );
    assert_eq!(state.courier.charged_bytes(), 0);
}
