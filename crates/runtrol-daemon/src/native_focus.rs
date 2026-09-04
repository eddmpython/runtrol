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
}
