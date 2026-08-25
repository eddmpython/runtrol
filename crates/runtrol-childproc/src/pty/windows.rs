//! `ConPTY`: a pseudo console, two pipes, and a process started with the console as its attribute.
//!
//! The shape follows the platform's own documented sequence: `CreatePipe` twice, `CreatePseudoConsole`
//! over the far ends, `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` on the new process, and the near ends kept
//! as the child's input and output. Every `unsafe` block carries the argument for why the call is sound.
//!
//! Two things measured rather than assumed: the child must be started with `bInheritHandles = FALSE`, or
//! the pipe ends leak into it and the output never reports end of stream; and the console must be closed
//! *after* the process handle is known to have ended, or `ClosePseudoConsole` blocks on a live client.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{PtySize, PtySpawn, child_environment, full_arguments};
use crate::error::SpawnError;

/// Exit code reported for a child this host terminated. The same value the job object reports, so a reader
/// of exit codes has one word for "runtrol stopped this".
const TERMINATED_BY_RUNTROL: u32 = 0x_C000_0409;

/// The Windows half of [`super::PtyChild`].
#[derive(Debug)]
pub(super) struct Child {
    /// The pseudo console, or 0 once closed. Closed once the process is known to have ended.
    console: AtomicIsize,
    /// The process handle.
    process: HANDLE,
    /// The process id, for the caller's own bookkeeping.
    pid: u32,
    /// Our end of the child's output.
    output: File,
    /// Our end of the child's input.
    input: File,
}

// SAFETY: the handles are kernel references, not thread-affine, and every call on them here is documented as
// callable from any thread. The one mutable field is an atomic. `File` is already `Send + Sync`.
#[expect(
    unsafe_code,
    reason = "kernel handles are thread safe and the raw pointer spelling is what hides that"
)]
unsafe impl Send for Child {}
#[expect(
    unsafe_code,
    reason = "kernel handles are thread safe and the raw pointer spelling is what hides that"
)]
unsafe impl Sync for Child {}

fn last_error() -> String {
    #[expect(
        unsafe_code,
        reason = "reading the thread's last error has no preconditions"
    )]
    // SAFETY: `GetLastError` reads a thread-local value and has no preconditions.
    let code = unsafe { GetLastError() };
    format!("Windows error {code}")
}

fn refused(doing: &'static str) -> SpawnError {
    SpawnError::Pty {
        doing,
        detail: last_error(),
    }
}

/// A pipe pair, closed on drop unless taken.
struct Pipe {
    read: HANDLE,
    write: HANDLE,
}

impl Pipe {
    #[expect(
        unsafe_code,
        reason = "creating an anonymous pipe is a kernel call with no safe wrapper"
    )]
    fn create(doing: &'static str) -> Result<Self, SpawnError> {
        let mut read: HANDLE = INVALID_HANDLE_VALUE;
        let mut write: HANDLE = INVALID_HANDLE_VALUE;
        // SAFETY: both out-pointers point at live locals; a null security descriptor asks for a
        // non-inheritable pipe with default security, which is what a pipe the child must not see needs;
        // a size of zero asks for the default buffer.
        let ok = unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 0) };
        if ok == 0 {
            return Err(refused(doing));
        }
        Ok(Self { read, write })
    }
}

#[expect(
    unsafe_code,
    reason = "closing a handle is a kernel call with no safe wrapper"
)]
fn close(handle: HANDLE) {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: every handle passed here came from a successful kernel call in this module and is closed
    // exactly once, at the one place that owns it.
    unsafe { CloseHandle(handle) };
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// One command-line token, quoted the way `CommandLineToArgvW` unquotes it.
///
/// The rules are the standard library's (`std::sys::windows::args`): quote when empty or when a space,
/// tab, or quote is present; a run of backslashes before a quote or the closing quote is doubled; a quote
/// is escaped with one backslash. `argv::check_all` has already refused the tokens no quoting can carry.
fn quote_token(token: &str, out: &mut Vec<u16>) {
    let needs_quotes = token.is_empty() || token.chars().any(|c| matches!(c, ' ' | '\t' | '"'));
    if !needs_quotes {
        out.extend(token.encode_utf16());
        return;
    }
    out.push(u16::from(b'"'));
    let mut backslashes = 0usize;
    for c in token.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
                backslashes = 0;
                out.push(u16::from(b'"'));
            }
            other => {
                out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
                backslashes = 0;
                let mut buffer = [0u16; 2];
                out.extend_from_slice(other.encode_utf16(&mut buffer));
            }
        }
    }
    out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    out.push(u16::from(b'"'));
}

fn command_line(spawn: &PtySpawn<'_>) -> Vec<u16> {
    let mut line = Vec::new();
    quote_token(spawn.program.path().as_str(), &mut line);
    for argument in full_arguments(spawn) {
        line.push(u16::from(b' '));
        quote_token(&argument, &mut line);
    }
    line.push(0);
    line
}

