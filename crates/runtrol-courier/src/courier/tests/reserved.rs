use super::*;

#[test]
fn initial_mail_is_charged_but_neither_party_can_cross_the_closed_recipient() {
    let mut fleet = fleet(Limits::INITIAL);
    let worker = ManagedSessionId::now();
    let envelope = CallEnvelope::tell(fleet.alpha, worker, body("한국어 task"), later(100));
    let message = envelope.message_id;
    let bytes = envelope.body.len();
    fleet
        .courier
        .reserve_initial(envelope, NOW)
        .expect("reserve");
    assert!(!fleet.courier.is_live(worker));
    assert_eq!(fleet.courier.charged_bytes(), bytes);
    assert!(fleet.courier.receive(worker, NOW).is_none());
    assert!(
        fleet
            .courier
            .send(
                CallEnvelope::tell(worker, fleet.alpha, body("reply"), later(100)),
                NOW
            )
            .is_err()
    );
    assert!(
        fleet
            .courier
            .send(
                CallEnvelope::tell(fleet.alpha, worker, body("another"), later(100)),
                NOW
            )
            .is_err()
    );
    assert!(fleet.courier.session_started(worker));
    assert_eq!(
        fleet
            .courier
            .receive(worker, NOW)
            .expect("activated")
            .message_id,
        message
    );
    assert_eq!(fleet.courier.charged_bytes(), 0);
    assert!(fleet.courier.receive(worker, NOW).is_none());
}

#[test]
fn reserved_mail_expires_and_cancellation_refunds_the_same_budget() {
    let mut fleet = fleet(Limits::INITIAL);
    let first = ManagedSessionId::now();
    fleet
        .courier
        .reserve_initial(
            CallEnvelope::tell(fleet.alpha, first, body("first"), later(10)),
            NOW,
        )
        .expect("first");
    assert_eq!(fleet.courier.next_deadline(), Some(later(10)));
    assert_eq!(fleet.courier.sweep(later(10)).bytes, 5);
    assert_eq!(fleet.courier.charged_bytes(), 0);
    fleet.courier.session_started(first);
    assert!(fleet.courier.receive(first, later(10)).is_none());
    let second = ManagedSessionId::now();
    fleet
        .courier
        .reserve_initial(
            CallEnvelope::tell(fleet.alpha, second, body("second"), later(20)),
            NOW,
        )
        .expect("second");
    assert_eq!(fleet.courier.session_ended(second).bytes, 6);
    assert_eq!(fleet.courier.charged_bytes(), 0);
}

#[test]
fn reservations_cannot_replace_live_mailboxes_or_gain_another_body_allowance() {
    let mut limits = Limits::INITIAL;
    limits.runtime_bytes = 5;
    let mut fleet = fleet(limits);
    let worker = ManagedSessionId::now();
    fleet
        .courier
        .reserve_initial(
            CallEnvelope::tell(fleet.alpha, worker, body("first"), later(100)),
            NOW,
        )
        .expect("fits");
    for target in [worker, fleet.bravo] {
        assert_eq!(
            fleet.courier.reserve_initial(
                CallEnvelope::tell(fleet.alpha, target, body("x"), later(100)),
                NOW
            ),
            Err(Refusal::RecipientAlreadyReserved(target))
        );
    }
    let other = ManagedSessionId::now();
    assert!(matches!(
        fleet.courier.reserve_initial(
            CallEnvelope::tell(fleet.alpha, other, body("x"), later(100)),
            NOW
        ),
        Err(Refusal::RuntimeBytes { .. })
    ));
    assert_eq!(fleet.courier.charged_bytes(), 5);
    assert_eq!(fleet.courier.waiting(other), None);
    assert_eq!(
        fleet.courier.reserve_initial(
            CallEnvelope::ask(fleet.alpha, other, body("x"), later(100)),
            NOW
        ),
        Err(Refusal::InitialMailKind)
    );
}
