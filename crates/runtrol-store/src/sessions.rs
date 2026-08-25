//! Reading and writing session rows.
//!
//! # Two durability settings, and the reason there are two
//!
//! Session rows are written durably. Losing one loses a session the operator can no longer find, and no other
//! copy exists anywhere.
//!
//! Source checkpoints are written without durability. They are advisory progress metadata and are not the
//! `WatchCursor` used for bounded reconnect. Losing one loses no session pointer or conversation content, and runtrol
//! does not scan a provider transcript to reconstruct it.
//!
//! # Why the list is a range scan and nothing else
//!
//! Keys are raw time-ordered identifier bytes, so a scan returns sessions in the order a person expects with
//! no secondary index, no sort after the read, and no full-table walk to find the recent ones. That is what
//! makes "the list opens with no wait" a property of the key encoding rather than a caching trick.

use redb::{Durability, ReadableDatabase as _};
use runtrol_provider::{NativeSessionId, ProviderId, SessionId};

use crate::codec::SessionRow;
use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{CURSORS, NATIVE_INDEX, SESSIONS, SessionKey};

/// The last observed live-stream source boundary and event sequence.
///
/// This diagnostic checkpoint is not a reconnect `WatchCursor`, which also binds a stream incarnation and epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// The last monotone source boundary reported by the live provider stream.
    pub src_end: u64,
    /// The position of the last relayed event within its attach.
    pub seq: u64,
}

