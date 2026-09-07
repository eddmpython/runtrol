//! The terminal surface: one hosted CLI on a pseudo terminal, its output fanned out to every viewer, its
//! input gathered from all of them.
//!
//! The conversation surface is the provider's own terminal interface (`docs/terminalSurface.md`). This
//! module owns the one thing that makes that work across a PC tab and a phone at once: the daemon, not a
//! viewer, is the terminal. It answers the questions a CLI asks its terminal at start (see [`xterm`]),
//! keeps the screen so a viewer that attaches late is handed the current picture, and forwards what a
//! viewer typed exactly. Presentation adapters consume host-owned queries before rendering them, so replies
//! cannot be confused with user keys on input. It reads nothing for meaning: bytes go to viewers as the CLI wrote them, and the screen model exists for geometry, not for
//! content.
//!
//! Memory is bounded by construction: the output fan-out is a fixed ring of chunks and the screen model has
//! no scrollback. A viewer that falls behind is told so and re-attached from the screen rather than fed
//! from an ever-growing buffer.
//!
//! One bounded ring feeds raw viewers and a lossless terminal authority. Raw bytes are published before
//! the authority applies them. Only overwriting an unapplied authority chunk causes backpressure; slow
//! viewers still lag independently. The authority owns the single screen and ordered query replies.
//! Checkpoints only read that screen. A lost authority closes input and the exact terminal generation;
//! snapshot-only failure makes checkpoints unavailable without changing the authority or raw output.

#[cfg(test)]
use std::io::Read;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc as blocking_mpsc};

use runtrol_provider::WallMs;
use std::time::Duration;

use bytes::Bytes;
use runtrol_childproc::pty::TerminalRead;
use runtrol_childproc::{Program, PtyChild, PtySize, PtySpawn, SpawnError};
use runtrol_provider::AbsPath;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};

mod completion;
pub use completion::{TerminalCompletion, TerminalFailure};
#[cfg(all(test, windows))]
#[path = "tests/exit.rs"]
mod exit_tests;
pub mod fed;
#[cfg(test)]
#[path = "tests/input.rs"]
mod input_tests;
mod opening;
mod operation;
use operation::{OperationAdmission, OperationGate, OperationReservation};
#[cfg(test)]
#[path = "tests/query_authority.rs"]
mod query_authority_tests;
#[cfg(test)]
#[path = "tests/reader.rs"]
mod reader_tests;
#[cfg(feature = "test-support")]
pub mod test_support;
#[cfg(windows)]
pub use opening::PreparedTerminal;
pub mod xterm;

use fed::{FedChild, FeedError};

/// How many output chunks the fan-out keeps for a slow viewer before it is told it lagged.
///
/// With chunks of at most [`CHUNK_BYTES`], this bounds the ring at 512 KiB per terminal.
const RING_CHUNKS: usize = 128;
/// The largest single read from the terminal, and so the largest chunk in the ring.
const CHUNK_BYTES: usize = 4096;
/// How long the host waits, after a partial read, for the rest of a burst before publishing the chunk.
const COALESCE_WAIT: Duration = Duration::from_millis(1);
/// How often the host looks for more of the burst while waiting.
const COALESCE_STEP: Duration = Duration::from_micros(200);
/// Pending writes retained between the async host and the blocking terminal handle.
const WRITE_QUEUE: usize = 1;
/// One operation may touch the terminal while one more waits in exact arrival order. A third caller is
/// refused before it can become another retained payload. The output host has one separate, structurally
/// bounded query-answer waiter so a terminal protocol answer never competes for public admission.
const TERMINAL_OPERATION_ADMISSIONS: usize = 2;
/// The public terminal input ceiling, applied here too so the central writer queue has a byte bound.
const MAX_WRITE_BYTES: usize = 64 * 1024;
/// A terminal that does not acknowledge one input write is no longer a usable shared terminal. Closing it
/// after this deadline bounds both latency and the lifetime of the blocking writer's byte ownership.
const TERMINAL_WRITE_DEADLINE: Duration = Duration::from_secs(2);
/// Reader and writer loops use shallow fixed frames. Two stacks at this size reserve less than the former
/// platform-default reader stack while keeping blocking terminal handles off the async executor.
const TERMINAL_IO_STACK_BYTES: usize = 256 * 1024;
/// How long an attaching viewer waits for authority progress before taking an unavailable checkpoint.
/// A held screen or a pending query writer can prevent the authority from reaching the current boundary.
const CHECKPOINT_WAIT: Duration = Duration::from_millis(250);
/// How often the exit of the hosted CLI is checked.
#[cfg(not(windows))]
const EXIT_POLL: Duration = Duration::from_millis(100);
/// Retry only an OS inspection failure. Failure never proves process exit or releases admission.
const EXIT_INSPECTION_RETRY: Duration = Duration::from_secs(1);
/// The largest screen width a viewer may ask for.
const MAX_COLS: u16 = 500;
/// The largest screen height before the total-cell ceiling is applied.
const MAX_ROWS: u16 = 200;
/// Maximum cells in one shared screen model.
///
/// The parser keeps a primary and a lazily allocated alternate grid. Both grids, both bounded chunk queues, and
/// their slot metadata are included in [`MAX_SHARED_TERMINAL_STATE_BYTES`].
const MAX_CELLS: u32 = 25_000;

/// Maximum steady heap state owned by one central terminal, excluding the provider process itself.
///
/// This is a release contract, not a description. The structural test below accounts for both screen grids, the
/// reader queue, and the viewer fan-out at their simultaneous maxima. A dependency layout change that no longer
/// fits must reduce another bound instead of silently raising this one.
pub const MAX_SHARED_TERMINAL_STATE_BYTES: usize = 3 * 1024 * 1024;

/// A viewer's size, made safe: at least two cells in either direction and never larger than
/// [`MAX_COLS`] by [`MAX_ROWS`].
///
/// `vt100` 0.16.2 has separate one-column wide-character and one-row wrapping panic paths. The hosted
/// terminal cannot expose those dependency states while the upstream fixes remain unreleased.
#[must_use]
pub fn bounded_size(size: PtySize) -> PtySize {
    let cols = size.cols.clamp(2, MAX_COLS);
    let rows = size.rows.clamp(2, MAX_ROWS);
    let rows = rows.min(u16::try_from(MAX_CELLS / u32::from(cols)).unwrap_or(2));
    PtySize { cols, rows }
}

/// Everything a terminal is opened with. The provider's manifest supplies the arguments and environment.
#[derive(Debug, Clone)]
pub struct TerminalLaunch<'a> {
    /// The Runtime's explicit process owner, or a standalone local lifetime.
    pub containment: Option<&'a runtrol_childproc::Containment>,
    /// The program, already resolved by the probe.
    pub program: &'a Program,
    /// The arguments after the program's own leading ones.
    pub arguments: Vec<String>,
    /// The working directory: the conversation's own folder.
    pub cwd: &'a AbsPath,
    /// Environment set for the CLI (the manifest's `[tui.env]`).
    pub env: Vec<(String, String)>,
    /// Environment removed before `env` applies (the manifest's `[tui] env_unset`).
    pub env_unset: Vec<String>,
    /// The first viewer's size.
    pub size: PtySize,
}

/// Why a terminal could not be opened or driven.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The platform refused the terminal or the child.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// Writing into the terminal failed: the child has gone.
    #[error("the terminal no longer accepts input: {0}")]
    Input(std::io::Error),
    /// Both bounded per-terminal operation slots are occupied.
    #[error("the terminal operation lane is full")]
    Busy,
    /// Control changed before this queued operation reached the terminal.
    #[error("the queued terminal operation was superseded")]
    Superseded,
    /// The runtime this was called from has no task executor.
    #[error("a terminal needs a runtime to watch its child: {0}")]
    Runtime(String),
    /// Host initialization failed, and immediate cleanup could not prove the root process ended.
    ///
    /// The caller must publish this terminal as an owned, failed launch and retain its admission claim
    /// until its exit watcher observes the exact process end. Input and resize are unavailable.
    #[error("terminal initialization failed: {cause}; process cleanup is incomplete: {cleanup}")]
    CleanupIncomplete {
        /// The original initialization failure.
        cause: Box<TerminalError>,
        /// Why termination could not be confirmed without waiting under the caller's admission locks.
        cleanup: SpawnError,
        /// The still-owned root process and its exit watcher.
        terminal: Terminal,
    },
    /// Feeding a terminal whose bytes come from its own process, not from a window.
    #[error("the terminal is not an observed mirror")]
    NotFed,
    /// The observed mirror refused a chunk.
    #[error(transparent)]
    Feed(#[from] FeedError),
}

/// One chunk the CLI wrote, exactly as the host read it, with its place in the terminal's output order.
#[derive(Debug, Clone)]
pub struct OutputChunk {
    /// One-based, monotonic per terminal. The checkpoint a viewer attaches from is the screen after some
    /// sequence `n`; its live chunks begin at `n + 1`.
    pub sequence: u64,
    /// The bytes, exactly as the host read them.
    pub bytes: Bytes,
}

