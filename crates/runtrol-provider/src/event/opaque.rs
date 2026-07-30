//! JSON that runtrol carries and never reads.
//!
//! This type is where the thin rule stops being a promise. runtrol supervises processes and moves
//! events; it does not interpret, rewrite, or keep a copy of a conversation. Everything a subscriber
//! renders (message text, reasoning, tool titles, diffs, terminal output, plan entries, error strings)
//! travels inside an [`Opaque`], and the supervisor reads exactly none of it.

use core::fmt;

use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The provider's own JSON, byte for byte.
///
/// Backed by the exact bytes the provider produced, shared rather than copied. Cloning is a refcount
/// bump, so fanning one event out to a dozen subscribers costs a dozen pointers and zero bytes. That
/// property is what lets the daemon stay inside its memory budget while a phone, a terminal, and a
/// window all watch the same session.
///
/// Constructed from a parse that already validated the JSON, so the text inside is always well formed.
#[derive(Clone, PartialEq, Eq)]
pub struct Opaque(Bytes);

impl Opaque {
    /// Nothing at all, as the JSON literal `null`.
    ///
    /// For frames runtrol itself originates, which have no provider payload to carry.
    #[must_use]
    pub fn none() -> Self {
        Self(Bytes::from_static(b"null"))
    }

    /// Take a slice of a line already in memory, without copying it.
    ///
    /// `slice` must point inside `line`. That is not a suggestion: the shared buffer is resolved by
    /// pointer arithmetic against the parent allocation, so a slice from somewhere else has no offset
    /// to compute and this returns `None` rather than guessing.
    ///
    /// The caller satisfies the requirement by construction: `slice` comes from parsing `line`.
    #[must_use]
    pub fn borrowed_from(line: &Bytes, slice: &str) -> Option<Self> {
        let start = line.as_ptr() as usize;
        let end = start.checked_add(line.len())?;
        let inner = slice.as_ptr() as usize;
        let inner_end = inner.checked_add(slice.len())?;
        if inner < start || inner_end > end {
            return None;
        }
        Some(Self(line.slice_ref(slice.as_bytes())))
    }

    /// Take ownership of JSON runtrol built itself.
    ///
    /// One copy, at a call site that already owns a `String`. Used for frames runtrol originates, never
    /// on the path that carries provider output.
    #[must_use]
    pub fn owned(json: String) -> Self {
        Self(Bytes::from(json))
    }

    /// The provider's JSON as text.
    ///
    /// Always valid UTF-8: the bytes came from a successful JSON parse, and JSON is UTF-8 by
    /// definition. The unreachable branch answers `null` rather than panicking, because a corrupted
    /// payload should surface as visibly empty content in one frame rather than take down a daemon
    /// supervising other sessions.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match core::str::from_utf8(&self.0) {
            Ok(text) => text,
            Err(_) => "null",
        }
    }

    /// The shared bytes. Cloning these is a refcount bump.
    ///
    /// What goes into the replay ring and out to every subscriber.
    #[must_use]
    pub fn bytes(&self) -> Bytes {
        self.0.clone()
    }

    /// How many bytes this payload occupies.
    ///
    /// Read by the fan-out to enforce its queue budget. Counting bytes is not reading them.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The name `serde_json` watches for when deciding to paste text instead of encoding a value.
///
/// Emitting this shape is how a payload reaches a subscriber byte for byte. It is `serde_json`'s
/// internal token, so this file does not get to assume it is stable: the round-trip test below is the
/// guard, and it goes red the day the mechanism changes rather than silently double-encoding every
/// payload in the product.
const RAW_JSON_TOKEN: &str = "$serde_json::private::RawValue";

impl Serialize for Opaque {
    /// Emitted verbatim: no re-encoding, no key reordering, no reformatting.
    ///
    /// A subscriber receives exactly the bytes the provider wrote. Round-tripping the payload through a
    /// JSON model would be reading it, and it would also silently normalize things (key order, number
    /// formatting, float precision) that are the provider's business and that the subscriber may be
    /// comparing against the provider's own store.
    ///
    /// Nothing is allocated here. Building an owned raw value per frame would put one copy of every
    /// payload on the fan-out path, which is the cost this whole type exists to avoid.
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut raw = ser.serialize_struct(RAW_JSON_TOKEN, 1)?;
        raw.serialize_field(RAW_JSON_TOKEN, self.as_str())?;
        raw.end()
    }
}