impl Store {
    /// Store a session row, and index it by the identifier its provider gave.
    ///
    /// Both writes happen in one transaction. A row without its index is a session that cannot be resumed,
    /// and an index without its row points at nothing; committing them separately would make either state
    /// reachable after a crash.
    ///
    /// # Errors
    ///
    /// [`StoreError::Codec`] when the row cannot be encoded, [`StoreError::Engine`] when the write fails.
    pub fn put_session(&self, session: SessionId, row: &SessionRow) -> Result<(), StoreError> {
        let encoded = row.encode()?;
        let key = SessionKey::of(session);

        let write = self.begin_durable_write("saving a session")?;
        {
            let mut sessions = write
                .open_table(SESSIONS)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the session table",
                    source: Box::new(error.into()),
                })?;
            sessions
                .insert(key, encoded.as_slice())
                .map_err(|error| StoreError::Engine {
                    doing: "writing a session row",
                    source: Box::new(error.into()),
                })?;
        }
        {
            let mut index = write
                .open_table(NATIVE_INDEX)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the native identifier index",
                    source: Box::new(error.into()),
                })?;
            index
                .insert((row.provider.as_str(), row.native.as_str()), key)
                .map_err(|error| StoreError::Engine {
                    doing: "indexing a session by its native identifier",
                    source: Box::new(error.into()),
                })?;
        }
        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing a session",
            source: Box::new(error.into()),
        })
    }

    /// Read one session row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the read fails, [`StoreError::Codec`] when the stored row is malformed.
    pub fn get_session(&self, session: SessionId) -> Result<Option<SessionRow>, StoreError> {
        let read = self
            .db()?
            .begin_read()
            .map_err(|error| StoreError::Engine {
                doing: "starting a read",
                source: Box::new(error.into()),
            })?;
        let table = match read.open_table(SESSIONS) {
            Ok(table) => table,
            // Nothing has been written yet. An empty list is the answer, not an error.
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the session table",
                    source: Box::new(error.into()),
                });
            }
        };
        let stored = table
            .get(SessionKey::of(session))
            .map_err(|error| StoreError::Engine {
                doing: "reading a session row",
                source: Box::new(error.into()),
            })?;
        match stored {
            None => Ok(None),
            Some(value) => SessionRow::decode(value.value()).map(Some),
        }
    }

    /// Change only the operator-owned display name of a stored session.
    ///
    /// Returns `false` when the provider has not produced a durable session pointer yet.
    ///
    /// # Errors
    ///
    /// [`StoreError::Codec`] when the updated row cannot be encoded, [`StoreError::Engine`] when the read or write
    /// fails.
    pub fn set_session_label(
        &self,
        session: SessionId,
        label: Option<Box<str>>,
    ) -> Result<bool, StoreError> {
        let Some(mut row) = self.get_session(session)? else {
            return Ok(false);
        };
        row.label = label;
        self.put_session(session, &row)?;
        Ok(true)
    }

    /// Every stored session, oldest first.
    ///
    /// A malformed row is skipped and reported rather than aborting the whole list. One bad row must not make
    /// the operator's other sessions unreachable, and returning them silently would hide the damage, so both
    /// the sessions and the failures come back.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the scan itself fails.
    pub fn list_sessions(&self) -> Result<ListedSessions, StoreError> {
        let read = self
            .db()?
            .begin_read()
            .map_err(|error| StoreError::Engine {
                doing: "starting a read",
                source: Box::new(error.into()),
            })?;
        let table = match read.open_table(SESSIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(ListedSessions::default()),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the session table",
                    source: Box::new(error.into()),
                });
            }
        };

        let mut listed = ListedSessions::default();
        let range = table
            .range(SessionKey::FIRST..=SessionKey::LAST)
            .map_err(|error| StoreError::Engine {
                doing: "scanning the session table",
                source: Box::new(error.into()),
            })?;
        for entry in range {
            let (key, value) = entry.map_err(|error| StoreError::Engine {
                doing: "reading a session during a scan",
                source: Box::new(error.into()),
            })?;
            let session = key.value().session();
            match SessionRow::decode(value.value()) {
                Ok(row) => listed.sessions.push((session, row)),
                Err(error) => listed.unreadable.push((session, error)),
            }
        }
        Ok(listed)
    }

    /// Find the session a provider's own identifier belongs to.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the read fails.
    pub fn find_by_native(
        &self,
        provider: ProviderId,
        native: &NativeSessionId,
    ) -> Result<Option<SessionId>, StoreError> {
        let read = self
            .db()?
            .begin_read()
            .map_err(|error| StoreError::Engine {
                doing: "starting a read",
                source: Box::new(error.into()),
            })?;
        let table = match read.open_table(NATIVE_INDEX) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the native identifier index",
                    source: Box::new(error.into()),
                });
            }
        };
        let found = table
            .get((provider.as_str(), native.as_str()))
            .map_err(|error| StoreError::Engine {
                doing: "looking up a native identifier",
                source: Box::new(error.into()),
            })?;
        Ok(found.map(|value| value.value().session()))
    }

    /// Forget a session, and its index entry, and its cursor.
    ///
    /// All three in one transaction, for the same reason they are written together: a half-removed session is
    /// a state nobody wrote code to handle.
    ///
    /// Returns whether there was anything to remove, so a caller can report "there was no such session"
    /// rather than implying it deleted one.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the write fails, [`StoreError::Codec`] when the row that was there cannot
    /// be decoded well enough to find its index entry.
    pub fn remove_session(&self, session: SessionId) -> Result<bool, StoreError> {
        let existing = self.get_session(session)?;
        let key = SessionKey::of(session);

        let write = self.begin_durable_write("removing a session")?;
        let removed;
        {
            let mut sessions = write
                .open_table(SESSIONS)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the session table",
                    source: Box::new(error.into()),
                })?;
            removed = sessions
                .remove(key)
                .map_err(|error| StoreError::Engine {
                    doing: "removing a session row",
                    source: Box::new(error.into()),
                })?
                .is_some();
        }
        if let Some(row) = existing {
            let mut index = write
                .open_table(NATIVE_INDEX)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the native identifier index",
                    source: Box::new(error.into()),
                })?;
            index
                .remove((row.provider.as_str(), row.native.as_str()))
                .map_err(|error| StoreError::Engine {
                    doing: "removing an index entry",
                    source: Box::new(error.into()),
                })?;
        }
        {
            let mut cursors = write
                .open_table(CURSORS)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the cursor table",
                    source: Box::new(error.into()),
                })?;
            cursors.remove(key).map_err(|error| StoreError::Engine {
                doing: "removing a cursor",
                source: Box::new(error.into()),
            })?;
        }
        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing a removal",
            source: Box::new(error.into()),
        })?;
        Ok(removed)
    }

    /// Record where a session's event stream had reached.
    ///
    /// Written without durability because this is advisory progress metadata. A power cut may lose the checkpoint,
    /// but not a durable session pointer or conversation content. It is not used to recover a transcript.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the write fails.
    pub fn put_cursor(&self, session: SessionId, cursor: Cursor) -> Result<(), StoreError> {
        let mut write = self
            .db()?
            .begin_write()
            .map_err(|error| StoreError::Engine {
                doing: "starting a cursor write",
                source: Box::new(error.into()),
            })?;
        write
            .set_durability(Durability::None)
            .map_err(|error| StoreError::Engine {
                doing: "relaxing durability for a cursor",
                source: Box::new(error.into()),
            })?;
        {
            let mut cursors = write
                .open_table(CURSORS)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the cursor table",
                    source: Box::new(error.into()),
                })?;
            cursors
                .insert(SessionKey::of(session), (cursor.src_end, cursor.seq))
                .map_err(|error| StoreError::Engine {
                    doing: "writing a cursor",
                    source: Box::new(error.into()),
                })?;
        }
        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing a cursor",
            source: Box::new(error.into()),
        })
    }

    /// Read where a session's event stream had reached.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the read fails.
    pub fn get_cursor(&self, session: SessionId) -> Result<Option<Cursor>, StoreError> {
        let read = self
            .db()?
            .begin_read()
            .map_err(|error| StoreError::Engine {
                doing: "starting a read",
                source: Box::new(error.into()),
            })?;
        let table = match read.open_table(CURSORS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the cursor table",
                    source: Box::new(error.into()),
                });
            }
        };
        let found = table
            .get(SessionKey::of(session))
            .map_err(|error| StoreError::Engine {
                doing: "reading a cursor",
                source: Box::new(error.into()),
            })?;
        Ok(found.map(|value| {
            let (src_end, seq) = value.value();
            Cursor { src_end, seq }
        }))
    }

    /// Begin a write whose result must survive a power cut.
    pub(crate) fn begin_durable_write(
        &self,
        doing: &'static str,
    ) -> Result<redb::WriteTransaction, StoreError> {
        let mut write = self
            .db()?
            .begin_write()
            .map_err(|error| StoreError::Engine {
                doing,
                source: Box::new(error.into()),
            })?;
        write
            .set_durability(Durability::Immediate)
            .map_err(|error| StoreError::Engine {
                doing,
                source: Box::new(error.into()),
            })?;
        Ok(write)
    }
}

