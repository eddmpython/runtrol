//! The scope wall. Every permission name in the product appears exactly once, in one of two enums.
//!
//! The split is not a policy written down somewhere. It is the type system:
//!
//! - [`DeviceScope`] is everything a paired device can ever hold.
//! - [`LocalScope`] is everything that can never attach to a device, answered per action at the
//!   machine and never stored against anyone.
//!
//! There is no `From<LocalScope> for DeviceScope`, and there must never be one. That single missing
//! conversion is what makes "a phone cannot pair another phone" a compile error rather than a runtime
//! check somebody can forget to write. [`crate::grant::GrantLedger::grant`] accepts a
//! [`DeviceScope`], so handing it a [`LocalScope`] does not fail a check, it fails to build.
//!
//! # Two names the design notes listed that are not here
//!
//! `Panic` and `DeviceRevokeSelf` were written down as device scopes. They are not, because the
//! security posture requires both to work unconditionally: a phone must always be able to kill every
//! session, and a device may always shrink its own authority. A scope that is always granted is not a
//! scope, and modelling one would suggest it could be withheld. Both live as unconditional methods
//! on the ledger, with tests asserting they ignore the grant table entirely.
//!
//! `ApprovalRespondMedium` is also absent. The risk classifier produces two classes, low and high, so
//! a third scope would be unreachable by construction, and an unreachable permission is dead weight
//! in exactly the surface that must stay readable.

use core::fmt;

use runtrol_provider::ProviderId;

use crate::id::WorkspaceRootId;

/// Everything a paired device can ever hold.
///
/// Acquired only through [`crate::grant::GrantLedger::grant`], which requires a
/// [`crate::presence::PcPresence`], which can only come from someone typing a displayed word at this
/// machine. There is no other route, and the absence of one is checked by the dependency graph rather
/// than by review.
///
/// `#[non_exhaustive]` so that adding a capability is not a breaking change for a future transport
/// crate that matches on this.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum DeviceScope {
    /// See which sessions exist, and their state. Not their content.
    SessionList,
    /// Read the events of a session as they arrive, and replay older ones.
    SessionOutputRead,
    /// Submit a turn to a session that already exists.
    SessionInputWrite,
    /// Start a new session in a workspace root the device also holds.
    SessionStart,
    /// Interrupt a running turn, or detach a session.
    SessionStop,
    /// Reattach to a session the provider still has.
    SessionResume,
    /// Remove a session from runtrol's list.
    ///
    /// Held separately from [`Self::SessionStop`] because it is the one session action that cannot be
    /// undone from the phone. Granting it still requires presence at the machine, like every scope
    /// here.
    SessionDelete,
    /// Answer an approval whose risk class is low.
    ApprovalRespondLow,
    /// Answer an approval whose risk class is high.
    ///
    /// High means saying yes changes policy beyond the action in front of you: a persistent allow
    /// rule, a widened permission profile, or a command execution. A device holding only
    /// [`Self::ApprovalRespondLow`] still receives the full option list, with the high-risk options
    /// marked unavailable and a reason, because a silently shortened list would misrepresent what the
    /// provider offered.
    ApprovalRespondHigh,
    /// Read the operator's configuration. Writing it is a [`LocalScope`].
    ConfigRead,
    /// Read the audit record of what was granted, spent, and refused.
    AuditRead,
    /// Run sessions under the provider's ordinary permission mode.
    ModeDefault,
    /// Run sessions under a mode that pre-approves edits inside the workspace.
    ///
    /// Distinct from [`Self::ModeDefault`] because it removes prompts the operator would otherwise
    /// see on their phone. Turning off prompts entirely is [`LocalScope::ModeDangerous`] and is not
    /// grantable at all.
    ModeAcceptEdits,
    /// Work inside one specific approved directory tree.
    ///
    /// Per root, not a blanket permission, so a device can be given one project without being given
    /// every project. A session start needs this scope for the root it names.
    Workspace(WorkspaceRootId),
    /// Drive one specific provider.
    ///
    /// Per provider, so a device can be trusted with one CLI and not another. Which CLIs exist is
    /// discovered at runtime, so this variant carries the discovered identifier rather than the enum
    /// growing a variant per vendor.
    Provider(ProviderId),
}

