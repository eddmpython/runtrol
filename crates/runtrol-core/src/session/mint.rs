//! Who names a session, and what runtrol does with the name.
//!
//! runtrol mints its own identifier for every session and never owns the conversation behind it. Two
//! identifiers exist for one session and they are not the same thing:
//!
//! - **runtrol's**, minted here, time-ordered, and the key its own records hang off.
//! - **the provider's**, which the provider decides, and which is what a resume command takes.
//!
//! # Why runtrol mints one at all, when the provider has one
//!
//! Because the provider's is not available when it is needed. A session that has never been started has no
//! provider identifier yet, and a phone answering an approval needs a stable name for the session it is
//! answering about. Measured on one CLI, the provider's own handle for a pending request is scoped to a single
//! connection and does not survive a reconnect at all.
//!
//! # The one provider that takes runtrol's name
//!
//! One of the two supported CLIs accepts an identifier at start time, so runtrol hands it the one it just
//! minted and the two identifiers are equal. The provider returns that identifier through its structured session
//! surface. That is a convenience of that provider, **not an invariant**: the code below never assumes the two are
//! equal, because the other provider issues its own.
//!
//! # The latest observation wins
//!
//! A provider can report a different identifier than the one runtrol last recorded, and it does: resuming can
//! produce a new one, and forking always does. The rule is that the newest observation replaces the older one,
//! because a resume command has to be given the identifier that names the conversation now. An older one names
//! something the provider may have already superseded.

use runtrol_provider::{NativeSessionId, ProviderId, SessionId};

/// A session's two names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identity {
    /// Which CLI it belongs to.
    provider: ProviderId,
    /// runtrol's own name for it. Never changes.
    session: SessionId,
    /// The provider's name for it, once the provider has said one.
    native: Option<NativeSessionId>,
}

impl Identity {
    /// Use a session name that was minted before a provider process was opened.
    #[must_use]
    pub const fn assigned(provider: ProviderId, session: SessionId) -> Self {
        Self {
            provider,
            session,
            native: None,
        }
    }

    /// Mint a name for a session that does not exist yet.
    ///
    /// Time-ordered, so the raw bytes sort by when the session was made. That is what lets the session list
    /// come back in the order a person expects from a single range scan, with no index and no sorting.
    #[must_use]
    pub fn mint(provider: ProviderId) -> Self {
        Self::assigned(provider, SessionId::now())
    }

    /// Take on an existing provider-owned session whose identifiers the caller already has.
    ///
    /// The provider's identifier is known from the start here, because the session already existed before
    /// runtrol heard of it.
    #[must_use]
    pub fn discovered(provider: ProviderId, session: SessionId, native: NativeSessionId) -> Self {
        Self {
            provider,
            session,
            native: Some(native),
        }
    }

    /// Which CLI it belongs to.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    /// runtrol's own name.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    /// The provider's name, once it has said one.
    #[must_use]
    pub const fn native(&self) -> Option<&NativeSessionId> {
        self.native.as_ref()
    }

    /// The name to hand a provider that accepts one.
    ///
    /// runtrol's own identifier, in the text form. A provider that takes this ends up agreeing with runtrol
    /// about the name, which is what makes a resume work after the operator has deleted every runtrol record.
    #[must_use]
    pub fn offer_to_provider(&self) -> String {
        self.session.to_string()
    }

    /// Record what the provider says it calls this session.
    ///
    /// Returns what was replaced, when the provider changed its mind. A caller that wants to know (a resume
    /// that produced a new identifier, a fork) gets a value rather than having to compare before and after.
    pub fn observe_native(&mut self, native: NativeSessionId) -> Option<NativeSessionId> {
        match &self.native {
            Some(held) if *held == native => None,
            // Newest wins. A resume command must be given the identifier that names the conversation now, and
            // an older one may name something the provider has already superseded.
            _ => self.native.replace(native),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> ProviderId {
        ProviderId::parse("example").expect("the test's own id must be valid")
    }

    fn native(text: &str) -> NativeSessionId {
        NativeSessionId::new(text).expect("the test's own id must be valid")
    }

    #[test]
    fn a_minted_name_exists_before_the_provider_has_one() {
        // A session that has never been started has no provider identifier, and a phone answering an approval
        // still needs a stable name for what it is answering about.
        let identity = Identity::mint(provider());
        assert_eq!(identity.native(), None);
        assert_eq!(identity.provider(), provider());
        assert!(!identity.offer_to_provider().is_empty());
    }

    #[test]
    fn minted_names_sort_by_when_they_were_made() {
        // What lets the session list come back in the order a person expects from one range scan, with no
        // index and no sorting after the read.
        let first = Identity::mint(provider());
        let second = Identity::mint(provider());
        let third = Identity::mint(provider());

        let mut names = [third.session(), first.session(), second.session()];
        names.sort_by_key(|id| *id.as_bytes());
        assert_eq!(names, [first.session(), second.session(), third.session()]);
    }

    #[test]
    fn runtrols_own_name_never_changes() {
        let mut identity = Identity::mint(provider());
        let minted = identity.session();

        identity.observe_native(native("thread_01"));
        identity.observe_native(native("thread_02"));

        assert_eq!(
            identity.session(),
            minted,
            "the key runtrol's own records hang off cannot move"
        );
    }

    #[test]
    fn the_newest_provider_name_wins_and_the_old_one_comes_back() {
        // Resuming can produce a new identifier and forking always does. A resume command has to be given the
        // one that names the conversation now.
        let mut identity = Identity::mint(provider());

        assert_eq!(identity.observe_native(native("thread_01")), None);
        assert_eq!(
            identity.native().map(NativeSessionId::as_str),
            Some("thread_01")
        );

        let replaced = identity.observe_native(native("thread_02"));
        assert_eq!(
            replaced.as_ref().map(NativeSessionId::as_str),
            Some("thread_01"),
            "a caller that needs to know what was superseded gets it as a value"
        );
        assert_eq!(
            identity.native().map(NativeSessionId::as_str),
            Some("thread_02")
        );
    }

    #[test]
    fn hearing_the_same_name_again_is_not_a_change() {
        // Every attach re-reports it. Treating each report as a change would make a fork indistinguishable
        // from an ordinary reconnect.
        let mut identity = Identity::mint(provider());
        identity.observe_native(native("thread_01"));
        assert_eq!(
            identity.observe_native(native("thread_01")),
            None,
            "the same name is not news"
        );
    }

    #[test]
    fn an_existing_provider_session_arrives_with_both_names() {
        let session = SessionId::now();
        let identity = Identity::discovered(provider(), session, native("thread_09"));
        assert_eq!(identity.session(), session);
        assert_eq!(
            identity.native().map(NativeSessionId::as_str),
            Some("thread_09")
        );
    }

    #[test]
    fn nothing_here_assumes_the_two_names_are_equal() {
        // One of the two supported CLIs accepts runtrol's identifier and the two end up equal. That is a
        // convenience of that provider, not an invariant, and the other one issues its own.
        let mut agrees = Identity::mint(provider());
        let offered = agrees.offer_to_provider();
        agrees.observe_native(native(&offered));
        assert_eq!(
            agrees.native().map(NativeSessionId::as_str),
            Some(offered.as_str())
        );

        let mut disagrees = Identity::mint(provider());
        disagrees.observe_native(native("thread_totally_different"));
        assert_ne!(
            disagrees.native().map(NativeSessionId::as_str),
            Some(disagrees.offer_to_provider().as_str()),
            "and a provider that names its own sessions is equally supported"
        );
    }
}
