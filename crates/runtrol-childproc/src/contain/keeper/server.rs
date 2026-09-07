//! The keeper owns the existing Job termination algorithm and a bounded dynamic scope table.

use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::{AsHandle as _, OwnedHandle};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use super::{
    KeeperCompletion, KeeperIdentity, KeeperLimits, KeeperTarget, Kind, handles, protocol,
};
use crate::SpawnError;
use crate::contain::job::{Job, failure, wait};

struct Scope {
    id: u64,
    kind: Kind,
    job: Arc<Job>,
    stop: Arc<OwnedHandle>,
    done: OwnedHandle,
    root: Option<oneshot::Sender<OwnedHandle>>,
}

async fn lifetime(
    job: Arc<Job>,
    stop: Arc<OwnedHandle>,
    root: oneshot::Receiver<OwnedHandle>,
) -> Result<(), SpawnError> {
    let root = tokio::select! {
        result = wait::signaled(stop.as_handle()) => { result?; None }
        result = root => { Some(result.map_err(|_| failure("retaining suspended root", "binding owner disappeared"))?) }
    };
    if let Some(root) = root.as_ref() {
        tokio::select! {
            result = wait::signaled(root.as_handle()) => result?,
            result = wait::signaled(stop.as_handle()) => result?,
        }
    }
    // Runtime membership authority observes this same event before any termination request.
    handles::signal(stop.as_handle())?;
    job.stopped().await?;
    if let Some(root) = root {
        wait::signaled(root.as_handle()).await?;
    }
    Ok(())
}

struct Server {
    parent: handles::Process,
    outer: Arc<Job>,
    panic: OwnedHandle,
    output: File,
    scopes: Vec<Scope>,
    tasks: JoinSet<(u64, Result<(), SpawnError>)>,
    limits: KeeperLimits,
    next: u64,
    admitting: bool,
    shutdown_ack: bool,
    terminate: bool,
}

impl Server {
    fn stop(&mut self) -> Result<(), SpawnError> {
        self.admitting = false;
        for scope in &self.scopes {
            handles::signal(scope.stop.as_handle())?;
        }
        Ok(())
    }

    fn create(&mut self, kind: Kind) -> Result<(), SpawnError> {
        let capacity = match kind {
            Kind::Terminal => self.limits.terminals,
            Kind::Command => self.limits.commands,
        };
        if self
            .scopes
            .iter()
            .filter(|scope| scope.kind == kind)
            .count()
            >= usize::from(capacity)
        {
            return protocol::answer(&mut self.output, "full");
        }
        let job = Arc::new(Job::new(super::super::TERMINATED_BY_RUNTROL)?);
        let stop = Arc::new(handles::event()?);
        let done = handles::event()?;
        let job_copy = handles::copy(
            job.handle.as_handle(),
            self.parent.handle.as_handle(),
            handles::JOB_ASSIGN_QUERY,
        )?;
        let stop_copy = handles::copy(
            stop.as_handle(),
            self.parent.handle.as_handle(),
            handles::EVENT_MODIFY | handles::SYNCHRONIZE,
        )?;
        let done_copy = handles::copy(
            done.as_handle(),
            self.parent.handle.as_handle(),
            handles::SYNCHRONIZE,
        )?;
        let (sender, receiver) = oneshot::channel();
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| failure("reserving keeper scope", "scope sequence exhausted"))?;
        let task_job = Arc::clone(&job);
        let task_stop = Arc::clone(&stop);
        self.tasks
            .spawn(async move { (id, lifetime(task_job, task_stop, receiver).await) });
        self.scopes.push(Scope {
            id,
            kind,
            job,
            stop,
            done,
            root: Some(sender),
        });
        protocol::answer(
            &mut self.output,
            &format!("scope {id} {job_copy} {stop_copy} {done_copy}"),
        )
    }

    fn bind(&mut self, fields: &mut std::str::SplitWhitespace<'_>) -> Result<(), SpawnError> {
        let id: u64 = protocol::number(fields.next())?;
        let root = super::job_handle(protocol::number(fields.next())?)?;
        let pid: u32 = protocol::number(fields.next())?;
        let birth: u64 = protocol::number(fields.next())?;
        if fields.next().is_some() {
            return Err(failure("binding keeper scope", "extra binding fields"));
        }
        let scope = self
            .scopes
            .iter_mut()
            .find(|scope| scope.id == id)
            .ok_or_else(|| failure("binding keeper scope", "the scope is absent"))?;
        let identity = handles::identity(root.as_handle())?;
        let valid = identity.pid() == pid
            && identity.started() == birth
            && handles::member(root.as_handle(), scope.job.handle.as_handle())?
            && !wait::is_signaled(root.as_handle())?
            && !wait::is_signaled(scope.stop.as_handle())?;
        let sender = scope
            .root
            .take()
            .ok_or_else(|| failure("binding keeper scope", "the scope already has a root"))?;
        sender.send(root).map_err(|_| {
            failure(
                "binding keeper scope",
                "the retained scope has already ended",
            )
        })?;
        if !valid {
            handles::signal(scope.stop.as_handle())?;
        }
        protocol::answer(&mut self.output, if valid { "bound" } else { "refused" })
    }

    fn command(&mut self, frame: &str) -> Result<(), SpawnError> {
        let mut fields = frame.split_whitespace();
        match fields.next() {
            Some("create") => {
                let kind = match fields.next() {
                    Some("terminal") => Kind::Terminal,
                    Some("command") => Kind::Command,
                    _ => {
                        return Err(failure(
                            "reading keeper scope request",
                            "invalid scope class",
                        ));
                    }
                };
                if fields.next().is_some() {
                    return Err(failure(
                        "reading keeper scope request",
                        "extra scope fields",
                    ));
                }
                self.create(kind)
            }
            Some("bind") => self.bind(&mut fields),
            Some("shutdown") if fields.next().is_none() => {
                self.shutdown_ack = true;
                self.stop()
            }
            _ => Err(failure(
                "reading private keeper command",
                "invalid structural command",
            )),
        }
    }

    async fn observe(
        &mut self,
        mut commands: mpsc::Receiver<Result<String, SpawnError>>,
    ) -> Result<(), SpawnError> {
        let mut parent_ended = false;
        loop {
            if !self.admitting && self.scopes.is_empty() {
                if parent_ended || self.terminate {
                    break;
                }
                if self.shutdown_ack {
                    protocol::answer(&mut self.output, "stopped")?;
                    self.shutdown_ack = false;
                }
            }
            tokio::select! {
                result = wait::signaled(self.panic.as_handle()), if !self.terminate => {
                    result?;
                    self.terminate = true;
                    self.stop()?;
                }
                result = wait::signaled(self.parent.handle.as_handle()), if !parent_ended => {
                    result?;
                    parent_ended = true;
                    self.stop()?;
                }
                result = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    let (id, completed) = result.ok_or_else(|| failure("observing keeper scope", "completion task absent"))?
                        .map_err(|error| failure("observing keeper scope", error.to_string()))?;
                    completed?;
                    let index = self.scopes.iter().position(|scope| scope.id == id)
                        .ok_or_else(|| failure("observing keeper scope", "completed scope absent"))?;
                    let scope = self.scopes.swap_remove(index);
                    // Capacity is reusable only after this same exact proof. Client-owned event handles
                    // retain the completion fact even after its bounded server row is retired.
                    handles::signal(scope.done.as_handle())?;
                }
                frame = commands.recv(), if self.admitting => {
                    match frame {
                        Some(Ok(frame)) => self.command(&frame)?,
                        Some(Err(error)) => {
                            eprintln!("runtrol: {error}");
                            self.stop()?;
                        }
                        None => self.stop()?,
                    }
                }
            }
        }
        self.outer.stopped().await
    }
}

