//! Which of an agent's conversations a live process has open, from the store the agent keeps itself.
//!
//! An agent that keeps one directory per conversation and holds a file inside it while that conversation is
//! open answers two questions for free: which conversations are live, and which process has each one. The
//! first keeps a live conversation from being resumed into a second process; the second binds a conversation
//! to the terminal it is already running in.
//!
//! Nothing here knows an agent by name. Where to look and what to look for come from that agent's own manifest
//! (`store.live`), so an agent joins by declaring, never by adding a branch here.
//!
//! Measured on grok 1.0.13 (2026-08-30): resuming one conversation left `events.jsonl` inside that
//! conversation's own directory held by the running process, and the Restart Manager named it. The same probe
//! against every other file in that directory named nobody, so the held file is the signal and the rest is not.
//!
//! # What this is not
//!
//! No conversation is opened and no message is read. This asks the filesystem who holds a file, which is the
//! same kind of knowledge as a process list.

use std::fs;
use std::path::{Path, PathBuf};

use runtrol_provider::{
    LiveSessionSpec, NativeProcessActivity, NativeProcessBinding, NativeSessionId,
};

/// How many workspace groups one look walks, newest first. A conversation a live process holds was written to
/// recently, so the newest groups are where a live one is; a machine with years of them pays for none of it.
const MAX_GROUPS: usize = 64;
/// How many conversation directories one look walks inside a group, newest first.
const MAX_PER_GROUP: usize = 32;
/// The most conversations one look asks the filesystem about, whatever the shape of the store.
const MAX_PROBES: usize = 256;

/// One conversation a live process is holding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Live {
    /// The conversation, named the way the agent's own listing names it.
    pub(super) native: NativeSessionId,
    /// The file that proves it, which is also what names the process holding it.
    pub(super) held: PathBuf,
    /// The workspace the store filed it under, when the store is grouped that way.
    pub(super) workspace: Option<String>,
}

/// Every conversation of this agent that a live process is holding right now.
///
/// An unreadable store answers with nothing: a store this build cannot walk proves no conversation live, and
/// saying so is the honest answer where inventing one is not.
pub(super) fn held_conversations(home: &Path, spec: &LiveSessionSpec) -> Vec<Live> {
    let mut root = home.to_path_buf();
    for part in &spec.root {
        for step in part.split('/') {
            root.push(step);
        }
    }
    let mut found = Vec::new();
    let mut probes = 0;
    if spec.grouped_by_workspace {
        for group in newest_directories(&root, MAX_GROUPS) {
            let workspace = decode_workspace(&group);
            for conversation in newest_directories(&group, MAX_PER_GROUP) {
                if probes >= MAX_PROBES {
                    return found;
                }
                probes += 1;
                if let Some(live) = live_in(&conversation, &spec.held, workspace.clone()) {
                    found.push(live);
                }
            }
        }
    } else {
        for conversation in newest_directories(&root, MAX_PROBES) {
            if probes >= MAX_PROBES {
                return found;
            }
            probes += 1;
            if let Some(live) = live_in(&conversation, &spec.held, None) {
                found.push(live);
            }
        }
    }
    found
}

/// The activity answer this evidence supports, with each conversation bound to the process holding it.
///
/// Asking which process holds a file is far more expensive than asking whether anybody does, so the second
/// question is only put about conversations the first has already answered yes for.
pub(super) fn activity(
    home: &Path,
    spec: &LiveSessionSpec,
    ask: fn(&Path) -> Option<u32>,
) -> NativeProcessActivity {
    let mut live = Vec::new();
    let mut processes = Vec::new();
    for held in held_conversations(home, spec) {
        if let Some(pid) = ask(&held.held) {
            processes.push(NativeProcessBinding {
                pid,
                native: held.native.clone(),
                cwd: held.workspace.clone(),
                // Whether that process draws a screen another window can join is a separate question this
                // evidence does not answer. Claiming a screen that is not there makes a row appear and vanish
                // (2026-08-30, an editor panel session), so nothing is claimed here.
                terminal_access: runtrol_provider::NativeTerminalAccess::Unavailable,
            });
        }
        live.push(held.native);
    }
    NativeProcessActivity {
        live,
        // A held file says a process has the conversation open, never that a model is answering in it. That
        // is a different question with a different surface, and answering it from this one would be a guess.
        active: Vec::new(),
        processes,
    }
}

/// One conversation directory, when a live process is holding the file that proves it open.
fn live_in(conversation: &Path, held_name: &str, workspace: Option<String>) -> Option<Live> {
    let name = conversation.file_name()?.to_str()?;
    // A directory whose name this agent's own identity rules would refuse is not a conversation of its, so it
    // is stepped over rather than named.
    let Ok(native) = NativeSessionId::new(name) else {
        return None;
    };
    let held = conversation.join(held_name);
    runtrol_childproc::write_locked(&held).then_some(Live {
        native,
        held,
        workspace,
    })
}

/// The directories directly under a path, newest first, bounded.
fn newest_directories(root: &Path, most: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dated: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten().take(MAX_PROBES * 4) {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let when = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        dated.push((when, path));
    }
    // Newest first, so a bound on how many are walked keeps the ones a live process could be holding.
    dated.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    dated.into_iter().take(most).map(|(_, path)| path).collect()
}

