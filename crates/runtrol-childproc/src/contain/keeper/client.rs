//! Synchronous admission runs on the existing spawn lane; completion uses exact kernel events.

#![expect(
    clippy::disallowed_types,
    reason = "the synchronous spawn control never holds its lock across await; Tokio blocking_lock panics when that spawn thread enters async publication"
)]

use std::fs::File;
use std::io::BufReader;
use std::os::windows::io::{AsHandle as _, BorrowedHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use super::{KeeperIdentity, KeeperLimits, KeeperTarget, Kind, handles, protocol};
use crate::SpawnError;
use crate::contain::job::{failure, wait};

const ACK_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct Control {
    input: File,
    replies: mpsc::Receiver<Result<String, SpawnError>>,
}

impl Control {
    fn receive(&self) -> Result<String, SpawnError> {
        self.replies
            .recv_timeout(ACK_DEADLINE)
            .map_err(|error| failure("waiting for keeper admission", error.to_string()))?
    }

    fn request(&mut self, message: &str) -> Result<String, SpawnError> {
        protocol::answer(&mut self.input, message)?;
        self.receive()
    }
}

#[derive(Debug)]
pub(crate) struct Client {
    keeper: handles::Process,
    identity: KeeperIdentity,
    panic: OwnedHandle,
    admitting: AtomicBool,
    control: Mutex<Control>,
}

impl Client {
    pub(crate) fn start(
        target: &KeeperTarget,
        limits: KeeperLimits,
    ) -> Result<(Arc<Self>, OwnedHandle), SpawnError> {
        let parent = handles::current()?;
        let handles::Started {
            keeper,
            mut input,
            output,
        } = handles::start(&parent)?;
        protocol::write_target(&mut input, target, limits)?;
        let (sender, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("keeper-replies".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(output);
                loop {
                    let reply = protocol::line(&mut reader);
                    let ended = reply.is_err();
                    if sender.send(reply).is_err() || ended {
                        break;
                    }
                }
            })
            .map_err(|error| failure("starting keeper reply observation", error.to_string()))?;
        let control = Control { input, replies };
        let hello = control.receive()?;
        let mut fields = hello.split_whitespace();
        let outer = super::job_handle(protocol::number(fields.next())?)?;
        let panic = super::job_handle(protocol::number(fields.next())?)?;
        if fields.next().is_some() {
            return Err(failure("receiving keeper ownership", "extra outer handles"));
        }
        let identity = KeeperIdentity {
            runtime: parent.identity,
            keeper: keeper.identity,
        };
        Ok((
            Arc::new(Self {
                keeper,
                identity,
                panic,
                admitting: AtomicBool::new(true),
                control: Mutex::new(control),
            }),
            outer,
        ))
    }

    pub(crate) fn register(&self) -> Result<(), SpawnError> {
        if self.request("register")? != "registered" {
            self.admitting.store(false, Ordering::Release);
            return Err(failure(
                "registering keeper ownership",
                "registration was refused",
            ));
        }
        Ok(())
    }

    pub(crate) const fn identity(&self) -> KeeperIdentity {
        self.identity
    }

    pub(crate) fn available(&self) -> Result<(), SpawnError> {
        if !self.admitting.load(Ordering::Acquire)
            || wait::is_signaled(self.keeper.handle.as_handle())?
        {
            self.admitting.store(false, Ordering::Release);
            return Err(failure(
                "admitting an owned process",
                "keeper supervision is unavailable; admission is closed",
            ));
        }
        Ok(())
    }

    fn request(&self, message: &str) -> Result<String, SpawnError> {
        self.available()?;
        // Only the blocking spawn lane calls admission. Completion and stop never take this lock.
        let mut control = self.lock_control()?;
        self.available()?;
        match control.request(message) {
            Ok(reply) => Ok(reply),
            Err(error) => {
                self.admitting.store(false, Ordering::Release);
                Err(error)
            }
        }
    }

    fn lock_control(&self) -> Result<MutexGuard<'_, Control>, SpawnError> {
        // The synchronous spawn lane may enter Tokio to publish admission around process birth.
        // This lock never crosses an await and must also work inside that blocking thread's runtime context.
        self.control.lock().map_err(|_| {
            self.admitting.store(false, Ordering::Release);
            failure("locking keeper admission", "the control owner was poisoned")
        })
    }

    pub(crate) fn create(self: &Arc<Self>, kind: Kind) -> Result<(OwnedHandle, Scope), SpawnError> {
        let reply = self.request(match kind {
            Kind::Terminal => "create terminal",
            Kind::Command => "create command",
        })?;
        if reply == "full" {
            return Err(failure(
                "reserving owned process capacity",
                "the scope class is full",
            ));
        }
        let admitted = self.scope(&reply);
        if admitted.is_err() {
            self.admitting.store(false, Ordering::Release);
        }
        admitted
    }

    fn scope(self: &Arc<Self>, reply: &str) -> Result<(OwnedHandle, Scope), SpawnError> {
        let mut fields = reply.split_whitespace();
        if fields.next() != Some("scope") {
            return Err(failure(
                "reserving owned process capacity",
                "invalid admission reply",
            ));
        }
        let id = protocol::number(fields.next())?;
        let job = super::job_handle(protocol::number(fields.next())?)?;
        let stop = super::job_handle(protocol::number(fields.next())?)?;
        let done = super::job_handle(protocol::number(fields.next())?)?;
        if fields.next().is_some() {
            return Err(failure(
                "reserving owned process capacity",
                "extra admission fields",
            ));
        }
        Ok((
            job,
            Scope {
                id,
                client: Arc::clone(self),
                stop,
                done,
            },
        ))
    }

    /// This blocking caller closes admission before asking for completion of all nested scopes.
    pub(crate) fn shutdown(&self) -> Result<(), SpawnError> {
        self.admitting.store(false, Ordering::Release);
        let mut control = self.lock_control()?;
        if control.request("shutdown")? != "stopped" {
            return Err(failure(
                "stopping keeper scopes",
                "completion was not acknowledged",
            ));
        }
        Ok(())
    }

    pub(crate) fn terminate_all(&self) -> Result<(), SpawnError> {
        self.admitting.store(false, Ordering::Release);
        handles::signal(self.panic.as_handle())
    }

    pub(crate) async fn failed(&self) -> Result<(), SpawnError> {
        wait::signaled(self.keeper.handle.as_handle()).await?;
        self.admitting.store(false, Ordering::Release);
        Err(failure(
            "observing keeper supervision",
            "the keeper process ended",
        ))
    }
}