/// What a viewer gets when it attaches: the screen as it is now, then everything after.
#[derive(Debug)]
pub struct Attachment {
    /// The bytes that redraw the current screen on a fresh viewer: the checkpoint at the sequence just before
    /// the first live chunk. Empty when `checkpoint_available` is false.
    pub snapshot: Bytes,
    /// Whether the snapshot is the CLI's current screen. False when the projector was stalled past its
    /// bounded wait, or reset after a panic or a lag and the CLI has not redrawn since (a resize makes it
    /// redraw). The viewer still receives every live chunk from the boundary on.
    pub checkpoint_available: bool,
    /// Every chunk written after the snapshot. `Lagged` means the viewer fell behind the ring; it should
    /// attach again and take a fresh snapshot.
    pub live: broadcast::Receiver<OutputChunk>,
    /// Structural failure and the actual exit code after all completion work has drained.
    pub exited: watch::Receiver<TerminalCompletion>,
}

/// One hosted CLI on one pseudo terminal.
#[derive(Debug, Clone)]
pub struct Terminal {
    shared: Arc<Shared>,
}

/// What is on the other side of the host: a pseudo terminal this process created, or a feed from a window
/// that owns the terminal and observes it. The host asks both the same five things.
#[derive(Debug)]
enum Child {
    Pty(PtyChild),
    Fed(FedChild),
}

/// The bounded reader lane carries accepted bytes followed by at most one operational failure.
type ReadChunk = std::io::Result<Bytes>;

impl Child {
    fn pid(&self) -> u32 {
        match self {
            Self::Pty(child) => child.pid(),
            Self::Fed(child) => child.pid(),
        }
    }

    fn reader(&self) -> Result<Box<dyn TerminalRead>, runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.reader(),
            Self::Fed(child) => child.reader(),
        }
    }

    fn writer(&self) -> Result<Box<dyn Write + Send>, runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.writer(),
            Self::Fed(_) => Ok(FedChild::writer()),
        }
    }

    /// An observed terminal keeps the size its window gave it; asking is not refused, it is simply not ours.
    fn resize(&self, size: PtySize) -> Result<(), runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.resize(size),
            Self::Fed(_) => Ok(()),
        }
    }

    async fn wait(&self) -> Result<i32, runtrol_childproc::SpawnError> {
        match self {
            #[cfg(windows)]
            Self::Pty(child) => child.wait().await,
            #[cfg(not(windows))]
            Self::Pty(child) => loop {
                if let Some(code) = child.try_wait()? {
                    break Ok(code);
                }
                tokio::time::sleep(EXIT_POLL).await;
            },
            Self::Fed(child) => child.wait().await,
        }
    }

    fn kill(&self) -> Result<(), runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.kill(),
            Self::Fed(child) => {
                child.kill();
                Ok(())
            }
        }
    }

    fn finish(&self) {
        match self {
            Self::Pty(child) => child.finish(),
            Self::Fed(child) => child.kill(),
        }
    }
}

struct Shared {
    child: Child,
    #[cfg(feature = "test-support")]
    trace: Option<Arc<test_support::Trace>>,
    /// Failed initialization retains only process ownership and exit observation, never an input lane.
    initialization_failed: bool,
    /// The raw lane's one ordering point: the next sequence to publish, held only while a chunk is sent.
    /// A viewer subscribes under it so its boundary is exact. No projector work ever runs under it.
    publish: Mutex<u64>,
    /// One lossless terminal authority; snapshots only borrow its screen.
    projector: Mutex<Projector>,
    /// Applied sequence and whether this live authority may hold publication at the ring bound.
    projected: watch::Sender<ProjectionProgress>,
    /// Wakes the projector task when a chunk was published.
    published: tokio::sync::Notify,
    /// Input framing is independent from output rendering.
    /// One current terminal operation plus one ordered waiter. Output query answers use the same order lock
    /// but have one separate producer, so public callers cannot crowd them out or create unbounded waiters.
    operations: OperationGate,
    writer: blocking_mpsc::SyncSender<WriteRequest>,
    output: broadcast::Sender<OutputChunk>,
    exited: watch::Sender<TerminalCompletion>,
    /// True only after the reader closed and every accepted chunk reached the raw publication ring.
    output_drained: watch::Sender<bool>,
    finished: AtomicBool,
    /// When this CLI last wrote anything, in unix milliseconds.
    ///
    /// **How many bytes, never which bytes.** The screen model reads the chunk because drawing is what it
    /// is for; this records only that a chunk arrived and when. A conversation held as its CLI's own
    /// terminal publishes no structured turn boundary, so this is the only honest signal that the CLI did
    /// something: it is process state, the same kind of fact as "the child is still running".
    ///
    /// One relaxed store per chunk. Nothing orders anything against it and a reader that is one chunk
    /// behind asks again a moment later.
    wrote_at: AtomicU64,
    /// Current shared PTY geometry packed as columns in the high half and rows in the low half.
    geometry: AtomicU32,
}

struct WriteRequest {
    bytes: Bytes,
    answered: oneshot::Sender<std::io::Result<()>>,
}

/// One bounded, ordered operation on an exact terminal.
///
/// Holding this value isolates a slow terminal from every other terminal while keeping input, resize, and
/// stop ordered for this one process. Only the terminal host constructs it.
pub struct TerminalOperation<'a> {
    shared: &'a Arc<Shared>,
    _admission: OperationAdmission,
}

/// One bounded reservation which has not yet touched the terminal.
pub struct PendingTerminalOperation<'a> {
    shared: &'a Arc<Shared>,
    reservation: OperationReservation<'a>,
}

impl<'a> PendingTerminalOperation<'a> {
    /// Wait for exact terminal ordering after the caller atomically reserves its authority.
    ///
    /// # Errors
    /// Returns [`TerminalError::Superseded`] when control replaces this pending reservation.
    pub async fn wait(self) -> Result<TerminalOperation<'a>, TerminalError> {
        Ok(TerminalOperation {
            shared: self.shared,
            _admission: self.reservation.wait().await?,
        })
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("pid", &self.child.pid())
            .field("exited", &*self.exited.borrow())
            .finish_non_exhaustive()
    }
}

/// The one screen and query authority. Its receiver cannot be overwritten while the terminal is live.
/// A checkpoint borrows this state but never consumes the feed or produces a query answer.
struct Projector {
    screen: vt100::Parser,
    queries: xterm::QueryCarry,
    feed: broadcast::Receiver<OutputChunk>,
    available: bool,
}

#[derive(Clone, Copy)]
struct ProjectionProgress {
    through: u64,
    /// Process exit or control failure releases raw draining even if the authority cannot advance.
    backpressure: bool,
    /// No authority task can discover another failure from a final raw chunk.
    drained: bool,
}

enum ProjectionStep {
    Applied(Vec<u8>),
    Empty,
    Lost,
    End,
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (rows, cols) = self.screen.screen().size();
        f.debug_struct("Projector")
            .field("rows", &rows)
            .field("cols", &cols)
            .field("available", &self.available)
            .finish_non_exhaustive()
    }
}

impl Projector {
    /// Apply one chunk: the screen first, the CLI's questions answered where they stand. The answers owed.
    fn project(&mut self, size: PtySize, chunk: &OutputChunk) -> Option<Vec<u8>> {
        let Self {
            screen,
            queries,
            available,
            ..
        } = self;
        let mut intact = true;
        let answers = queries.answer_in_order(&chunk.bytes, |bytes| {
            if intact && !process_screen_or_reset(screen, size, bytes) {
                intact = false;
                *available = false;
            }
            screen.screen().cursor_position()
        });
        // A reset cursor is never an answer. The caller ends this generation if its state was lost.
        intact.then_some(answers)
    }
}

impl Terminal {
    /// Start the CLI on a fresh terminal and begin hosting it.
    ///
    /// Must be called from within a Tokio runtime: the reader is a thread (the terminal read blocks), and
    /// the exit watcher is a task.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses; [`TerminalError::Runtime`] outside a runtime.
    /// After process birth, an ordinary error proves the owned root has ended. On Windows, the failed
    /// `ConPTY` is closed before returning. [`TerminalError::CleanupIncomplete`] retains the process and its
    /// exit watcher; the caller must retain its admission until that watcher confirms root exit.
    pub fn open(launch: &TerminalLaunch<'_>) -> Result<Self, TerminalError> {
        opening::open(launch, Self::host)
    }

    /// Host a suspended Windows child before it can execute provider code.
    ///
    /// The caller retains admission and [`PreparedTerminal::terminal`] until exact exit is observed.
    /// The process executes only when [`PreparedTerminal::resume`] activates it.
    ///
    /// # Errors
    ///
    /// Reports process creation or host initialization failures with the same ownership as [`Self::open`].
    #[cfg(windows)]
    pub fn prepare(launch: &TerminalLaunch<'_>) -> Result<PreparedTerminal, TerminalError> {
        opening::prepare(launch, Self::host)
    }