/// The workspace a group directory names, when its name is a path the agent escaped to make one.
///
/// Grok writes the working directory percent-escaped, so `C%3A%5CUsers` is `C:\Users`. A name that is not
/// escaped comes back as it was written, which is what a store that groups by something else would want.
fn decode_workspace(group: &Path) -> Option<String> {
    let name = group.file_name()?.to_str()?;
    let mut out = String::with_capacity(name.len());
    let mut bytes = name.bytes();
    let mut raw: Vec<u8> = Vec::with_capacity(name.len());
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(hex);
            let low = bytes.next().and_then(hex);
            match (high, low) {
                (Some(high), Some(low)) => raw.push(high * 16 + low),
                // Not an escape after all: keep what was written rather than losing it.
                _ => return Some(name.to_owned()),
            }
        } else {
            raw.push(byte);
        }
    }
    match String::from_utf8(raw) {
        Ok(decoded) => {
            out.push_str(&decoded);
            Some(out)
        }
        Err(_) => Some(name.to_owned()),
    }
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A store nobody declared answers with nothing, without touching the disk.
pub(super) fn nothing() -> NativeProcessActivity {
    NativeProcessActivity::default()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("runtrol-acp-live-{}-{serial}", std::process::id()));
        fs::create_dir_all(&root).expect("the scratch root is created");
        root
    }

    fn spec(grouped: bool) -> LiveSessionSpec {
        LiveSessionSpec {
            root: vec!["store".into()],
            grouped_by_workspace: grouped,
            held: "events.jsonl".into(),
        }
    }

    const OPEN: &str = "01a03d97-af4f-7272-ae66-9033b75f6645";
    const CLOSED: &str = "01a03d98-f0aa-7b33-8f15-0f5623114d5f";

    fn conversation(home: &Path, group: Option<&str>, id: &str) -> PathBuf {
        let mut path = home.join("store");
        if let Some(group) = group {
            path.push(group);
        }
        path.push(id);
        fs::create_dir_all(&path).expect("the conversation directory is created");
        fs::write(path.join("events.jsonl"), b"{}").expect("the held file is written");
        path
    }

    /// Only the conversation whose file a process is holding is live, and the grouping names its workspace.
    #[cfg(windows)]
    #[test]
    fn a_held_file_names_a_live_conversation_and_its_workspace() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;

        let home = scratch();
        let group = "C%3A%5Cwork%5Capp";
        let open = conversation(&home, Some(group), OPEN);
        conversation(&home, Some(group), CLOSED);
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(open.join("events.jsonl"))
            .expect("this process takes the file");

        let found = held_conversations(&home, &spec(true));
        let names: Vec<&str> = found.iter().map(|live| live.native.as_str()).collect();
        assert_eq!(names, vec![OPEN], "only the held conversation is live");
        assert_eq!(
            found.first().and_then(|live| live.workspace.as_deref()),
            Some(r"C:\work\app")
        );

        drop(holder);
        assert!(
            held_conversations(&home, &spec(true)).is_empty(),
            "letting go of the file ends the conversation's liveness"
        );
        drop(fs::remove_dir_all(&home));
    }

    /// What this reads on the machine it is run on, printed rather than asserted.
    ///
    /// Ignored by default because it depends on whether the person has the agent open right now. It is how the
    /// claim in this module's notes was checked against the real thing:
    /// `cargo test -p runtrol-drivers --lib acp::live::tests::this_machine -- --ignored --nocapture`
    #[ignore = "reads the operator's own agent store"]
    #[test]
    fn this_machine_reports_what_the_agent_has_open() {
        let Some(home) = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
        else {
            println!("no operator home on this machine");
            return;
        };
        let spec = LiveSessionSpec {
            root: vec![".grok/sessions".into()],
            grouped_by_workspace: true,
            held: "events.jsonl".into(),
        };
        // Asked in this process: a test binary is not a helper this executable knows how to be.
        let answer = activity(&home, &spec, runtrol_childproc::holder_of_here);
        println!("open conversations: {}", answer.live.len());
        for conversation in &answer.live {
            let bound = answer
                .processes
                .iter()
                .find(|process| process.native == *conversation);
            let held = bound.map_or_else(
                || "unbound".to_owned(),
                |process| {
                    format!(
                        "pid={} cwd={}",
                        process.pid,
                        process.cwd.as_deref().unwrap_or("unknown")
                    )
                },
            );
            println!("  {} {held}", conversation.as_str());
        }
    }

    #[test]
    fn a_store_that_is_not_there_proves_nothing_live() {
        let home = scratch();
        assert!(held_conversations(&home, &spec(true)).is_empty());
        assert!(held_conversations(&home, &spec(false)).is_empty());
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn a_directory_whose_name_is_not_a_conversation_is_stepped_over() {
        let home = scratch();
        let path = home.join("store").join("group").join("not a session id");
        fs::create_dir_all(&path).expect("the directory is created");
        fs::write(path.join("events.jsonl"), b"{}").expect("the file is written");
        assert!(held_conversations(&home, &spec(true)).is_empty());
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn an_escaped_group_name_becomes_the_path_it_was_made_from() {
        let home = scratch();
        let escaped = home.join(r"C%3A%5CUsers%5CMSI%5Ctaxly");
        assert_eq!(
            decode_workspace(&escaped).as_deref(),
            Some(r"C:\Users\MSI\taxly")
        );
        let plain = home.join("no-project");
        assert_eq!(decode_workspace(&plain).as_deref(), Some("no-project"));
        // A name that starts an escape and does not finish it is kept as written rather than mangled.
        let broken = home.join("half%2");
        assert_eq!(decode_workspace(&broken).as_deref(), Some("half%2"));
    }
}
