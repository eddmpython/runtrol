//! Identifiers this crate owns: a paired device, and a configured workspace root.
//!
//! These live here rather than in `runtrol-provider` because a provider driver has no business
//! knowing that devices or workspace roots exist. A driver spawns a CLI; who was allowed to ask for
//! that, and where it was allowed to run, are decisions taken before the driver is reached.

use core::fmt;

use uuid::Uuid;

/// A device that has been paired with this machine.
///
/// UUIDv7, so devices sort by pairing time and an audit listing reads chronologically without a
/// second stored field.
///
/// Not derived from anything the device controls. A device-chosen identifier would let a second
/// device claim the first one's grants by presenting the same value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(Uuid);

impl DeviceId {
    /// Mint an identifier for a device being paired.
    ///
    /// Called at the machine, during pairing, which is a [`crate::LocalScope`] action. There is no
    /// path by which a remote caller reaches this.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }

    /// Rebuild from stored bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    /// The 16 bytes, big-endian.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.0.as_hyphenated())
    }
}

/// A directory tree the operator has approved as a place work may happen.
///
/// Minted when the root is added, rather than derived from the path. A derived identifier (a hash of
/// the path, say) would mean that removing a root and later adding the same directory silently
/// resurrects every grant that named it. Minting makes removal final.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceRootId(Uuid);

impl WorkspaceRootId {
    /// Mint an identifier for a root being added.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }

    /// Rebuild from stored bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    /// The 16 bytes, big-endian.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for WorkspaceRootId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for WorkspaceRootId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkspaceRootId({})", self.0.as_hyphenated())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_sort_by_mint_time() {
        assert!(DeviceId::now() < DeviceId::now());
        assert!(WorkspaceRootId::now() < WorkspaceRootId::now());
    }

    #[test]
    fn identifiers_round_trip_through_bytes() {
        let device = DeviceId::now();
        assert_eq!(device, DeviceId::from_bytes(*device.as_bytes()));
        let root = WorkspaceRootId::now();
        assert_eq!(root, WorkspaceRootId::from_bytes(*root.as_bytes()));
    }

    #[test]
    fn a_reminted_root_is_never_the_old_root() {
        // Removing a root and re-adding the same directory must not resurrect its grants.
        let first = WorkspaceRootId::now();
        let second = WorkspaceRootId::now();
        assert_ne!(first, second);
    }
}
