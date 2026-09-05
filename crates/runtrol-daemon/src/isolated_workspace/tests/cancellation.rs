//! Real Git cancellation must retain ownership until every operation child has ended.

use std::time::{Duration, Instant};

use runtrol_childproc::Containment;
use runtrol_provider::{ProcessIdentity, TerminalId};

use super::{Scratch, process::current_process};
use crate::isolated_workspace::{
    IsolatedWorkspaceController, VerifiedProject,
    ownership::{EndedSpawn, ProcessStamp, SpawnTicket},
};

#[cfg(windows)]
#[tokio::test]
async fn cancelled_git_cannot_release_beneath_a_live_checkout_hook() {
    let scratch = Scratch::make();
    let shell_quote = |path: &std::path::Path| {
        format!(
            "'{}'",
            path.to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''")
        )
    };
    let executable = std::env::current_exe().unwrap();
    let hook = format!(
        "#!/bin/sh\nRUNTROL_WORKTREE_HOOK_ROOT={} {} --exact isolated_workspace::tests::cancellation::checkout_hook --ignored\n",
        shell_quote(&scratch.root),
        shell_quote(&executable)
    );
    std::fs::write(
        scratch
            .project
            .as_std_path()
            .join(".git/hooks/post-checkout"),
        hook,
    )
    .unwrap();
    let containment = Containment::without_any();
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    let ticket =
        SpawnTicket::new(current_process(), TerminalId::now(), TerminalId::now(), 1).unwrap();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let ready = scratch.root.join("hook-ready");
    let wait_ready = async {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if ready.exists() {
                break;
            }
            assert!(Instant::now() < deadline, "checkout hook startup deadline");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let mut creating = Box::pin(controller.prepare_terminal(&containment, &ticket, &project));
    tokio::select! {
        result = &mut creating => panic!("Git finished before its held hook: {result:?}"),
        () = wait_ready => {},
    }
    let identity = std::fs::read_to_string(ready).unwrap();
    let (pid, started) = identity.split_once(' ').unwrap();
    let identity = ProcessIdentity::new(pid.parse().unwrap(), started.parse().unwrap()).unwrap();
    drop(creating);
    let live_before = ProcessStamp::from(identity).is_live();
    let released = controller
        .release_terminal(&containment, &EndedSpawn::after_gate_retired(ticket))
        .await;
    let live_after = ProcessStamp::from(identity).is_live();
    let deadline = Instant::now() + Duration::from_secs(5);
    while ProcessStamp::from(identity).is_live() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let stopped_by_containment = !ProcessStamp::from(identity).is_live();
    // Always release and reap the exact helper before asserting the contract or removing fixture files.
    std::fs::write(scratch.root.join("hook-release"), "release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while ProcessStamp::from(identity).is_live() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !ProcessStamp::from(identity).is_live(),
        "owned checkout helper stopped"
    );
    assert!(
        !live_before || !live_after || released.is_err(),
        "cleanup removed a worktree while the cancelled operation child was live"
    );
    assert!(
        stopped_by_containment,
        "cancellation stopped the entire Git command Job"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match controller
            .release_terminal(&containment, &EndedSpawn::after_gate_retired(ticket))
            .await
        {
            Ok(_) => break,
            Err(error)
                if error.starts_with("worktree ownership is busy:")
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(error) => panic!("the ended command's resource lease did not release: {error}"),
        }
    }
}

#[test]
#[ignore = "bounded Git checkout hook entry point"]
fn checkout_hook() {
    let root = std::path::PathBuf::from(std::env::var_os("RUNTROL_WORKTREE_HOOK_ROOT").unwrap());
    assert!(root.join("fixture-owner").is_file());
    // A hook may legitimately work elsewhere. Its cwd must not accidentally serve as a Windows
    // deletion lock that hides the process-lifetime defect this scenario checks.
    std::env::set_current_dir(&root).unwrap();
    let identity = current_process();
    let temporary = root.join("hook-starting");
    std::fs::write(
        &temporary,
        format!("{} {}", identity.pid(), identity.started()),
    )
    .unwrap();
    std::fs::rename(temporary, root.join("hook-ready")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("hook-release").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}