/// The result of listing sessions: the ones that read, and the ones that did not.
///
/// Both halves are returned on purpose. Aborting the list on one malformed row would make every other session
/// unreachable, and dropping the bad row silently would hide damage to the operator's data. Neither is
/// acceptable, so the caller gets both and decides what to show.
#[derive(Debug, Default)]
pub struct ListedSessions {
    /// Sessions that decoded, oldest first.
    pub sessions: Vec<(SessionId, SessionRow)>,
    /// Sessions whose stored row could not be decoded, with the reason.
    pub unreadable: Vec<(SessionId, StoreError)>,
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{AbsPath, WallMs};

    use super::*;

    /// A scratch database that cleans up after itself.
    struct Scratch {
        root: AbsPath,
        store: Store,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-sessions-{name}"));
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the scratch directory");
            let root = AbsPath::canonicalize(base.to_str().expect("temp dir is UTF-8"))
                .expect("canonicalize");
            let store = Store::open(&root.join("runtrol.redb").expect("valid file name"))
                .expect("a fresh database must open");
            Self { root, store }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    fn a_path() -> AbsPath {
        let text = if cfg!(windows) {
            r"C:\Users\me\projects\app"
        } else {
            "/home/me/projects/app"
        };
        AbsPath::new(text).expect("a valid absolute path")
    }

