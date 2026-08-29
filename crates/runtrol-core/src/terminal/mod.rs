//! The terminal surface: one hosted CLI on a pseudo terminal, its output fanned out to every viewer, its
//! input gathered from all of them.
//!
//! The conversation surface is the provider's own terminal interface (`docs/terminalSurface.md`). This
//! module owns the one thing that makes that work across a PC tab and a phone at once: the daemon, not a
//! viewer, is the terminal. It answers the questions a CLI asks its terminal at start (see [`xterm`]),
//! keeps the screen so a viewer that attaches late is handed the current picture, and turns a viewer's
//! mouse into keys on that screen (see [`mouse`]). It reads nothing for meaning: bytes go to viewers as the
//! CLI wrote them, and the screen model exists for geometry, not for content.
//!
//! Memory is bounded by construction: the output fan-out is a fixed ring of chunks and the screen model has
//! no scrollback. A viewer that falls behind is told so and re-attached from the screen rather than fed
//! from an ever-growing buffer.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use runtrol_provider::WallMs;
use std::time::Duration;

use bytes::Bytes;
use runtrol_childproc::{MirrorChild, Program, PtyChild, PtySize, PtySpawn, SpawnError};
use runtrol_provider::AbsPath;
use tokio::sync::{Mutex, broadcast, mpsc, watch};

pub mod mouse;
pub mod xterm;

/// How many output chunks the fan-out keeps for a slow viewer before it is told it lagged.
///
/// With chunks of at most [`CHUNK_BYTES`], this bounds the ring at 512 KiB per terminal.
const RING_CHUNKS: usize = 128;
/// The largest single read from the terminal, and so the largest chunk in the ring.
const CHUNK_BYTES: usize = 4096;
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

/// A viewer's size, made safe: never zero in either direction (the screen model panics on an empty
/// screen) and never larger than [`MAX_COLS`] by [`MAX_ROWS`].
#[must_use]
pub fn bounded_size(size: PtySize) -> PtySize {
    let cols = size.cols.clamp(2, MAX_COLS);
    let rows = size.rows.clamp(1, MAX_ROWS);
    let rows = rows.min(u16::try_from(MAX_CELLS / u32::from(cols)).unwrap_or(1));
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
    /// The runtime this was called from has no task executor.
    #[error("a terminal needs a runtime to watch its child: {0}")]
    Runtime(String),
}

/// What a viewer gets when it attaches: the screen as it is now, then everything after.
#[derive(Debug)]
pub struct Attachment {
    /// The bytes that redraw the current screen on a fresh viewer, followed by the viewer-side mouse
    /// enable that makes the viewer report clicks and wheel to this host.
    pub snapshot: Bytes,
    /// Every chunk written after the snapshot. `Lagged` means the viewer fell behind the ring; it should
    /// attach again and take a fresh snapshot.
    pub live: broadcast::Receiver<Bytes>,
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
    /// The screen model, the mouse translator's carry, and the query carry: one lock because every input
    /// and output byte touches the same picture.
    state: Mutex<State>,
    writer: Mutex<Box<dyn Write + Send>>,
    output: broadcast::Sender<Bytes>,
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

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("pid", &self.child.pid())
            .field("exited", &*self.exited.borrow())
            .finish_non_exhaustive()
    }
}

/// What is on the other side of a view.
///
/// A terminal emulator (an editor's xterm.js, a console, Windows Terminal) has its own mouse: it selects on
/// drag and scrolls on wheel, and the host must not report mouse toward it. A touch screen has no keys to
/// send, so the host reports mouse to it and turns each report into keys (`mouse`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerKind {
    /// A terminal emulator with its own mouse.
    Terminal,
    /// A finger on a phone.
    Touch,
}

