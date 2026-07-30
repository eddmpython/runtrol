//! Anything runtrol has no binding for, carried through anyway.
//!
//! This is the single most load-bearing type in the event model, and it exists because of a specific
//! failure other people have already had: a wrapper built around a coding CLI, where the CLI changed
//! constantly until the wrapper became unmaintainable.
//!
//! runtrol's answer is structural rather than diligent. A provider can ship fifty notifications runtrol
//! has never seen, and each becomes an [`Unmapped`] frame that reaches the subscriber verbatim. Nothing
//! breaks, nothing is dropped, and the subscriber can render what it understands.
//!
//! The bound surface is small on purpose. One CLI offers 126 requests, 70 notifications, and 11
//! server-to-client requests; runtrol binds roughly a quarter of that. Everything else passes through
//! here. Binding one more method is a one-line change to a table, and it is a reviewable event.

use serde::Serialize;

use crate::event::Opaque;
use crate::id::TurnId;

/// A frame runtrol did not translate.
#[derive(Debug, Clone, Serialize)]
pub struct Unmapped {
    /// The provider's own discriminator, verbatim.
    ///
    /// Not normalized, not lowercased, not prefixed. A subscriber matching on this is matching on what the
    /// provider actually said, which is the only thing that stays true across a vendor change.
    pub tag: Box<str>,
    /// The turn it belongs to, when runtrol could tell.
    pub turn: Option<TurnId>,
    /// The whole frame, verbatim.
    pub payload: Opaque,
    /// runtrol has never seen this tag before.
    ///
    /// This one bit is the entire drift metric. `false` means the tag is in a driver's table and its
    /// disposition is to ignore it, which is deliberate, expected noise. `true` means a vendor shipped
    /// something new.
    ///
    /// Without the distinction, an unmapped-rate alarm mixes known noise with genuine vendor change and
    /// becomes useless: it either fires constantly or is tuned until it never fires.
    pub unknown_to_binding: bool,
}

impl Unmapped {
    /// Whether this frame is evidence that a provider changed underneath runtrol.
    ///
    /// The question a drift alarm asks. Deliberate noise does not count.
    #[must_use]
    pub const fn is_drift(&self) -> bool {
        self.unknown_to_binding
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_tag_reaches_the_subscriber_whole() {
        // A vendor can ship anything. Nothing is dropped, and the payload arrives unaltered.
        let body = r#"{"method":"thread/somethingBrandNew","params":{"a":1}}"#;
        let frame = Unmapped {
            tag: "thread/somethingBrandNew".into(),
            turn: None,
            payload: Opaque::owned(body.to_owned()),
            unknown_to_binding: true,
        };
        assert!(frame.is_drift());
        let encoded = serde_json::to_string(&frame).expect("serializable");
        assert!(encoded.contains(body), "payload altered: {encoded}");
    }

    #[test]
    fn deliberate_noise_is_not_drift() {
        // One CLI emits six startup-status notifications per turn, sub-second, not user facing. Counting
        // those as drift would make the alarm fire constantly and then be turned off.
        let noise = Unmapped {
            tag: "mcpServer/startupStatus/updated".into(),
            turn: None,
            payload: Opaque::owned("{}".to_owned()),
            unknown_to_binding: false,
        };
        assert!(!noise.is_drift());
    }

    #[test]
    fn the_tag_is_the_providers_own_spelling() {
        // Normalizing it would break a subscriber matching on what the provider actually said.
        let frame = Unmapped {
            tag: "item/fileChange/patchUpdated".into(),
            turn: None,
            payload: Opaque::none(),
            unknown_to_binding: true,
        };
        assert_eq!(&*frame.tag, "item/fileChange/patchUpdated");
    }

    #[test]
    fn an_unmapped_payload_never_reaches_a_log_line() {
        // An unmapped frame is the most likely thing to be logged during an investigation, which makes it
        // the most likely place for a transcript to leak.
        let frame = Unmapped {
            tag: "assistant".into(),
            turn: None,
            payload: Opaque::owned(r#"{"text":"my private message"}"#.to_owned()),
            unknown_to_binding: true,
        };
        let printed = format!("{frame:?}");
        assert!(!printed.contains("private"), "leaked: {printed}");
        assert!(printed.contains("assistant"), "but the tag is visible");
    }
}