    /// Host a terminal some VS Code window owns and observes: the window feeds the raw bytes it captured
    /// through [`Self::feed`] and ends the feed through [`Self::end_feed`]. `pid` is the observed shell's, or
    /// zero when the window could not resolve one. Nothing is spawned.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Runtime`] outside a runtime.
    pub fn fed(pid: u32, size: PtySize) -> Result<Self, TerminalError> {
        Self::host(Child::Fed(FedChild::new(pid)), bounded_size(size))
            .map_err(|failure| *failure.cause)
    }

    fn host(child: Child, size: PtySize) -> Result<Self, opening::FailedHost> {
        let reader = match child.reader() {
            Ok(reader) => reader,
            Err(error) => {
                return Err(opening::FailedHost {
                    child,
                    cause: Box::new(error.into()),
                });
            }
        };
        let writer = match child.writer() {
            Ok(writer) => writer,
            Err(error) => {
                return Err(opening::FailedHost {
                    child,
                    cause: Box::new(error.into()),
                });
            }
        };
        Self::host_with(child, reader, writer, size)
    }

    /// Host a child through the given reader and writer: the seam the writer-fault tests use to put a
    /// short write, a broken pipe, or a lost acknowledgement between the host and a real process.
    fn host_with(
        child: Child,
        reader: Box<dyn TerminalRead>,
        writer: Box<dyn Write + Send>,
        size: PtySize,
    ) -> Result<Self, opening::FailedHost> {
        Self::host_with_reader(child, reader, writer, size, start_reader)
    }

    fn host_with_reader(
        child: Child,
        reader: Box<dyn TerminalRead>,
        writer: Box<dyn Write + Send>,
        size: PtySize,
        start: impl FnOnce(Box<dyn TerminalRead>, mpsc::Sender<ReadChunk>) -> std::io::Result<()>,
    ) -> Result<Self, opening::FailedHost> {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle,
            Err(error) => {
                return Err(opening::FailedHost {
                    child,
                    cause: Box::new(TerminalError::Runtime(error.to_string())),
                });
            }
        };
        let writer = match terminal_writer(writer) {
            Ok(writer) => writer,
            Err(cause) => {
                return Err(opening::FailedHost {
                    child,
                    cause: Box::new(cause),
                });
            }
        };
        let shared = Shared::new(child, size, writer, false);
        #[cfg(feature = "test-support")]
        let reader = test_support::reader(reader, shared.trace.as_ref());
        let (chunks, mut incoming) = mpsc::channel::<ReadChunk>(RING_CHUNKS);
        if let Err(error) = start(reader, chunks) {
            return Err(opening::FailedHost {
                child: shared.child,
                cause: Box::new(TerminalError::Runtime(error.to_string())),
            });
        }
        // Nothing fallible remains after the reader starts. Until now the child could be returned to
        // the opening owner for confirmed cleanup without any shared task having taken it.
        let shared = Arc::new(shared);
        let host = Arc::clone(&shared);
        handle.spawn(async move {
            while let Some(chunk) = incoming.recv().await {
                match chunk {
                    Ok(bytes) => host.take_output(bytes).await,
                    Err(error) => {
                        report_lifetime_failure("reading terminal output", &error);
                        if let Err(error) = host
                            .close_failed_generation(TerminalFailure::OutputReadFailed)
                            .await
                        {
                            report_lifetime_failure("closing an unreadable terminal", &error);
                        }
                    }
                }
            }
            _ = host.output_drained.send_replace(true);
            host.published.notify_one();
        });
        let projector = Arc::clone(&shared);
        handle.spawn(async move { projector.project_forever().await });
        let watcher = Arc::clone(&shared);
        handle.spawn(async move { watcher.watch_exit().await });
        Ok(Self { shared })
    }

    /// The hosted CLI's process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.shared.child.pid()
    }

    /// Exact Windows process membership for caller admission, absent for a terminal owned by another window.
    #[cfg(windows)]
    #[must_use]
    pub fn process_scope(&self) -> Option<runtrol_childproc::contain::ProcessScope> {
        match &self.shared.child {
            Child::Pty(child) => Some(child.process_scope()),
            Child::Fed(_) => None,
        }
    }

    /// Attach a viewer: the current screen, then live output.
    ///
    /// Nothing is added to the screen: the host never switches mouse reporting on toward a viewer, whose
    /// own terminal keeps its selection and wheel (2026-08-29).
    ///
    /// Atomic at one sequence: the checkpoint is the screen after sequence `n` and the live receiver begins at
    /// `n + 1`, whatever the projector had reached when the viewer arrived. A projector that cannot be reached
    /// within [`CHECKPOINT_WAIT`] (stalled inside the screen model) yields an empty, unavailable checkpoint
    /// with the live receiver still exact: the viewer sees everything from its boundary on.
    pub async fn attach(&self) -> Attachment {
        let shared = &self.shared;
        if let Ok(attachment) = tokio::time::timeout(CHECKPOINT_WAIT, shared.checkpoint()).await {
            return attachment;
        }
        shared.unavailable_checkpoint().await
    }

    /// Bytes a viewer typed, delivered exactly as written. A terminal reply and a user key may have identical
    /// bytes, so the host never classifies, carries, or rewrites input.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Input`] when the terminal no longer accepts input.
    pub async fn input(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        self.operation().await?.input(bytes).await
    }

    /// The viewer changed size. The CLI redraws for the new one.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses the size.
    pub async fn resize(&self, size: PtySize) -> Result<(), TerminalError> {
        self.operation().await?.resize(size).await
    }

    /// Reserve this terminal's bounded operation lane.
    ///
    /// Runtime mutations use the guard across their short authority reservation and the exact PTY action, so
    /// they never keep daemon-wide authority state locked during input, resize, or a provider stop.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Busy`] when this terminal already has one current operation and one bounded waiter.
    pub async fn operation(&self) -> Result<TerminalOperation<'_>, TerminalError> {
        self.reserve_operation()?.wait().await
    }

    /// Reserve bounded capacity without awaiting, while the caller holds its authority lock.
    ///
    /// # Errors
    /// Returns [`TerminalError::Busy`] when both public operation slots are occupied.
    pub fn reserve_operation(&self) -> Result<PendingTerminalOperation<'_>, TerminalError> {
        Ok(PendingTerminalOperation {
            shared: &self.shared,
            reservation: self.shared.operations.reserve()?,
        })
    }

    /// Retire queued reservations synchronously when their control authority changes.
    /// An operation already touching the terminal and its protocol query writer retain their order.
    pub fn supersede_pending_operations(&self) {
        self.shared.operations.supersede();
    }

    /// Current shared PTY geometry.
    #[must_use]
    pub fn size(&self) -> PtySize {
        unpack_size(self.shared.geometry.load(Ordering::Acquire))
    }

    /// The exit code, once the CLI has ended.
    #[must_use]
    pub fn exit(&self) -> Option<i32> {
        self.shared.exited.borrow().exit_code
    }

    /// Watch the first structural host failure and the eventual complete process exit.
    #[must_use]
    pub fn exited(&self) -> watch::Receiver<TerminalCompletion> {
        self.shared.exited.subscribe()
    }

    /// End the CLI now, the way closing its window would.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses.
    pub fn kill(&self) -> Result<(), TerminalError> {
        Ok(self.shared.child.kill()?)
    }

    /// One chunk the owner window observed, exactly.
    ///
    /// # Errors
    ///
    /// [`TerminalError::NotFed`] for a terminal with a process of its own; [`TerminalError::Feed`] when the
    /// feed ended or the reader is behind.
    pub fn feed(&self, bytes: Vec<u8>) -> Result<(), TerminalError> {
        match &self.shared.child {
            Child::Fed(child) => Ok(child.feed(bytes)?),
            Child::Pty(_) => Err(TerminalError::NotFed),
        }
    }

    /// The observed command ended, or the owner window stops feeding.
    ///
    /// # Errors
    ///
    /// [`TerminalError::NotFed`] for a terminal with a process of its own.
    pub fn end_feed(&self, exit_code: Option<i32>) -> Result<(), TerminalError> {
        match &self.shared.child {
            Child::Fed(child) => {
                child.end(exit_code);
                Ok(())
            }
            Child::Pty(_) => Err(TerminalError::NotFed),
        }
    }

    /// Let go of the child without ending its process: what an observed mirror does when its window stops
    /// feeding a process the Runtime never started; that process runs on. For a process the Runtime started this
    /// only releases the console handles, which is what exit does.
    pub fn release(&self) {
        self.shared.finish();
        self.shared.child.finish();
    }

    /// When this CLI last wrote anything, or nothing if it has not written yet.
    ///
    /// Output-adjacent bookkeeping can coalesce a burst before asking for fresh structural data. Quiet output
    /// never proves a model turn ended and never authorizes ending the owned process.
    #[must_use]
    pub fn wrote_at(&self) -> Option<WallMs> {
        match self.shared.wrote_at.load(Ordering::Relaxed) {
            0 => None,
            millis => Some(WallMs::from_millis(millis)),
        }
    }

    /// How many viewers are attached right now.
    ///
    /// Every attach subscribes to the output fan-out and every viewer that goes away drops its receiver, so
    /// the fan-out's receiver count, less the projector's own receiver, is exactly the number of windows and
    /// phones watching this terminal. A draining generation may release an unused attachment renderer;
    /// an owned CLI retains its lifetime regardless of viewer count.
    #[must_use]
    pub fn viewer_count(&self) -> usize {
        self.shared.output.receiver_count().saturating_sub(1)
    }
}