/// `NAME=value\0` for each variable, then a final `\0`, in UTF-16, sorted by name as the platform expects.
fn environment_block(spawn: &PtySpawn<'_>) -> Vec<u16> {
    let mut block = Vec::new();
    for (name, value) in child_environment(spawn) {
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

const fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols.cast_signed(),
        Y: size.rows.cast_signed(),
    }
}

impl Child {
    #[expect(
        unsafe_code,
        reason = "creating a pseudo console and a process are kernel calls with no safe wrapper. the safety argument is at each block"
    )]
    pub(super) fn spawn(spawn: PtySpawn<'_>) -> Result<Self, SpawnError> {
        // Input flows from us (write) to the child (read); output from the child (write) to us (read).
        let input = Pipe::create("creating the terminal input pipe")?;
        let output = Pipe::create("creating the terminal output pipe")?;

        let mut console: HPCON = 0;
        // SAFETY: the size is a plain value; the two handles are the far ends of pipes this function just
        // created and still owns; flags 0 asks for the default console; the out-pointer is a live local.
        let created = unsafe {
            CreatePseudoConsole(
                coord(spawn.size),
                input.read,
                output.write,
                0,
                &raw mut console,
            )
        };
        if created < 0 {
            close(input.read);
            close(input.write);
            close(output.read);
            close(output.write);
            return Err(SpawnError::Pty {
                doing: "creating the pseudo console",
                detail: format!("HRESULT {created:#x}"),
            });
        }
        // The console holds its own references to the far ends; keeping ours open would keep the output
        // pipe from ever reporting end of stream.
        close(input.read);
        close(output.write);

        // The attribute list is sized by the platform, so the buffer is asked for first with a null list.
        let mut attribute_size: usize = 0;
        // SAFETY: a null list with a count of one is the documented way to ask for the required size; the
        // out-pointer is a live local. The call reports failure for this query by design, so its result is
        // not checked here, only the size it wrote.
        unsafe {
            InitializeProcThreadAttributeList(core::ptr::null_mut(), 1, 0, &raw mut attribute_size);
        }
        let mut attributes: Vec<u8> = vec![0; attribute_size.max(1)];
        let attribute_list: LPPROC_THREAD_ATTRIBUTE_LIST = attributes.as_mut_ptr().cast();
        // SAFETY: the buffer is at least the size the platform asked for and lives until after the process
        // is created; count and flags are as in the query.
        let initialized = unsafe {
            InitializeProcThreadAttributeList(attribute_list, 1, 0, &raw mut attribute_size)
        };
        if initialized == 0 {
            let error = refused("initializing the process attribute list");
            close_console(console);
            close(input.write);
            close(output.read);
            return Err(error);
        }
        // SAFETY: the list was initialized above; the attribute is the pseudo console one; the value is the
        // console handle itself spelled as the pointer argument with the handle's size, exactly as the
        // platform documents this attribute; the last two arguments are optional and null.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                attribute_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                core::ptr::with_exposed_provenance::<core::ffi::c_void>(console.cast_unsigned()),
                size_of::<HPCON>(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        if updated == 0 {
            let error = refused("attaching the pseudo console to the process attributes");
            // SAFETY: the list was initialized and is deleted exactly once.
            unsafe { DeleteProcThreadAttributeList(attribute_list) };
            close_console(console);
            close(input.write);
            close(output.read);
            return Err(error);
        }

        // The standard handles are named and set invalid on purpose (measured 2026-08-25): without this a
        // console-process parent passes its own standard handle values to the child even with inheritance
        // off, the child writes to those instead of to the pseudo console, and the console renders nothing
        // beyond its first frame. Invalid handles make the child take the console's own.
        let mut startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: u32::try_from(size_of::<STARTUPINFOEXW>()).unwrap_or(0),
                dwFlags: STARTF_USESTDHANDLES,
                hStdInput: INVALID_HANDLE_VALUE,
                hStdOutput: INVALID_HANDLE_VALUE,
                hStdError: INVALID_HANDLE_VALUE,
                ..Default::default()
            },
            lpAttributeList: attribute_list,
        };
        let mut process = PROCESS_INFORMATION::default();
        let mut line = command_line(&spawn);
        let mut environment = environment_block(&spawn);
        let cwd = wide(OsStr::new(spawn.cwd.as_str()));
        // SAFETY: the command line is a mutable, NUL-terminated UTF-16 buffer as the call requires; handle
        // inheritance is off so the pipe ends stay ours; the flags say the extended startup info and a
        // UTF-16 environment block are present, and both are live locals; the working directory is
        // NUL-terminated; the out-pointer is a live local.
        let started = unsafe {
            CreateProcessW(
                core::ptr::null(),
                line.as_mut_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_mut_ptr().cast(),
                cwd.as_ptr(),
                &raw mut startup.StartupInfo,
                &raw mut process,
            )
        };
        // SAFETY: the list was initialized and is deleted exactly once, after the process that used it exists.
        unsafe { DeleteProcThreadAttributeList(attribute_list) };
        if started == 0 {
            let error = refused("starting the program on the pseudo console");
            close_console(console);
            close(input.write);
            close(output.read);
            return Err(error);
        }
        close(process.hThread);

        // SAFETY: each handle is a pipe end this function created, owned by nothing else, and handed to
        // exactly one `File`, which closes it.
        let (output_file, input_file) = unsafe {
            (
                File::from_raw_handle(output.read.cast()),
                File::from_raw_handle(input.write.cast()),
            )
        };
        Ok(Self {
            console: AtomicIsize::new(console),
            process: process.hProcess,
            pid: process.dwProcessId,
            output: output_file,
            input: input_file,
        })
    }

    pub(super) fn pid(&self) -> u32 {
        self.pid
    }

    pub(super) fn reader(&self) -> Result<Box<dyn Read + Send>, SpawnError> {
        let file = self.output.try_clone().map_err(|error| SpawnError::Pty {
            doing: "duplicating the terminal output",
            detail: error.to_string(),
        })?;
        Ok(Box::new(file))
    }

    pub(super) fn writer(&self) -> Result<Box<dyn Write + Send>, SpawnError> {
        let file = self.input.try_clone().map_err(|error| SpawnError::Pty {
            doing: "duplicating the terminal input",
            detail: error.to_string(),
        })?;
        Ok(Box::new(file))
    }

    #[expect(
        unsafe_code,
        reason = "resizing a pseudo console is a kernel call with no safe wrapper"
    )]
    pub(super) fn resize(&self, size: PtySize) -> Result<(), SpawnError> {
        let console = self.console.load(Ordering::SeqCst);
        if console == 0 {
            return Err(SpawnError::Pty {
                doing: "resizing the pseudo console",
                detail: "the console is already closed".to_owned(),
            });
        }
        // SAFETY: the console handle came from `CreatePseudoConsole` and is still open (the slot held it).
        let result = unsafe { ResizePseudoConsole(console, coord(size)) };
        if result < 0 {
            return Err(SpawnError::Pty {
                doing: "resizing the pseudo console",
                detail: format!("HRESULT {result:#x}"),
            });
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "waiting on and reading a process exit code are kernel calls with no safe wrapper"
    )]
    pub(super) fn try_wait(&self) -> Result<Option<i32>, SpawnError> {
        // SAFETY: the process handle came from `CreateProcessW` and is closed only in `Drop`; a zero
        // timeout makes this a poll.
        let waited = unsafe { WaitForSingleObject(self.process, 0) };
        if waited != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut code: u32 = 0;
        // SAFETY: the process handle is valid, and the out-pointer is a live local.
        let ok = unsafe { GetExitCodeProcess(self.process, &raw mut code) };
        if ok == 0 {
            return Err(refused("reading the exit code"));
        }
        Ok(Some(code.cast_signed()))
    }

    #[expect(
        unsafe_code,
        reason = "terminating a process is a kernel call with no safe wrapper"
    )]
    pub(super) fn kill(&self) -> Result<(), SpawnError> {
        // SAFETY: the process handle is valid until `Drop`. Terminating an already-ended process fails with
        // access denied, which is reported rather than hidden.
        let ok = unsafe { TerminateProcess(self.process, TERMINATED_BY_RUNTROL) };
        if ok == 0 && self.try_wait()?.is_none() {
            return Err(refused("terminating the program"));
        }
        Ok(())
    }
}

