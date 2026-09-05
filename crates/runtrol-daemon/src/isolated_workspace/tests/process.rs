use super::Scratch;
use runtrol_provider::ProcessIdentity;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
pub(super) fn current_process() -> ProcessIdentity {
    runtrol_childproc::process_identity(std::process::id()).unwrap()
}

pub(crate) struct ProcessScratch {
    child: Child,
    pub(crate) identity: ProcessIdentity,
}

impl ProcessScratch {
    pub(crate) fn start(fixture: &Scratch) -> Self {
        Self::start_with_lock(fixture, false, false)
    }

    pub(super) fn holding_registry(fixture: &Scratch) -> Self {
        Self::start_with_lock(fixture, true, false)
    }

    #[cfg(windows)]
    pub(super) fn waiting_legacy_commit(fixture: &Scratch) -> Self {
        Self::start_with_lock(fixture, false, true)
    }

    fn start_with_lock(fixture: &Scratch, lock: bool, legacy: bool) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "isolated_workspace::tests::process::process_helper",
                "--ignored",
            ])
            .env("RUNTROL_WORKTREE_FIXTURE", &fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if lock {
            command.env("RUNTROL_WORKTREE_FIXTURE_LOCK", "1");
        }
        if legacy {
            command.env("RUNTROL_WORKTREE_FIXTURE_LEGACY", "1");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().unwrap();
        let identity = runtrol_childproc::process_identity(child.id()).unwrap();
        let fixture_process = Self { child, identity };
        if lock || legacy {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let ready = fixture
                .root
                .join(format!("lock-{}-ready", fixture_process.child.id()));
            while !ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "fixture registry lock deadline"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        fixture_process
    }

    pub(crate) fn stop(&mut self) {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture process exit deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!super::super::ownership::ProcessStamp::from(self.identity).is_live());
    }
}

impl Drop for ProcessScratch {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
}

#[test]
#[ignore = "bounded child process entry point"]
fn process_helper() {
    use std::io::{Read as _, Write as _};
    let root = PathBuf::from(std::env::var("RUNTROL_WORKTREE_FIXTURE").unwrap());
    assert!(root.join("fixture-owner").is_file());
    if std::env::var_os("RUNTROL_WORKTREE_FIXTURE_LEGACY").is_some() {
        let canonical = root.join("home/isolated-workspaces.json");
        let temporary = root.join("home/isolated-workspaces.json.writing");
        let bytes = std::fs::read(&canonical).unwrap();
        let mut file = std::fs::File::create(&temporary).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        drop(file);
        std::fs::write(
            root.join(format!("lock-{}-ready", std::process::id())),
            b"cached",
        )
        .unwrap();
        assert_eq!(std::io::stdin().read(&mut [0]).unwrap(), 0);
        assert!(std::fs::rename(&temporary, &canonical).is_err());
        std::fs::write(&temporary, &bytes).unwrap();
        assert!(std::fs::rename(&temporary, &canonical).is_err());
        return;
    }
    let _held = if std::env::var_os("RUNTROL_WORKTREE_FIXTURE_LOCK").is_some() {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("home/isolated-workspaces.lock"))
            .unwrap();
        file.lock().unwrap();
        std::fs::write(
            root.join(format!("lock-{}-ready", std::process::id())),
            "held",
        )
        .unwrap();
        Some(file)
    } else {
        None
    };
    let mut byte = [0_u8];
    assert_eq!(std::io::stdin().read(&mut byte).unwrap(), 0);
}
