//! A finite extension of the existing containment fixture, using the shipping keeper APIs.

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use runtrol_childproc::{
    Containment, KeeperLimits, KeeperTarget, PtyChild, PtyContainment, PtySize, PtySpawn,
};
use runtrol_provider::AbsPath;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn run(words: &[String]) -> Option<Result<()>> {
    if let Some(result) = runtrol_childproc::keeper_bootstrap_if_requested(words, |proof| {
        std::fs::write(
            proof.target().directory().join("completed"),
            format!(
                "{} {} {} {}",
                proof.owner().runtime().pid(),
                proof.owner().runtime().started(),
                proof.owner().keeper().pid(),
                proof.owner().keeper().started()
            ),
        )
        .map_err(|error| error.to_string())
    }) {
        return Some(result.map_err(Into::into));
    }
    match words.first().map(String::as_str) {
        Some("--keeper-parent") => Some(parent(words)),
        Some("--keeper-worker") => Some(worker(words)),
        _ => None,
    }
}

fn directory(words: &[String]) -> Result<PathBuf> {
    let path = PathBuf::from(words.get(1).ok_or("fixture directory missing")?);
    if !path.is_absolute() || !path.join("fixture-owner").is_file() {
        return Err("fixture ownership marker absent".into());
    }
    Ok(path)
}

fn worker(words: &[String]) -> Result<()> {
    let directory = directory(words)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("--leaf-until-killed")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    runtrol_childproc::hide_console_window(&mut command);
    let leaf = command.spawn()?;
    std::fs::write(directory.join("leaf"), leaf.id().to_string())?;
    std::thread::sleep(Duration::from_mins(1));
    Ok(())
}

fn parent(words: &[String]) -> Result<()> {
    let directory = directory(words)?;
    let owner = Containment::establish_kept(
        &KeeperTarget::new(directory.clone(), [1; 24])?,
        KeeperLimits::new(8, 2)?,
    )?;
    announce(&owner)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    let program = runtrol_childproc::resolve(
        std::env::current_exe()?
            .to_str()
            .ok_or("fixture image is not UTF-8")?,
    )?;
    let cwd = AbsPath::canonicalize(directory.to_str().ok_or("fixture path is not UTF-8")?)?;
    let mut children = Vec::new();
    for index in 0..8 {
        let child_path = directory.join(format!("terminal-{index}"));
        std::fs::create_dir(&child_path)?;
        std::fs::write(child_path.join("fixture-owner"), b"keeper")?;
        let arguments = [
            "--keeper-worker".to_owned(),
            child_path
                .to_str()
                .ok_or("fixture path is not UTF-8")?
                .to_owned(),
        ];
        let spawn = PtySpawn {
            containment: PtyContainment::Terminal(&owner),
            program: &program,
            arguments: &arguments,
            cwd: &cwd,
            env: &[],
            env_unset: &[],
            size: PtySize { cols: 80, rows: 24 },
        };
        // Production enters its async publication path on a blocking spawn thread before these calls.
        let (child, resume) = runtime.block_on(async { PtyChild::prepare(spawn) })?;
        if child_path.join("leaf").exists() {
            return Err("the child executed before keeper admission".into());
        }
        runtime.block_on(async { child.admit() })?;
        let drain = drain(&child)?;
        resume.resume()?;
        let until = Instant::now() + Duration::from_secs(10);
        while !child_path.join("leaf").is_file() {
            if Instant::now() >= until {
                return Err("the fixture child did not start".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        children.push((child, drain));
    }
    let arguments = ["--exit-with".to_owned(), "0".to_owned()];
    let full = PtySpawn {
        containment: PtyContainment::Terminal(&owner),
        program: &program,
        arguments: &arguments,
        cwd: &cwd,
        env: &[],
        env_unset: &[],
        size: PtySize { cols: 80, rows: 24 },
    };
    if PtyChild::prepare(full).is_ok() {
        return Err("ninth terminal escaped the shared capacity".into());
    }
    let output = runtime.block_on(runtrol_childproc::capture(
        &program,
        &arguments,
        Duration::from_secs(10),
        &owner,
    ))?;
    if !output.succeeded() {
        return Err("a command could not proceed beside eight terminal scopes".into());
    }
    println!("ready");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if line.trim() == "lost" {
        lost(&owner, &runtime, &program, &children)?;
    }
    if line.trim() != "stop" {
        return Err("the finite fixture was not stopped".into());
    }
    runtime.block_on(owner.shutdown_scopes())?;
    for (child, drain) in children {
        runtime.block_on(child.wait())?;
        child.finish();
        drain.join().map_err(|_| "fixture output reader failed")??;
    }
    Ok(())
}

fn drain(child: &PtyChild) -> Result<std::thread::JoinHandle<std::io::Result<()>>> {
    let mut reader = child.reader()?;
    Ok(std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) => return Err(error),
            }
        }
    }))
}

fn announce(owner: &Containment) -> Result<()> {
    let identity = owner.keeper_identity().ok_or("keeper identity absent")?;
    println!(
        "{} {} {} {}",
        identity.runtime().pid(),
        identity.runtime().started(),
        identity.keeper().pid(),
        identity.keeper().started()
    );
    std::io::stdout().flush()?;
    Ok(())
}

fn lost(
    owner: &Containment,
    runtime: &tokio::runtime::Runtime,
    program: &runtrol_childproc::Program,
    children: &[(PtyChild, std::thread::JoinHandle<std::io::Result<()>>)],
) -> Result<()> {
    runtime.block_on(async {
        if tokio::time::timeout(Duration::from_secs(3), owner.keeper_failed())
            .await?
            .is_ok()
        {
            return Err("keeper loss was reported as completion".into());
        }
        if runtrol_childproc::capture(
            program,
            &["--exit-with".to_owned(), "0".to_owned()],
            Duration::from_secs(3),
            owner,
        )
        .await
        .is_ok()
        {
            return Err("keeper loss allowed another command".into());
        }
        let (child, _) = children.first().ok_or("the held child is absent")?;
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await?
            .is_ok()
        {
            return Err("keeper loss fabricated a terminal completion".into());
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })?;
    println!("unconfirmed");
    std::io::stdout().flush()?;
    // Deliberately model a failed generation. The OS closes its retained Jobs, but the external
    // observer must still reject completion because the exact keeper ended unsuccessfully.
    std::process::exit(1)
}
