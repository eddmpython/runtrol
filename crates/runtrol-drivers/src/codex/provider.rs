//! The provider half: stateless, built once, opens sessions onto one shared daemon.
//!
//! # Why this one holds something and the other does not
//!
//! The CLI that runs a process per session needs no shared state at all. This one is a daemon, so the first
//! session to open starts it and every session after joins it. What is held is a **weak** handle: the sessions
//! own the connection, and when the last of them goes away the process stops on its own. A strong handle here
//! would keep an `app-server` running with nobody attached, which is exactly the stray process the containment
//! design exists to prevent.
//!
//! Starting is serialized by the same lock that reads the handle, so two sessions opening at once produce one
//! daemon rather than two.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{Agent, OpenIntent, Provider, ProviderError, ProviderId};
use tokio::sync::Mutex;

use crate::codex::agent::CodexAgent;
use crate::codex::conn::Connection;

/// What runtrol calls itself in the handshake.
///
/// The provider records it against the conversation. A name is the honest thing to send: a client that hid
/// behind the CLI's own name would make an operator's own thread list unable to say what started a session.
pub const CLIENT_NAME: &str = "runtrol";

/// The driver for the CLI whose sessions share one daemon.
pub struct CodexProvider {
    /// Which provider this is, as its manifest declares it.
    id: ProviderId,
    /// The program to run, already resolved with its launchers unwrapped.
    program: Program,
    /// The containment every child joins.
    contained_by: Arc<Containment>,
    /// The daemon, for as long as any session is on it.
    ///
    /// Weak on purpose. See the module notes.
    shared: Mutex<Weak<Connection>>,
}

impl core::fmt::Debug for CodexProvider {
    /// Prints what identifies the driver. The connection is deliberately absent: whether one exists is a fact
    /// about the sessions that are open, not about the driver.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CodexProvider")
            .field("id", &self.id.as_str())
            .field("program", &self.program.path().as_str())
            .finish_non_exhaustive()
    }
}

impl CodexProvider {
    /// Build the driver.
    ///
    /// Starts nothing. A provider is a way to open sessions, not a session, and the daemon does not exist
    /// until somebody opens one.
    #[must_use]
    pub fn new(id: ProviderId, program: Program, contained_by: Arc<Containment>) -> Self {
        Self {
            id,
            program,
            contained_by,
            shared: Mutex::new(Weak::new()),
        }
    }

    /// The program this driver will run.
    #[must_use]
    pub const fn program(&self) -> &Program {
        &self.program
    }

    /// The daemon, starting it if no session is on one.
    ///
    /// # Errors
    ///
    /// Whatever [`Connection::start`] returns.
    async fn connection(&self) -> Result<Arc<Connection>, ProviderError> {
        // Held across the start on purpose. It is what makes two sessions opening at once produce one daemon,
        // and it is only ever contended while a session is being opened.
        let mut shared = self.shared.lock().await;
        if let Some(live) = shared.upgrade() {
            return Ok(live);
        }

        let started = Arc::new(
            Connection::start(
                self.id,
                &self.program,
                &self.contained_by,
                CLIENT_NAME,
                env!("CARGO_PKG_VERSION"),
            )
            .await?,
        );
        *shared = Arc::downgrade(&started);
        Ok(started)
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        let conn = self.connection().await?;
        let agent = CodexAgent::start(conn, self.id, &intent).await?;
        Ok(Box::new(agent))
    }
}

#[cfg(test)]
mod tests {
    use runtrol_childproc::resolve;

    use super::*;

    fn a_provider_id() -> ProviderId {
        ProviderId::parse("codex").expect("the test's own id must be valid")
    }

    /// The test binary, which is by definition installed: it is running this test.
    fn a_resolved_program() -> Program {
        let exe = std::env::current_exe().expect("a test binary has a path");
        let exe = exe.to_str().expect("the test binary's path is UTF-8");
        resolve(exe).expect("the test binary resolves")
    }

    fn a_driver() -> CodexProvider {
        CodexProvider::new(
            a_provider_id(),
            a_resolved_program(),
            Arc::new(Containment::without_any()),
        )
    }

    #[test]
    fn building_a_driver_starts_nothing() {
        // A build assembles every provider at boot. If constructing one started a daemon, a fresh start would
        // put a process launch in front of the operator's first list, for a provider they may never use.
        let driver = a_driver();
        assert_eq!(driver.id().as_str(), "codex");
        assert!(driver.program().path().as_std_path().exists());
    }

    #[tokio::test]
    async fn no_daemon_exists_until_a_session_asks_for_one() {
        let driver = a_driver();
        assert!(
            driver.shared.lock().await.upgrade().is_none(),
            "a driver that had already started one would be holding a process nobody asked for"
        );
    }

    #[test]
    fn a_driver_can_be_held_without_naming_its_type() {
        // What the kind table hands back. The kernel holds one of these without a line that mentions which
        // CLI it is, which is what makes adding a provider not touch the kernel.
        let held: Box<dyn Provider> = Box::new(a_driver());
        assert_eq!(held.id().as_str(), "codex");
    }

    #[test]
    fn the_driver_does_not_print_a_connection_it_may_not_have() {
        // The realistic leak is a debug format written during an investigation. Whether a daemon exists is a
        // fact about open sessions, and printing a handle here would invite reading one out of the driver.
        let printed = format!("{:?}", a_driver());
        assert!(printed.contains("codex"), "{printed}");
        assert!(!printed.contains("shared"), "{printed}");
    }

    #[tokio::test]
    async fn opening_a_session_against_a_program_that_is_not_the_cli_fails_by_name() {
        // The test binary is a real program that is not this CLI. What is checked is that a failure to open
        // arrives as a named error rather than as a panic or a hang, because that error becomes session state.
        use runtrol_provider::{AbsPath, Disposition, SessionId};

        let driver = a_driver();
        let intent = OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::from_os(&std::env::temp_dir()).expect("the temporary directory"),
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        };

        match driver.open(intent).await {
            Err(error) => {
                let said = error.to_string();
                assert!(
                    said.contains("codex"),
                    "an error has to name its provider: {said}"
                );
            }
            Ok(_) => panic!("a program that is not the CLI must not open a session"),
        }
    }
}
