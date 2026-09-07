//! A transient member query can recover; losing the complete initial list cannot prove cleanup.

use super::*;

#[tokio::test]
async fn a_transient_member_query_keeps_opened_handles_and_retries_the_unresolved_pid() {
    let mut fixture = Fixture::new();
    let mut command = tokio::process::Command::from(helper_command(&fixture.root, "held_root"));
    crate::console_window::hide_console_window_with_flags(command.as_std_mut(), CREATE_SUSPENDED);
    let mut child = command.kill_on_drop(true).spawn().unwrap();
    let job = CommandJob::new(None).unwrap();
    job.assign_and_resume(&child).unwrap();
    let deadline = Instant::now() + OBSERVE_LIMIT;
    while !fixture.root.join("root-ready").is_file() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fixture.observe().unwrap();
    eprintln!(
        "owned member-recovery tree: {:?}",
        fixture.identities().unwrap()
    );
    job.job.fail_one_member_open();
    let first = job.request_stop();
    let retained = job.job.retained_members();
    let premature = job.is_empty();
    let retry = job.stop().await;
    child.wait().await.unwrap();
    let stopped = fixture.recorded_processes_stopped();
    let final_retained = job.job.retained_members();
    drop(fixture);
    assert!(
        first.is_err(),
        "the injected kernel query failure must be reported"
    );
    assert!(
        premature.is_err(),
        "an unresolved PID cannot authorize release"
    );
    assert!(
        retained > 0,
        "the successful sibling open must remain retained"
    );
    assert!(
        retry.is_ok(),
        "the exact unresolved member can be retried: {retry:?}"
    );
    assert!(stopped && job.is_empty().unwrap());
    assert!(
        final_retained >= retained,
        "recovery must not discard previously opened handles"
    );
}

#[tokio::test]
async fn an_unavailable_initial_snapshot_cannot_be_replaced_with_an_empty_count() {
    use windows_sys::Win32::System::SystemServices::JOB_OBJECT_SET_ATTRIBUTES;
    let mut fixture = Fixture::new();
    let mut child = suspended_child(&fixture.root);
    fixture
        .known
        .push(process_identity(child.id().unwrap()).unwrap());
    fixture.observe().unwrap();
    let mut job = CommandJob::new(None).unwrap();
    job.assign_and_resume(&child).unwrap();
    let full = duplicate(job.job.handle.as_raw_handle(), 0, DUPLICATE_SAME_ACCESS);
    job.job.handle = duplicate(
        job.job.handle.as_raw_handle(),
        JOB_OBJECT_SET_ATTRIBUTES | JOB_OBJECT_TERMINATE,
        0,
    );
    let first = job.request_stop();
    child.wait().await.unwrap();
    job.job.handle = full;
    let retry = job.request_stop();
    let premature = job.is_empty();
    let stopped = fixture.recorded_processes_stopped();
    drop(fixture);
    assert!(stopped, "the exact fixture root really ended");
    assert!(
        first.is_err() && retry.is_err() && premature.is_err(),
        "no complete sealed member list was ever observed"
    );
}