impl TerminalOperation<'_> {
    /// Forward one input frame while this terminal's bounded operation lane is held.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Input`] when the terminal rejects or does not acknowledge the input by its deadline.
    pub async fn input(&mut self, bytes: &[u8]) -> Result<(), TerminalError> {
        self.shared.require_initialized()?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.shared
            .write_ordered(Bytes::copy_from_slice(bytes))
            .await
            .map_err(TerminalError::Input)
    }

    /// Resize this exact terminal while its bounded operation lane is held.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses the new geometry.
    pub async fn resize(&mut self, size: PtySize) -> Result<(), TerminalError> {
        self.shared.require_initialized()?;
        let size = bounded_size(size);
        self.shared.child.resize(size)?;
        let restored = {
            let mut projector = self.shared.projector.lock().await;
            let restored = rebuild_screen_for_resize(&mut projector.screen, size);
            projector.available = restored;
            self.shared
                .geometry
                .store(pack_size(size), Ordering::Release);
            restored
        };
        if !restored {
            self.shared.fail_authority().await;
            return Err(TerminalError::Runtime(
                "terminal control state was lost during resize".to_owned(),
            ));
        }
        Ok(())
    }
}

fn terminal_writer(
    mut writer: Box<dyn Write + Send>,
) -> Result<blocking_mpsc::SyncSender<WriteRequest>, TerminalError> {
    let (outbound, incoming) = blocking_mpsc::sync_channel::<WriteRequest>(WRITE_QUEUE);
    std::thread::Builder::new()
        .name("runtrol-terminal-write".to_owned())
        .stack_size(TERMINAL_IO_STACK_BYTES)
        .spawn(move || {
            while let Ok(request) = incoming.recv() {
                let outcome = writer
                    .write_all(&request.bytes)
                    .and_then(|()| writer.flush());
                let failed = outcome.is_err();
                drop(request.answered.send(outcome));
                if failed {
                    break;
                }
            }
        })
        .map_err(|error| TerminalError::Runtime(error.to_string()))?;
    Ok(outbound)
}

fn start_reader(
    reader: Box<dyn TerminalRead>,
    chunks: mpsc::Sender<ReadChunk>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("runtrol-terminal-read".to_owned())
        .stack_size(TERMINAL_IO_STACK_BYTES)
        .spawn(move || read_terminal(reader, &chunks))
        .map(drop)
}

const fn pack_size(size: PtySize) -> u32 {
    (size.cols as u32) << 16 | size.rows as u32
}

fn unpack_size(packed: u32) -> PtySize {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each field was packed from one u16 and masked back to that exact width"
    )]
    PtySize {
        cols: (packed >> 16) as u16,
        rows: (packed & u32::from(u16::MAX)) as u16,
    }
}

fn new_screen(size: PtySize) -> vt100::Parser {
    vt100::Parser::new(size.rows, size.cols, 0)
}

/// Rebuild instead of mutating rows through `Screen::set_size`.
///
/// `vt100` 0.16.2 leaves a dangling wide cell when a resize truncates its continuation. Replaying the
/// bounded formatted screen into a fresh parser preserves the visible state without ever creating that
/// invalid row. Resize is a cold path, so the bounded snapshot allocation does not tax output throughput.
fn rebuild_screen_for_resize(screen: &mut vt100::Parser, size: PtySize) -> bool {
    let mut replacement = new_screen(size);
    let restored = catch_unwind(AssertUnwindSafe(|| {
        let snapshot = screen.screen().state_formatted();
        replacement.process(&snapshot);
    }));
    if restored.is_err() {
        report_screen_reset("resize");
        replacement = new_screen(size);
    }
    *screen = replacement;
    restored.is_ok()
}

/// Whether the screen model took the bytes without a contained panic.
fn process_screen_or_reset(screen: &mut vt100::Parser, size: PtySize, bytes: &[u8]) -> bool {
    mutate_screen_or_reset(screen, size, "output", |screen| {
        screen.process(bytes);
    })
}

fn mutate_screen_or_reset(
    screen: &mut vt100::Parser,
    size: PtySize,
    operation: &str,
    mutate: impl FnOnce(&mut vt100::Parser),
) -> bool {
    if catch_unwind(AssertUnwindSafe(|| mutate(screen))).is_ok() {
        true
    } else {
        report_screen_reset(operation);
        *screen = new_screen(size);
        false
    }
}

fn screen_snapshot(projector: &mut Projector) -> Vec<u8> {
    if let Ok(snapshot) = catch_unwind(AssertUnwindSafe(|| {
        projector.screen.screen().state_formatted()
    })) {
        snapshot
    } else {
        report_screen_reset("snapshot");
        // Serialization never mutated the authoritative parser. Only this checkpoint becomes unavailable.
        projector.available = false;
        Vec::new()
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a contained screen-model panic has no caller or log sink; stderr is the daemon's operational failure channel"
)]
fn report_screen_reset(operation: &str) {
    eprintln!(
        "runtrol: terminal screen model panicked during {operation}; its caller invalidates the affected state"
    );
}

impl Shared {
    fn new(
        child: Child,
        size: PtySize,
        writer: blocking_mpsc::SyncSender<WriteRequest>,
        initialization_failed: bool,
    ) -> Self {
        let (output, _) = broadcast::channel(RING_CHUNKS);
        let (exited, _) = watch::channel(TerminalCompletion {
            exit_code: None,
            failure: initialization_failed.then_some(TerminalFailure::HostInitializationFailed),
        });
        let (output_drained, _) = watch::channel(initialization_failed);
        Self {
            #[cfg(feature = "test-support")]
            trace: test_support::Trace::new(child.pid()),
            child,
            initialization_failed,
            publish: Mutex::new(1),
            projector: Mutex::new(Projector {
                screen: vt100::Parser::new(size.rows, size.cols, 0),
                queries: xterm::QueryCarry::default(),
                feed: output.subscribe(),
                available: !initialization_failed,
            }),
            projected: watch::channel(ProjectionProgress {
                through: 0,
                backpressure: !initialization_failed,
                drained: initialization_failed,
            })
            .0,
            published: tokio::sync::Notify::new(),
            operations: OperationGate::default(),
            writer,
            output,
            exited,
            output_drained,
            finished: AtomicBool::new(false),
            wrote_at: AtomicU64::new(0),
            geometry: AtomicU32::new(pack_size(size)),
        }
    }

    fn require_initialized(&self) -> Result<(), TerminalError> {
        if self.initialization_failed {
            return Err(TerminalError::Runtime(
                "the terminal host did not initialize".to_owned(),
            ));
        }
        Ok(())
    }