impl<'de> Deserialize<'de> for Opaque {
    /// Read a payload back as the bytes it was written as.
    ///
    /// The other half of the pass-through. Without it a payload could cross runtrol's own wire and not be readable at
    /// the far end, which would leave the command surface unable to receive an event at all.
    ///
    /// Taken as a raw value rather than through a JSON model, for the same reason writing is: round-tripping through a
    /// model would be reading it, and it would silently normalize key order and number formatting that the provider
    /// chose and that a subscriber may be comparing against the provider's own store.
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let raw = <&serde_json::value::RawValue as Deserialize>::deserialize(de)?;
        Ok(Self(Bytes::copy_from_slice(raw.get().as_bytes())))
    }
}

impl fmt::Debug for Opaque {
    /// Prints the size and never the contents.
    ///
    /// Not a nicety. This is the mechanical guarantee that no `tracing::debug!("{event:?}")` anywhere
    /// in the tree can spill a transcript into a log file. The one place conversation text could leak
    /// without anybody intending it is a debug format, so the debug format does not have it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opaque({} bytes)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_borrowed_payload_shares_the_line_it_came_from() {
        // The whole reason this type exists: fan-out must cost pointers, not bytes.
        let line = Bytes::from_static(br#"{"type":"assistant","text":"hello there"}"#);
        let text = core::str::from_utf8(&line).expect("ascii");
        let inner = text
            .split_once(r#""text":"#)
            .map(|(_, tail)| tail)
            .expect("payload present");

        let opaque = Opaque::borrowed_from(&line, inner).expect("the slice is inside the line");
        assert_eq!(opaque.as_str(), r#""hello there"}"#);

        // Same allocation, so cloning cannot copy.
        let cloned = opaque.clone();
        assert_eq!(cloned.bytes().as_ptr(), opaque.bytes().as_ptr());
    }

    #[test]
    fn a_slice_from_elsewhere_is_refused_rather_than_guessed() {
        // Resolving the offset by pointer arithmetic against the wrong buffer would compute nonsense.
        let line = Bytes::from_static(b"{\"a\":1}");
        let stranger = String::from("not from that line");
        assert!(Opaque::borrowed_from(&line, &stranger).is_none());
    }

    #[test]
    fn debug_prints_a_size_and_never_the_content() {
        // A log line must not be able to carry a transcript, whatever anyone writes.
        let secret = r#"{"text":"the user's private message"}"#;
        let opaque = Opaque::owned(secret.to_owned());
        let printed = format!("{opaque:?}");
        assert!(!printed.contains("private"), "content leaked: {printed}");
        assert!(printed.contains(&secret.len().to_string()));
    }

    #[test]
    fn serialization_is_byte_for_byte() {
        // A subscriber renders the provider's payload, so the payload has to arrive unaltered. Key
        // order and spacing are the provider's business, not runtrol's.
        let original = r#"{"z":1,"a":[2,3],"nested":{"k":"v"}}"#;
        let opaque = Opaque::owned(original.to_owned());
        let encoded = serde_json::to_string(&opaque).expect("serializable");
        assert_eq!(encoded, original);
    }

    #[test]
    fn an_empty_payload_is_json_null() {
        let nothing = Opaque::none();
        assert_eq!(nothing.as_str(), "null");
        assert_eq!(
            serde_json::to_string(&nothing).expect("serializable"),
            "null"
        );
        assert!(
            !nothing.is_empty(),
            "the four bytes of `null` are still bytes"
        );
        assert_eq!(nothing.len(), 4);
    }

    #[test]
    fn size_is_one_shared_buffer_handle() {
        // Every event carries at least one of these, so its width is part of the memory contract.
        assert_eq!(size_of::<Opaque>(), size_of::<Bytes>());
    }
}
