//! Bounded filesystem discovery away from the terminal and request executor.

use std::sync::{Arc, LazyLock};

use runtrol_childproc::Program;
use runtrol_provider::Manifest;
use tokio::sync::Semaphore;

use super::{BinFacts, ProbeError, locate};

static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

/// File identities needed to reuse an already resolved program without repeating PATH discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramFacts {
    /// The executable and any interpreted entry point it invokes.
    pub binary: BinFacts,
    /// Launchers whose replacement can redirect the next resolution to another program.
    pub launchers: Vec<BinFacts>,
}

/// Recheck a known program and its launcher chain through the same bounded filesystem admission as discovery.
///
/// This does not resolve candidate names and cannot authorize a new launch in place of full preparation.
///
/// # Errors
///
/// Returns the inspection or file identity error when any required file cannot be examined.
pub async fn inspect_program(program: &Program) -> Result<ProgramFacts, ProbeError> {
    let program = program.clone();
    run(Arc::clone(&SLOTS), move || program_facts(&program)).await
}

fn program_facts(program: &Program) -> Result<ProgramFacts, ProbeError> {
    Ok(ProgramFacts {
        binary: BinFacts::of_program(program)?,
        launchers: program
            .via()
            .iter()
            .map(BinFacts::of)
            .collect::<Result<_, _>>()?,
    })
}

/// Bind the first resolution to a stable launcher observation before the provider can be asked.
fn resolved_facts(program: &Program) -> Result<ProgramFacts, ProbeError> {
    let before = program_facts(program)?;
    if let Some(launcher) = program.via().first() {
        // Revisit only the chain already selected, not PATH. Bracketing this resolution with facts catches
        // replacement both between the first resolve and stat, and while the launcher is being reread.
        let confirmed = runtrol_childproc::resolve(launcher.as_str())?;
        if confirmed.path() != program.path()
            || confirmed.leading() != program.leading()
            || confirmed.via() != program.via()
            || before != program_facts(program)?
        {
            return Err(ProbeError::Inspection {
                detail: "the provider launcher changed while its program was being resolved"
                    .to_owned(),
            });
        }
    }
    Ok(before)
}

/// Resolve a manifest and verify its program identity within the shared filesystem admission.
///
/// This performs no provider launch and grants no permission to reuse a previous preparation.
///
/// # Errors
/// Returns a discovery or identity error when the registered program cannot be proved.
pub async fn inspect_manifest(manifest: &Manifest) -> Result<(Program, ProgramFacts), ProbeError> {
    let manifest = manifest.clone();
    run(Arc::clone(&SLOTS), move || {
        let program = locate(&manifest)?;
        let facts = resolved_facts(&program)?;
        Ok((program, facts))
    })
    .await
}

async fn run<T: Send + 'static>(
    slots: Arc<Semaphore>,
    inspect: impl FnOnce() -> Result<T, ProbeError> + Send + 'static,
) -> Result<T, ProbeError> {
    let permit = slots
        .acquire_owned()
        .await
        .map_err(|error| ProbeError::Inspection {
            detail: error.to_string(),
        })?;
    // Filesystem work cannot be interrupted once it starts. Its slot belongs to the worker, so cancelling a
    // caller never admits replacement work on top of an inspection the operating system has not finished.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        inspect()
    })
    .await
    .map_err(|error| ProbeError::Inspection {
        detail: error.to_string(),
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn known_program_checks_launcher_redirection_even_when_old_binary_remains() {
        let directory =
            std::env::temp_dir().join(format!("runtrol-known-program-{}", std::process::id()));
        std::fs::create_dir(&directory).expect("create owned fixture");
        let executable = directory.join("original.exe");
        std::fs::write(&executable, "unexecuted binary").expect("write original executable");
        std::fs::write(
            directory.join("replacement.exe"),
            "another unexecuted binary",
        )
        .expect("write replacement executable");
        let launcher = directory.join("observer.cmd");
        std::fs::write(
            &launcher,
            "@ECHO off\r\nSET dp0=%~dp0\r\n\"%dp0%\\original.exe\" %*\r\n",
        )
        .expect("write launcher");
        let program = runtrol_childproc::resolve(launcher.to_str().expect("UTF-8 launcher"))
            .expect("resolve launcher");
        assert_eq!(program.via().len(), 1);
        let original = inspect_program(&program)
            .await
            .expect("inspect known program");
        std::fs::write(
            &launcher,
            "@ECHO off\r\nSET dp0=%~dp0\r\n\"%dp0%\\replacement.exe\" %*\r\n",
        )
        .expect("redirect launcher");
        let redirected = inspect_program(&program)
            .await
            .expect("inspect original program after launcher change");
        assert_eq!(
            original.binary, redirected.binary,
            "the original executable remains installed"
        );
        assert_ne!(
            original, redirected,
            "a changed launcher invalidates observer reuse"
        );
        assert!(
            resolved_facts(&program).is_err(),
            "a launcher redirected before the initial stat cannot seed facts for the old program"
        );
        let replacement = runtrol_childproc::resolve(launcher.to_str().expect("UTF-8 launcher"))
            .expect("resolve replacement");
        assert!(resolved_facts(&replacement).is_ok());
        std::fs::remove_file(&launcher).expect("remove launcher");
        assert!(
            inspect_program(&program).await.is_err(),
            "a removed launcher cannot validate the cached observer"
        );
        std::fs::remove_dir_all(&directory).expect("remove owned fixture");
    }

    #[tokio::test]
    async fn cancelling_a_stalled_inspection_keeps_its_slot_and_the_executor_live() {
        let slots = Arc::new(Semaphore::new(1));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let first = tokio::spawn(run(Arc::clone(&slots), move || {
            started.send(()).expect("the test waits for the worker");
            blocked
                .recv()
                .expect("the test releases the filesystem stand-in");
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .expect("blocking discovery leaves the single-thread executor responsive")
            .expect("worker started");
        first.abort();
        assert!(
            first
                .await
                .expect_err("the caller is cancelled")
                .is_cancelled()
        );
        assert_eq!(
            slots.available_permits(),
            0,
            "the running worker still owns admission"
        );
        let (second_started, mut second_ready) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(run(Arc::clone(&slots), move || {
            second_started
                .send(())
                .expect("the second observer remains open");
            Ok(())
        }));
        tokio::task::yield_now().await;
        assert!(matches!(
            second_ready.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).expect("the first worker is alive");
        tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .expect("the completed inspection releases capacity")
            .expect("the second caller remains alive")
            .expect("the second inspection succeeds");
        second_ready
            .await
            .expect("only the finished worker admits its replacement");
    }
}