impl DeviceScope {
    /// A stable name, for messages and for the audit record.
    ///
    /// Written by hand rather than derived from the variant name, because these strings appear in
    /// what the operator reads and in what is stored, and renaming a Rust variant must not silently
    /// rewrite either.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::SessionList => "session.list",
            Self::SessionOutputRead => "session.output.read",
            Self::SessionInputWrite => "session.input.write",
            Self::SessionStart => "session.start",
            Self::SessionStop => "session.stop",
            Self::SessionResume => "session.resume",
            Self::SessionDelete => "session.delete",
            Self::ApprovalRespondLow => "approval.respond.low",
            Self::ApprovalRespondHigh => "approval.respond.high",
            Self::ConfigRead => "config.read",
            Self::AuditRead => "audit.read",
            Self::ModeDefault => "mode.default",
            Self::ModeAcceptEdits => "mode.acceptEdits",
            Self::Workspace(_) => "workspace",
            Self::Provider(_) => "provider",
        }
    }

    /// Whether granting this alone lets a device change the state of the operator's machine.
    ///
    /// Drives how loudly the grant prompt reads. Reading is recoverable; writing, starting, and
    /// deleting are not, and the prompt should not present them in the same tone.
    #[must_use]
    pub const fn changes_the_machine(&self) -> bool {
        match self {
            Self::SessionInputWrite
            | Self::SessionStart
            | Self::SessionDelete
            | Self::ApprovalRespondLow
            | Self::ApprovalRespondHigh
            | Self::ModeAcceptEdits => true,
            Self::SessionList
            | Self::SessionOutputRead
            | Self::SessionStop
            | Self::SessionResume
            | Self::ConfigRead
            | Self::AuditRead
            | Self::ModeDefault
            | Self::Workspace(_)
            | Self::Provider(_) => false,
        }
    }
}

impl fmt::Display for DeviceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(root) => write!(f, "workspace({root})"),
            Self::Provider(provider) => write!(f, "provider({provider})"),
            other => f.write_str(other.name()),
        }
    }
}

/// Everything that can never attach to a device.
///
/// These are answered per action, by someone at the machine, and are never written into the grant
/// ledger. There is no `grant_local`, and no conversion from here into [`DeviceScope`].
///
/// The reason each one is here is the same in every case: holding it would let a remote caller widen
/// its own authority, and the second invariant of the security posture is that a phone may shrink its
/// authority and never grow it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum LocalScope {
    /// Pair a new device.
    ///
    /// A device that could pair another device could grant itself anything through the new one, which
    /// makes every other scope decorative.
    DevicePair,
    /// Change the operator's configuration.
    ///
    /// Configuration decides where work may happen and which executables are trusted. Writing it
    /// remotely is equivalent to holding every scope at once, later.
    ConfigWrite,
    /// Answer approvals automatically, without a human.
    ///
    /// The security posture requires that the worst thing a hostile relay can cause is a denial. An
    /// automatic yes is the one setting that breaks that guarantee, so it cannot be turned on from
    /// anywhere but the keyboard.
    ApprovalAuto,
    /// Run a provider with its permission prompts bypassed.
    ///
    /// The provider's own prompt is the last thing standing between an agent and the operator's
    /// disk. Removing it is a decision that has to be taken in front of the disk in question.
    ModeDangerous,
}

impl LocalScope {
    /// A stable name, for messages and for the audit record.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::DevicePair => "device.pair",
            Self::ConfigWrite => "config.write",
            Self::ApprovalAuto => "approval.auto",
            Self::ModeDangerous => "mode.dangerous",
        }
    }
}

