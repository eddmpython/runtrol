//! Mirror a console that some other process owns: the door to a session Runtrol did not start.
//!
//! The central session engine's promise is one session, wherever it was started, streamed to every
//! viewer. For a CLI Runtrol launched, the daemon owns the pseudo terminal from the first byte. For a CLI
//! somebody else launched (another window, another app, or a Runtime generation from before a fix that no
//! longer answers), Windows still offers a door: `AttachConsole(pid)` joins that process's console, its
//! screen buffer can be read whole, and key events can be written into its input queue. Measured
//! 2026-08-29 (`tests/_attempts/consoleAttach/`): a Claude Code session held by an unreachable Runtime
//! generation gave up its 97x54 screen, and an `echo` typed into a hidden `cmd.exe` came back on its
//! screen 117 ms later.
//!
//! Two halves live here.
//!
//! - [`run_mirror`] is the helper process. A process can be attached to one console at a time, so each
//!   mirrored session gets a helper of its own (`runtrol console-mirror <pid>`). It writes the screen as
//!   terminal bytes on its stdout and takes viewer input on its stdin, which is exactly the shape of a
//!   hosted pseudo terminal seen from the daemon.
//! - [`MirrorChild`] is that helper as the daemon holds it: the same five verbs a [`crate::PtyChild`]
//!   answers (`pid`, `reader`, `writer`, `try_wait`, `kill`), so the terminal host treats a mirrored session and a
//!   hosted one alike.
//!
//! What the console gives is the screen, not the byte stream the application wrote: text with the
//! sixteen console colours, the cursor, and the visible window. The frames rendered here say exactly that
//! and nothing more. Nothing here reads a word for meaning.

use std::time::Duration;

use crate::SpawnError;

/// How often the helper reads the target's screen. Two frames of a 60 Hz display; a keystroke's echo
/// shows within that, and fifty polls a second of a bounded buffer is cheap.
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The subcommand the helper answers to. One place, read by the daemon that spawns it and the entry that
/// dispatches it.
pub const SUBCOMMAND: &str = "console-mirror";

/// One console cell as the frame renderer sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The UTF-16 unit the console holds. A wide character occupies two cells; the trailing one is marked.
    pub unit: u16,
    /// The console's colour attributes: four bits of foreground, four of background, plus the wide-character
    /// leading and trailing marks.
    pub attributes: u16,
}

/// One reading of the target's visible window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Width of the visible window, in cells.
    pub columns: u16,
    /// Height of the visible window, in cells.
    pub rows: u16,
    /// `rows * columns` cells, row-major.
    pub cells: Vec<Cell>,
    /// Zero-based cursor column and row within the window.
    pub cursor: (u16, u16),
}

const FOREGROUND_BLUE: u16 = 0x0001;
const FOREGROUND_GREEN: u16 = 0x0002;
const FOREGROUND_RED: u16 = 0x0004;
const FOREGROUND_INTENSITY: u16 = 0x0008;
const BACKGROUND_SHIFT: u16 = 4;
const COMMON_LVB_TRAILING_BYTE: u16 = 0x0200;

/// The console's sixteen colours, as the terminal's SGR numbers: the low three attribute bits are
/// blue, green, red, which is the reverse of the terminal's red, green, blue ordering.
fn sgr_colour(bits: u16, base: u8) -> u8 {
    let red = u8::from(bits & FOREGROUND_RED != 0);
    let green = u8::from(bits & FOREGROUND_GREEN != 0);
    let blue = u8::from(bits & FOREGROUND_BLUE != 0);
    let index = red | (green << 1) | (blue << 2);
    if bits & FOREGROUND_INTENSITY != 0 {
        base + 60 + index
    } else {
        base + index
    }
}

fn sgr_for(attributes: u16) -> String {
    let foreground = sgr_colour(attributes & 0x000F, 30);
    let background = sgr_colour((attributes >> BACKGROUND_SHIFT) & 0x000F, 40);
    format!("\x1b[0;{foreground};{background}m")
}

