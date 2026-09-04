//! Which window can be brought forward for a live conversation Runtrol does not own.
//!
//! A provider started in an ordinary VS Code terminal is reachable in two independent ways, and confusing them is
//! how a row ends up claiming something it cannot do. Shell integration decides whether the terminal's output can
//! be mirrored (`docs/vscodeSurface.md`, observed mirror). Process ancestry decides whose terminal it is: the
//! window publishes each observed terminal's shell, and the provider process runs somewhere under one of them.
//! Measured 2026-09-02 with shell integration turned off: no mirror opened, the registry still knew the terminal
//! and its shell, and the provider process sat two hops below that shell (`claude.exe` under `cmd.exe` under the
//! terminal's `powershell.exe`). So a terminal with no shell integration can still be shown by its window, which
//! is exactly what a row must say instead of promising a mirror.

use std::collections::BTreeMap;

use runtrol_childproc::ProcessTree;
use runtrol_provider::NativeProcessActivity;

use crate::window_registry::ObservedShell;

/// Where a live conversation can be shown when Runtrol does not own its terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FocusTarget {
    /// A registered VS Code window observes the terminal the conversation runs in. That window can show the exact
    /// terminal, and the Runtime can bring the window forward.
    Window {
        window_session_id: String,
        terminal_key: String,
    },
    /// No registered window owns the terminal, but the process's own terminal host has a window on this desktop
    /// (Windows Terminal, a console window: measured 2026-09-05, the shell's parent `WindowsTerminal.exe` owns the
    /// one and the console shell itself owns the other). The Runtime can bring that window forward; it cannot pick
    /// a tab inside it.
    Desktop {
        /// The process and its ancestors, nearest first, as the window search walks them.
        process_ids: Vec<u32>,
    },
}

/// Every live conversation of this observation that no registered window owns and whose own process chain owns a
/// desktop window, as `located` proves it. `located` answers with the owning process, so the search that proves the
/// target is the same one the click repeats to raise it.
pub(crate) fn desktop_targets(
    activity: &NativeProcessActivity,
    tree: &ProcessTree,
    taken: &BTreeMap<String, FocusTarget>,
    located: &dyn Fn(&[u32]) -> Option<u32>,
) -> BTreeMap<String, FocusTarget> {
    let mut targets = BTreeMap::new();
    for process in &activity.processes {
        let native = process.native.to_string();
        if !activity.live.contains(&process.native) || taken.contains_key(&native) {
            continue;
        }
        let mut process_ids = vec![process.pid];
        process_ids.extend(tree.ancestors_of(process.pid));
        if located(&process_ids).is_some() {
            targets.insert(native, FocusTarget::Desktop { process_ids });
        }
    }
    targets
}

/// Every live conversation of this observation whose process is proved to run in a registered window's terminal.
///
/// Only live conversations are considered: a focus target that outlives its process would offer to show a terminal
/// that has nothing in it. When a provider sits under nested shells that a window observes both of, the nearest one
/// wins, because that is the terminal the person is looking at.
pub(crate) fn window_targets(
    activity: &NativeProcessActivity,
    shells: &[ObservedShell],
    tree: &ProcessTree,
) -> BTreeMap<String, FocusTarget> {
    let mut targets = BTreeMap::new();
    for process in &activity.processes {
        if !activity.live.contains(&process.native) {
            continue;
        }
        let Some(provider) = runtrol_childproc::process_identity(process.pid) else {
            continue;
        };
        let matched: Vec<&ObservedShell> = shells
            .iter()
            .filter(|shell| tree.contains_identity(shell.shell, provider))
            .collect();
        let Some(nearest) = nearest_shell(&matched, tree) else {
            continue;
        };
        targets.insert(
            process.native.to_string(),
            FocusTarget::Window {
                window_session_id: nearest.window_session_id.clone(),
                terminal_key: nearest.terminal_key.clone(),
            },
        );
    }
    targets
}

