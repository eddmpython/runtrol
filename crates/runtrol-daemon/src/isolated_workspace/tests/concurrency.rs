//! A held real Git hook must own only its worktree operation, never the Runtime's whole registry.

use std::future::Future as _;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use runtrol_core::SessionManager;
use runtrol_ipc::wire::{Request, Response};
use runtrol_provider::ProcessIdentity;

use super::Scratch;
use crate::Composed;
use crate::dispatch::{Conversation, Prepared, Reply, answer_prepared, prepare_isolated_workspace};
use crate::isolated_workspace::ownership::ProcessStamp;

async fn greeted(composed: &Composed) -> Conversation {
    let mut conversation = Conversation::at_the_machine();
    assert!(matches!(
        answer_prepared(
            &mut conversation,
            composed,
            &mut SessionManager::new(),
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION
            },
            Prepared::None,
            None,
        )
        .await,
        Reply::One(Response::Welcome { .. })
    ));
    conversation
}

fn shell_quote(path: &std::path::Path) -> String {
    format!(
        "'{}'",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''")
    )
}

fn install_hook(scratch: &Scratch) {
    let hook = format!(
        "#!/bin/sh\nRUNTROL_WORKTREE_HOOK_ROOT={} {} --exact isolated_workspace::tests::cancellation::checkout_hook --ignored\n",
        shell_quote(&scratch.root),
        shell_quote(&std::env::current_exe().unwrap()),
    );
    std::fs::write(
        scratch
            .project
            .as_std_path()
            .join(".git/hooks/post-checkout"),
        hook,
    )
    .unwrap();
}

pub(crate) fn install_status_hook(scratch: &Scratch) {
    install_hook(scratch);
    let checkout = scratch
        .project
        .as_std_path()
        .join(".git/hooks/post-checkout");
    let status = scratch
        .project
        .as_std_path()
        .join(".git/hooks/fixture-fsmonitor");
    let command = std::fs::read_to_string(&checkout).unwrap();
    std::fs::write(
        &status,
        format!(
            "{} >/dev/null\nprintf 'fixture\\0/\\0'\n",
            command.trim_end()
        ),
    )
    .unwrap();
    std::fs::remove_file(checkout).unwrap();
    super::git(
        scratch.project.as_std_path(),
        &["config", "core.fsmonitor", &shell_quote(&status)],
    );
    super::git(
        scratch.project.as_std_path(),
        &["config", "core.fsmonitorHookVersion", "2"],
    );
}

pub(crate) async fn wait_for_hook(scratch: &Scratch) -> ProcessIdentity {
    let ready = scratch.root.join("hook-ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            Instant::now() < deadline,
            "owned status hook startup deadline"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let identity = std::fs::read_to_string(ready).unwrap();
    let (pid, started) = identity.split_once(' ').unwrap();
    ProcessIdentity::new(pid.parse().unwrap(), started.parse().unwrap()).unwrap()
}

pub(crate) async fn release_hook(scratch: &Scratch, identity: ProcessIdentity) {
    std::fs::write(scratch.root.join("hook-release"), "release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while ProcessStamp::from(identity).is_live() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !ProcessStamp::from(identity).is_live(),
        "the exact status hook child has exited"
    );
}

fn prepared(response: Prepared) -> Option<runtrol_ipc::wire::IsolatedWorkspaceLine> {
    match response {
        Prepared::IsolatedWorkspacePrepare {
            response: Response::IsolatedWorkspace(line),
            ..
        } => Some(*line),
        _ => None,
    }
}

#[tokio::test]
async fn cancelled_release_retains_its_owner_until_the_exact_git_hook_has_ended() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Owner(Arc<AtomicBool>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let scratch = Scratch::make();
    let controller = super::IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let containment = runtrol_childproc::Containment::without_any();
    let request = "01234567-89ab-cdef-0123-456789abcde0";
    let Response::IsolatedWorkspace(workspace) = controller
        .prepare(&containment, request, scratch.project.as_str())
        .await
        .unwrap()
    else {
        panic!("prepared workspace");
    };
    install_status_hook(&scratch);
    let retired = Arc::new(AtomicBool::new(false));
    let owner = Arc::new(Owner(Arc::clone(&retired)));
    let mut releasing = Box::pin(controller.release_retaining(
        &containment,
        Some(request),
        None,
        &workspace.workspace,
        owner.clone(),
    ));
    let hook = tokio::select! {
        result = &mut releasing => panic!("cleanup escaped the owned Git hook: {result:?}"),
        hook = wait_for_hook(&scratch) => hook,
    };
    drop(owner);
    assert!(!retired.load(Ordering::Acquire));
    drop(releasing);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !retired.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "the exact cancelled Git operation did not retire"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !ProcessStamp::from(hook).is_live(),
        "the session owner was released beneath a live Git hook"
    );
    let _operation = super::super::registry::operation(&scratch.registry, request).unwrap();
}