#[derive(Debug)]
pub(crate) struct Scope {
    id: u64,
    client: Arc<Client>,
    stop: OwnedHandle,
    done: OwnedHandle,
}

impl Scope {
    pub(crate) async fn wait_root(&self, root: BorrowedHandle<'_>) -> Result<(), SpawnError> {
        tokio::select! {
            result = wait::signaled(root) => result,
            result = self.client.failed() => result,
        }
    }
    pub(crate) fn admit(&self, root: BorrowedHandle<'_>) -> Result<(), SpawnError> {
        self.client.available()?;
        let identity = handles::identity(root)?;
        let copied = handles::copy(
            root,
            self.client.keeper.handle.as_handle(),
            handles::PROCESS_QUERY_WAIT,
        )?;
        let reply = self.client.request(&format!(
            "bind {} {copied} {} {}",
            self.id,
            identity.pid(),
            identity.started()
        ))?;
        if reply != "bound" {
            self.request_stop()?;
            return Err(failure(
                "binding suspended process ownership",
                "the keeper refused the exact root",
            ));
        }
        self.ready()
    }

    pub(crate) fn ready(&self) -> Result<(), SpawnError> {
        self.client.available()?;
        if self.is_stopping()? {
            return Err(failure(
                "executing a prepared process",
                "its scope is stopping",
            ));
        }
        Ok(())
    }

    pub(crate) fn is_stopping(&self) -> Result<bool, SpawnError> {
        wait::is_signaled(self.stop.as_handle())
    }

    pub(crate) fn request_stop(&self) -> Result<(), SpawnError> {
        handles::signal(self.stop.as_handle())
    }

    pub(crate) fn is_empty(&self) -> Result<bool, SpawnError> {
        if wait::is_signaled(self.done.as_handle())? {
            return Ok(true);
        }
        if wait::is_signaled(self.client.keeper.handle.as_handle())? {
            return Err(failure(
                "checking owned scope completion",
                "the keeper ended without completion proof",
            ));
        }
        Ok(false)
    }

    pub(crate) async fn stopped(&self) -> Result<(), SpawnError> {
        self.request_stop()?;
        tokio::select! {
            result = wait::signaled(self.done.as_handle()) => result,
            result = wait::signaled(self.client.keeper.handle.as_handle()) => {
                result?;
                if wait::is_signaled(self.done.as_handle())? { Ok(()) }
                else { Err(failure("waiting for owned scope completion", "the keeper ended without completion proof")) }
            }
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Err(error) = self.request_stop() {
            eprintln!("runtrol: {error}");
        }
    }
}