/// Terminal bytes that make a viewer's screen match `frame`.
///
/// Rows that `previous` already showed are skipped, so an idle screen costs nothing and a keystroke costs
/// its row. Everything is absolute (cursor addressed by row, colours reset per run), so a viewer that
/// joined late and got a full frame is in the same state as one that followed every diff.
#[must_use]
pub fn render(frame: &Frame, previous: Option<&Frame>) -> Vec<u8> {
    let mut out = Vec::new();
    let full = previous
        .is_none_or(|earlier| earlier.columns != frame.columns || earlier.rows != frame.rows);
    out.extend_from_slice(b"\x1b[?25l");
    if full {
        out.extend_from_slice(b"\x1b[2J");
    }
    let width = usize::from(frame.columns);
    for row in 0..usize::from(frame.rows) {
        let cells = frame
            .cells
            .get(row * width..(row + 1) * width)
            .unwrap_or(&[]);
        let unchanged = !full
            && previous.and_then(|earlier| earlier.cells.get(row * width..(row + 1) * width))
                == Some(cells);
        if unchanged {
            continue;
        }
        out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        let mut attributes: Option<u16> = None;
        let mut text = String::new();
        let mut pending_high: Option<u16> = None;
        for cell in cells {
            if cell.attributes & COMMON_LVB_TRAILING_BYTE != 0 {
                continue;
            }
            if attributes != Some(cell.attributes) {
                out.extend_from_slice(text.as_bytes());
                text.clear();
                out.extend_from_slice(sgr_for(cell.attributes).as_bytes());
                attributes = Some(cell.attributes);
            }
            // Surrogate pairs arrive as two units in two cells; only a completed pair is a character.
            if let Some(high) = pending_high.take() {
                if let Some(Ok(character)) = char::decode_utf16([high, cell.unit]).next() {
                    text.push(character);
                    continue;
                }
                text.push(char::REPLACEMENT_CHARACTER);
            }
            if (0xD800..0xDC00).contains(&cell.unit) {
                pending_high = Some(cell.unit);
                continue;
            }
            text.push(char::from_u32(u32::from(cell.unit)).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        out.extend_from_slice(text.trim_end().as_bytes());
        out.extend_from_slice(b"\x1b[0m\x1b[K");
    }
    let (column, row) = frame.cursor;
    out.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", row + 1, column + 1).as_bytes());
    out
}

/// One key event for the console's input queue, in the shape `WriteConsoleInputW` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// The virtual key, or zero for a character that has none of its own.
    pub virtual_key: u16,
    /// The UTF-16 unit typed, or zero for a key that types nothing.
    pub unit: u16,
    /// Control key state flags: Ctrl or Alt held, or zero.
    pub control: u32,
}

// Virtual key codes and control state flags are Windows ABI constants, the same on every platform this
// translation is compiled for, so the translation and its tests do not depend on the Windows crate.
const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_INSERT: u16 = 0x2D;
const VK_DELETE: u16 = 0x2E;
const LEFT_ALT_PRESSED: u32 = 0x0002;
const LEFT_CTRL_PRESSED: u32 = 0x0008;

/// Terminal input bytes, as the key events the console understands.
///
/// Printable text goes through as characters. Enter, Tab, Backspace and the control characters carry
/// their virtual key, which is what a console application reads for them. The escape sequences a terminal
/// emits for arrows and the editing keys become those keys; an escape that prefixes a plain character is
/// Alt; an escape on its own is Escape. Unknown sequences pass their characters through rather than being
/// dropped, so nothing typed disappears.
#[must_use]
pub fn keys(bytes: &[u8]) -> Vec<Key> {
    let text = String::from_utf8_lossy(bytes);
    let characters: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < characters.len() {
        let Some(&character) = characters.get(at) else {
            break;
        };
        if character == '\x1b' {
            if let Some((key, used)) = escape_sequence(&characters, at) {
                out.push(key);
                at += used;
                continue;
            }
            if let Some(&next) = characters.get(at + 1)
                && next != '\x1b'
            {
                out.push(Key {
                    virtual_key: 0,
                    unit: unit_of(next),
                    control: LEFT_ALT_PRESSED,
                });
                at += 2;
                continue;
            }
            out.push(Key {
                virtual_key: VK_ESCAPE,
                unit: 0x1B,
                control: 0,
            });
            at += 1;
            continue;
        }
        out.push(plain_key(character));
        at += 1;
    }
    out
}

fn unit_of(character: char) -> u16 {
    let mut units = [0u16; 2];
    character.encode_utf16(&mut units);
    units[0]
}

fn plain_key(character: char) -> Key {
    match character {
        '\r' | '\n' => Key {
            virtual_key: VK_RETURN,
            unit: 0x0D,
            control: 0,
        },
        '\t' => Key {
            virtual_key: VK_TAB,
            unit: 0x09,
            control: 0,
        },
        '\x7f' | '\x08' => Key {
            virtual_key: VK_BACK,
            unit: 0x08,
            control: 0,
        },
        control if (control as u32) < 0x20 => Key {
            // Ctrl+letter arrives as the letter's position in the alphabet; the console wants the letter's
            // key with the control flag and the same control character.
            virtual_key: 0x40 + control as u16,
            unit: control as u16,
            control: LEFT_CTRL_PRESSED,
        },
        other => Key {
            virtual_key: 0,
            unit: unit_of(other),
            control: 0,
        },
    }
}

/// `ESC [ ... final` or `ESC O final`: the key and how many characters it took.
fn escape_sequence(characters: &[char], start: usize) -> Option<(Key, usize)> {
    let introducer = characters.get(start + 1)?;
    let body_start = match introducer {
        '[' | 'O' => start + 2,
        _ => return None,
    };
    let mut end = body_start;
    while let Some(&character) = characters.get(end) {
        if character.is_ascii_digit() || character == ';' {
            end += 1;
        } else {
            break;
        }
    }
    let final_byte = *characters.get(end)?;
    let parameters: String = characters.get(body_start..end)?.iter().collect();
    let modifier = match parameters.rsplit(';').next().map(str::parse::<u32>) {
        Some(Ok(value)) if value > 1 && parameters.contains(';') => Some(value),
        _ => None,
    };
    let control = match modifier {
        Some(3) => LEFT_ALT_PRESSED,
        Some(5) => LEFT_CTRL_PRESSED,
        // Some(2) is shift, and everything else including no modifier: the console gets no modifier flag.
        _ => 0,
    };
    #[expect(
        clippy::match_same_arms,
        reason = "Home from CSI H and from CSI 1~ are distinct sequences that name the same key"
    )]
    let virtual_key = match (final_byte, parameters.split(';').next().unwrap_or("")) {
        ('A', _) => VK_UP,
        ('B', _) => VK_DOWN,
        ('C', _) => VK_RIGHT,
        ('D', _) => VK_LEFT,
        ('H', _) => VK_HOME,
        ('F', _) => VK_END,
        ('~', "1" | "7") => VK_HOME,
        ('~', "2") => VK_INSERT,
        ('~', "3") => VK_DELETE,
        ('~', "4" | "8") => VK_END,
        ('~', "5") => VK_PRIOR,
        ('~', "6") => VK_NEXT,
        _ => return None,
    };
    Some((
        Key {
            virtual_key,
            unit: 0,
            control,
        },
        end + 1 - start,
    ))
}