async fn release_other_project(
    composed: &Arc<Composed>,
    workspace: &runtrol_ipc::wire::IsolatedWorkspaceLine,
) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            composed.isolated_workspaces.release(
                &composed.containment,
                Some(&workspace.workspace_id),
                None,
                &workspace.workspace,
            )
        )
        .await,
        Ok(Ok(Response::IsolatedWorkspaceReleased(line))) if line.outcome.as_ref() == "removed"
    )
}

#[tokio::test]
async fn a_held_git_checkout_does_not_block_another_project_or_ownership_refusal() {
    let first = Scratch::make();
    let second = Scratch::make();
    let home = first.root.join("runtime");
    std::fs::create_dir(&home).unwrap();
    let composed =
        Arc::new(Composed::for_tests(home.to_str().unwrap(), runtrol_drivers::builtin()).unwrap());
    let conversation = greeted(&composed).await;
    let mut listing_conversation = greeted(&composed).await;
    install_hook(&first);
    // Different stripes make the scope under test the global controller, not its existing bounded collisions.
    let first_request = Request::WorkspaceIsolatePrepare {
        request_id: "01234567-89ab-cdef-0123-456789abcde0".into(),
        project: first.project.as_str().into(),
    };
    let second_request = Request::WorkspaceIsolatePrepare {
        request_id: "01234567-89ab-cdef-0123-456789abcde1".into(),
        project: second.project.as_str().into(),
    };
    let mut creating = Box::pin(prepare_isolated_workspace(
        &conversation,
        &composed,
        &first_request,
    ));
    let identity = tokio::select! {
        _ = &mut creating => panic!("Git returned before its held checkout hook"),
        identity = wait_for_hook(&first) => identity,
    };
    let duplicate_refused = {
        let mut duplicate = std::pin::pin!(prepare_isolated_workspace(
            &conversation,
            &composed,
            &first_request
        ));
        matches!(duplicate.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Prepared::IsolatedWorkspacePrepare { response: Response::Failed(error), .. }) if error.message.contains("busy"))
    };
    let listed = matches!(
        answer_prepared(
            &mut listing_conversation,
            &composed,
            &mut SessionManager::new(),
            Request::WorkspaceIsolateList,
            Prepared::None,
            None,
        )
        .await,
        Reply::One(Response::IsolatedWorkspaces(_))
    );
    let second_result = tokio::time::timeout(
        Duration::from_secs(5),
        prepare_isolated_workspace(&conversation, &composed, &second_request),
    )
    .await;
    let second_workspace = match second_result {
        Ok(result) => prepared(result),
        // Keep the failed observation until after the held child has been released and reaped.
        Err(_) => None,
    };
    let second_released = if let Some(workspace) = &second_workspace {
        release_other_project(&composed, workspace).await
    } else {
        false
    };
    let hook_still_held = ProcessStamp::from(identity).is_live();
    // Complete and reap the actual command before asserting any red result or removing either fixture.
    std::fs::write(first.root.join("hook-release"), "release").unwrap();
    let first_workspace = prepared(creating.await).expect("held checkout completes after release");
    assert!(
        !ProcessStamp::from(identity).is_live(),
        "the exact hook child has exited"
    );
    let first_released = matches!(
        composed.isolated_workspaces.release(
            &composed.containment,
            Some(&first_workspace.workspace_id),
            None,
            &first_workspace.workspace,
        ).await,
        Ok(Response::IsolatedWorkspaceReleased(line)) if line.outcome.as_ref() == "removed"
    );
    drop(composed);
    assert!(
        duplicate_refused,
        "a held same-owner operation must refuse immediately instead of waiting"
    );
    assert!(
        listed,
        "a Git operation cannot make read-only registry listing fail"
    );
    assert!(
        hook_still_held && second_workspace.is_some() && second_released,
        "another project's prepare and cleanup must finish while the first Git hook is held"
    );
    assert!(
        first_released,
        "the held worktree remains independently releasable"
    );
}

