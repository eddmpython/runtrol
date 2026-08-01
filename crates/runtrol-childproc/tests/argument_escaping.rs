//! A real Windows command file cannot turn one argument into another command.

#[cfg(windows)]
mod windows {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use runtrol_childproc::{check_all, hide_console_window};

    struct Scratch(PathBuf);

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    impl Scratch {
        fn make() -> std::io::Result<Self> {
            let nonce = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-argument-escaping-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("cannot remove test scratch {}: {error}", self.0.display());
            }
        }
    }

    #[test]
    fn shell_metacharacters_stay_inside_one_argument() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::make()?;
        let script = scratch.path().join("receive.cmd");
        let marker = scratch.path().join("injected.txt");
        fs::write(
            &script,
            "@echo off\r\nsetlocal DisableDelayedExpansion\r\necho [%~1]\r\n",
        )?;

        let attacks = [
            format!("safe&echo injected>\"{}\"", marker.display()),
            format!("safe|echo injected>\"{}\"", marker.display()),
            format!("safe>\"{}\"", marker.display()),
            "%PATH%".to_owned(),
            "safe^value".to_owned(),
        ];
        for attack in attacks {
            check_all(&[&attack])?;
            let mut command = Command::new(&script);
            command.arg(&attack);
            hide_console_window(&mut command);
            let output = command.output()?;
            assert!(output.status.success(), "fixture failed for {attack:?}");
            assert!(
                !marker.exists(),
                "the argument escaped command quoting and created {}",
                marker.display()
            );
        }
        Ok(())
    }
}
