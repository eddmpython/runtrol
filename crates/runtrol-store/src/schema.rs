//! The on-disk shape, and the version byte that stops a stale binary from reinterpreting it.
//!
//! # Why the version is in every type name
//!
//! The storage engine records a type name per table and refuses a table whose recorded name does not match
//! what the code asks for. Baking the schema version into those names turns "an old binary opened a new
//! file" from silent byte reinterpretation into a loud, immediate refusal. It costs nothing and it is the
//! difference between a confusing session list and an error message.
//!
//! # Why session keys are raw UUID bytes
//!
//! A UUIDv7's first 48 bits are a big-endian millisecond timestamp, so comparing the raw bytes **is**
//! comparing creation time. A range scan therefore returns sessions in the order a person expects with no
//! secondary index and no sorting at read time, which is what makes "the list opens with no wait" cheap
//! rather than clever.
//!
//! The engine's own optional UUID support is deliberately not used: it would name the type after the UUID
//! crate, discarding both the version tag and the newtype that keeps one kind of identifier from being
//! stored where another belongs.

use core::cmp::Ordering;

use redb::{Key, TableDefinition, TypeName, Value};
use runtrol_provider::SessionId;

/// The on-disk format this build writes and understands.
///
/// One number, and every type name below carries it. A change here without a migration is caught by the
/// schema gate rather than by a user losing their session list.
pub const SCHEMA_VERSION: u8 = 1;

/// Type name for [`SessionKey`], version included.
const T_SESSION_KEY: &str = "runtrol::SessionKey@1";

/// Type name for [`DeviceKey`], version included.
const T_DEVICE_KEY: &str = "runtrol::DeviceKey@1";

/// Key of the `meta` entry holding the schema version.
pub const META_SCHEMA_VERSION: &str = "schema_version";

/// Key of the `meta` entry naming the runtrol that wrote the file.
///
/// Recorded so a schema refusal can say which build to look for, instead of leaving the operator guessing
/// which version of runtrol they were running last month.
pub const META_WRITTEN_BY: &str = "written_by";

/// Small named values about the file itself.
pub const META: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("meta");

/// The session pointers runtrol owns. Roughly 200 bytes each, and never a transcript.
pub const SESSIONS: TableDefinition<'static, SessionKey, &[u8]> = TableDefinition::new("sessions");

/// Paired device identities and the exact remotely grantable authority approved at the PC.
///
/// Rows contain public identity material and a bearer-token fingerprint, never the bearer token itself.
pub const DEVICES: TableDefinition<'static, DeviceKey, &[u8]> = TableDefinition::new("devices");

/// Provider identifier and native session identifier, to the runtrol session.
///
/// Two jobs: resuming by the identifier a provider gave, and de-duplicating when the live list is unioned
/// with the stored rows.
///
/// Typed as a pair of strings rather than a pair of newtypes, and the safety lives one level up: the public
/// surface takes typed identifiers, so no caller ever assembles this tuple by hand and no caller can swap
/// the halves. The on-disk form is readable text, which matters the day somebody has to inspect a database
/// without runtrol.
pub const NATIVE_INDEX: TableDefinition<'static, (&str, &str), SessionKey> =
    TableDefinition::new("native_ix");

/// The last observed live-stream source boundary and event sequence for each session.
///
/// The one table written without durability. These values are advisory diagnostics, not reconnect
/// `WatchCursor` values. Losing one does not lose a durable session pointer or cause a transcript scan.
pub const CURSORS: TableDefinition<'static, SessionKey, (u64, u64)> =
    TableDefinition::new("cursors");

/// A session's primary key: the raw bytes of its identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SessionKey([u8; 16]);

impl SessionKey {
    /// The key for a session.
    #[must_use]
    pub const fn of(session: SessionId) -> Self {
        Self(*session.as_bytes())
    }

    /// The session this key belongs to.
    #[must_use]
    pub const fn session(self) -> SessionId {
        SessionId::from_bytes(self.0)
    }

    /// The lowest possible key, for opening a range scan.
    pub const FIRST: Self = Self([0; 16]);

    /// The highest possible key, for closing a range scan.
    pub const LAST: Self = Self([0xFF; 16]);
}