struct State {
    screen: vt100::Parser,
    queries: xterm::QueryCarry,
    mouse: mouse::InputCarry,
    /// The CLI's mouse-mode switches are taken out of its output here, before the model or a viewer sees it.
    strip: mouse::OutputCarry,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (rows, cols) = self.screen.screen().size();
        f.debug_struct("State")
            .field("rows", &rows)
            .field("cols", &cols)
            .finish_non_exhaustive()
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
        let writer = child.writer()?;
        let (output, _) = broadcast::channel(RING_CHUNKS);
        let (exited, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            child,
            state: Mutex::new(State {
                screen: vt100::Parser::new(size.rows, size.cols, 0),
                queries: xterm::QueryCarry::default(),
                mouse: mouse::InputCarry::default(),
                strip: mouse::OutputCarry::default(),
            }),
            writer: Mutex::new(writer),
            output,
            exited,
            finished: AtomicBool::new(false),
            wrote_at: AtomicU64::new(0),
            geometry: AtomicU32::new(pack_size(size)),
        });
        let (chunks, mut incoming) = mpsc::channel::<Bytes>(RING_CHUNKS);
        std::thread::Builder::new()
            .name("runtrol-terminal-read".to_owned())
            .spawn(move || read_terminal(reader, &chunks))
            .map_err(|error| TerminalError::Runtime(error.to_string()))?;
        let host = Arc::clone(&shared);
        handle.spawn(async move {
            while let Some(chunk) = incoming.recv().await {
                host.take_output(chunk).await;
            }
        });
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
    /// Mouse reporting is switched on toward a touch viewer only. A terminal emulator has its own mouse,
    /// and reporting to it took its drag selection away and turned clicks into keys (2026-08-29).
    pub async fn attach(&self, viewer: ViewerKind) -> Attachment {
        let state = self.shared.state.lock().await;
        let mut snapshot = state.screen.screen().state_formatted();
        if viewer == ViewerKind::Touch {
            snapshot.extend_from_slice(mouse::VIEWER_MOUSE_ON);
        }
        Attachment {
            snapshot: Bytes::from(snapshot),
            live: self.shared.output.subscribe(),
            exited: self.shared.exited.subscribe(),
        }
    }

    /// Bytes a viewer typed. A touch viewer's mouse reports are translated on the screen; terminal answers
    /// the viewer sent on its own are dropped (this host already answered); everything else reaches the
    /// CLI as it is.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Input`] when the terminal no longer accepts input.
    pub async fn input(&self, bytes: &[u8], viewer: ViewerKind) -> Result<(), TerminalError> {
        let forwarded = {
            let mut state = self.shared.state.lock().await;
            let State { screen, mouse, .. } = &mut *state;
            mouse.translate(bytes, screen.screen(), viewer)
        };
        if forwarded.is_empty() {
            return Ok(());
        }
        self.write(&forwarded).await
    }

    /// The viewer changed size. The CLI redraws for the new one.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Spawn`] when the platform refuses the size.
    pub async fn resize(&self, size: PtySize) -> Result<(), TerminalError> {
        let size = bounded_size(size);
        self.shared.child.resize(size)?;
        self.shared
            .state
            .lock()
            .await
            .screen
            .screen_mut()
            .set_size(size.rows, size.cols);
        self.shared
            .geometry
            .store(pack_size(size), Ordering::Release);
        Ok(())
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
    /// the fan-out's receiver count is exactly the number of windows and phones watching this terminal. A
    /// draining generation reads it to decide it may close a conversation nobody is looking at.
    #[must_use]
    pub fn viewer_count(&self) -> usize {
        self.shared.output.receiver_count()
    }

    async fn write(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut writer = self.shared.writer.lock().await;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(TerminalError::Input)
    }
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

impl Shared {
    /// One chunk the CLI wrote: into the screen, answered if it asked anything, and out to every viewer.
    async fn take_output(&self, chunk: Bytes) {
        let (chunk, answers) = {
            let mut state = self.state.lock().await;
            // The mouse is a touch-screen concept here (`mouse`): a CLI switching a terminal's mouse on
            // never reaches the model or a viewer.
            let chunk = Bytes::from(state.strip.strip(&chunk));
            state.screen.process(&chunk);
            let cursor = state.screen.screen().cursor_position();
            let answers = state.queries.answers(&chunk, cursor);
            (chunk, answers)
        };
        if !answers.is_empty() {
            let mut writer = self.writer.lock().await;
            // A failed answer means the child is gone; its exit is reported by the watcher, which is the
            // one place that state belongs. ok: nothing downstream waits on this write.
            drop(writer.write_all(&answers).and_then(|()| writer.flush()));
        }
        self.wrote_at
            .store(WallMs::now().as_millis(), Ordering::Relaxed);
        // ok: no receiver means no viewer is attached right now; the ring keeps nothing for nobody and the
        // screen model already holds what a later viewer needs.
        drop(self.output.send(chunk));
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
        }
    }
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
            PtySize { cols: 2, rows: 1 }
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
        let fixed_state = std::mem::size_of::<State>() + CHUNK_BYTES;
        let structural_maximum =
            screen_cells + screen_rows + chunk_payloads + chunk_slots + fixed_state;
        assert!(
            structural_maximum <= MAX_SHARED_TERMINAL_STATE_BYTES,
            "central terminal state needs {structural_maximum} bytes, over the {MAX_SHARED_TERMINAL_STATE_BYTES} byte contract"
        );
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
        let mut early = terminal.attach(ViewerKind::Terminal).await;
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
            seen.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.contains("host-hello"),
            "the early viewer saw the echo: {text:?}"
        );
        let late = terminal.attach(ViewerKind::Terminal).await;
        let snapshot = String::from_utf8_lossy(&late.snapshot);
        assert!(
            snapshot.contains("host-hello"),
            "the late viewer's snapshot carries the screen: {snapshot:?}"
        );
        let mouse_on = std::str::from_utf8(mouse::VIEWER_MOUSE_ON).expect("ascii");
        assert!(
            !snapshot.contains(mouse_on),
            "a terminal viewer keeps its own mouse: no reporting is switched on toward it"
        );
        let touch = terminal.attach(ViewerKind::Touch).await;
        assert!(
            String::from_utf8_lossy(&touch.snapshot).ends_with(mouse_on),
            "a touch viewer is asked to report mouse"
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
        let mut attachment = first_view.attach(ViewerKind::Terminal).await;

        first_view
            .input(format!("first{line_end}").as_bytes(), ViewerKind::Terminal)
            .await
            .expect("the first viewer writes");
        second_view
            .input(format!("second{line_end}").as_bytes(), ViewerKind::Terminal)
            .await
            .expect("the second viewer writes");

        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    received = attachment.live.recv() => {
                        if let Ok(chunk) = received {
                            output.extend_from_slice(&chunk);
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