    async fn write_ordered(self: &Arc<Self>, bytes: Bytes) -> std::io::Result<()> {
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "terminal input exceeds the central writer byte limit",
            ));
        }
        if self.finished.load(Ordering::Acquire) {
            return Err(terminal_writer_closed());
        }
        let (answered, answer) = oneshot::channel();
        if self
            .writer
            .try_send(WriteRequest { bytes, answered })
            .is_err()
        {
            return Err(self.close_uncertain_writer(&terminal_writer_closed()).await);
        }
        match await_writer_answer(answer, TERMINAL_WRITE_DEADLINE).await {
            Ok(()) => Ok(()),
            // A short write, a broken pipe, or an unanswered write all leave the same question: how much of
            // the input reached the process? Nothing may be written again on top of an unknown, so the
            // terminal ends here with one receipt and no replay (docs/terminalSurface.md, input authority).
            Err(cause) => Err(self.close_uncertain_writer(&cause).await),
        }
    }

    async fn write_query_answer(self: &Arc<Self>, bytes: Bytes) -> std::io::Result<()> {
        // Output has exactly one host task, so this is one bounded internal waiter rather than another public
        // slot. It shares ordering with viewer operations and cannot multiply with the number of viewers.
        let mut projected = self.projected.subscribe();
        let ordered = self.operations.order.lock();
        tokio::pin!(ordered);
        loop {
            if self.finished.load(Ordering::Acquire) {
                return Err(terminal_writer_closed());
            }
            tokio::select! {
                _ordered = &mut ordered => return self.write_ordered(bytes).await,
                changed = projected.changed() => {
                    if changed.is_err() {
                        return Err(terminal_writer_closed());
                    }
                }
            }
        }
    }

    /// End this terminal generation because a write's outcome is unknown. The process is killed and the
    /// host finished, so no later input can land after a partial one.
    async fn close_uncertain_writer(self: &Arc<Self>, cause: &std::io::Error) -> std::io::Error {
        let killed = self
            .close_failed_generation(TerminalFailure::InputDeliveryUnknown)
            .await;
        let message = match killed {
            Ok(()) => format!(
                "terminal input outcome is uncertain ({cause}); the terminal was closed so nothing is written twice"
            ),
            Err(error) => format!(
                "terminal input outcome is uncertain ({cause}); closing the terminal failed: {error}"
            ),
        };
        std::io::Error::new(cause.kind(), message)
    }

    async fn close_failed_generation(
        self: &Arc<Self>,
        failure: TerminalFailure,
    ) -> Result<(), String> {
        self.exited
            .send_modify(|completion| completion.fail(failure));
        self.finish();
        let closing = Arc::clone(self);
        tokio::task::spawn_blocking(move || closing.child.kill())
            .await
            .map_err(|error| format!("terminal stop worker failed: {error}"))?
            .map_err(|error| error.to_string())
    }

    async fn fail_authority(self: &Arc<Self>) {
        if let Err(error) = self
            .close_failed_generation(TerminalFailure::ControlStateLost)
            .await
        {
            report_lifetime_failure("closing a terminal with lost control state", &error);
        }
    }

    /// One chunk the CLI wrote, out to every viewer and the projector alike, exactly as the host read it.
    ///
    /// Publish unchanged before projection, waiting only before an unapplied authority chunk would be
    /// overwritten. Viewers do not participate in this bound. Exit releases it so raw EOF can always drain.
    async fn take_output(self: &Arc<Self>, chunk: Bytes) {
        let mut projected = self.projected.subscribe();
        loop {
            let progress = *projected.borrow_and_update();
            let mut next = self.publish.lock().await;
            if progress.backpressure && next.saturating_sub(progress.through) > RING_CHUNKS as u64 {
                drop(next);
                if projected.changed().await.is_err() {
                    // A missing authority cannot authorize dropping accepted raw bytes. End input and drain.
                    self.fail_authority().await;
                }
                continue;
            }
            let sequence = *next;
            *next = sequence.saturating_add(1);
            #[cfg(feature = "test-support")]
            if let Some(trace) = &self.trace {
                trace.publish(sequence, chunk.len());
            }
            // ok: the projector always holds one receiver, so a send fails only after this terminal is gone.
            drop(self.output.send(OutputChunk {
                sequence,
                bytes: chunk,
            }));
            break;
        }
        self.wrote_at
            .store(WallMs::now().as_millis(), Ordering::Relaxed);
        self.published.notify_one();
    }

    /// A viewer's receiver and its boundary: the sequence of the first chunk it will receive. Taken under the
    /// publication lock so no chunk can fall between the two.
    async fn subscribe(&self) -> (broadcast::Receiver<OutputChunk>, u64) {
        let next = self.publish.lock().await;
        (self.output.subscribe(), *next)
    }

    async fn unavailable_checkpoint(&self) -> Attachment {
        #[cfg(feature = "test-support")]
        let (live, boundary) = self.subscribe().await;
        #[cfg(not(feature = "test-support"))]
        let (live, _) = self.subscribe().await;
        #[cfg(feature = "test-support")]
        if let Some(trace) = &self.trace {
            trace.attached(boundary);
        }
        Attachment {
            snapshot: Bytes::new(),
            checkpoint_available: false,
            live,
            exited: self.exited.subscribe(),
        }
    }

    /// A checkpoint never advances the authority. It waits for an exact boundary or its caller's deadline.
    async fn checkpoint(&self) -> Attachment {
        let mut projected = self.projected.subscribe();
        loop {
            let mut projector = self.projector.lock().await;
            let (live, boundary) = self.subscribe().await;
            let progress = *projected.borrow_and_update();
            if !projector.available || progress.through.saturating_add(1) == boundary {
                let snapshot = if projector.available {
                    screen_snapshot(&mut projector)
                } else {
                    Vec::new()
                };
                #[cfg(feature = "test-support")]
                if let Some(trace) = &self.trace {
                    trace.attached(boundary);
                }
                return Attachment {
                    snapshot: Bytes::from(snapshot),
                    checkpoint_available: projector.available,
                    live,
                    exited: self.exited.subscribe(),
                };
            }
            drop(projector);
            drop(live);
            if projected.changed().await.is_err() {
                return self.unavailable_checkpoint().await;
            }
        }
    }

    /// The projector task: every published chunk into the screen, in order, off the raw path.
    async fn project_forever(self: &Arc<Self>) {
        loop {
            let answers = {
                let mut projector = self.projector.lock().await;
                let size = unpack_size(self.geometry.load(Ordering::Acquire));
                match projector.feed.try_recv() {
                    Ok(chunk) => {
                        if let Some(answers) = projector.project(size, &chunk) {
                            self.projected
                                .send_modify(|progress| progress.through = chunk.sequence);
                            ProjectionStep::Applied(answers)
                        } else {
                            ProjectionStep::Lost
                        }
                    }
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        projector.available = false;
                        if self.finished.load(Ordering::Acquire) {
                            // Exit already released raw backpressure. No later query may be answered.
                            ProjectionStep::End
                        } else {
                            ProjectionStep::Lost
                        }
                    }
                    Err(
                        broadcast::error::TryRecvError::Empty
                        | broadcast::error::TryRecvError::Closed,
                    ) => ProjectionStep::Empty,
                }
            };
            match answers {
                ProjectionStep::Applied(answers) => self.answer(answers).await,
                ProjectionStep::Lost => {
                    self.fail_authority().await;
                    break;
                }
                ProjectionStep::End => break,
                ProjectionStep::Empty if *self.output_drained.borrow() => break,
                ProjectionStep::Empty => self.published.notified().await,
            }
        }
        self.projected
            .send_modify(|progress| progress.drained = true);
    }

    /// The terminal control authority's replies, into the CLI.
    async fn answer(self: &Arc<Self>, answers: Vec<u8>) {
        if answers.is_empty() {
            return;
        }
        // A failed answer means the child is gone; its exit is reported by the watcher, which is the one
        // place that state belongs. ok: nothing downstream waits on this write.
        drop(self.write_query_answer(Bytes::from(answers)).await);
    }

    /// A process exit authorizes console closure; reader EOF proves the last frame was published.
    async fn watch_exit(self: Arc<Self>) {
        let code = loop {
            match self.child.wait().await {
                Ok(code) => break code,
                Err(error) => {
                    report_lifetime_failure("observing process exit", &error);
                    tokio::time::sleep(EXIT_INSPECTION_RETRY).await;
                }
            }
        };
        self.finish();
        // Older Windows versions can block ClosePseudoConsole until the reader drains. Keep the existing
        // reader and publication task running on their own paths while closure runs off the executor.
        loop {
            let closing = Arc::clone(&self);
            match tokio::task::spawn_blocking(move || closing.child.finish()).await {
                Ok(()) => break,
                Err(error) => {
                    report_lifetime_failure("closing the terminal", &error);
                    tokio::time::sleep(EXIT_INSPECTION_RETRY).await;
                }
            }
        }
        let mut drained = self.output_drained.subscribe();
        while !*drained.borrow_and_update() {
            if let Err(error) = drained.changed().await {
                report_lifetime_failure("observing output completion", &error);
                tokio::time::sleep(EXIT_INSPECTION_RETRY).await;
                drained = self.output_drained.subscribe();
            }
        }
        let mut projected = self.projected.subscribe();
        while !projected.borrow_and_update().drained {
            if let Err(error) = projected.changed().await {
                report_lifetime_failure("observing terminal authority completion", &error);
                tokio::time::sleep(EXIT_INSPECTION_RETRY).await;
                projected = self.projected.subscribe();
            }
        }
        // A durable admission bind may subscribe after exit. Preserve the exact final state without receivers.
        self.exited
            .send_modify(|completion| completion.complete(code));
    }

    fn finish(&self) {
        if !self.finished.swap(true, Ordering::SeqCst) {
            // Input closes immediately; the projector continues until the output reader has drained.
            self.projected
                .send_modify(|progress| progress.backpressure = false);
            self.published.notify_one();
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "an unproved process lifetime has no caller; stderr is the operational failure channel"
)]
fn report_lifetime_failure(operation: &str, error: &dyn std::fmt::Display) {
    eprintln!("runtrol: {operation} failed; terminal ownership is retained: {error}");
}

async fn await_writer_answer(
    answer: oneshot::Receiver<std::io::Result<()>>,
    deadline: Duration,
) -> std::io::Result<()> {
    match tokio::time::timeout(deadline, answer).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_closed)) => Err(terminal_writer_closed()),
        Err(_elapsed) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "terminal writer did not acknowledge input before its deadline",
        )),
    }
}

fn terminal_writer_closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "the terminal writer stopped before acknowledging input",
    )
}