impl Child {
    /// Close the console, which is what lets the output pipe report end of stream.
    ///
    /// Not done from `try_wait`, and measured: the console host flushes its last frame a moment after
    /// the client exits, and closing on the exit itself dropped a one-line echo entirely. The host calls
    /// this once the output has settled.
    pub(super) fn finish(&self) {
        self.close_console_once();
    }

    /// Close the console exactly once, whichever of finish and drop comes first.
    fn close_console_once(&self) {
        let console = self.console.swap(0, Ordering::SeqCst);
        if console != 0 {
            close_console(console);
        }
    }
}

#[expect(
    unsafe_code,
    reason = "closing a pseudo console is a kernel call with no safe wrapper"
)]
fn close_console(console: HPCON) {
    // SAFETY: the handle came from `CreatePseudoConsole` and each call site closes it exactly once (the
    // slot in `Child` is taken before this is called).
    unsafe { ClosePseudoConsole(console) };
}

impl Drop for Child {
    fn drop(&mut self) {
        // A dropped terminal ends its child the way a closed window does. The console is closed first so a
        // client that exits on end of input goes quietly; whatever is still running is then terminated.
        self.close_console_once();
        // Reported nowhere on purpose: `Drop` has no error channel, and a process that already ended makes
        // this call fail by design.
        drop(self.kill());
        close(self.process);
    }
}

#[cfg(test)]
mod tests {
    use super::quote_token;

    fn quoted(token: &str) -> String {
        let mut out = Vec::new();
        quote_token(token, &mut out);
        String::from_utf16_lossy(&out)
    }

    #[test]
    fn tokens_are_quoted_the_way_the_platform_unquotes_them() {
        assert_eq!(quoted("plain"), "plain");
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("has space"), "\"has space\"");
        assert_eq!(quoted("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(
            quoted("C:\\path with space\\"),
            "\"C:\\path with space\\\\\""
        );
        assert_eq!(quoted("back\\slash"), "back\\slash");
    }
}
