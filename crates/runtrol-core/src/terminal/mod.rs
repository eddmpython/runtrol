//! The terminal surface: one hosted CLI on a pseudo terminal, its output fanned out to every viewer, its
//! input gathered from all of them.
//!
//! The conversation surface is the provider's own terminal interface (`docs/terminalSurface.md`). This
//! module owns the one thing that makes that work across a PC tab and a phone at once: the daemon, not a
//! viewer, is the terminal. It answers the questions a CLI asks its terminal at start (see [`xterm`]),
//! keeps the screen so a viewer that attaches late is handed the current picture, and forwards what a
//! viewer typed exactly, dropping only the answers it already gave (see [`input`]). It reads nothing for
//! meaning: bytes go to viewers as the CLI wrote them, and the screen model exists for geometry, not for
//! content.
//!
//! Memory is bounded by construction: the output fan-out is a fixed ring of chunks and the screen model has
//! no scrollback. A viewer that falls behind is told so and re-attached from the screen rather than fed
//! from an ever-growing buffer.
//!
//! Three lanes, one ring (`terminalTransportIntegrity`): the raw lane publishes each chunk the host read,
//! exactly and first; the passive checkpoint lane (the projector) reads that same ring afterwards to keep
//! the screen a late viewer starts from; the control lane answers the CLI's terminal questions from that
//! screen. The projector can neither delay nor change what a viewer receives, and its failure leaves the
//! CLI and every viewer live.

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc as blocking_mpsc};

use runtrol_provider::WallMs;
use std::time::Duration;

use bytes::Bytes;
use runtrol_childproc::{MirrorChild, Program, PtyChild, PtySize, PtySpawn, SpawnError};
use runtrol_provider::AbsPath;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, broadcast, mpsc, oneshot, watch};

pub mod input;
pub mod xterm;

/// How many output chunks the fan-out keeps for a slow viewer before it is told it lagged.
///
/// With chunks of at most [`CHUNK_BYTES`], this bounds the ring at 512 KiB per terminal.
const RING_CHUNKS: usize = 128;
/// The largest single read from the terminal, and so the largest chunk in the ring.
const CHUNK_BYTES: usize = 4096;
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
/// How long an attaching viewer waits for the projector before taking an empty, unavailable checkpoint. The
/// projector applies one chunk in microseconds; only a stalled screen model reaches this.
const CHECKPOINT_WAIT: Duration = Duration::from_millis(250);
/// How often the exit of the hosted CLI is checked.
const EXIT_POLL: Duration = Duration::from_millis(100);
/// How long after the exit the terminal is kept so its last frame can drain (measured on Windows: the
/// console host flushes a beat after the client ends, and releasing on the exit itself loses that frame).
const EXIT_SETTLE: Duration = Duration::from_millis(250);
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
    /// The runtime this was called from has no task executor.
    #[error("a terminal needs a runtime to watch its child: {0}")]
    Runtime(String),
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
    /// The exit code once the CLI has ended.
    pub exited: watch::Receiver<Option<i32>>,
}

/// One hosted CLI on one pseudo terminal.
#[derive(Debug, Clone)]
pub struct Terminal {
    shared: Arc<Shared>,
}

/// What is on the other side of the host: a pseudo terminal this process created, or a helper that joined
/// a console some other process owns. The host asks both the same five things.
#[derive(Debug)]
enum Child {
    Pty(PtyChild),
    Mirror(MirrorChild),
}

impl Child {
    fn pid(&self) -> u32 {
        match self {
            Self::Pty(child) => child.pid(),
            Self::Mirror(child) => child.pid(),
        }
    }

    fn reader(&self) -> Result<Box<dyn std::io::Read + Send>, runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.reader(),
            Self::Mirror(child) => child.reader(),
        }
    }

    fn writer(&self) -> Result<Box<dyn Write + Send>, runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.writer(),
            Self::Mirror(child) => child.writer(),
        }
    }

    /// A mirrored console keeps the size its own host gave it; asking is not refused, it is simply not ours.
    fn resize(&self, size: PtySize) -> Result<(), runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.resize(size),
            Self::Mirror(_) => Ok(()),
        }
    }

    fn try_wait(&self) -> Result<Option<i32>, runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.try_wait(),
            Self::Mirror(child) => child.try_wait(),
        }
    }

    fn kill(&self) -> Result<(), runtrol_childproc::SpawnError> {
        match self {
            Self::Pty(child) => child.kill(),
            Self::Mirror(child) => child.kill(),
        }
    }

    fn finish(&self) {
        match self {
            Self::Pty(child) => child.finish(),
            Self::Mirror(child) => child.finish(),
        }
    }
}