#[tokio::test]
async fn a_held_git_cleanup_does_not_block_another_project() {
    use super::IsolatedWorkspaceController;
    let first = Scratch::make();
    let second = Scratch::make();
    let controller = IsolatedWorkspaceController::open(first.registry.clone()).unwrap();
    let containment = runtrol_childproc::Containment::without_any();
    let Response::IsolatedWorkspace(workspace) = controller
        .prepare(
            &containment,
            "01234567-89ab-cdef-0123-456789abcde0",
            first.project.as_str(),
        )
        .await
        .unwrap()
    else {
        panic!("prepared workspace");
    };
    install_status_hook(&first);
    let mut removing = Box::pin(controller.release(
        &containment,
        Some(&workspace.workspace_id),
        None,
        &workspace.workspace,
    ));
    let identity = tokio::select! {
        result = &mut removing => panic!("Git returned before its held status hook: {result:?}"),
        identity = wait_for_hook(&first) => identity,
    };
    let duplicate_refused = {
        let mut duplicate = std::pin::pin!(controller.release(
            &containment,
            Some(&workspace.workspace_id),
            None,
            &workspace.workspace,
        ));
        matches!(duplicate.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Err(error)) if error.contains("busy"))
    };
    // Bound each Git-backed operation independently, as in the checkout case. Combining their
    // deadlines would turn this ownership barrier into an accidental whole-lifecycle speed test.
    let independent = async {
        let prepared = tokio::time::timeout(
            Duration::from_secs(5),
            controller.prepare(
                &containment,
                "01234567-89ab-cdef-0123-456789abcde1",
                second.project.as_str(),
            ),
        )
        .await
        .map_err(|_| {
            "independent prepare did not complete while Git cleanup was held".to_owned()
        })??;
        let Response::IsolatedWorkspace(other) = prepared else {
            return Err("prepared workspace expected".to_owned());
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            controller.release(
                &containment,
                Some(&other.workspace_id),
                None,
                &other.workspace,
            ),
        )
        .await
        .map_err(|_| "independent cleanup did not complete while Git cleanup was held".to_owned())?
    }
    .await;
    let hook_still_held = ProcessStamp::from(identity).is_live();
    release_hook(&first, identity).await;
    let removed = removing.await.unwrap();
    assert!(
        matches!(removed, Response::IsolatedWorkspaceReleased(line) if line.outcome.as_ref() == "removed")
    );
    assert!(
        duplicate_refused,
        "the existing cleanup lease refuses duplicate ownership immediately"
    );
    assert!(
        hook_still_held,
        "the controlled cleanup hook ended before its release"
    );
    assert!(
        matches!(&independent, Ok(Response::IsolatedWorkspaceReleased(line)) if line.outcome.as_ref() == "removed"),
        "another project's lifecycle must complete while cleanup Git is held: {independent:?}"
    );
}