/// The reader thread: block on the terminal, hand each chunk to the host task, stop at end of stream.
/// The host's read loop: one blocking read, then whatever is already waiting up to the chunk size, then one
/// publication. Bytes and order are exactly the terminal's; only the read boundary is the host's, and a
/// burst that would have been hundreds of scraps is a few full chunks (docs/terminalSurface.md, raw byte lane).
fn read_terminal(mut reader: Box<dyn TerminalRead>, chunks: &mpsc::Sender<ReadChunk>) {
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        let mut filled = match read_uninterrupted(reader.as_mut(), &mut buffer) {
            Ok(0) => return,
            Ok(n) => n,
            Err(error) => {
                // A dropped host has no failure observer left. Otherwise this follows all accepted chunks.
                drop(chunks.blocking_send(Err(error)));
                return;
            }
        };
        // Collect a burst within one elapsed-time budget. OS queries and scheduling consume that budget
        // too; counting requested sleep durations would keep an already late keystroke echo waiting.
        let collecting = std::time::Instant::now();
        let mut ended = None;
        while filled < CHUNK_BYTES {
            let available = reader.available();
            let remaining = COALESCE_WAIT.saturating_sub(collecting.elapsed());
            if remaining.is_zero() {
                break;
            }
            if available == 0 {
                std::thread::sleep(COALESCE_STEP.min(remaining));
                continue;
            }
            let Some(rest) = buffer.get_mut(filled..) else {
                break;
            };
            match read_uninterrupted(reader.as_mut(), rest) {
                Ok(0) => {
                    ended = Some(Ok(()));
                    break;
                }
                Ok(more) => filled += more,
                Err(error) => {
                    ended = Some(Err(error));
                    break;
                }
            }
        }
        let chunk = Bytes::copy_from_slice(buffer.get(..filled).unwrap_or(&[]));
        if chunks.blocking_send(Ok(chunk)).is_err() {
            return;
        }
        if let Some(outcome) = ended {
            if let Err(error) = outcome {
                // Preserve the partial chunk before reporting the fault that ended its read.
                drop(chunks.blocking_send(Err(error)));
            }
            return;
        }
    }
}

