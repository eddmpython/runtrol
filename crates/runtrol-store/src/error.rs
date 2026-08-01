//! Why the database could not be opened, read, or trusted.

use runtrol_provider::AbsPath;

/// Storage failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// Another runtrol already has the database open.
    ///
    /// Not an unexpected error. It is how a second daemon discovers the first one, and it is the reason the
    /// command surface asks the daemon instead of opening the file: the exclusive lock is a measured fact on
    /// all three platforms, so two openers is not a thing to work around.
    #[error("another runtrol is already running and has {path} open")]
    AlreadyOpen {
        /// The database file.
        path: AbsPath,
    },

    /// The database file could not be opened.
    #[error("cannot open {path}: {source}")]
    Open {
        /// The database file.
        path: AbsPath,
        /// What the storage engine said.
        #[source]
        source: Box<redb::DatabaseError>,
    },

    /// The file was written by a newer runtrol.
    ///
    /// Refused, and never migrated downward. A newer version may have written fields this build does not
    /// know about, and dropping them would corrupt the operator's session list silently.
    #[error(
        "{path} was written by a newer runtrol (schema {found}, this build understands {understood}). \
         upgrade runtrol, or move that file aside"
    )]
    SchemaTooNew {
        /// The database file.
        path: AbsPath,
        /// The version found in the file.
        found: u8,
        /// The version this build writes.
        understood: u8,
    },

    /// The file was written by an older runtrol and no migration exists yet.
    ///
    /// Names the backup path in the message, because the operator's next question is where their sessions
    /// went and the answer has to be in the error rather than in a document.
    #[error(
        "{path} uses schema {found} and this build understands {understood}, with no migration between \
         them. a copy will be kept at {backup} when migrations arrive"
    )]
    SchemaTooOld {
        /// The database file.
        path: AbsPath,
        /// The version found in the file.
        found: u8,
        /// The version this build writes.
        understood: u8,
        /// Where a copy would be kept.
        backup: String,
    },

    /// A stored row could not be decoded.
    ///
    /// The encoding is a compatibility promise to a file on the operator's disk, so a row that does not
    /// decode names which field it stopped at rather than reporting a length mismatch.
    #[error("a stored session row is malformed at {field}: {why}")]
    Codec {
        /// Which field the decoder stopped at.
        field: &'static str,
        /// What was wrong.
        why: &'static str,
    },

    /// A paired-device authorization row could not be decoded.
    ///
    /// Unlike one malformed session pointer, this stops startup. Continuing would silently change who holds what,
    /// so the operator must repair or revoke the damaged device row before a remote listener can exist.
    #[error("a stored device authorization row is malformed at {field}: {why}")]
    DeviceCodec {
        /// Which field the decoder stopped at.
        field: &'static str,
        /// What was wrong.
        why: &'static str,
    },

    /// The storage engine failed while runtrol was doing something specific.
    ///
    /// `doing` is required. An engine error without the operation that produced it tells the operator that
    /// something failed and nothing else.
    #[error("storage failed while {doing}: {source}")]
    Engine {
        /// What runtrol was doing.
        doing: &'static str,
        /// What the storage engine said.
        #[source]
        source: Box<redb::Error>,
    },
}

impl StoreError {
    /// Whether the operator has to do something before runtrol can start.
    ///
    /// Every schema refusal, a lock conflict, and damaged authorization need a person. A decode failure of one
    /// session row does not: the rest of that list is still readable, which is why only the device codec is here.
    #[must_use]
    pub const fn needs_the_operator(&self) -> bool {
        matches!(
            self,
            Self::AlreadyOpen { .. }
                | Self::SchemaTooNew { .. }
                | Self::SchemaTooOld { .. }
                | Self::DeviceCodec { .. }
        )
    }
}