impl Value for SessionKey {
    type SelfType<'a>
        = Self
    where
        Self: 'a;
    type AsBytes<'a>
        = [u8; 16]
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        Some(16)
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self
    where
        Self: 'a,
    {
        let mut bytes = [0_u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(data) {
            *slot = *byte;
        }
        Self(bytes)
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self) -> [u8; 16]
    where
        Self: 'b,
    {
        value.0
    }

    fn type_name() -> TypeName {
        TypeName::new(T_SESSION_KEY)
    }
}

impl Key for SessionKey {
    /// Byte order, which for a time-ordered identifier is creation order.
    ///
    /// This is the whole reason the key is raw bytes. A range scan comes back in the order a person reads a
    /// session list, with no secondary index and no sort after the read.
    fn compare(left: &[u8], right: &[u8]) -> Ordering {
        left.cmp(right)
    }
}

/// A paired device's primary key: the raw bytes of its locally minted identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DeviceKey([u8; 16]);

impl DeviceKey {
    /// Rebuild a device key from the locally minted identifier bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Return the locally minted identifier bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// The lowest possible key, for opening a range scan.
    pub const FIRST: Self = Self([0; 16]);

    /// The highest possible key, for closing a range scan.
    pub const LAST: Self = Self([0xFF; 16]);
}

impl Value for DeviceKey {
    type SelfType<'a>
        = Self
    where
        Self: 'a;
    type AsBytes<'a>
        = [u8; 16]
    where
        Self: 'a;

    fn fixed_width() -> Option<usize> {
        Some(16)
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self
    where
        Self: 'a,
    {
        let mut bytes = [0_u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(data) {
            *slot = *byte;
        }
        Self(bytes)
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self) -> [u8; 16]
    where
        Self: 'b,
    {
        value.0
    }

    fn type_name() -> TypeName {
        TypeName::new(T_DEVICE_KEY)
    }
}

impl Key for DeviceKey {
    fn compare(left: &[u8], right: &[u8]) -> Ordering {
        left.cmp(right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_name_carries_the_schema_version() {
        // The engine refuses a table whose recorded type name differs from the one the code asks for. That
        // refusal is what turns "an old binary opened a new file" into an error instead of silent byte
        // reinterpretation, and it only works if the version is actually in the name.
        let suffix = format!("@{SCHEMA_VERSION}");
        assert!(
            T_SESSION_KEY.ends_with(&suffix),
            "{T_SESSION_KEY} does not carry schema version {SCHEMA_VERSION}"
        );
        assert!(
            T_DEVICE_KEY.ends_with(&suffix),
            "{T_DEVICE_KEY} does not carry schema version {SCHEMA_VERSION}"
        );
    }

    #[test]
    fn a_session_key_round_trips() {
        let session = SessionId::now();
        let key = SessionKey::of(session);
        assert_eq!(key.session(), session);

        let encoded = <SessionKey as Value>::as_bytes(&key);
        assert_eq!(<SessionKey as Value>::from_bytes(&encoded), key);
    }

    #[test]
    fn keys_sort_in_creation_order() {
        // The property the whole key encoding exists for: a range scan returns sessions oldest first with no
        // secondary index and no sorting at read time.
        let first = SessionId::now();
        let second = SessionId::now();
        let third = SessionId::now();

        let mut keys = vec![
            SessionKey::of(third),
            SessionKey::of(first),
            SessionKey::of(second),
        ];
        keys.sort_by(|left, right| {
            <SessionKey as Key>::compare(
                &<SessionKey as Value>::as_bytes(left),
                &<SessionKey as Value>::as_bytes(right),
            )
        });
        assert_eq!(
            keys,
            vec![
                SessionKey::of(first),
                SessionKey::of(second),
                SessionKey::of(third)
            ]
        );
    }

    #[test]
    fn the_scan_bounds_enclose_every_real_key() {
        let key = SessionKey::of(SessionId::now());
        assert!(SessionKey::FIRST < key);
        assert!(key < SessionKey::LAST);
    }

    #[test]
    fn a_session_key_is_exactly_sixteen_bytes() {
        // Fixed width is what lets the engine store these without a length prefix, and it is part of the
        // roughly 200 bytes per session contract.
        assert_eq!(<SessionKey as Value>::fixed_width(), Some(16));
        assert_eq!(size_of::<SessionKey>(), 16);
    }

    #[test]
    fn a_device_key_round_trips_and_stays_fixed_width() {
        let bytes = [0xA5; 16];
        let key = DeviceKey::from_bytes(bytes);
        assert_eq!(key.to_bytes(), bytes);

        let encoded = <DeviceKey as Value>::as_bytes(&key);
        assert_eq!(<DeviceKey as Value>::from_bytes(&encoded), key);
        assert_eq!(<DeviceKey as Value>::fixed_width(), Some(16));
        assert_eq!(size_of::<DeviceKey>(), 16);
    }
}
