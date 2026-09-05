//! The courier gate: which process may speak on the courier endpoint, and as which managed session.
//!
//! A managed process is born with four environment values ([`runtrol_courier::env`]). This gate hands them out
//! when a terminal is launched, learns the launched root process once it exists, and admits a connection only
//! when three independent authorities agree with the token: the endpoint's owner check has already proved the
//! peer is this user, the kernel says the peer is inside the daemon's containment, and the process tree says
//! the peer is the session's root process or one of its descendants. The token is compared in constant time
//! and exists in exactly two places: this gate's memory and the child's environment.

use core::fmt;
use std::collections::BTreeMap;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_childproc::{Containment, ProcessTree};
use runtrol_courier::env::{
    COURIER_ENDPOINT_ENV, COURIER_EXE_ENV, COURIER_TOKEN_ENV, MANAGED_SESSION_ENV, TOKEN_BYTES,
};
use runtrol_courier::wire::Hello;
use runtrol_courier::{Courier, Limits, ManagedSessionId, PROTOCOL_VERSION};
use runtrol_provider::{ProcessIdentity, TerminalId};
use zeroize::Zeroizing;

pub(crate) mod serve;

#[cfg(test)]
mod tests;

/// One managed session the gate knows: the secret its process was born with, and that process once it exists.
struct Registered {
    token: Zeroizing<[u8; TOKEN_BYTES]>,
    root: Option<ProcessIdentity>,
}

/// A token and the environment for a process about to launch, minted without touching any shared state.
///
/// Held by the launch until the process exists, then handed to [`CourierGate::open_session`]. A launch that
/// fails drops this and leaves the gate exactly as it was: nothing was opened to leak.
pub(crate) struct Minted {
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
    sessions: tokio::sync::Mutex<BTreeMap<ManagedSessionId, Registered>>,
    /// The mailboxes and calls of this generation. Opened and ended with the sessions above.
    courier: tokio::sync::Mutex<Courier>,
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
            sessions: tokio::sync::Mutex::new(BTreeMap::new()),
            courier: tokio::sync::Mutex::new(Courier::new(Limits::INITIAL)),
        }
    }

    /// Where this generation's courier listens.
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Mint the token and environment a process launched for `terminal` is born with, touching no shared state.
    ///
    /// The gate learns nothing yet: a launch that fails after this drops the [`Minted`] and leaves the gate
    /// unchanged. [`Self::open_session`] is what opens the session, once the process exists.
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
            token,
            env: vec![
                (COURIER_EXE_ENV.to_owned(), self.exe.clone()),
                (COURIER_ENDPOINT_ENV.to_owned(), self.endpoint.clone()),
                (COURIER_TOKEN_ENV.to_owned(), spelled),
                (MANAGED_SESSION_ENV.to_owned(), session.to_string()),
            ],
        })
    }

    /// Open the session for `terminal`, now that its process exists: the token admits connections, and the
    /// courier holds its mailbox. A process that could not be identified stays `root: None`, so admission
    /// refuses it (`RootUnbound`) rather than guessing.
    pub(crate) async fn open_session(
        &self,
        terminal: TerminalId,
        minted: Minted,
        root: Option<ProcessIdentity>,
    ) {
        let Ok(session) = session_of(terminal) else {
            return;
        };
        self.sessions.lock().await.insert(
            session,
            Registered {
                token: minted.token,
                root,
            },
        );
        self.courier.lock().await.session_started(session);
    }

    /// The terminal is gone: its token admits nobody, and its mail and calls are released now.
    pub(crate) async fn forget(&self, terminal: TerminalId) {
        let Ok(session) = session_of(terminal) else {
            return;
        };
        let known = self.sessions.lock().await.remove(&session).is_some();
        if known {
            self.courier.lock().await.session_ended(session);
        }
    }

    /// Admit a hello from `peer`, or say exactly why not.
    ///
    /// # Errors
    ///
    /// The first authority that disagrees, in this order: the layout version, the peer's existence, the session,
    /// its bound root, the token, the kernel's containment, then the process tree.
    pub(crate) async fn admit(
        &self,
        containment: &Containment,
        peer: Option<ProcessIdentity>,
        hello: &Hello,
    ) -> Result<ManagedSessionId, Denied> {
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(Denied::Version(hello.protocol_version));
        }
        let peer = peer.ok_or(Denied::NoPeer)?;
        let (expected, root) = {
            let sessions = self.sessions.lock().await;
            let registered = sessions.get(&hello.session).ok_or(Denied::UnknownSession)?;
            (registered.token.clone(), registered.root)
        };
        let root = root.ok_or(Denied::RootUnbound)?;
        let offered =
            Base64UrlUnpadded::decode_vec(&hello.token).map_err(|_malformed| Denied::Token)?;
        if !same_token(&expected, &offered) {
            return Err(Denied::Token);
        }
        match containment.contains(root, peer) {
            Ok(true) => {}
            Ok(false) => return Err(Denied::OutsideContainment),
            Err(error) => return Err(Denied::Unanswered(error.to_string())),
        }
        let tree = ProcessTree::capture().map_err(|error| Denied::Unanswered(error.to_string()))?;
        if !tree.contains_identity(root, peer) {
            return Err(Denied::OutsideTree);
        }
        Ok(hello.session)
    }
}

fn session_of(terminal: TerminalId) -> Result<ManagedSessionId, GateError> {
    let text = terminal.to_string();
    text.parse()
        .map_err(|_not_canonical| GateError::Session(text))
}

/// Equal or not, in time that does not depend on where the two first differ.
fn same_token(expected: &[u8; TOKEN_BYTES], offered: &[u8]) -> bool {
    if offered.len() != TOKEN_BYTES {
        return false;
    }
    let mut difference = 0_u8;
    for (mine, theirs) in expected.iter().zip(offered) {
        difference |= mine ^ theirs;
    }
    difference == 0
}
