//! A terminal must retain its process lifetime after the CLI root stops but a descendant remains.

use super::*;
use crate::{PtyChild, PtySize, PtySpawn};

async fn terminal_tree(exit_first: bool) {
    let mut fixture = Fixture::new();
    let executable = std::env::current_exe().expect("the test executable exists");
    let program = resolve(executable.to_str().expect("the executable path is UTF-8"))
        .expect("the owned helper resolves");
    let directory = AbsPath::canonicalize(fixture.root.to_str().expect("UTF-8 fixture path"))
        .expect("the fixture directory exists");
    let arguments = helper_args(if exit_first {
        "exiting_root"
    } else {
        "held_root"
    });
    let child = PtyChild::spawn(PtySpawn {
        containment: crate::PtyContainment::Local,
        program: &program,
        arguments: &arguments,
        cwd: &directory,
        env: &[],
        env_unset: &[],
        size: PtySize { cols: 80, rows: 24 },
    })
    .expect("the owned terminal tree starts");
    let mut reader = child.reader().expect("the terminal output opens");
    let reading = std::thread::spawn(move || std::io::copy(&mut reader, &mut std::io::sink()));
    let deadline = Instant::now() + OBSERVE_LIMIT;
    while !fixture.root.join("root-ready").exists() {
        assert!(
            Instant::now() < deadline,
            "the owned terminal tree starts in time"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fixture
        .observe()
        .expect("retain the exact root and descendant handles");
    eprintln!(
        "owned terminal tree: {:?}",
        fixture.identities().expect("recorded identities")
    );
    let scope = child.process_scope();
    assert_membership(&fixture, &scope);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), child.wait())
            .await
            .is_err()
    );
    assert!(
        !fixture.recorded_processes_stopped(),
        "cancelling a wait never kills its owner"
    );
    std::fs::write(fixture.root.join("observer-ready"), b"observed")
        .expect("the root may now exit independently of its descendant");
    if !exit_first {
        child
            .kill()
            .expect("request termination of the owned terminal");
    }
    let event = tokio::time::timeout(OBSERVE_LIMIT, child.wait()).await;
    let exit = child.try_wait().expect("inspect terminal completion");
    let all_stopped = fixture.recorded_processes_stopped();
    if !exit_first {
        assert_eq!(
            exit,
            Some(crate::contain::TERMINATED_BY_RUNTROL.cast_signed())
        );
    }
    assert!(
        fixture
            .identities()
            .unwrap()
            .into_iter()
            .all(|id| !scope.contains(id).unwrap())
    );
    // Always reap the fixture's exact handles before an assertion, including the baseline failure.
    drop(fixture);
    tokio::task::spawn_blocking(move || {
        child.finish();
        reading
            .join()
            .expect("the owned output reader ends")
            .expect("the owned output drains");
    })
    .await
    .expect("the bounded terminal close completes");
    assert!(exit.is_some(), "the terminal did not report completion");
    assert_eq!(
        event
            .expect("exact process events completed")
            .expect("Job proof"),
        exit.unwrap()
    );
    assert!(
        all_stopped,
        "terminal completion preceded exact descendant termination"
    );
}

fn assert_membership(fixture: &Fixture, scope: &crate::ProcessScope) {
    for identity in fixture.identities().expect("exact identities") {
        assert!(
            scope
                .contains(identity)
                .expect("member identity is inspectable")
        );
        let stale = ProcessIdentity::new(identity.pid(), identity.started() - 1).unwrap();
        assert!(
            !scope
                .contains(stale)
                .expect("reused PID cannot authenticate")
        );
    }
    assert!(
        !scope
            .contains(process_identity(std::process::id()).unwrap())
            .unwrap()
    );
}

#[tokio::test]
async fn stopping_a_terminal_keeps_ownership_until_its_detached_descendant_ends() {
    terminal_tree(false).await;
}

#[tokio::test]
async fn natural_terminal_root_exit_cannot_leave_its_detached_descendant_running() {
    terminal_tree(true).await;
}

#[tokio::test]
async fn terminal_membership_is_nested_and_excludes_sibling_sessions() {
    let mut fixture = Fixture::new();
    let result = run_helper(
        &mut fixture,
        "terminal::nested_terminal_owner",
        OBSERVE_LIMIT,
    )
    .await;
    let proved = fixture.root.join("nested-proved").is_file();
    let stopped = fixture.recorded_processes_stopped();
    drop(fixture);
    assert!(
        result
            .expect("the nested terminal owner completed")
            .succeeded()
    );
    assert!(
        proved && stopped,
        "both boundaries and exact completion were observed"
    );
}

#[tokio::test]
#[ignore = "only this disposable helper may establish the self-containing outer Job"]
async fn nested_terminal_owner() {
    let root = helper_root();
    record_process(&root, "root");
    let outer = Containment::establish().expect("the disposable owner establishes containment");
    let program = resolve(std::env::current_exe().unwrap().to_str().unwrap()).unwrap();
    let cwd = AbsPath::canonicalize(root.to_str().unwrap()).unwrap();
    let arguments = helper_args("leaf");
    let mut children = Vec::new();
    for _ in 0..2 {
        let (mut child, permission) = PtyChild::prepare(PtySpawn {
            containment: crate::PtyContainment::Local,
            program: &program,
            arguments: &arguments,
            cwd: &cwd,
            env: &[],
            env_unset: &[],
            size: PtySize { cols: 80, rows: 24 },
        })
        .expect("membership is atomic with suspended creation");
        child.abandon_output();
        children.push((child, permission));
    }
    let first = children.first().unwrap().0.process_scope();
    let second = children.last().unwrap().0.process_scope();
    let identity = process_identity(children.first().unwrap().0.pid()).unwrap();
    assert!(outer.contains(identity, identity).unwrap());
    assert!(first.contains(identity).unwrap());
    assert!(!second.contains(identity).unwrap());
    assert!(
        !first
            .contains(process_identity(std::process::id()).unwrap())
            .unwrap()
    );
    for (child, permission) in children {
        child.kill().expect("stop only this session Job");
        child.wait().await.expect("exact child and Job completion");
        drop(permission);
        drop(child);
    }
    std::fs::write(root.join("root-ready"), b"ready").unwrap();
    wait_for_file(&root.join("observer-ready"));
    std::fs::write(root.join("nested-proved"), b"outer survived").unwrap();
    #[expect(
        clippy::disallowed_methods,
        reason = "the helper's self-containing Job remains owned until OS teardown after the test runner reports success"
    )]
    std::mem::forget(outer);
}
