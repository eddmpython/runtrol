//! A generation handover must preserve a quiet, unwatched process and its admission.

use super::*;

#[tokio::test]
async fn a_quiet_unwatched_owner_survives_generation_draining() {
    let (composed, home) = composed_for(&format!("draining-{}", TerminalId::now()));
    let workspace = AbsPath::canonicalize(&home).expect("the fixture home canonicalizes");
    let (shell, arguments, input) = if cfg!(windows) {
        (
            "cmd",
            vec![
                "/q",
                "/d",
                "/v:on",
                "/c",
                "set /p first=& echo draining-!first!",
            ],
            b"survived\r\n".as_slice(),
        )
    } else {
        (
            "sh",
            vec![
                "-c",
                "IFS= read -r first; printf 'draining-%s\\n' \"$first\"",
            ],
            b"survived\n".as_slice(),
        )
    };
    let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
    let terminal = Terminal::open(&TerminalLaunch {
        containment: None,
        program: &program,
        arguments: arguments.into_iter().map(str::to_owned).collect(),
        cwd: &workspace,
        env: Vec::new(),
        env_unset: Vec::new(),
        size: runtrol_childproc::PtySize { cols: 80, rows: 24 },
    })
    .expect("the owned shell opens");
    let owned = OwnedTerminals(vec![terminal.clone()]);
    let identity = runtrol_childproc::process_identity(terminal.pid())
        .expect("the exact owned child is inspectable");
    eprintln!("owned draining fixture: identity={identity:?}, home={home}");
    let id = TerminalId::now();
    composed.terminals.lock().await.insert(
        id,
        ProviderId::parse("fixture").expect("a fixture provider identifier"),
        None,
        terminal.clone(),
        workspace,
        None,
    );
    forget_on_exit(Arc::clone(&composed), id, &terminal);
    assert_eq!(terminal.viewer_count(), 0);

    // Advance only the drain policy's clock. A real process is waiting for input the whole time.
    close_idle_at(&composed, u64::MAX, DRAINING_IDLE_GRACE_MS).await;
    let kept = composed
        .terminals
        .lock()
        .await
        .hosted(id)
        .is_some_and(|hosted| !hosted.stopping);
    let mut view = terminal.attach().await;
    let sent = terminal.input(input).await.is_ok();
    let echoed = if sent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let chunk = view
                    .live
                    .recv()
                    .await
                    .expect("the fixture ring remains live");
                if chunk
                    .bytes
                    .windows(b"draining-survived".len())
                    .any(|window| window == b"draining-survived")
                {
                    break;
                }
            }
        })
        .await
        .is_ok()
    } else {
        false
    };
    drop(view);
    drop(owned);
    let retired = gone(&composed, id).await;
    drop(terminal);
    drop(composed);
    std::fs::remove_dir_all(home).expect("remove the owned fixture home");
    assert!(retired, "explicit cleanup retires the fixture");
    assert!(
        kept && sent && echoed,
        "draining ended a quiet owner before its next input"
    );
    assert!(!runtrol_childproc::matches_process_start(
        identity.pid(),
        identity.started()
    ));
}
