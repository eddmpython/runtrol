//! What the window is allowed to know about a session.
//!
//! # Why the window gets its own shapes rather than the wire's
//!
//! Two reasons, and neither is tidiness. The wire types carry identifiers as values a screen cannot render
//! without knowing their encoding, and a window that serialized them directly would pin the wire format to
//! whatever the page happens to read today. And the phone surface will need exactly these shapes, so the
//! translation belongs in one place rather than in two pages.
//!
//! **Nothing here holds a conversation.** A session row is a name, a state, and where to continue it. What was
//! said travels straight from the provider to the surface as the provider wrote it, and never through a struct
//! in this file.

use runtrol_ipc::wire::{SessionLine, SessionListing};
use serde::Serialize;

/// A session list plus any stored rows that could not be read.
///
/// Readable rows remain usable when one stored row is damaged, while the warning keeps that damage visible.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedRows {
    /// Every readable session.
    pub sessions: Vec<Row>,
    /// Storage damage that did not prevent the other rows from being read.
    pub warnings: Vec<String>,
}

impl From<&SessionListing> for ListedRows {
    fn from(listing: &SessionListing) -> Self {
        Self {
            sessions: listing.sessions.iter().map(Row::from).collect(),
            warnings: listing.warnings.iter().map(ToString::to_string).collect(),
        }
    }
}

/// One session, as a row on the screen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// runtrol's own name for it, as text the page can put in an attribute.
    pub session: String,
    /// Which CLI it belongs to.
    pub provider: String,
    /// The provider's own name for the conversation, when it has one.
    ///
    /// `None` before the provider has named it, which for one of the two CLIs is until its first turn. The
    /// page shows a row either way; what it cannot offer is continuing a conversation that does not exist yet.
    pub native: Option<String>,
    /// Where the agent works, in full.
    pub workspace: String,
    /// The last part of that path, which is what a person calls the project.
    ///
    /// Computed here rather than in the page, because splitting a path is platform knowledge and a page that
    /// did it would get one separator right and the other wrong.
    pub folder: String,
    /// Enough of the path to tell two projects apart, keeping its end.
    ///
    /// A rail is a few hundred pixels wide, so a path is going to lose characters somewhere. Which end it
    /// loses decides whether the line is useful: measured on this machine, the rail showed
    /// `C:\Users\MSI\AppData\Local\Temp\rtDemo...` for two different sessions, because CSS truncation drops
    /// the tail and every deep path shares its head. What distinguishes two workspaces is always at the end.
    pub trail: String,
    /// Whether it has a process right now.
    pub hot: bool,
    /// What it is doing, in one word.
    pub doing: String,
    /// It has gone quiet with a turn still running.
    ///
    /// Both halves reach the screen. Showing only the first would read as a completion runtrol never saw, and
    /// showing only the second would hide that work is still going.
    pub looks_stuck: bool,
}

impl From<&SessionLine> for Row {
    fn from(line: &SessionLine) -> Self {
        Self {
            session: line.session.to_string(),
            provider: line.provider.to_string(),
            native: line.native.as_ref().map(ToString::to_string),
            workspace: line.workspace.to_string(),
            folder: folder_of(&line.workspace),
            trail: trail_of(&line.workspace),
            hot: line.hot,
            doing: line.doing.to_string(),
            looks_stuck: line.looks_stuck,
        }
    }
}

/// The last part of a path, which is what a person calls the project.
///
/// Both separators are cut, because a daemon and a window do not have to be on the same platform for a path to
/// arrive with the other one in it, and a root with nothing after it keeps the whole path rather than becoming
/// an empty heading.
fn folder_of(workspace: &str) -> String {
    let trimmed = workspace.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => workspace.to_owned(),
    }
}

/// How many path segments from the end are enough to tell two projects apart.
///
/// Two, because one is the folder name that is already the heading, and the segment above it is what
/// separates `client/src` from `server/src`. More than that and the line is back to being a path nobody
/// can read at this width.
const TRAIL_SEGMENTS: usize = 2;

/// How many characters of path the rail can show before the browser starts cutting it.
///
/// Measured against the rail's own width (276 px by default, at the small step of the type scale). A path
/// under this fits whole, and shortening one that already fits would add a mark meaning "there is more"
/// when there is not.
const TRAIL_BUDGET: usize = 40;

/// The end of a path, with a mark where the rest was cut.
///
/// The whole path stays available to whoever shows this, as the value a hover reveals. What this refuses
/// to do is keep the beginning, which is the part every path on a machine has in common.
fn trail_of(workspace: &str) -> String {
    if workspace.chars().count() <= TRAIL_BUDGET {
        return workspace.to_owned();
    }
    let trimmed = workspace.trim_end_matches(['/', '\\']);
    let segments: Vec<&str> = trimmed
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if segments.len() <= TRAIL_SEGMENTS {
        return workspace.to_owned();
    }
    let kept = segments.split_at(segments.len() - TRAIL_SEGMENTS).1;
    // One separator spelling for the shortened form. It is a label rather than a path somebody will
    // retype, and mixing the machine's separator into an already elided string reads as a real path.
    format!("…/{}", kept.join("/"))
}

/// One provider this build can drive, as the page offers it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offered {
    /// The manifest's identifier, which is what a start request names.
    pub id: String,
    /// What a person calls it.
    pub display_name: String,
    /// Whether a session can be started on it at all.
    pub usable: bool,
    /// Why not, when it cannot.
    ///
    /// Shown rather than hidden. An operator with a perfectly good manifest for a kind this build has no
    /// driver for should see it marked, not wonder where their provider went.
    pub why_not: Option<String>,
}