fn read_uninterrupted(reader: &mut dyn TerminalRead, bytes: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(bytes) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            outcome => return outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen survives a saved cursor being restored after the pane shrank.
    ///
    /// Measured 2026-08-29 on the operator's machine: the daemon died twice in one afternoon at
    /// `vt100 0.15.2 screen.rs:977`, `Option::unwrap()` on `None`. A CLI saves its cursor (DECSC), the editor
    /// shrinks the pane, the CLI restores it (DECRC) past the new edge and prints. Every window then lost its
    /// connection at once. vt100 0.16.2 fixed it; this pins that a downgrade turns red here, not in a crash log.
    #[test]
    fn a_cursor_restored_past_a_shrunken_screen_does_not_panic() {
        let mut screen = vt100::Parser::new(24, 80, 0);
        screen.process(b"\x1b[20;70H\x1b7");
        screen.screen_mut().set_size(10, 40);
        screen.process("\x1b8abc \u{AC00}\u{B098}".as_bytes());
        assert_eq!(screen.screen().size(), (10, 40));
    }

    /// The platform shell on a hosted terminal: its echo reaches a viewer that attached before it ran, a
    /// viewer that attaches after sees it on the snapshot, and the exit is reported to both.
    #[test]
    fn sizes_are_bounded_before_they_reach_the_screen_model() {
        assert_eq!(
            bounded_size(PtySize { cols: 0, rows: 0 }),
            PtySize { cols: 2, rows: 2 }
        );
        assert_eq!(
            bounded_size(PtySize {
                cols: 65535,
                rows: 65535
            }),
            PtySize {
                cols: MAX_COLS,
                rows: 50
            }
        );
        assert_eq!(
            bounded_size(PtySize {
                cols: 120,
                rows: 40
            }),
            PtySize {
                cols: 120,
                rows: 40
            }
        );
        assert!(
            u32::from(
                bounded_size(PtySize {
                    cols: MAX_COLS,
                    rows: MAX_ROWS,
                })
                .cols
            ) * u32::from(
                bounded_size(PtySize {
                    cols: MAX_COLS,
                    rows: MAX_ROWS,
                })
                .rows
            ) <= MAX_CELLS
        );
    }

    /// Regression for doy/vt100-rust#28 and the observed `row.rs:89 len=85 index=85` crash.
    ///
    /// The upstream minimal sequence is `Parser::new(2, 4)`, a wide character, `set_size(2, 1)`, then
    /// `CSI K`. The product path clamps the unsafe one-column request and rebuilds rather than calling the
    /// dependency's corrupting resize operation.
    #[test]
    fn a_resize_through_a_wide_character_cannot_leave_a_dangling_cell() {
        let initial = PtySize { cols: 4, rows: 2 };
        let mut screen = new_screen(initial);
        process_screen_or_reset(&mut screen, initial, "你".as_bytes());

        let resized = bounded_size(PtySize { cols: 1, rows: 2 });
        rebuild_screen_for_resize(&mut screen, resized);
        process_screen_or_reset(&mut screen, resized, b"\x1b[K");

        assert_eq!(screen.screen().size(), (resized.rows, resized.cols));
    }

    #[test]
    fn the_upstream_wide_character_resize_panic_is_contained() {
        let initial = PtySize { cols: 4, rows: 2 };
        let safe = bounded_size(PtySize { cols: 1, rows: 2 });
        let mut screen = new_screen(initial);
        screen.process("你".as_bytes());

        let completed = mutate_screen_or_reset(&mut screen, safe, "test resize", |screen| {
            screen.screen_mut().set_size(2, 1);
            screen.process(b"\x1b[K");
        });
        if completed {
            // Once upstream ships the fix, restore the same product invariant without coupling this test to
            // whether the dependency still panics internally.
            rebuild_screen_for_resize(&mut screen, safe);
        }

        assert_eq!(screen.screen().size(), (safe.rows, safe.cols));
    }

    #[test]
    fn an_observed_wide_character_resize_boundary_stays_live() {
        let initial = PtySize { cols: 86, rows: 2 };
        let mut screen = new_screen(initial);
        let mut line = vec![b'x'; 84];
        line.extend_from_slice("你".as_bytes());
        process_screen_or_reset(&mut screen, initial, &line);

        let resized = PtySize { cols: 85, rows: 2 };
        rebuild_screen_for_resize(&mut screen, resized);
        process_screen_or_reset(&mut screen, resized, b"\x1b[K");

        assert_eq!(screen.screen().size(), (resized.rows, resized.cols));
    }

    #[test]
    fn shared_terminal_state_has_a_hard_memory_budget() {
        const SCREEN_GRIDS: usize = 2;
        const CHUNK_QUEUES: usize = 2;
        const MAX_CELL_BYTES: usize = 40;

        let cell_bytes = std::mem::size_of::<vt100::Cell>();
        assert!(
            cell_bytes <= MAX_CELL_BYTES,
            "vt100 cell grew to {cell_bytes} bytes; reduce the screen bound or remeasure the contract"
        );
        let screen_cells = usize::try_from(MAX_CELLS).expect("cell ceiling fits usize")
            * cell_bytes
            * SCREEN_GRIDS;
        let screen_rows =
            usize::from(MAX_ROWS) * std::mem::size_of::<Vec<vt100::Cell>>() * SCREEN_GRIDS;
        let chunk_payloads = RING_CHUNKS * CHUNK_BYTES * CHUNK_QUEUES;
        let chunk_slots = RING_CHUNKS * std::mem::size_of::<Bytes>() * CHUNK_QUEUES;
        // One active payload, one admitted public waiter, and the single output host's query answer may coexist.
        // The sync queue owns the active payload rather than another copy, so only its slot metadata is added.
        let writer_payloads = (TERMINAL_OPERATION_ADMISSIONS + 1) * MAX_WRITE_BYTES;
        let writer_state = writer_payloads + WRITE_QUEUE * std::mem::size_of::<WriteRequest>();
        // The projector reads the shared ring through its own receiver: slot indexes, no payload of its own.
        let fixed_state = std::mem::size_of::<Shared>()
            + std::mem::size_of::<Projector>()
            + std::mem::size_of::<ProjectionProgress>()
            + CHUNK_BYTES;
        let structural_maximum =
            screen_cells + screen_rows + chunk_payloads + chunk_slots + writer_state + fixed_state;
        assert!(
            structural_maximum <= MAX_SHARED_TERMINAL_STATE_BYTES,
            "central terminal state needs {structural_maximum} bytes, over the {MAX_SHARED_TERMINAL_STATE_BYTES} byte contract"
        );
    }

    #[tokio::test]
    async fn a_stalled_terminal_has_one_waiter_and_does_not_block_another_terminal() {
        let stalled = OperationGate::default();
        let independent = OperationGate::default();
        let held = stalled
            .admit()
            .await
            .expect("the first operation is admitted");
        let mut waiting = Box::pin(stalled.admit());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut waiting)
                .await
                .is_err(),
            "the one bounded waiter remains ordered behind the stalled operation"
        );
        assert!(
            matches!(stalled.admit().await, Err(TerminalError::Busy)),
            "a third operation is refused instead of becoming another retained waiter"
        );

        let other = tokio::time::timeout(Duration::from_millis(50), independent.admit())
            .await
            .expect("another terminal is not behind the stalled one")
            .expect("the other terminal has its own admission");
        drop(other);
        drop(held);
        drop(
            tokio::time::timeout(Duration::from_millis(50), waiting)
                .await
                .expect("the bounded waiter advances after release")
                .expect("the waiter retained its admission"),
        );
    }

    #[tokio::test]
    async fn a_missing_writer_acknowledgement_has_a_finite_deadline() {
        let (_answering, answer) = oneshot::channel::<std::io::Result<()>>();
        let error = await_writer_answer(answer, Duration::from_millis(10))
            .await
            .expect_err("an unacknowledged writer must time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn packed_geometry_round_trips_without_crossing_fields() {
        for size in [
            PtySize { cols: 2, rows: 1 },
            PtySize { cols: 80, rows: 24 },
            PtySize {
                cols: MAX_COLS,
                rows: MAX_ROWS,
            },
        ] {
            assert_eq!(unpack_size(pack_size(size)), size);
        }
    }

    #[tokio::test]
    async fn every_chunk_the_host_reads_reaches_a_viewer_byte_for_byte() {
        // The raw lane's promise: what the host read is what a viewer gets, in order, including the
        // sequences the old path rewrote (mouse-mode switches) or answered (terminal queries), and a
        // sequence cut across two reads. ConPTY renders what a child writes, so the fixture enters at the
        // host's read boundary, which is the boundary the promise is about.
        let (shell, arguments) = if cfg!(windows) {
            ("cmd", vec!["/c".to_owned(), "echo raw-lane".to_owned()])
        } else {
            ("sh", vec!["-c".to_owned(), "echo raw-lane".to_owned()])
        };
        let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let terminal = Terminal::open(&TerminalLaunch {
            containment: None,
            program: &program,
            arguments,
            cwd: &cwd,
            env: Vec::new(),
            env_unset: Vec::new(),
            size: PtySize { cols: 40, rows: 10 },
        })
        .expect("a terminal opens");
        let mut viewer = terminal.attach().await;
        let script: [&[u8]; 4] = [
            b"plain \x1b[?10",
            b"00h\x1b[?1006h\x1b[?1049;1000;25h",
            b"\x1b[6n\x1b[c tail",
            b"\x1b[?1000l done",
        ];
        for chunk in script {
            terminal
                .shared
                .take_output(Bytes::copy_from_slice(chunk))
                .await;
        }
        let mut matched = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while matched < script.len() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let received = tokio::time::timeout(remaining, viewer.live.recv())
                .await
                .expect("the viewer sees every fixture chunk in time")
                .expect("the ring stays ahead of one viewer");
            if script.get(matched).copied() == Some(received.bytes.as_ref()) {
                matched += 1;
            }
        }
        assert_eq!(matched, script.len());
        terminal.kill().expect("the fixture child ends");
    }

    #[tokio::test]
    async fn a_hosted_shell_reaches_early_and_late_viewers() {
        let (shell, arguments) = if cfg!(windows) {
            ("cmd", vec!["/c".to_owned(), "echo host-hello".to_owned()])
        } else {
            ("sh", vec!["-c".to_owned(), "echo host-hello".to_owned()])
        };
        let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let terminal = Terminal::open(&TerminalLaunch {
            containment: None,
            program: &program,
            arguments,
            cwd: &cwd,
            env: Vec::new(),
            env_unset: Vec::new(),
            size: PtySize { cols: 80, rows: 24 },
        })
        .expect("the shell opens on a hosted terminal");
        let mut early = terminal.attach().await;
        let mut exited = early.exited.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                exited.changed().await.expect("the exit channel lives");
                if exited.borrow().exit_code.is_some() {
                    break;
                }
            }
        })
        .await
        .expect("the shell exits within the deadline");
        assert_eq!(terminal.exit(), Some(0));
        let mut seen = Vec::new();
        while let Ok(chunk) = early.live.try_recv() {
            seen.extend_from_slice(&chunk.bytes);
        }
        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.contains("host-hello"),
            "the early viewer saw the echo: {text:?}"
        );
        let late = terminal.attach().await;
        let snapshot = String::from_utf8_lossy(&late.snapshot);
        assert!(
            snapshot.contains("host-hello"),
            "the late viewer's snapshot carries the screen: {snapshot:?}"
        );
        assert!(
            !snapshot.contains("\x1b[?1000h"),
            "no mouse reporting is ever switched on toward a viewer: {snapshot:?}"
        );
    }

    /// A hosted shell that reads one line and answers `reply-<line>`.
    fn echo_fixture() -> (Terminal, &'static str) {
        let (shell, arguments, line_end) = if cfg!(windows) {
            (
                "cmd",
                vec![
                    "/q".to_owned(),
                    "/d".to_owned(),
                    "/v:on".to_owned(),
                    "/c".to_owned(),
                    "set /p first=& echo reply-!first!".to_owned(),
                ],
                "\r\n",
            )
        } else {
            (
                "sh",
                vec![
                    "-c".to_owned(),
                    "IFS= read -r first; printf 'reply-%s\\n' \"$first\"".to_owned(),
                ],
                "\n",
            )
        };
        let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let terminal = Terminal::open(&TerminalLaunch {
            containment: None,
            program: &program,
            arguments,
            cwd: &cwd,
            env: Vec::new(),
            env_unset: Vec::new(),
            size: PtySize { cols: 80, rows: 24 },
        })
        .expect("the shell opens on a hosted terminal");
        (terminal, line_end)
    }

    /// Everything the viewer receives until `needle` has appeared, or the deadline.
    async fn live_until(viewer: &mut Attachment, needle: &str, deadline: Duration) -> String {
        let mut seen = Vec::new();
        let until = std::time::Instant::now() + deadline;
        while !String::from_utf8_lossy(&seen).contains(needle) {
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            let Ok(Ok(chunk)) = tokio::time::timeout(remaining, viewer.live.recv()).await else {
                break;
            };
            seen.extend_from_slice(&chunk.bytes);
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    /// Attach until the checkpoint satisfies `accept`, within the deadline.
    async fn checkpoint_until(
        terminal: &Terminal,
        deadline: Duration,
        accept: impl Fn(&Attachment) -> bool,
    ) -> Attachment {
        let until = std::time::Instant::now() + deadline;
        loop {
            let attachment = terminal.attach().await;
            if accept(&attachment) || std::time::Instant::now() >= until {
                return attachment;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_stalled_authority_with_spare_ring_capacity_delays_no_raw_viewer_or_input() {
        let (terminal, line_end) = echo_fixture();
        let mut viewer = terminal.attach().await;
        // Below ring capacity only authority and checkpoint work wait on this lock.
        let stalled = terminal.shared.projector.lock().await;
        terminal
            .shared
            .take_output(Bytes::from_static(b"raw-while-stalled"))
            .await;
        let chunk = tokio::time::timeout(Duration::from_secs(1), viewer.live.recv())
            .await
            .expect("a raw viewer receives bytes before authority work when the ring has space")
            .expect("the ring stays ahead of one viewer");
        assert_eq!(chunk.bytes.as_ref(), b"raw-while-stalled");
        terminal
            .input(format!("hello{line_end}").as_bytes())
            .await
            .expect("input never waits for the projector");
        let echoed = live_until(&mut viewer, "reply-hello", Duration::from_secs(10)).await;
        assert!(
            echoed.contains("reply-hello"),
            "the CLI answered through the raw lane while the projector stood still: {echoed:?}"
        );
        let asked = std::time::Instant::now();
        let late = terminal.attach().await;
        assert!(
            asked.elapsed() < CHECKPOINT_WAIT * 4,
            "a late viewer waits a bounded time for a stalled projector"
        );
        assert!(
            !late.checkpoint_available && late.snapshot.is_empty(),
            "a stalled projector yields an empty checkpoint that says so"
        );
        drop(stalled);
        let recovered = checkpoint_until(&terminal, Duration::from_secs(5), |attachment| {
            attachment.checkpoint_available
                && String::from_utf8_lossy(&attachment.snapshot).contains("reply-hello")
        })
        .await;
        assert!(
            recovered.checkpoint_available,
            "once released, the projector catches up on the ring it never delayed"
        );
        assert!(String::from_utf8_lossy(&recovered.snapshot).contains("reply-hello"));
    }

    #[tokio::test]
    async fn a_lost_query_authority_closes_the_exact_cli_after_raw_publication() {
        let (terminal, _) = echo_fixture();
        let mut viewer = terminal.attach().await;
        // Put the screen model where `vt100` 0.16.2 panics on the next erase (the upstream wide-character bug
        // the resize test contains), so the next chunk is a real panic inside the projector.
        {
            let mut projector = terminal.shared.projector.lock().await;
            projector.screen = new_screen(PtySize { cols: 4, rows: 2 });
            projector.screen.process("你".as_bytes());
            projector.screen.screen_mut().set_size(2, 1);
        }
        terminal
            .shared
            .take_output(Bytes::from_static(b"\x1b[K"))
            .await;
        let chunk = tokio::time::timeout(Duration::from_secs(1), viewer.live.recv())
            .await
            .expect("the raw viewer receives the chunk the projector will panic on")
            .expect("the ring stays ahead of one viewer");
        assert_eq!(
            chunk.bytes.as_ref(),
            b"\x1b[K",
            "the raw lane carried it unchanged"
        );
        let after_fault = checkpoint_until(&terminal, Duration::from_secs(5), |attachment| {
            !attachment.checkpoint_available
        })
        .await;
        assert!(
            !after_fault.checkpoint_available && after_fault.snapshot.is_empty(),
            "the contained panic marks the checkpoint unavailable (vt100 is pinned at 0.16.2; a moved pin needs another contained fault)"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while viewer.exited.borrow().exit_code.is_none() {
                viewer
                    .exited
                    .changed()
                    .await
                    .expect("the exact process exit is observed");
            }
        })
        .await
        .expect("the failed authority retains its child through exact exit and EOF");
        assert!(
            terminal.input(b"later input").await.is_err(),
            "a reset screen cannot invent a cursor for another input"
        );
    }

    /// The number an active-TUI fixture chunk or screen names, as `n=<number>;`.
    fn numbered(text: &str) -> Option<u64> {
        let digits = text.split("n=").nth(1)?.split(';').next()?;
        if digits.is_empty() {
            return None;
        }
        digits.bytes().try_fold(0u64, |value, byte| {
            byte.is_ascii_digit().then_some(())?;
            value
                .checked_mul(10)?
                .checked_add(u64::from(byte.wrapping_sub(b'0')))
        })
    }

    #[tokio::test]
    async fn a_late_viewer_checkpoint_and_live_stream_meet_at_one_sequence_boundary() {
        // An active TUI: a chunk every few hundred microseconds, each overwriting one line with its own number,
        // so a screen names exactly which chunk it reflects. Late viewers keep arriving while it runs; for each,
        // the number on its checkpoint plus one must be the number of its first live chunk. No gap, no duplicate,
        // whatever the projector had reached.
        let (terminal, _) = echo_fixture();
        let publisher = Arc::clone(&terminal.shared);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let writer = tokio::spawn(async move {
            let mut number = 0u64;
            while !stopping.load(Ordering::Acquire) {
                number += 1;
                publisher
                    .take_output(Bytes::from(format!("\r\x1b[Kn={number};")))
                    .await;
                tokio::time::sleep(Duration::from_micros(300)).await;
            }
        });
        let mut checked = 0;
        for _ in 0..40 {
            let mut late = terminal.attach().await;
            assert!(
                late.checkpoint_available,
                "a healthy projector is always reached"
            );
            let mut screen = vt100::Parser::new(24, 80, 0);
            screen.process(&late.snapshot);
            let contents = screen.screen().contents();
            let on_screen = numbered(&contents);
            let first_live = tokio::time::timeout(Duration::from_secs(2), late.live.recv())
                .await
                .expect("the active TUI keeps writing")
                .expect("the ring stays ahead of a fresh viewer");
            let live_number = numbered(&String::from_utf8_lossy(&first_live.bytes))
                .expect("every chunk names its number");
            match on_screen {
                Some(seen) => {
                    assert_eq!(
                        live_number,
                        seen + 1,
                        "the checkpoint ends at {seen} and live output begins right after: {contents:?}"
                    );
                    checked += 1;
                }
                // Before the first chunk, the screen is the shell's own and the first live chunk is number one.
                None => assert_eq!(live_number, 1),
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        stop.store(true, Ordering::Release);
        writer.await.expect("the publisher ends");
        assert!(
            checked >= 30,
            "most late viewers arrived mid-stream: {checked}"
        );
    }

    /// How a faulted writer misbehaves.
    #[derive(Clone, Copy)]
    enum WriterFault {
        /// Accepts three bytes, then reports that nothing more could be written.
        Short,
        /// Refuses the first byte: the other end is gone.
        BrokenPipe,
        /// Takes the bytes and never answers within the host's deadline.
        LostAcknowledgement,
    }

    /// A writer between the host and a real process that fails the way a pipe can. It counts every write call
    /// so a test can see that nothing was tried again after the failure.
    struct FaultyWriter {
        fault: WriterFault,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Write for FaultyWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            match self.fault {
                WriterFault::Short if call == 0 => Ok(buf.len().min(3)),
                WriterFault::Short => Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "the terminal took no more bytes",
                )),
                WriterFault::BrokenPipe => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "the terminal is gone",
                )),
                WriterFault::LostAcknowledgement => {
                    std::thread::sleep(TERMINAL_WRITE_DEADLINE * 3);
                    Ok(buf.len())
                }
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A real shell on a real pseudo terminal, hosted through a writer that will fail.
    fn hosted_through(fault: WriterFault) -> (Terminal, Arc<std::sync::atomic::AtomicUsize>) {
        let (shell, arguments) = if cfg!(windows) {
            (
                "cmd",
                vec![
                    "/q".to_owned(),
                    "/d".to_owned(),
                    "/c".to_owned(),
                    "set /p first=".to_owned(),
                ],
            )
        } else {
            ("sh", vec!["-c".to_owned(), "IFS= read -r first".to_owned()])
        };
        let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let size = PtySize { cols: 80, rows: 24 };
        let child = PtyChild::spawn(PtySpawn {
            containment: runtrol_childproc::PtyContainment::Local,
            program: &program,
            arguments: &arguments,
            cwd: &cwd,
            env: &[],
            env_unset: &[],
            size,
        })
        .expect("the shell spawns on a pseudo terminal");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = child.reader().expect("the terminal reads");
        let writer = Box::new(FaultyWriter {
            fault,
            calls: Arc::clone(&calls),
        });
        let terminal = Terminal::host_with(Child::Pty(child), reader, writer, size)
            .expect("the shell is hosted through the faulty writer");
        (terminal, calls)
    }

    async fn uncertain_write_is_fatal_and_never_replayed(
        fault: WriterFault,
        calls_before_failure: usize,
    ) {
        let (terminal, calls) = hosted_through(fault);
        let mut viewer = terminal.attach().await;
        let refused = terminal
            .input(b"first line\r\n")
            .await
            .expect_err("a write whose outcome is unknown is refused");
        assert!(
            matches!(refused, TerminalError::Input(_)),
            "the refusal names the input path: {refused:?}"
        );
        assert!(
            terminal.shared.finished.load(Ordering::Acquire),
            "the terminal generation ended with the uncertain write"
        );
        let exit = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if viewer.exited.borrow().exit_code.is_some() {
                    return viewer.exited.borrow().exit_code;
                }
                viewer.exited.changed().await.expect("the exit watch lives");
            }
        })
        .await
        .expect("the killed process is reported as exited");
        assert!(exit.is_some());
        assert_eq!(
            viewer.exited.borrow().failure,
            Some(TerminalFailure::InputDeliveryUnknown)
        );
        let again = terminal
            .input(b"second line\r\n")
            .await
            .expect_err("nothing is written into an ended terminal");
        assert!(matches!(again, TerminalError::Input(_)));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            calls_before_failure,
            "no write was tried again after the failure"
        );
    }

    #[tokio::test]
    async fn a_short_write_ends_the_terminal_and_is_never_replayed() {
        uncertain_write_is_fatal_and_never_replayed(WriterFault::Short, 2).await;
    }

    #[tokio::test]
    async fn a_broken_pipe_ends_the_terminal_and_is_never_replayed() {
        uncertain_write_is_fatal_and_never_replayed(WriterFault::BrokenPipe, 1).await;
    }

    #[tokio::test]
    async fn a_lost_acknowledgement_ends_the_terminal_and_is_never_replayed() {
        uncertain_write_is_fatal_and_never_replayed(WriterFault::LostAcknowledgement, 1).await;
    }

    #[tokio::test]
    async fn two_viewers_write_to_the_same_hosted_process() {
        let (shell, arguments, line_end) = if cfg!(windows) {
            (
                "cmd",
                vec![
                    "/q".to_owned(),
                    "/d".to_owned(),
                    "/v:on".to_owned(),
                    "/c".to_owned(),
                    "set /p first=& set /p second=& echo !first!-!second!".to_owned(),
                ],
                "\r\n",
            )
        } else {
            (
                "sh",
                vec![
                    "-c".to_owned(),
                    "IFS= read -r first; IFS= read -r second; printf '%s-%s\\n' \"$first\" \"$second\""
                        .to_owned(),
                ],
                "\n",
            )
        };
        let program = runtrol_childproc::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let first_view = Terminal::open(&TerminalLaunch {
            containment: None,
            program: &program,
            arguments,
            cwd: &cwd,
            env: Vec::new(),
            env_unset: Vec::new(),
            size: PtySize { cols: 80, rows: 24 },
        })
        .expect("the shell opens on a hosted terminal");
        let second_view = first_view.clone();
        let mut attachment = first_view.attach().await;

        first_view
            .input(format!("first{line_end}").as_bytes())
            .await
            .expect("the first viewer writes");
        second_view
            .input(format!("second{line_end}").as_bytes())
            .await
            .expect("the second viewer writes");

        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    received = attachment.live.recv() => {
                        if let Ok(chunk) = received {
                            output.extend_from_slice(&chunk.bytes);
                            if String::from_utf8_lossy(&output).contains("first-second") {
                                break;
                            }
                        }
                    }
                    changed = attachment.exited.changed() => {
                        changed.expect("the exit channel lives");
                        if attachment.exited.borrow().exit_code.is_some() {
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("both inputs are handled within the deadline");
        assert!(
            String::from_utf8_lossy(&output).contains("first-second"),
            "both viewers reached one process: {:?}",
            String::from_utf8_lossy(&output)
        );
    }
}
