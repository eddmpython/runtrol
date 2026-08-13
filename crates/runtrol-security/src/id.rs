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

    /// Rebuild from stored UUIDv7 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Option<Self> {
        if is_uuid_v7(&bytes) {
            Some(Self(Uuid::from_bytes(bytes)))
        } else {
            None
        }
    }

    /// The 16 bytes, big-endian.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Rebuild a device identifier from its canonical stored text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match Uuid::parse_str(text) {
            Ok(id) => Self::from_bytes(*id.as_bytes()),
            Err(_) => None,
        }
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

    /// Rebuild from stored UUIDv7 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Option<Self> {
        if is_uuid_v7(&bytes) {
            Some(Self(Uuid::from_bytes(bytes)))
        } else {
            None
        }
    }

    /// The 16 bytes, big-endian.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Rebuild a workspace root identifier from its canonical stored text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match Uuid::parse_str(text) {
            Ok(id) => Self::from_bytes(*id.as_bytes()),
            Err(_) => None,
        }
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

const fn is_uuid_v7(bytes: &[u8; 16]) -> bool {
    bytes[6] & 0xF0 == 0x70 && bytes[8] & 0xC0 == 0x80
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
        assert_eq!(Some(device), DeviceId::from_bytes(*device.as_bytes()));
        assert_eq!(Some(device), DeviceId::parse(&device.to_string()));
        assert_eq!(None, DeviceId::parse("not-a-device"));
        let root = WorkspaceRootId::now();
        assert_eq!(Some(root), WorkspaceRootId::from_bytes(*root.as_bytes()));
        assert_eq!(DeviceId::from_bytes([0; 16]), None);
        assert_eq!(WorkspaceRootId::from_bytes([0; 16]), None);
    }

    #[test]
    fn workspace_roots_round_trip_through_canonical_text() {
        let root = WorkspaceRootId::now();
        assert_eq!(WorkspaceRootId::parse(&root.to_string()), Some(root));
        assert_eq!(WorkspaceRootId::parse("not-a-root"), None);
    }

    #[test]
    fn a_reminted_root_is_never_the_old_root() {
        // Removing a root and re-adding the same directory must not resurrect its grants.
        let first = WorkspaceRootId::now();
        let second = WorkspaceRootId::now();
        assert_ne!(first, second);
    }
}