pub(super) fn run(
    parent: handles::Process,
    mut input: BufReader<File>,
    mut output: File,
    target: KeeperTarget,
    limits: KeeperLimits,
) -> Result<KeeperCompletion, SpawnError> {
    let me = handles::current()?;
    let owner = KeeperIdentity {
        runtime: parent.identity,
        keeper: me.identity,
    };
    let outer = Arc::new(Job::new(super::super::TERMINATED_BY_RUNTROL)?);
    let copied = handles::copy(
        outer.handle.as_handle(),
        parent.handle.as_handle(),
        handles::JOB_ASSIGN_QUERY,
    )?;
    let panic = handles::event()?;
    let panic_copy = handles::copy(
        panic.as_handle(),
        parent.handle.as_handle(),
        handles::EVENT_MODIFY,
    )?;
    protocol::answer(&mut output, &format!("{copied} {panic_copy}"))?;
    if protocol::line(&mut input)? != "register"
        || !handles::member(parent.handle.as_handle(), outer.handle.as_handle())?
    {
        return Err(failure(
            "registering keeper generation",
            "the exact Runtime is not contained",
        ));
    }
    let (sender, commands) = mpsc::channel(1);
    std::thread::Builder::new()
        .name("keeper-control".to_owned())
        .spawn(move || {
            loop {
                let frame = protocol::line(&mut input);
                let ended = frame.is_err();
                if sender.blocking_send(frame).is_err() || ended {
                    break;
                }
            }
        })
        .map_err(|error| failure("starting keeper control observation", error.to_string()))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .map_err(|error| failure("assembling keeper event waits", error.to_string()))?;
    let mut server = Server {
        parent,
        outer,
        panic,
        output,
        scopes: Vec::with_capacity(limits.total()),
        tasks: JoinSet::new(),
        limits,
        next: 1,
        admitting: true,
        shutdown_ack: false,
        terminate: false,
    };
    // Assembly has completed. This is the existing one-time release, never a recurring trim.
    crate::footprint::release_unused_memory();
    protocol::answer(&mut server.output, "registered")?;
    runtime.block_on(server.observe(commands))?;
    Ok(KeeperCompletion { target, owner })
}