impl From<&runtrol_ipc::wire::ProviderLine> for Offered {
    fn from(line: &runtrol_ipc::wire::ProviderLine) -> Self {
        Self {
            id: line.id.to_string(),
            display_name: line.display_name.to_string(),
            usable: line.usable,
            why_not: line.why_not.as_ref().map(ToString::to_string),
        }
    }
}

/// One cross-consult direction, as the page shows it.
///
/// The page renders a toggle per direction and never derives state on its own: what it shows is the daemon's
/// answer, which is itself the CLIs' own configuration asked fresh.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsultDirection {
    /// The provider that gains a consultant.
    pub from: String,
    /// The provider whose opinion becomes reachable.
    pub to: String,
    /// `wired`, `unwired`, or `unsupported`, exactly as the wire spells them.
    pub state: String,
    /// The sentence behind an unsupported direction or an unreadable answer.
    pub why: Option<String>,
}

impl From<&runtrol_ipc::wire::ConsultLine> for ConsultDirection {
    fn from(line: &runtrol_ipc::wire::ConsultLine) -> Self {
        let state = match line.state {
            runtrol_ipc::wire::ConsultState::Wired => "wired",
            runtrol_ipc::wire::ConsultState::Unwired => "unwired",
            runtrol_ipc::wire::ConsultState::Unsupported => "unsupported",
        };
        Self {
            from: line.from.to_string(),
            to: line.to.to_string(),
            state: state.to_owned(),
            why: line.why.as_ref().map(ToString::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::SessionId;

    use super::*;

    fn a_line(native: Option<&str>) -> SessionLine {
        SessionLine {
            session: SessionId::now(),
            provider: "codex".into(),
            native: native.map(Into::into),
            workspace: r"C:\work\dartlab".into(),
            hot: true,
            doing: "idle".into(),
            looks_stuck: false,
        }
    }

    #[test]
    fn a_shortened_path_keeps_the_end_because_that_is_what_differs() {
        // Measured on this machine: the rail showed `C:\Users\MSI\AppData\Local\Temp\rtDemo...` for two
        // different workspaces. Every deep path on one machine shares its head, so a form that keeps the
        // head is a label that cannot tell two projects apart, which is the only thing it is for.
        let deep = trail_of(r"C:\Users\MSI\AppData\Local\Temp\rtDemo9k\dartlab");
        assert_eq!(deep, "…/rtDemo9k/dartlab");
        assert!(
            !deep.contains("Users"),
            "the common head must be the part that goes: {deep}"
        );

        // Both separators, because a daemon and a window do not have to be on the same platform.
        assert_eq!(
            trail_of("/home/msi/Development/projects/acme/client/src"),
            "…/client/src"
        );
    }

    #[test]
    fn a_path_short_enough_to_read_is_left_alone() {
        // Eliding something that already fits would add a mark that means "there is more" when there is not.
        // The separator also stays the machine's own here, because nothing was cut.
        assert_eq!(trail_of(r"C:\work"), r"C:\work");
        assert_eq!(trail_of("/work/thing"), "/work/thing");
        assert_eq!(trail_of(r"C:\work\dartlab"), r"C:\work\dartlab");
        assert_eq!(
            trail_of("/a/b/c/d/e/f"),
            "/a/b/c/d/e/f",
            "a deep path that still fits is about width, not about how many segments it has"
        );
    }

    #[test]
    fn a_row_carries_the_name_a_resume_takes() {
        // The same rule the terminal listing learned: a surface that shows a session and withholds the
        // conversation's own name shows a session nobody can pick back up.
        let row = Row::from(&a_line(Some("thread_abc")));
        assert_eq!(row.native.as_deref(), Some("thread_abc"));
        assert_eq!(row.provider, "codex");
    }

    #[test]
    fn a_session_the_provider_has_not_named_is_still_a_row() {
        // One of the two CLIs has no conversation until its first turn. The row exists; what it cannot offer
        // is continuing something that does not exist.
        let row = Row::from(&a_line(None));
        assert!(row.native.is_none());
    }

    #[test]
    fn nothing_on_a_row_can_hold_a_conversation() {
        // The thin rule as a shape. Every field is an identifier, a flag, or a single word, and there is
        // nowhere here for what somebody said to be put.
        let row = Row::from(&a_line(Some("thread_abc")));
        let encoded = serde_json::to_string(&row).expect("serializable");
        for field in [
            "session",
            "provider",
            "native",
            "workspace",
            "folder",
            "trail",
            "hot",
            "doing",
            "looksStuck",
        ] {
            assert!(encoded.contains(field), "{encoded}");
        }
        // Counted as keys rather than as punctuation. Counting colons was the first attempt and a Windows path
        // carries one of its own, so the check failed on the value instead of on the shape.
        let parsed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&encoded).expect("an object");
        assert_eq!(
            parsed.len(),
            9,
            "a row grew a field, and the only fields it may have are these nine: {encoded}"
        );
    }

    #[test]
    fn the_project_name_is_the_last_part_of_the_path() {
        // What the sidebar groups by. Both separators are cut, because the daemon and the window do not have
        // to be on the same platform for a path to arrive with the other one in it.
        assert_eq!(folder_of(r"C:\work\dartlab"), "dartlab");
        assert_eq!(folder_of("/home/me/work/dartlab/"), "dartlab");
        assert_eq!(folder_of("dartlab"), "dartlab");
        // A root with nothing after it keeps the whole path rather than becoming an empty heading.
        assert_eq!(folder_of("/"), "/");
        assert_eq!(folder_of(""), "");
    }
}