/// What the last focus proof for one provider was taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusProof {
    pub(crate) live_pids: std::collections::BTreeSet<u32>,
    pub(crate) taken_at_ms: u64,
}

/// How long a focus proof stands before it is taken again although nothing changed: a terminal host window can
/// close under a live process, and a stale `Focus owner` would answer `notFound` at the click.
pub(crate) const FOCUS_PROOF_MAX_AGE_MS: u64 = 5_000;

/// The shell no other matching shell contains: the innermost terminal of the ones that match.
fn nearest_shell<'a>(
    matched: &[&'a ObservedShell],
    tree: &ProcessTree,
) -> Option<&'a ObservedShell> {
    matched
        .iter()
        .copied()
        .find(|candidate| {
            !matched.iter().any(|other| {
                other.shell != candidate.shell
                    && tree.contains_identity(candidate.shell, other.shell)
            })
        })
        .or_else(|| matched.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process is its own descendant, so a shell stamped as this process attributes a conversation running
    /// here to that window. A shell that is a different live process does not.
    #[test]
    fn a_conversation_is_attributed_to_the_window_whose_shell_contains_it() {
        let Ok(tree) = ProcessTree::capture() else {
            return;
        };
        let mine = std::process::id();
        let Some(identity) = runtrol_childproc::process_identity(mine) else {
            return;
        };
        let native =
            runtrol_provider::NativeSessionId::new("conversation-1").expect("valid native id");
        let activity = NativeProcessActivity {
            live: vec![native.clone()],
            active: Vec::new(),
            processes: vec![runtrol_provider::NativeProcessBinding {
                pid: mine,
                native: native.clone(),
                cwd: None,
                terminal_access: runtrol_provider::NativeTerminalAccess::Unavailable,
            }],
        };
        let shells = vec![ObservedShell {
            window_session_id: "window-1".to_owned(),
            terminal_key: "t1".to_owned(),
            shell: identity,
        }];
        assert_eq!(
            window_targets(&activity, &shells, &tree).get("conversation-1"),
            Some(&FocusTarget::Window {
                window_session_id: "window-1".to_owned(),
                terminal_key: "t1".to_owned(),
            })
        );
        // A conversation the roster does not call live is offered no window at all.
        let stopped = NativeProcessActivity {
            live: Vec::new(),
            ..activity
        };
        assert!(window_targets(&stopped, &shells, &tree).is_empty());
    }

    /// A live conversation no window owns is offered its own terminal host's window when the search finds one on
    /// its process chain, and nothing when it does not; a conversation a window already owns is left to that window.
    #[test]
    fn a_conversation_no_window_owns_is_offered_the_desktop_window_of_its_own_chain() {
        let Ok(tree) = ProcessTree::capture() else {
            return;
        };
        let mine = std::process::id();
        let native =
            runtrol_provider::NativeSessionId::new("conversation-2").expect("valid native id");
        let activity = NativeProcessActivity {
            live: vec![native.clone()],
            active: Vec::new(),
            processes: vec![runtrol_provider::NativeProcessBinding {
                pid: mine,
                native,
                cwd: None,
                terminal_access: runtrol_provider::NativeTerminalAccess::Unavailable,
            }],
        };
        let found = desktop_targets(&activity, &tree, &BTreeMap::new(), &|chain| {
            chain.first().copied()
        });
        match found.get("conversation-2") {
            Some(FocusTarget::Desktop { process_ids }) => {
                assert_eq!(process_ids.first(), Some(&mine));
            }
            other => panic!("expected a desktop target, got {other:?}"),
        }
        assert!(desktop_targets(&activity, &tree, &BTreeMap::new(), &|_| None).is_empty());
        let mut taken = BTreeMap::new();
        taken.insert(
            "conversation-2".to_owned(),
            FocusTarget::Window {
                window_session_id: "window-1".to_owned(),
                terminal_key: "t1".to_owned(),
            },
        );
        assert!(
            desktop_targets(&activity, &tree, &taken, &|chain| chain.first().copied()).is_empty()
        );
    }
}
