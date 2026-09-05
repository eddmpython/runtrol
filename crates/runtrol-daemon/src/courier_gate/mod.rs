//! The courier gate: which process may speak on the courier endpoint, and as which managed session.
//!
//! A managed process is born with four environment values ([`runtrol_courier::env`]). This gate hands them out
//! when a terminal is launched, learns the launched root process once it exists, and admits a connection only
//! when three independent authorities agree with the token: the endpoint has already proved the pipe client's
//! logon, the kernel says the peer is inside the daemon's containment, and the process tree says
//! the peer is the session's root process or one of its descendants. The token is compared in constant time
//! and exists in exactly two places: this gate's memory and the child's environment.

use core::fmt;
use std::collections::BTreeMap;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_courier::env::{
    COURIER_ENDPOINT_ENV, COURIER_EXE_ENV, COURIER_TOKEN_ENV, MANAGED_SESSION_ENV, TOKEN_BYTES,
};
use runtrol_courier::{Courier, Limits, ManagedSessionId, PROTOCOL_VERSION};
use runtrol_provider::{ProcessIdentity, TerminalId};
use zeroize::Zeroizing;

mod admission;
mod commands;
mod rooms;
pub(crate) mod serve;
mod spawn_command;
mod spawning;

#[cfg(test)]
mod tests;

/// One managed session the gate knows: the secret its process was born with, and that process once it exists.
struct Registered {
    token: Zeroizing<[u8; TOKEN_BYTES]>,
    root: Option<ProcessIdentity>,
    waits: std::sync::Arc<tokio::sync::Semaphore>,
    activation: u64,
    enabled: bool,
    authority: Option<std::sync::Arc<crate::runtime_auth::AuthorizedIntegration>>,
}

/// The exact mailbox lifetime observed while authenticating this connection's hello.
/// A disabled hello proves process identity only and cannot gain command authority after later activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub(crate) session: ManagedSessionId,
    activation: Option<u64>,
}

impl Registered {
    fn admission(&self, session: ManagedSessionId) -> Admitted {
        Admitted {
            session,
            activation: self.enabled.then_some(self.activation),
        }
    }
}

/// A connection can retire only work admitted during its own activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingCall {
    activation: u64,
    call: runtrol_courier::CallRef,
}

pub(crate) enum DialogueFailure<E> {
    Session(&'static str),
    Control(E),
}

/// A token and the environment for a process about to launch, minted without touching any shared state.
///
/// Handed to [`CourierGate::launch`] with the process creation operation. A launch that
/// fails drops this and leaves the gate exactly as it was: nothing was opened to leak.
pub(crate) struct Minted {
    session: ManagedSessionId,
    token: Zeroizing<[u8; TOKEN_BYTES]>,
    env: Vec<(String, String)>,
}

impl Minted {
    /// The environment values the launching process is born with.
    pub(crate) fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

/// The courier gate of one Runtime generation.
pub(crate) struct CourierGate {
    /// The executable a managed process is told to run as its courier command.
    exe: String,
    /// Where this generation's courier listens.
    endpoint: String,
    state: tokio::sync::Mutex<GateState>,
    changed: tokio::sync::Notify,
}

/// Session authority and mailbox lifetime change together under one admission lock.
struct GateState {
    sessions: BTreeMap<ManagedSessionId, Registered>,
    courier: Courier,
    workers: BTreeMap<TerminalId, spawning::Worker>,
}

/// Why a hello was not admitted. Said on the daemon's error stream; the peer hears only that it was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Denied {
    /// The hello speaks another layout.
    Version(u16),
    /// The endpoint did not identify the peer process, so nothing can be proved about it.
    NoPeer,
    /// No managed session has that identifier.
    UnknownSession,
    /// The session's process has not been bound yet, so no tree can vouch for the peer.
    RootUnbound,
    /// The token is not the one that session was born with.
    Token,
    /// The kernel says the peer is outside the daemon's containment.
    OutsideContainment,
    /// The process tree says the peer is neither the session's process nor a descendant of it.
    OutsideTree,
    /// The kernel or the process table would not answer.
    Unanswered(String),
}

impl fmt::Display for Denied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(offered) => write!(
                formatter,
                "it speaks courier layout {offered} where {PROTOCOL_VERSION} is required"
            ),
            Self::NoPeer => formatter.write_str("the endpoint did not identify its process"),
            Self::UnknownSession => formatter.write_str("it names no managed session"),
            Self::RootUnbound => formatter.write_str("its session's process is not known yet"),
            Self::Token => formatter.write_str("its token is not its session's"),
            Self::OutsideContainment => {
                formatter.write_str("the kernel puts it outside the Runtime's containment")
            }
            Self::OutsideTree => formatter.write_str("it is not under its session's process"),
            Self::Unanswered(detail) => write!(formatter, "the system would not say: {detail}"),
        }
    }
}

/// The gate could not mint what a launch needs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GateError {
    /// The operating system gave no randomness.
    #[error("the operating system gave no randomness for a courier token")]
    Randomness,
    /// A terminal identifier that is not a managed session identifier. Cannot happen for minted terminals.
    #[error("terminal {0} is not a managed session identifier")]
    Session(String),
}

