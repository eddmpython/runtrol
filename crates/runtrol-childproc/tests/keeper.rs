//! Shipping same-image keeper wiring, including the crash boundary outside the test process.

#![cfg(windows)]

use std::io::{BufRead as _, Write as _};
use std::process::{Command, Stdio};

#[test]
fn kept_scopes_finish_after_normal_stop_and_exact_runtime_crash() {
    for scenario in ["normal", "crash", "keeper"] {
        let directory =
            std::env::temp_dir().join(format!("keeper-product-{}-{scenario}", std::process::id()));
        assert!(!directory.exists());
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("fixture-owner"), b"keeper").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_containedParent"));
        command
            .args(["--keeper-parent", directory.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        runtrol_childproc::hide_console_window(&mut command);
        let mut runtime = command.spawn().unwrap();
        let mut output = std::io::BufReader::new(runtime.stdout.take().unwrap());
        let mut identity = String::new();
        output.read_line(&mut identity).unwrap();
        let fields: Vec<u64> = identity
            .split_whitespace()
            .map(|word| word.parse().unwrap())
            .collect();
        let [runtime_pid, runtime_start, keeper_pid, keeper_start] = fields.as_slice() else {
            panic!("exact helper identities were not returned");
        };
        let parent = runtrol_provider::ProcessIdentity::new(
            u32::try_from(*runtime_pid).unwrap(),
            *runtime_start,
        )
        .unwrap();
        let keeper = runtrol_provider::ProcessIdentity::new(
            u32::try_from(*keeper_pid).unwrap(),
            *keeper_start,
        )
        .unwrap();
        let held = runtrol_childproc::KeeperWait::open(parent, keeper).unwrap();
        eprintln!(
            "keeper fixture {scenario}: Runtime {}@{} keeper {}@{}",
            parent.pid(),
            parent.started(),
            keeper.pid(),
            keeper.started()
        );
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready.trim(), "ready");
        if scenario == "crash" {
            runtime.kill().unwrap();
        } else if scenario == "keeper" {
            terminate_keeper(keeper);
            runtime
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"lost\n")
                .unwrap();
            let mut observed = String::new();
            output.read_line(&mut observed).unwrap();
            assert_eq!(observed.trim(), "unconfirmed");
        } else {
            runtime
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"stop\n")
                .unwrap();
        }
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        executor.block_on(async {
            let result = tokio::time::timeout(std::time::Duration::from_secs(20), held.wait())
                .await
                .unwrap();
            assert_eq!(result.is_ok(), scenario != "keeper");
        });
        let status = runtime.wait().unwrap();
        assert_eq!(status.success(), scenario == "normal");
        if scenario == "keeper" {
            assert!(!directory.join("completed").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(directory.join("completed")).unwrap(),
                identity.trim()
            );
        }
        assert_descendants_finished(&directory).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
    }
}

fn assert_descendants_finished(
    directory: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for index in 0..8 {
        let pid: u32 =
            std::fs::read_to_string(directory.join(format!("terminal-{index}/leaf")))?.parse()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runtrol_childproc::alive(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "a fixture descendant remains"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "the fault targets only an exact retained fixture keeper process"
)]
fn terminate_keeper(identity: runtrol_provider::ProcessIdentity) {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess,
    };
    // SAFETY: access is confined to this fixture's keeper; the birth is checked on the retained handle.
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            0,
            identity.pid(),
        )
    };
    assert!(!raw.is_null());
    // SAFETY: OpenProcess returned one owned handle, transferred once.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut birth = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    assert_ne!(
        // SAFETY: four writable initialized time fields and the live process handle remain valid.
        unsafe {
            GetProcessTimes(
                handle.as_raw_handle(),
                &raw mut birth,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        },
        0
    );
    assert_eq!(
        (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime),
        identity.started()
    );
    // SAFETY: the exact process object just validated belongs solely to this finite fixture.
    assert_ne!(unsafe { TerminateProcess(handle.as_raw_handle(), 1) }, 0);
}