    fn a_row(provider: &str, native: &str) -> SessionRow {
        SessionRow {
            provider: ProviderId::parse(provider).expect("valid provider id"),
            native: NativeSessionId::new(native).expect("valid native id"),
            cwd: a_path(),
            label: None,
            created_at: WallMs::from_millis(1_767_225_600_000),
            last_seen_at: WallMs::from_millis(1_767_225_700_000),
            pinned: false,
            archived: false,
            forked_from: None,
            live: None,
        }
    }

    #[test]
    fn a_session_round_trips() {
        let scratch = Scratch::make("roundtrip");
        let session = SessionId::now();
        let row = a_row("codex", "thread-1");

        scratch.store.put_session(session, &row).expect("stored");
        assert_eq!(
            scratch.store.get_session(session).expect("readable"),
            Some(row)
        );
    }

    #[test]
    fn an_unknown_session_is_absent_rather_than_an_error() {
        let scratch = Scratch::make("absent");
        assert_eq!(
            scratch
                .store
                .get_session(SessionId::now())
                .expect("readable"),
            None
        );
    }

    #[test]
    fn an_empty_database_lists_nothing_without_failing() {
        // A fresh install has no tables at all, and "no sessions yet" is an answer rather than a failure.
        let scratch = Scratch::make("empty");
        let listed = scratch.store.list_sessions().expect("listable");
        assert!(listed.sessions.is_empty());
        assert!(listed.unreadable.is_empty());
    }

    #[test]
    fn the_list_comes_back_oldest_first_with_no_sort_at_read_time() {
        // The property the raw time-ordered key encoding exists for. Written out of order on purpose.
        let scratch = Scratch::make("order");
        let first = SessionId::now();
        let second = SessionId::now();
        let third = SessionId::now();

        scratch
            .store
            .put_session(third, &a_row("codex", "c"))
            .expect("stored");
        scratch
            .store
            .put_session(first, &a_row("codex", "a"))
            .expect("stored");
        scratch
            .store
            .put_session(second, &a_row("codex", "b"))
            .expect("stored");

        let listed = scratch.store.list_sessions().expect("listable");
        let order: Vec<SessionId> = listed.sessions.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, vec![first, second, third]);
    }

    #[test]
    fn a_session_is_findable_by_the_identifier_its_provider_gave() {
        // This is what makes resuming possible: the provider knows its own identifier and nothing else.
        let scratch = Scratch::make("native");
        let session = SessionId::now();
        let row = a_row("claude", "0199c0de-1234-7000-8000-abcdef012345");
        scratch.store.put_session(session, &row).expect("stored");

        let found = scratch
            .store
            .find_by_native(row.provider, &row.native)
            .expect("searchable");
        assert_eq!(found, Some(session));
    }

    #[test]
    fn the_same_native_identifier_under_a_different_provider_is_a_different_session() {
        // Two CLIs can hand out the same string. The index is keyed by both halves precisely so that a
        // collision between providers cannot resume the wrong session.
        let scratch = Scratch::make("collide");
        let codex_session = SessionId::now();
        let claude_session = SessionId::now();
        let shared = "same-id-by-coincidence";

        scratch
            .store
            .put_session(codex_session, &a_row("codex", shared))
            .expect("stored");
        scratch
            .store
            .put_session(claude_session, &a_row("claude", shared))
            .expect("stored");

        let codex = ProviderId::parse("codex").expect("valid");
        let claude = ProviderId::parse("claude").expect("valid");
        let native = NativeSessionId::new(shared).expect("valid");

        assert_eq!(
            scratch.store.find_by_native(codex, &native).expect("found"),
            Some(codex_session)
        );
        assert_eq!(
            scratch
                .store
                .find_by_native(claude, &native)
                .expect("found"),
            Some(claude_session)
        );
    }