impl fmt::Display for LocalScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_root() -> WorkspaceRootId {
        WorkspaceRootId::now()
    }

    fn a_provider() -> ProviderId {
        ProviderId::parse("codex").expect("valid provider id")
    }

    /// Every device scope, so the exhaustive matches above cannot be satisfied by a subset.
    fn every_device_scope() -> Vec<DeviceScope> {
        vec![
            DeviceScope::SessionList,
            DeviceScope::SessionOutputRead,
            DeviceScope::SessionInputWrite,
            DeviceScope::SessionStart,
            DeviceScope::SessionStop,
            DeviceScope::SessionResume,
            DeviceScope::SessionDelete,
            DeviceScope::ApprovalRespondLow,
            DeviceScope::ApprovalRespondHigh,
            DeviceScope::ConfigRead,
            DeviceScope::AuditRead,
            DeviceScope::ModeDefault,
            DeviceScope::ModeAcceptEdits,
            DeviceScope::Workspace(a_root()),
            DeviceScope::Provider(a_provider()),
        ]
    }

    fn every_local_scope() -> Vec<LocalScope> {
        vec![
            LocalScope::DevicePair,
            LocalScope::ConfigWrite,
            LocalScope::ApprovalAuto,
            LocalScope::ModeDangerous,
        ]
    }

    #[test]
    fn no_name_is_shared_between_the_two_walls() {
        // A shared name would let an audit line or a config key be read as either kind, which is the
        // one ambiguity this split exists to remove.
        for device in every_device_scope() {
            for local in every_local_scope() {
                assert_ne!(
                    device.name(),
                    local.name(),
                    "{device} and {local} share a name"
                );
            }
        }
    }

    #[test]
    fn device_scope_names_are_unique() {
        // The parametrized variants intentionally share a base name with each other and are
        // distinguished by Display, so uniqueness is asserted over the full rendering.
        let rendered: Vec<String> = every_device_scope()
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut sorted = rendered.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), rendered.len(), "duplicate scope rendering");
    }

    #[test]
    fn names_are_stable_shapes_not_variant_spellings() {
        // These strings reach the operator and the stored audit record. A Rust rename must not
        // rewrite them, and this test is what makes that true.
        assert_eq!(DeviceScope::SessionOutputRead.name(), "session.output.read");
        assert_eq!(
            DeviceScope::ApprovalRespondHigh.name(),
            "approval.respond.high"
        );
        assert_eq!(DeviceScope::ModeAcceptEdits.name(), "mode.acceptEdits");
        assert_eq!(LocalScope::ModeDangerous.name(), "mode.dangerous");
    }

    #[test]
    fn parametrized_scopes_render_what_they_name() {
        let root = a_root();
        assert_eq!(
            DeviceScope::Workspace(root).to_string(),
            format!("workspace({root})")
        );
        assert_eq!(
            DeviceScope::Provider(a_provider()).to_string(),
            "provider(codex)"
        );
    }

    #[test]
    fn write_shaped_scopes_are_marked_as_changing_the_machine() {
        for scope in every_device_scope() {
            let expected = matches!(
                scope,
                DeviceScope::SessionInputWrite
                    | DeviceScope::SessionStart
                    | DeviceScope::SessionDelete
                    | DeviceScope::ApprovalRespondLow
                    | DeviceScope::ApprovalRespondHigh
                    | DeviceScope::ModeAcceptEdits
            );
            assert_eq!(
                scope.changes_the_machine(),
                expected,
                "{scope} is classified wrongly"
            );
        }
    }

    #[test]
    fn two_workspace_scopes_are_different_scopes() {
        // Per-root, not blanket. A device given one project must not thereby hold another.
        let one = DeviceScope::Workspace(a_root());
        let two = DeviceScope::Workspace(a_root());
        assert_ne!(one, two);
    }

    #[test]
    fn scopes_are_copy_and_ordered_for_use_as_ledger_keys() {
        // The ledger stores these in a sorted set, so ordering has to be total and cheap.
        let mut scopes = every_device_scope();
        scopes.sort_unstable();
        assert_eq!(scopes.len(), 15);
        let copied = scopes.first().copied();
        assert!(copied.is_some());
    }
}
