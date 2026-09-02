//! Bringing another program's top-level window forward on Windows (`docs/vscodeSurface.md`, owner reveal).
//!
//! A VS Code window belongs to the editor's main process, not to the extension host that registered it, and one
//! main process owns every window of that editor instance. So the window is found among the top-level windows of
//! the host's ancestors by a fragment of its title (the folder name VS Code puts there), and it is raised the way
//! Windows permits: a plain foreground request first, then one after this process has sent one input event (a
//! mouse move of zero pixels, which no window sees as anything), since Windows grants the foreground to the process
//! that last sent input (measured 2026-09-02: a service process that has sent none is refused outright). When both
//! are refused the taskbar button flashes instead. Nothing here reads or types into any window.
//!
//! What this deliberately does not do is attach this thread's input queue to the foreground window's thread
//! (`AttachThreadInput`), the usual third resort. Attached input queues make two threads share one queue, so a stall
//! in either freezes both, and the thread on the other side is the operator's editor. A reveal that flashes a
//! taskbar button is a smaller loss than an editor that stops repainting.

/// What happened to the window that was asked forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealOutcome {
    /// The window is in the foreground now.
    Raised,
    /// Windows refused the foreground change; the window's taskbar button flashes.
    Flashed,
    /// No visible top-level window of those processes carries the title fragment.
    NotFound,
    /// More than one visible window of those processes carries the title fragment.
    Ambiguous,
    /// This platform has no window to raise.
    Unsupported,
}

impl RevealOutcome {
    /// The outcome as the public wire spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raised => "raised",
            Self::Flashed => "flashed",
            Self::NotFound => "notFound",
            Self::Ambiguous => "ambiguous",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Bring forward the editor window of the nearest process in `process_ids` (nearest first: the host, then its
/// ancestors) that owns any visible titled top-level window; among that process's windows the one whose title
/// contains `title_fragment` (case-insensitive), or the only one. Stopping at the nearest such process matters:
/// the chain above an editor started from another editor's terminal reaches that other editor too (measured
/// 2026-09-02), and its windows are not the owner's.
#[must_use]
pub fn reveal_window(process_ids: &[u32], title_fragment: &str) -> RevealOutcome {
    #[cfg(windows)]
    {
        windows::reveal(process_ids, title_fragment)
    }
    #[cfg(not(windows))]
    {
        // No window system to ask on these platforms; the arguments name what a Windows build would look for.
        let (_unused_processes, _unused_fragment) = (process_ids, title_fragment);
        RevealOutcome::Unsupported
    }
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "top-level window enumeration and the foreground request are Win32 calls with no safe wrapper"
)]
mod windows {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GW_OWNER, GetWindow,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    use super::RevealOutcome;

    struct Search<'a> {
        process_ids: &'a [u32],
        /// Every visible, unowned, titled top-level window of the processes, with its process and folded title.
        candidates: Vec<(u32, HWND, String)>,
    }

    pub(super) fn reveal(process_ids: &[u32], title_fragment: &str) -> RevealOutcome {
        let mut search = Search {
            process_ids,
            candidates: Vec::new(),
        };
        // SAFETY: the callback receives the pointer to `search` for exactly the duration of this call and
        // touches nothing else; EnumWindows returns before `search` goes out of scope.
        unsafe {
            EnumWindows(Some(collect), std::ptr::addr_of_mut!(search) as LPARAM);
        }
        let Some(editor) = process_ids
            .iter()
            .find(|pid| search.candidates.iter().any(|(owner, _, _)| owner == *pid))
        else {
            return RevealOutcome::NotFound;
        };
        let fragment = folded(title_fragment);
        let owned: Vec<HWND> = search
            .candidates
            .iter()
            .filter(|(owner, _, _)| owner == editor)
            .map(|(_, window, _)| *window)
            .collect();
        let matches: Vec<HWND> = search
            .candidates
            .iter()
            .filter(|(owner, _, title)| owner == editor && title.contains(&fragment))
            .map(|(_, window, _)| *window)
            .collect();
        // The title names the folder in VS Code's default title; a person's own `window.title` may not. When the
        // editor owns exactly one window there is nothing to tell apart and the title is not needed.
        let window = match (matches.as_slice(), owned.as_slice()) {
            ([one], _) | ([], [one]) => *one,
            _ => return RevealOutcome::Ambiguous,
        };
        raise(window)
    }

    unsafe extern "system" fn collect(window: HWND, parameter: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `parameter` is the pointer `reveal` passed for this enumeration and outlives it.
        let search = unsafe { &mut *(parameter as *mut Search<'_>) };
        // SAFETY: plain queries on a window handle the system just handed us.
        unsafe {
            if IsWindowVisible(window) == 0 {
                return 1;
            }
            let mut process_id = 0_u32;
            GetWindowThreadProcessId(window, &raw mut process_id);
            if !search.process_ids.contains(&process_id) {
                return 1;
            }
            // Tool windows, tooltips and notifications are owned by the main window and carry no title; the
            // editor window is unowned and titled.
            if !GetWindow(window, GW_OWNER).is_null() {
                return 1;
            }
            let length = GetWindowTextLengthW(window);
            if length <= 0 {
                return 1;
            }
            let mut buffer = vec![0_u16; usize::try_from(length).unwrap_or(0) + 1];
            let copied = GetWindowTextW(window, buffer.as_mut_ptr(), length + 1);
            let title = String::from_utf16_lossy(
                buffer
                    .get(..usize::try_from(copied).unwrap_or(0))
                    .unwrap_or(&[]),
            );
            search.candidates.push((process_id, window, folded(&title)));
        }
        1
    }

    /// Case folding for a title comparison: Unicode-aware per character, so a Korean folder name matches itself and
    /// `Runtrol` matches `runtrol`.
    fn folded(text: &str) -> String {
        text.chars().flat_map(char::to_lowercase).collect()
    }

    fn raise(window: HWND) -> RevealOutcome {
        // SAFETY: the handle came from this process's own enumeration a moment ago; every call is a request the
        // system is free to refuse, and a refused request leaves the window as it was.
        unsafe {
            if IsIconic(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            if SetForegroundWindow(window) != 0 {
                return RevealOutcome::Raised;
            }
            // Windows grants the foreground to the process that sent the last input event. A relative mouse move of
            // zero pixels is an input event that moves nothing and reaches no window as a click or a key.
            let mut nothing = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let sent = SendInput(
                1,
                &raw mut nothing,
                i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(0),
            );
            if sent == 1 && SetForegroundWindow(window) != 0 {
                return RevealOutcome::Raised;
            }
            let mut flash = FLASHWINFO {
                cbSize: u32::try_from(std::mem::size_of::<FLASHWINFO>()).unwrap_or(0),
                hwnd: window,
                dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            FlashWindowEx(&raw mut flash);
        }
        RevealOutcome::Flashed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_process_owns_a_window_with_an_impossible_title() {
        assert_eq!(
            reveal_window(&[std::process::id()], "runtrol-no-such-window-title-7f3a"),
            if cfg!(windows) {
                RevealOutcome::NotFound
            } else {
                RevealOutcome::Unsupported
            }
        );
        assert_eq!(RevealOutcome::Flashed.as_str(), "flashed");
    }
}