    #[test]
    fn removing_a_session_takes_its_index_entry_and_cursor_with_it() {
        // A half-removed session is a state nobody wrote code to handle.
        let scratch = Scratch::make("remove");
        let session = SessionId::now();
        let row = a_row("codex", "thread-9");
        scratch.store.put_session(session, &row).expect("stored");
        scratch
            .store
            .put_cursor(
                session,
                Cursor {
                    src_end: 4096,
                    seq: 12,
                },
            )
            .expect("cursor stored");

        assert!(scratch.store.remove_session(session).expect("removable"));
        assert_eq!(scratch.store.get_session(session).expect("readable"), None);
        assert_eq!(
            scratch
                .store
                .find_by_native(row.provider, &row.native)
                .expect("searchable"),
            None
        );
        assert_eq!(scratch.store.get_cursor(session).expect("readable"), None);
    }

    #[test]
    fn removing_a_session_that_is_not_there_says_so() {
        // Reporting success for a removal that found nothing would tell the operator their session is gone
        // when it never existed under that identifier.
        let scratch = Scratch::make("removeabsent");
        assert!(
            !scratch
                .store
                .remove_session(SessionId::now())
                .expect("removable")
        );
    }

    #[test]
    fn a_cursor_round_trips() {
        let scratch = Scratch::make("cursor");
        let session = SessionId::now();
        let cursor = Cursor {
            src_end: 22_339,
            seq: 41,
        };
        scratch.store.put_cursor(session, cursor).expect("stored");
        assert_eq!(
            scratch.store.get_cursor(session).expect("readable"),
            Some(cursor)
        );
    }

    #[test]
    fn overwriting_a_session_replaces_it_rather_than_duplicating_it() {
        let scratch = Scratch::make("overwrite");
        let session = SessionId::now();
        scratch
            .store
            .put_session(session, &a_row("codex", "thread-1"))
            .expect("stored");

        let mut updated = a_row("codex", "thread-1");
        updated.label = Some(Box::from("renamed"));
        updated.pinned = true;
        scratch
            .store
            .put_session(session, &updated)
            .expect("stored");

        let listed = scratch.store.list_sessions().expect("listable");
        assert_eq!(listed.sessions.len(), 1);
        assert_eq!(
            scratch.store.get_session(session).expect("readable"),
            Some(updated)
        );
    }

    #[test]
    fn changing_a_session_name_preserves_every_other_pointer_field() {
        let scratch = Scratch::make("rename");
        let session = SessionId::now();
        let row = a_row("codex", "thread-1");
        scratch.store.put_session(session, &row).expect("stored");

        assert!(
            scratch
                .store
                .set_session_label(session, Some("Release repair".into()))
                .expect("renamed")
        );
        let renamed = scratch
            .store
            .get_session(session)
            .expect("readable")
            .expect("present");
        assert_eq!(renamed.label.as_deref(), Some("Release repair"));
        assert_eq!(renamed.provider, row.provider);
        assert_eq!(renamed.native, row.native);
        assert_eq!(renamed.cwd, row.cwd);

        assert!(
            !scratch
                .store
                .set_session_label(SessionId::now(), Some("Absent".into()))
                .expect("absence is an answer")
        );
    }

    #[test]
    fn one_unreadable_row_does_not_hide_the_others() {
        // Aborting the list on a malformed row would make every other session unreachable, and dropping it
        // silently would hide damage to the operator's data. Both halves come back.
        let scratch = Scratch::make("damaged");
        let good = SessionId::now();
        let bad = SessionId::now();
        scratch
            .store
            .put_session(good, &a_row("codex", "good"))
            .expect("stored");

        // Write bytes no decoder will accept, straight into the table.
        {
            let write = scratch
                .store
                .begin_durable_write("planting a damaged row")
                .expect("write");
            {
                let mut sessions = write.open_table(SESSIONS).expect("session table");
                sessions
                    .insert(SessionKey::of(bad), [0xFF_u8, 0xFF].as_slice())
                    .expect("insert");
            }
            write.commit().expect("commit");
        }

        let listed = scratch.store.list_sessions().expect("listable");
        assert_eq!(listed.sessions.len(), 1, "the good row is still reachable");
        assert_eq!(listed.unreadable.len(), 1, "and the bad one is reported");
        assert_eq!(listed.unreadable.first().map(|(id, _)| *id), Some(bad));
    }
}