struct Shared {
    child: Child,
    /// The raw lane's one ordering point: the next sequence to publish, held only while a chunk is sent.
    /// A viewer subscribes under it so its boundary is exact. No projector work ever runs under it.
    publish: Mutex<u64>,
    /// The passive checkpoint lane, fed from the same ring as every viewer and never on the raw path.
    projector: Mutex<Projector>,
    /// Wakes the projector task when a chunk was published.
    published: tokio::sync::Notify,
    /// Input framing is independent from output rendering.
    input: Mutex<input::InputCarry>,
    /// One current terminal operation plus one ordered waiter. Output query answers use the same order lock
    /// but have one separate producer, so public callers cannot crowd them out or create unbounded waiters.
    operations: OperationGate,
    writer: blocking_mpsc::SyncSender<WriteRequest>,
    output: broadcast::Sender<OutputChunk>,
    exited: watch::Sender<Option<i32>>,
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

struct OperationGate {
    slots: Arc<Semaphore>,
    order: Mutex<()>,
}

struct OperationAdmission<'a> {
    _slot: OwnedSemaphorePermit,
    _ordered: tokio::sync::MutexGuard<'a, ()>,
}

/// One bounded, ordered operation on an exact terminal.
///
/// Holding this value isolates a slow terminal from every other terminal while keeping input, resize, and
/// stop ordered for this one process. Only the terminal host constructs it.
pub struct TerminalOperation<'a> {
    shared: &'a Shared,
    _admission: OperationAdmission<'a>,
}

impl Default for OperationGate {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(TERMINAL_OPERATION_ADMISSIONS)),
            order: Mutex::new(()),
        }
    }
}

