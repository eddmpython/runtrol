//! The gate admits only the token it minted, and only when every authority agrees.
//!
//! The refusals that never reach the kernel (version, peer, session, root, token) are checked against a
//! containment that holds nothing, so they run everywhere. The one path that reaches the kernel is proved on
//! Windows with a real job object, which is where the product and its gates run.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_childproc::Containment;
use runtrol_courier::env::COURIER_TOKEN_ENV;
use runtrol_courier::wire::Hello;
use runtrol_courier::{ManagedSessionId, PROTOCOL_VERSION};
use runtrol_provider::{ProcessIdentity, TerminalId};

use super::{CourierGate, Denied, Minted};

fn gate() -> CourierGate {
    CourierGate::new(
        "C:/runtrol.exe".to_owned(),
        r"\\.\pipe\runtrol-courier-test".to_owned(),
    )
}

fn session_id(terminal: TerminalId) -> ManagedSessionId {
    terminal
        .to_string()
        .parse()
        .expect("a terminal identifier is a managed session identifier")
}

fn token_of(minted: &Minted) -> String {
    minted
        .env()
        .iter()
        .find(|(name, _)| name == COURIER_TOKEN_ENV)
        .map(|(_, value)| value.clone())
        .expect("the environment carries the token")
}

fn here() -> ProcessIdentity {
    runtrol_childproc::process_identity(std::process::id()).expect("this process has an identity")
}

#[tokio::test]
async fn a_hello_is_refused_before_the_kernel_for_every_reason_but_the_authorities() {
    let gate = gate();
    let nothing = Containment::without_any();
    let terminal = TerminalId::now();
    let session = session_id(terminal);
    let minted = gate.mint(terminal).expect("a token is minted");
    let token = token_of(&minted);

    // A hello of another layout is refused before anything is looked up.
    assert_eq!(
        gate.admit(
            &nothing,
            Some(here()),
            &Hello {
                protocol_version: PROTOCOL_VERSION.saturating_add(1),
                session,
                token: token.clone(),
            }
        )
        .await,
        Err(Denied::Version(PROTOCOL_VERSION.saturating_add(1)))
    );
    // The endpoint that did not identify its peer proves nothing about it.
    assert_eq!(
        gate.admit(&nothing, None, &Hello::new(session, token.clone()))
            .await,
        Err(Denied::NoPeer)
    );
    // No session by that name.
    assert_eq!(
        gate.admit(&nothing, Some(here()), &Hello::new(session, token.clone()))
            .await,
        Err(Denied::UnknownSession)
    );

    // The session exists but its process is not known yet.
    gate.open_session(terminal, minted, None).await;
    assert_eq!(
        gate.admit(&nothing, Some(here()), &Hello::new(session, token.clone()))
            .await,
        Err(Denied::RootUnbound)
    );

    // A second session whose root is bound, so the token is the last thing before the kernel.
    let other = TerminalId::now();
    let other_session = session_id(other);
    let other_minted = gate.mint(other).expect("a token is minted");
    let other_token = token_of(&other_minted);
    gate.open_session(other, other_minted, Some(here())).await;
    let wrong = Base64UrlUnpadded::encode_string(&[0_u8; 32]);
    assert_ne!(wrong, other_token);
    assert_eq!(
        gate.admit(&nothing, Some(here()), &Hello::new(other_session, wrong))
            .await,
        Err(Denied::Token)
    );
    // The right token now reaches the kernel, which a containment that holds nothing answers "outside".
    assert_eq!(
        gate.admit(
            &nothing,
            Some(here()),
            &Hello::new(other_session, other_token)
        )
        .await,
        Err(Denied::OutsideContainment)
    );

    // The terminal ending forgets the token: it admits nobody afterward.
    gate.forget(other).await;
    let stale = token_of(&gate.mint(other).expect("mint")); // a fresh token, never opened
    assert_eq!(
        gate.admit(&nothing, Some(here()), &Hello::new(other_session, stale))
            .await,
        Err(Denied::UnknownSession)
    );
}

// The full-agreement success path (a peer inside a real job, under its session's root, with the minted token)
// is proved by the real journey, not here: establishing a kill-on-close job in this test process and dropping
// it would terminate the test runner itself. The gate's own kernel query is proved separately in
// `runtrol-childproc`'s containment membership test, which runs the job in a helper process for that reason.