#[cfg(windows)]
mod platform {
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::RwLock;

    use windows_sys::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
        STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, CHAR_INFO, CONSOLE_SCREEN_BUFFER_INFO, COORD, FreeConsole,
        GetConsoleScreenBufferInfo, GetStdHandle, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT,
        KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, ReadConsoleOutputW, SMALL_RECT, STD_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, WriteConsoleInputW,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, GetCurrentProcess, GetExitCodeProcess, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    use super::{Cell, Frame, Key, POLL_INTERVAL, SUBCOMMAND};
    use crate::SpawnError;

    const TERMINATED_BY_RUNTROL: u32 = 0xC000_0001;

    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }

    /// An owned duplicate of one of this process's standard handles, as a File.
    ///
    /// Duplicated so it survives `AttachConsole` rebinding the process's standard handles, and owned so the
    /// daemon's pipe closes when the mirror ends. `which` is `STD_OUTPUT_HANDLE` or `STD_INPUT_HANDLE`.
    #[expect(
        unsafe_code,
        reason = "reading and duplicating a standard handle are kernel calls with no safe wrapper"
    )]
    fn duplicated_std(which: STD_HANDLE) -> Result<std::fs::File, SpawnError> {
        // SAFETY: GetStdHandle reads a per-process value; the returned handle is not owned by us.
        let source = unsafe { GetStdHandle(which) };
        if source.is_null() || source == INVALID_HANDLE_VALUE {
            return Err(SpawnError::Pty {
                doing: "reading the daemon's pipe handle",
                detail: format!("standard handle {which} is not set"),
            });
        }
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: `GetCurrentProcess` is a pseudo handle valid for both process arguments; `source` is a
        // live handle; `duplicate` is a valid out-pointer. DUPLICATE_SAME_ACCESS asks for the same rights.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &raw mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(SpawnError::Pty {
                doing: "duplicating the daemon's pipe handle",
                detail: last_error(),
            });
        }
        // SAFETY: `duplicate` is a fresh handle this process owns; wrapping it in a File transfers that
        // ownership so it is closed exactly once, when the File drops.
        Ok(unsafe { std::fs::File::from_raw_handle(duplicate as std::os::windows::io::RawHandle) })
    }

    fn console_file(name: &str) -> Result<std::fs::File, SpawnError> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .map_err(|error| SpawnError::Pty {
                doing: "opening the mirrored console",
                detail: format!("{name}: {error}"),
            })
    }

    #[expect(
        unsafe_code,
        reason = "the console screen buffer is read through kernel calls with no safe wrapper"
    )]
    fn read_frame(output: HANDLE) -> Result<Frame, SpawnError> {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO {
            dwSize: COORD { X: 0, Y: 0 },
            dwCursorPosition: COORD { X: 0, Y: 0 },
            wAttributes: 0,
            srWindow: SMALL_RECT {
                Left: 0,
                Top: 0,
                Right: 0,
                Bottom: 0,
            },
            dwMaximumWindowSize: COORD { X: 0, Y: 0 },
        };
        // SAFETY: `output` is an open console output handle for this process's attached console, and `info`
        // is a valid, writable CONSOLE_SCREEN_BUFFER_INFO the call fills.
        if unsafe { GetConsoleScreenBufferInfo(output, &raw mut info) } == 0 {
            return Err(SpawnError::Pty {
                doing: "reading the mirrored console geometry",
                detail: last_error(),
            });
        }
        let columns = u16::try_from(info.srWindow.Right - info.srWindow.Left + 1).unwrap_or(1);
        let rows = u16::try_from(info.srWindow.Bottom - info.srWindow.Top + 1).unwrap_or(1);
        let mut region = info.srWindow;
        let mut buffer = vec![
            CHAR_INFO {
                Char: windows_sys::Win32::System::Console::CHAR_INFO_0 { UnicodeChar: 0 },
                Attributes: 0,
            };
            usize::from(columns) * usize::from(rows)
        ];
        // SAFETY: `buffer` holds exactly `columns * rows` CHAR_INFO cells, which is the size passed, and
        // `region` is the window rectangle the same call reported a moment ago.
        let ok = unsafe {
            ReadConsoleOutputW(
                output,
                buffer.as_mut_ptr(),
                COORD {
                    X: i16::try_from(columns).unwrap_or(i16::MAX),
                    Y: i16::try_from(rows).unwrap_or(i16::MAX),
                },
                COORD { X: 0, Y: 0 },
                &raw mut region,
            )
        };
        if ok == 0 {
            return Err(SpawnError::Pty {
                doing: "reading the mirrored console screen",
                detail: last_error(),
            });
        }
        let cells = buffer
            .iter()
            .map(|info| Cell {
                // SAFETY: every bit pattern is a valid u16, and the console filled the union's UnicodeChar
                // member because the wide API was called.
                unit: unsafe { info.Char.UnicodeChar },
                attributes: info.Attributes,
            })
            .collect();
        let column = u16::try_from(info.dwCursorPosition.X - info.srWindow.Left).unwrap_or(0);
        let row = u16::try_from(info.dwCursorPosition.Y - info.srWindow.Top).unwrap_or(0);
        Ok(Frame {
            columns,
            rows,
            cells,
            cursor: (
                column.min(columns.saturating_sub(1)),
                row.min(rows.saturating_sub(1)),
            ),
        })
    }

    /// `KEY_EVENT` as the `u16` the record's `EventType` field takes. The constant is one, so nothing is lost.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "KEY_EVENT is the ABI constant 1"
    )]
    const KEY_EVENT_TYPE: u16 = KEY_EVENT as u16;

    fn record(key: Key, down: bool) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: KEY_EVENT_TYPE,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: i32::from(down),
                    wRepeatCount: 1,
                    wVirtualKeyCode: key.virtual_key,
                    wVirtualScanCode: 0,
                    uChar: KEY_EVENT_RECORD_0 {
                        UnicodeChar: key.unit,
                    },
                    dwControlKeyState: key.control,
                },
            },
        }
    }

    #[expect(
        unsafe_code,
        reason = "key events enter the console's input queue through a kernel call with no safe wrapper"
    )]
    fn write_keys(input: HANDLE, keys: &[Key]) -> Result<(), SpawnError> {
        if keys.is_empty() {
            return Ok(());
        }
        let records: Vec<INPUT_RECORD> = keys
            .iter()
            .flat_map(|key| [record(*key, true), record(*key, false)])
            .collect();
        let mut written = 0u32;
        // SAFETY: `records` is a valid array of the length passed, and `input` is the attached console's
        // input handle.
        let ok = unsafe {
            WriteConsoleInputW(
                input,
                records.as_ptr(),
                u32::try_from(records.len()).unwrap_or(u32::MAX),
                &raw mut written,
            )
        };
        if ok == 0 {
            return Err(SpawnError::Pty {
                doing: "typing into the mirrored console",
                detail: last_error(),
            });
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "a process exit code is read through a kernel call with no safe wrapper"
    )]
    fn target_running(process: HANDLE) -> Result<Option<i32>, SpawnError> {
        let mut code = 0u32;
        // SAFETY: `process` is an open handle with query rights and `code` a valid writable u32.
        if unsafe { GetExitCodeProcess(process, &raw mut code) } == 0 {
            return Err(SpawnError::Pty {
                doing: "asking whether the mirrored process still runs",
                detail: last_error(),
            });
        }
        // STILL_ACTIVE (259) is also an ordinary exit code, so a process that ended with it would read as
        // running; the daemon's own liveness check settles that from the process table, and a mirror whose
        // console reads fail ends on that failure instead.
        if code == STILL_ACTIVE as u32 {
            Ok(None)
        } else {
            Ok(Some(i32::try_from(code).unwrap_or(i32::MAX)))
        }
    }

    /// The helper process: attach to `pid`'s console, stream its screen out, and type stdin into it.
    ///
    /// Returns the target's exit code once it ends.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] when the console cannot be attached to or read, or the daemon's pipe cannot be
    /// captured; the daemon then shows the session as unmirrorable rather than as blank.
    #[expect(
        unsafe_code,
        reason = "attaching to another process's console and opening it are kernel calls with no safe wrapper"
    )]
    pub fn run_mirror(pid: u32) -> Result<i32, SpawnError> {
        // Capture the daemon's pipes before attaching. AttachConsole rebinds this process's standard
        // handles to the target's console, so anything written to `std::io::stdout` after the attach would
        // land on the target's screen, and `std::io::stdin` would read the target's own typed input. The
        // owned copies taken here keep talking to the daemon regardless (measured 2026-08-29: a piped
        // helper streamed zero bytes until its stdout was captured before the attach).
        let daemon_out = duplicated_std(STD_OUTPUT_HANDLE)?;
        let daemon_in = duplicated_std(STD_INPUT_HANDLE)?;
        // SAFETY: plain calls with no pointer arguments; failure of FreeConsole (no console to free) is fine.
        unsafe { FreeConsole() };
        // SAFETY: plain call; the pid is what the daemon named.
        if unsafe { AttachConsole(pid) } == 0 {
            return Err(SpawnError::Pty {
                doing: "attaching to the session's console",
                detail: format!("pid {pid}: {}", last_error()),
            });
        }
        let output = console_file("CONOUT$")?;
        let input = console_file("CONIN$")?;
        // SAFETY: plain call; a null handle is reported below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(SpawnError::Pty {
                doing: "opening the mirrored process",
                detail: format!("pid {pid}: {}", last_error()),
            });
        }
        let input_handle = input.as_raw_handle() as usize;
        let typing = std::thread::Builder::new()
            .name("runtrol-console-mirror-input".to_owned())
            .spawn(move || {
                let mut viewer = daemon_in;
                let mut buffer = [0u8; 4096];
                loop {
                    match viewer.read(&mut buffer) {
                        Ok(0) | Err(_) => return,
                        Ok(count) => {
                            let typed = super::keys(buffer.get(..count).unwrap_or(&[]));
                            if write_keys(input_handle as HANDLE, &typed).is_err() {
                                return;
                            }
                        }
                    }
                }
            })
            .map_err(|error| SpawnError::Pty {
                doing: "starting the mirror's input thread",
                detail: error.to_string(),
            })?;
        let output_handle = output.as_raw_handle() as HANDLE;
        let mut stdout = daemon_out;
        let mut previous: Option<Frame> = None;
        let outcome = loop {
            let frame = match read_frame(output_handle) {
                Ok(frame) => frame,
                Err(error) => break Err(error),
            };
            if previous.as_ref() != Some(&frame) {
                let bytes = super::render(&frame, previous.as_ref());
                if stdout
                    .write_all(&bytes)
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    // The daemon closed its end: the mirror is over, the session is not.
                    break Ok(0);
                }
                previous = Some(frame);
            }
            match target_running(process) {
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Ok(Some(code)) => break Ok(code),
                Err(error) => break Err(error),
            }
        };
        // SAFETY: `process` was opened above and is closed exactly once here.
        unsafe { CloseHandle(process) };
        drop(typing);
        outcome
    }

    /// The helper as the daemon holds it: five verbs, the same as a hosted pseudo terminal's child.
    #[derive(Debug)]
    pub struct MirrorChild {
        target: u32,
        helper: RwLock<Child>,
        stdout: RwLock<Option<ChildStdout>>,
        stdin: RwLock<Option<ChildStdin>>,
    }

    impl MirrorChild {
        /// Start the helper for `target`, with `helper` being this executable.
        ///
        /// # Errors
        ///
        /// The helper could not be started.
        pub fn spawn(helper: &std::path::Path, target: u32) -> Result<Self, SpawnError> {
            use std::os::windows::process::CommandExt as _;
            let mut command = Command::new(helper);
            command
                .arg(SUBCOMMAND)
                .arg(target.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(|error| SpawnError::Pty {
                doing: "starting the console mirror helper",
                detail: error.to_string(),
            })?;
            let stdout = child.stdout.take();
            let stdin = child.stdin.take();
            Ok(Self {
                target,
                helper: RwLock::new(child),
                stdout: RwLock::new(stdout),
                stdin: RwLock::new(stdin),
            })
        }

        /// The mirrored session's process, which is the one a person means by "this conversation".
        pub fn pid(&self) -> u32 {
            self.target
        }

        /// The screen stream, handed out once.
        ///
        /// # Errors
        ///
        /// It was already taken.
        pub fn reader(&self) -> Result<Box<dyn crate::pty::TerminalRead>, SpawnError> {
            self.stdout
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .map(|stdout| Box::new(stdout) as Box<dyn crate::pty::TerminalRead>)
                .ok_or(SpawnError::Pty {
                    doing: "reading the console mirror",
                    detail: "the mirror's output was already taken".to_owned(),
                })
        }

        /// The input sink, handed out once.
        ///
        /// # Errors
        ///
        /// It was already taken.
        pub fn writer(&self) -> Result<Box<dyn Write + Send>, SpawnError> {
            self.stdin
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .map(|stdin| Box::new(stdin) as Box<dyn Write + Send>)
                .ok_or(SpawnError::Pty {
                    doing: "writing to the console mirror",
                    detail: "the mirror's input was already taken".to_owned(),
                })
        }

        /// Whether the helper (and so the mirrored process) has ended, with its exit code.
        ///
        /// # Errors
        ///
        /// [`SpawnError::Pty`] when the helper's status cannot be read.
        pub fn try_wait(&self) -> Result<Option<i32>, SpawnError> {
            self.helper
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or(-1)))
                .map_err(|error| SpawnError::Pty {
                    doing: "asking whether the console mirror still runs",
                    detail: error.to_string(),
                })
        }

        /// End the mirrored session's process. Stop on a mirrored conversation means the conversation's own
        /// process, exactly as it does for a hosted one; the helper follows it out.
        ///
        /// # Errors
        ///
        /// [`SpawnError::Pty`] when the process cannot be opened or terminated.
        #[expect(
            unsafe_code,
            reason = "ending another process is a kernel call with no safe wrapper"
        )]
        pub fn kill(&self) -> Result<(), SpawnError> {
            // SAFETY: plain call; a null handle is reported.
            let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, self.target) };
            if process.is_null() {
                return Err(SpawnError::Pty {
                    doing: "opening the mirrored process to stop it",
                    detail: last_error(),
                });
            }
            // SAFETY: `process` is an open handle with terminate rights, closed right after.
            let ok = unsafe { TerminateProcess(process, TERMINATED_BY_RUNTROL) };
            // SAFETY: closed exactly once.
            unsafe { CloseHandle(process) };
            if ok == 0 {
                return Err(SpawnError::Pty {
                    doing: "stopping the mirrored process",
                    detail: last_error(),
                });
            }
            Ok(())
        }

        /// Let the helper go once the session ended. The mirror has nothing of its own to release.
        pub fn finish(&self) {
            let mut helper = self
                .helper
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(helper.try_wait(), Ok(None)) {
                // ok: a helper that will not die takes its stdin's end as the signal to exit; nothing more to do.
                drop(helper.kill());
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io::{Read, Write};

    use crate::SpawnError;

    fn unsupported(doing: &'static str) -> SpawnError {
        SpawnError::Pty {
            doing,
            detail: "mirroring another process's console is a Windows door; here a session is one only when Runtrol started it".to_owned(),
        }
    }

    /// The helper's own entry point. Not a door on this platform; it says so.
    ///
    /// # Errors
    ///
    /// Always [`SpawnError::Pty`]: mirroring a foreign console is a Windows door.
    pub fn run_mirror(_pid: u32) -> Result<i32, SpawnError> {
        Err(unsupported("attaching to the session's console"))
    }

    /// The same shape the Windows mirror has, so callers compile everywhere; never constructed here.
    #[derive(Debug)]
    pub struct MirrorChild {
        target: u32,
    }

    impl MirrorChild {
        /// Start a mirror of `target`. Refused on this platform.
        ///
        /// # Errors
        ///
        /// Always [`SpawnError::Pty`].
        pub fn spawn(_helper: &std::path::Path, _target: u32) -> Result<Self, SpawnError> {
            Err(unsupported("starting the console mirror helper"))
        }

        /// The mirrored process.
        #[must_use]
        pub const fn pid(&self) -> u32 {
            self.target
        }

        /// The mirror's screen bytes. Refused on this platform.
        ///
        /// # Errors
        ///
        /// Always [`SpawnError::Pty`].
        pub fn reader(&self) -> Result<Box<dyn crate::pty::TerminalRead>, SpawnError> {
            Err(unsupported("reading the console mirror"))
        }

        /// The mirror's input. Refused on this platform.
        ///
        /// # Errors
        ///
        /// Always [`SpawnError::Pty`].
        pub fn writer(&self) -> Result<Box<dyn Write + Send>, SpawnError> {
            Err(unsupported("writing to the console mirror"))
        }

        /// Whether the mirror ended. Refused on this platform.
        ///
        /// # Errors
        ///
        /// Always [`SpawnError::Pty`].
        pub fn try_wait(&self) -> Result<Option<i32>, SpawnError> {
            Err(unsupported("asking whether the console mirror still runs"))
        }

        /// End the mirrored process. Refused on this platform.
        ///
        /// # Errors
        ///
        /// Always [`SpawnError::Pty`].
        pub fn kill(&self) -> Result<(), SpawnError> {
            Err(unsupported("stopping the mirrored process"))
        }

        /// Release the mirror. Nothing to release here.
        pub const fn finish(&self) {}
    }
}

pub use platform::{MirrorChild, run_mirror};

/// Where the helper lives: this very executable, answering [`SUBCOMMAND`].
///
/// # Errors
///
/// The running executable's path could not be read.
pub fn helper_program() -> Result<std::path::PathBuf, SpawnError> {
    std::env::current_exe().map_err(|error| SpawnError::Pty {
        doing: "locating the console mirror helper",
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &[&str], cursor: (u16, u16)) -> Frame {
        let columns = u16::try_from(
            text.iter()
                .map(|row| row.chars().count())
                .max()
                .unwrap_or(0),
        )
        .expect("small");
        let rows = u16::try_from(text.len()).expect("small");
        let mut cells = Vec::new();
        for row in text {
            let mut count = 0;
            for character in row.chars() {
                cells.push(Cell {
                    unit: unit_of(character),
                    attributes: 0x0007,
                });
                count += 1;
            }
            for _ in count..columns {
                cells.push(Cell {
                    unit: u16::from(b' '),
                    attributes: 0x0007,
                });
            }
        }
        Frame {
            columns,
            rows,
            cells,
            cursor,
        }
    }

    #[test]
    fn a_first_frame_clears_and_paints_every_row_then_places_the_cursor() {
        let rendered = render(&frame(&["hello", "world"], (2, 1)), None);
        let text = String::from_utf8(rendered).expect("ascii");
        assert!(
            text.starts_with("\x1b[?25l\x1b[2J"),
            "hide the cursor and clear: {text:?}"
        );
        assert!(
            text.contains("\x1b[1;1H\x1b[0;37;40mhello"),
            "row one: {text:?}"
        );
        assert!(
            text.contains("\x1b[2;1H\x1b[0;37;40mworld"),
            "row two: {text:?}"
        );
        assert!(
            text.ends_with("\x1b[2;3H\x1b[?25h"),
            "cursor at row 2 column 3, shown: {text:?}"
        );
    }

    #[test]
    fn an_unchanged_row_costs_nothing_and_a_changed_one_is_repainted_whole() {
        let earlier = frame(&["hello", "world"], (0, 0));
        let later = frame(&["hello", "there"], (0, 0));
        let text = String::from_utf8(render(&later, Some(&earlier))).expect("ascii");
        assert!(
            !text.contains("hello"),
            "the unchanged row is not sent again: {text:?}"
        );
        assert!(
            text.contains("\x1b[2;1H\x1b[0;37;40mthere"),
            "the changed row is: {text:?}"
        );
        assert!(
            !text.contains("\x1b[2J"),
            "no clear between frames of one size: {text:?}"
        );
    }

    #[test]
    fn console_colours_become_the_terminal_colours_in_the_right_order() {
        // Console red is bit 2; the terminal's red is SGR 31. Blue is bit 0; terminal blue is 34.
        assert_eq!(sgr_for(0x0004), "\x1b[0;31;40m");
        assert_eq!(sgr_for(0x0001), "\x1b[0;34;40m");
        assert_eq!(sgr_for(0x000F), "\x1b[0;97;40m", "intense white");
        assert_eq!(
            sgr_for(0x0027),
            "\x1b[0;37;42m",
            "white on green background"
        );
    }

    #[test]
    fn typed_text_and_the_editing_keys_become_console_key_events() {
        let typed = keys(b"ab\r");
        assert_eq!(typed.len(), 3);
        assert_eq!(typed.first().map(|key| key.unit), Some(u16::from(b'a')));
        assert_eq!(
            typed.get(2).map(|key| (key.virtual_key, key.unit)),
            Some((VK_RETURN, 0x0D))
        );
        assert_eq!(
            keys(b"\x1b[A").first().map(|key| key.virtual_key),
            Some(VK_UP)
        );
        assert_eq!(
            keys(b"\x1b[3~").first().map(|key| key.virtual_key),
            Some(VK_DELETE)
        );
        assert_eq!(
            keys(b"\x1bOH").first().map(|key| key.virtual_key),
            Some(VK_HOME)
        );
        assert_eq!(
            keys(b"\x1b[1;5C")
                .first()
                .map(|key| (key.virtual_key, key.control)),
            Some((VK_RIGHT, LEFT_CTRL_PRESSED))
        );
    }

    #[test]
    fn a_lone_escape_is_escape_and_an_escape_before_a_letter_is_alt() {
        assert_eq!(
            keys(b"\x1b").first().map(|key| key.virtual_key),
            Some(VK_ESCAPE)
        );
        let alt = keys(b"\x1bx");
        assert_eq!(alt.len(), 1);
        assert_eq!(
            alt.first().map(|key| (key.unit, key.control)),
            Some((u16::from(b'x'), LEFT_ALT_PRESSED))
        );
    }

    #[test]
    fn control_characters_carry_their_letter_and_the_control_flag() {
        let interrupt = keys(b"\x03");
        assert_eq!(
            interrupt
                .first()
                .map(|key| (key.virtual_key, key.unit, key.control)),
            Some((u16::from(b'C'), 3, LEFT_CTRL_PRESSED))
        );
    }

    #[test]
    fn a_wide_character_is_sent_once_from_its_leading_cell() {
        let mut wide = frame(&["ab"], (0, 0));
        // "한" occupies two cells: the leading one carries the unit, the trailing one is marked.
        wide.cells = vec![
            Cell {
                unit: unit_of('한'),
                attributes: 0x0107,
            },
            Cell {
                unit: unit_of('한'),
                attributes: 0x0207,
            },
        ];
        let text = String::from_utf8(render(&wide, None)).expect("utf-8");
        assert_eq!(text.matches('한').count(), 1, "{text:?}");
    }
}