impl OperationGate {
    async fn admit(&self) -> Result<OperationAdmission<'_>, TerminalError> {
        let slot = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| TerminalError::Busy)?;
        let ordered = self.order.lock().await;
        Ok(OperationAdmission {
            _slot: slot,
            _ordered: ordered,
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

/// The passive checkpoint lane and the terminal control authority, together because a cursor report is
/// answered from the screen the projector keeps.
///
/// It consumes the same ring every viewer reads, after publication, so it can neither delay nor change what a
/// viewer receives. A panic inside the screen model, or falling a whole ring behind, resets the screen and marks
/// the checkpoint unavailable; the CLI and every raw viewer stay live. The CLI's next full redraw (a resize
/// makes one) brings the checkpoint back.
struct Projector {
    screen: vt100::Parser,
    queries: xterm::QueryCarry,
    feed: broadcast::Receiver<OutputChunk>,
    /// A chunk taken from the feed that lies beyond an attaching viewer's boundary; projected next.
    pending: Option<OutputChunk>,
    /// The sequence the screen reflects.
    processed: u64,
    available: bool,
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (rows, cols) = self.screen.screen().size();
        f.debug_struct("Projector")
            .field("rows", &rows)
            .field("cols", &cols)
            .field("processed", &self.processed)
            .field("available", &self.available)
            .finish_non_exhaustive()
    }
}

impl Projector {
    /// Apply one chunk: the screen first, the CLI's questions answered where they stand. The answers owed.
    fn project(&mut self, size: PtySize, chunk: &OutputChunk) -> Vec<u8> {
        let Self {
            screen,
            queries,
            available,
            ..
        } = self;
        let answers = queries.answer_in_order(&chunk.bytes, |bytes| {
            if !process_screen_or_reset(screen, size, bytes) {
                *available = false;
            }
            screen.screen().cursor_position()
        });
        self.processed = chunk.sequence;
        answers
    }

    /// The next chunk to project, if one is waiting: the one an attach set aside, else the feed's next.
    fn next_chunk(&mut self, size: PtySize) -> Option<OutputChunk> {
        if let Some(chunk) = self.pending.take() {
            return Some(chunk);
        }
        loop {
            match self.feed.try_recv() {
                Ok(chunk) => return Some(chunk),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // A whole ring went by unprojected: this screen no longer describes the CLI's. It restarts
                    // empty and untrusted; the raw viewers never waited for it.
                    self.screen = new_screen(size);
                    self.queries = xterm::QueryCarry::default();
                    self.available = false;
                }
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => {
                    return None;
                }
            }
        }
    }

    /// Bring the screen exactly to the state after sequence `boundary - 1`, leaving later chunks for the task.
    fn project_before(&mut self, size: PtySize, boundary: u64) -> Vec<u8> {
        let mut answers = Vec::new();
        while self.processed + 1 < boundary {
            let Some(chunk) = self.next_chunk(size) else {
                break;
            };
            if chunk.sequence >= boundary {
                self.pending = Some(chunk);
                break;
            }
            answers.extend(self.project(size, &chunk));
        }
        answers
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
    pub fn open(launch: &TerminalLaunch<'_>) -> Result<Self, TerminalError> {
        let size = bounded_size(launch.size);
        let child = PtyChild::spawn(PtySpawn {
            program: launch.program,
            arguments: &launch.arguments,
            cwd: launch.cwd,
            env: &launch.env,
            env_unset: &launch.env_unset,
            size,
        })?;
        Self::host(Child::Pty(child), size)
    }

    /// Join a console some other process owns and host it as if it were ours.
    ///
    /// The session keeps its own process; a helper (`helper` is this executable, answering
    /// `console-mirror`) attaches to that process's console and relays its screen and input. From here on
    /// the terminal is a hosted one: viewers, leases, the sidebar row and Stop all apply. Windows only; on
    /// other platforms the spawn refuses and says why.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the helper cannot start; [`TerminalError::Runtime`] outside a runtime.
    pub fn mirror(
        helper: &std::path::Path,
        target_pid: u32,
        size: PtySize,
    ) -> Result<Self, TerminalError> {
        let child = MirrorChild::spawn(helper, target_pid)?;
        Self::host(Child::Mirror(child), bounded_size(size))
    }

    fn host(child: Child, size: PtySize) -> Result<Self, TerminalError> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|error| TerminalError::Runtime(error.to_string()))?;
        let reader = child.reader()?;
        let writer = terminal_writer(child.writer()?)?;
        let (output, _) = broadcast::channel(RING_CHUNKS);
        let (exited, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            child,
            publish: Mutex::new(1),
            projector: Mutex::new(Projector {
                screen: vt100::Parser::new(size.rows, size.cols, 0),
                queries: xterm::QueryCarry::default(),
                feed: output.subscribe(),
                pending: None,
                processed: 0,
                available: true,
            }),
            published: tokio::sync::Notify::new(),
            input: Mutex::new(input::InputCarry::default()),
            operations: OperationGate::default(),
            writer,
            output,
            exited,
            finished: AtomicBool::new(false),
            wrote_at: AtomicU64::new(0),
            geometry: AtomicU32::new(pack_size(size)),
        });
        let (chunks, mut incoming) = mpsc::channel::<Bytes>(RING_CHUNKS);
        std::thread::Builder::new()
            .name("runtrol-terminal-read".to_owned())
            .stack_size(TERMINAL_IO_STACK_BYTES)
            .spawn(move || read_terminal(reader, &chunks))
            .map_err(|error| TerminalError::Runtime(error.to_string()))?;
        let host = Arc::clone(&shared);
        handle.spawn(async move {
            while let Some(chunk) = incoming.recv().await {
                host.take_output(chunk).await;
            }
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
        let size = unpack_size(shared.geometry.load(Ordering::Acquire));
        let exited = shared.exited.subscribe();
        let Ok(mut projector) =
            tokio::time::timeout(CHECKPOINT_WAIT, shared.projector.lock()).await
        else {
            let (live, _) = shared.subscribe().await;
            return Attachment {
                snapshot: Bytes::new(),
                checkpoint_available: false,
                live,
                exited,
            };
        };
        let (live, boundary) = shared.subscribe().await;
        let answers = projector.project_before(size, boundary);
        let checkpoint_available = projector.available && projector.processed + 1 == boundary;
        let snapshot = if checkpoint_available {
            screen_snapshot_or_reset(&mut projector, size)
        } else {
            Vec::new()
        };
        drop(projector);
        shared.answer(answers).await;
        Attachment {
            snapshot: Bytes::from(snapshot),
            checkpoint_available,
            live,
            exited,
        }
    }

    /// Bytes a viewer typed. They reach the CLI exactly as written; only the terminal answers the viewer's
    /// own terminal sent are dropped (this host already answered).
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
        Ok(TerminalOperation {
            shared: &self.shared,
            _admission: self.shared.operations.admit().await?,
        })
    }

    /// Current shared PTY geometry.
    #[must_use]
    pub fn size(&self) -> PtySize {
        unpack_size(self.shared.geometry.load(Ordering::Acquire))
    }

    /// The exit code, once the CLI has ended.
    #[must_use]
    pub fn exit(&self) -> Option<i32> {
        *self.shared.exited.borrow()
    }

    /// A watch on the exit, for whoever keeps the table of terminals: it changes exactly once.
    #[must_use]
    pub fn exited(&self) -> watch::Receiver<Option<i32>> {
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

    /// Let go of the child without ending its process: what a mirror does when the Runtime stops watching a
    /// process it never started. The helper that relayed the console exits; the mirrored process runs on.
    /// For a process the Runtime started this only releases the console handles, which is what exit does.
    pub fn release(&self) {
        self.shared.finish();
    }

    /// When this CLI last wrote anything, or nothing if it has not written yet.
    ///
    /// What it is for: a conversation held as a terminal has no turn boundary anybody can subscribe to, so
    /// "it was writing and then it stopped" is the only signal that a turn ended. Something that wants to
    /// ask the service a question afterwards asks this rather than a clock, which is the difference
    /// between asking when the answer changed and asking every ninety seconds in case it did.
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
    /// phones watching this terminal. A draining generation reads it to decide it may close a conversation
    /// nobody is looking at.
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
        let forwarded = self.shared.input.lock().await.forward(bytes);
        if forwarded.is_empty() {
            return Ok(());
        }
        self.shared
            .write_ordered(Bytes::from(forwarded))
            .await
            .map_err(TerminalError::Input)
    }

    /// Resize this exact terminal while its bounded operation lane is held.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses the new geometry.
    pub async fn resize(&mut self, size: PtySize) -> Result<(), TerminalError> {
        let size = bounded_size(size);
        self.shared.child.resize(size)?;
        {
            let mut projector = self.shared.projector.lock().await;
            rebuild_screen_for_resize(&mut projector.screen, size);
            // A TUI redraws its whole screen for a new size, so the rebuilt projection is current again.
            projector.available = true;
        }
        self.shared
            .geometry
            .store(pack_size(size), Ordering::Release);
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
fn rebuild_screen_for_resize(screen: &mut vt100::Parser, size: PtySize) {
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

fn screen_snapshot_or_reset(projector: &mut Projector, size: PtySize) -> Vec<u8> {
    if let Ok(snapshot) = catch_unwind(AssertUnwindSafe(|| {
        projector.screen.screen().state_formatted()
    })) {
        snapshot
    } else {
        report_screen_reset("snapshot");
        projector.screen = new_screen(size);
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
        "runtrol: terminal screen model panicked during {operation}; the bounded screen was reset while the CLI and byte relay stayed live"
    );
}

impl Shared {
    async fn write_ordered(&self, bytes: Bytes) -> std::io::Result<()> {
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
        self.writer
            .try_send(WriteRequest { bytes, answered })
            .map_err(|_| terminal_writer_closed())?;
        match await_writer_answer(answer, TERMINAL_WRITE_DEADLINE).await {
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                Err(self.close_stalled_writer())
            }
            outcome => outcome,
        }
    }

    async fn write_query_answer(&self, bytes: Bytes) -> std::io::Result<()> {
        // Output has exactly one host task, so this is one bounded internal waiter rather than another public
        // slot. It shares ordering with viewer operations and cannot multiply with the number of viewers.
        let _ordered = self.operations.order.lock().await;
        self.write_ordered(bytes).await
    }

    fn close_stalled_writer(&self) -> std::io::Error {
        let killed = self.child.kill();
        self.finish();
        let message = match killed {
            Ok(()) => {
                "terminal input exceeded its writer deadline; the stalled terminal was closed"
                    .to_owned()
            }
            Err(error) => format!(
                "terminal input exceeded its writer deadline; closing the stalled terminal failed: {error}"
            ),
        };
        std::io::Error::new(std::io::ErrorKind::TimedOut, message)
    }

    /// One chunk the CLI wrote, out to every viewer and the projector alike, exactly as the host read it.
    ///
    /// The raw lane: a sequence number, one send into the shared ring, a wake for the projector. Nothing is
    /// rewritten and nothing waits for the screen model; a viewer that must keep its own mouse filters that one
    /// control family at its own edge.
    async fn take_output(&self, chunk: Bytes) {
        {
            let mut next = self.publish.lock().await;
            let sequence = *next;
            *next = sequence.saturating_add(1);
            // ok: the projector always holds one receiver, so a send fails only after this terminal is gone.
            drop(self.output.send(OutputChunk {
                sequence,
                bytes: chunk,
            }));
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

    /// The projector task: every published chunk into the screen, in order, off the raw path.
    async fn project_forever(&self) {
        loop {
            let answers = {
                let mut projector = self.projector.lock().await;
                let size = unpack_size(self.geometry.load(Ordering::Acquire));
                projector
                    .next_chunk(size)
                    .map(|chunk| projector.project(size, &chunk))
            };
            if let Some(answers) = answers {
                self.answer(answers).await;
            } else {
                if self.finished.load(Ordering::Acquire) {
                    return;
                }
                self.published.notified().await;
            }
        }
    }

    /// The terminal control authority's replies, into the CLI.
    async fn answer(&self, answers: Vec<u8>) {
        if answers.is_empty() {
            return;
        }
        // A failed answer means the child is gone; its exit is reported by the watcher, which is the one
        // place that state belongs. ok: nothing downstream waits on this write.
        drop(self.write_query_answer(Bytes::from(answers)).await);
    }

    /// Watch the CLI's exit; on exit, let the last frame drain, then release the terminal.
    async fn watch_exit(&self) {
        loop {
            tokio::time::sleep(EXIT_POLL).await;
            match self.child.try_wait() {
                Ok(None) => {}
                Ok(Some(code)) => {
                    tokio::time::sleep(EXIT_SETTLE).await;
                    self.finish();
                    // ok: no receiver means nobody is attached to hear the exit; the value stays in the
                    // channel for whoever attaches next.
                    _ = self.exited.send(Some(code));
                    return;
                }
                Err(error) => {
                    self.finish();
                    // The platform could not say. Reported as an exit with no code rather than left
                    // running forever in every viewer's eyes. ok: a missing receiver is the same as above.
                    _ = self.exited.send(Some(-1));
                    drop(error);
                    return;
                }
            }
        }
    }

    fn finish(&self) {
        if !self.finished.swap(true, Ordering::SeqCst) {
            self.child.finish();
            // The projector task ends when it next wakes and finds nothing to project.
            self.published.notify_one();
        }
    }
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
fn read_terminal(mut reader: Box<dyn Read + Send>, chunks: &mpsc::Sender<Bytes>) {
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let chunk = Bytes::copy_from_slice(buffer.get(..n).unwrap_or(&[]));
                if chunks.blocking_send(chunk).is_err() {
                    return;
                }
            }
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
        let fixed_state = std::mem::size_of::<Projector>() + CHUNK_BYTES;
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
                if exited.borrow().is_some() {
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
    async fn a_stalled_projector_delays_no_raw_viewer_and_no_input() {
        let (terminal, line_end) = echo_fixture();
        let mut viewer = terminal.attach().await;
        // The projector is held: the projector task and every checkpoint wait here, nothing else does.
        let stalled = terminal.shared.projector.lock().await;
        terminal
            .shared
            .take_output(Bytes::from_static(b"raw-while-stalled"))
            .await;
        let chunk = tokio::time::timeout(Duration::from_secs(1), viewer.live.recv())
            .await
            .expect("a raw viewer never waits for the projector")
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
    async fn a_faulting_projector_leaves_the_cli_and_raw_viewers_live() {
        let (terminal, line_end) = echo_fixture();
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
        terminal
            .input(format!("hello{line_end}").as_bytes())
            .await
            .expect("the CLI still takes input after the projector panicked");
        let echoed = live_until(&mut viewer, "reply-hello", Duration::from_secs(10)).await;
        assert!(
            echoed.contains("reply-hello"),
            "the CLI is live and the raw lane still reaches the viewer: {echoed:?}"
        );
        terminal
            .resize(PtySize {
                cols: 100,
                rows: 30,
            })
            .await
            .expect("the terminal still resizes");
        let redrawn = terminal.attach().await;
        assert!(
            redrawn.checkpoint_available,
            "a resize, which makes the CLI redraw, brings the checkpoint back"
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
                        if attachment.exited.borrow().is_some() {
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