impl CourierGate {
    /// A gate whose managed processes run `exe` as their courier and reach it at `endpoint`.
    pub(crate) fn new(exe: String, endpoint: String) -> Self {
        Self {
            exe,
            endpoint,
            state: tokio::sync::Mutex::new(GateState {
                sessions: BTreeMap::new(),
                courier: Courier::new(Limits::INITIAL),
                workers: BTreeMap::new(),
            }),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// Where this generation's courier listens.
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Mint the token and environment a process launched for `terminal` is born with, touching no shared state.
    ///
    /// The gate learns nothing yet: a launch that fails after this drops the [`Minted`] and leaves the gate
    /// unchanged. [`Self::launch`] binds the process and opens its session together.
    ///
    /// # Errors
    ///
    /// [`GateError::Session`] when the terminal is not a managed session identifier, [`GateError::Randomness`]
    /// when no token can be minted; the launch must not proceed without one.
    pub(crate) fn mint(&self, terminal: TerminalId) -> Result<Minted, GateError> {
        let session = session_of(terminal)?;
        let mut token = Zeroizing::new([0_u8; TOKEN_BYTES]);
        getrandom::fill(&mut *token).map_err(|_unavailable| GateError::Randomness)?;
        let spelled = Base64UrlUnpadded::encode_string(&*token);
        Ok(Minted {
            session,
            token,
            env: vec![
                (COURIER_EXE_ENV.to_owned(), self.exe.clone()),
                (COURIER_ENDPOINT_ENV.to_owned(), self.endpoint.clone()),
                (COURIER_TOKEN_ENV.to_owned(), spelled),
                (MANAGED_SESSION_ENV.to_owned(), session.to_string()),
            ],
        })
    }

    /// Launch and register a process as one admission transaction. Its first hello waits for registration,
    /// even if the child's first instruction connects before the launch returns. A failed launch changes
    /// nothing, and there is no cancellation point between launching and registering the live process.
    ///
    /// # Errors
    ///
    /// Returns the launch error unchanged. A root that cannot be identified remains unbound and is refused.
    pub(crate) async fn launch<T, E>(
        &self,
        minted: Minted,
        start: impl FnOnce() -> Result<(T, Option<ProcessIdentity>), E>,
    ) -> Result<T, E> {
        let mut state = self.state.lock().await;
        let (started, root) = start()?;
        state.register(minted, root);
        self.changed.notify_waiters();
        Ok(started)
    }

    /// Local terminal control arms a live session. Disarming retires its entire mailbox lifetime.
    #[cfg(test)]
    pub(crate) async fn set_dialogue(
        &self,
        terminal: TerminalId,
        enabled: bool,
    ) -> Result<(), &'static str> {
        self.set_dialogue_checked(terminal, enabled, None, || Ok(()))
            .await
            .map_err(|error| match error {
                DialogueFailure::Session(message) | DialogueFailure::Control(message) => message,
            })
    }

    /// The caller rechecks terminal authority after the final asynchronous lock and before any state change.
    pub(crate) async fn set_dialogue_checked<E>(
        &self,
        terminal: TerminalId,
        enabled: bool,
        authority: Option<std::sync::Arc<crate::runtime_auth::AuthorizedIntegration>>,
        check: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), DialogueFailure<E>> {
        let session = session_of(terminal)
            .map_err(|_| DialogueFailure::Session("invalid managed terminal"))?;
        let mut state = self.state.lock().await;
        check().map_err(DialogueFailure::Control)?;
        let registered = state
            .sessions
            .get_mut(&session)
            .ok_or(DialogueFailure::Session("the managed session ended"))?;
        if registered.enabled == enabled {
            return Ok(());
        }
        registered.activation =
            registered
                .activation
                .checked_add(1)
                .ok_or(DialogueFailure::Session(
                    "the session exhausted its activation generations",
                ))?;
        registered.enabled = enabled;
        registered.authority = if enabled { authority } else { None };
        registered.waits.close();
        if enabled {
            registered.waits = std::sync::Arc::new(tokio::sync::Semaphore::new(
                runtrol_courier::wire::SESSION_WAIT_SLOTS,
            ));
            state.courier.session_started(session);
        } else {
            state.courier.session_ended(session);
        }
        self.changed.notify_waiters();
        Ok(())
    }

    pub(crate) async fn dialogue_enabled(&self, terminal: TerminalId) -> bool {
        let Ok(session) = session_of(terminal) else {
            return false;
        };
        self.state.lock().await.courier.is_live(session)
    }

    /// The terminal is gone: its token admits nobody, and its mail and calls are released now.
    pub(crate) async fn forget(&self, terminal: TerminalId) {
        let Ok(session) = session_of(terminal) else {
            return;
        };
        let mut state = self.state.lock().await;
        let known = state.sessions.remove(&session).is_some();
        if known {
            state.courier.session_ended(session);
            self.changed.notify_waiters();
        }
    }
}

fn session_of(terminal: TerminalId) -> Result<ManagedSessionId, GateError> {
    let text = terminal.to_string();
    text.parse()
        .map_err(|_not_canonical| GateError::Session(text))
}
