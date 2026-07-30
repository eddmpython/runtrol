//! The stored session row, encoded by hand.
//!
//! # Why by hand
//!
//! This layout is a compatibility promise to a file on the operator's disk. A derive macro hides it, and
//! then a field reordered during a refactor becomes silent corruption of somebody's session list rather than
//! a compile error. Writing it out costs one file and buys a layout that cannot change by accident.
//!
//! The cost is paid back by the golden fixture at the bottom: a checked-in byte array with its expected
//! decode. Any change to the layout that does not also bump the schema version turns that test red.
//!
//! # What is deliberately not here
//!
//! No transcript. No message previews. No titles, turn counts, or token counts. Everything a person reads is
//! owned by the provider and read live, and runtrol stores only the pointer it needs to find the session
//! again. That is not an optimization; it is the reason this product is allowed to exist alongside the CLIs
//! it supervises.

use runtrol_provider::{AbsPath, NativeSessionId, ProviderId, SessionId, WallMs};

use crate::error::StoreError;
use crate::schema::SCHEMA_VERSION;

/// Bit positions in the row's flag byte.
///
/// Written out rather than derived from an enum, because these are on-disk positions: renumbering them is a
/// format change, and a format change has to look like one.
mod flag {
    /// The operator pinned this session to the top of the list.
    pub(super) const PINNED: u8 = 1 << 0;
    /// This session was forked from another, whose identifier follows.
    pub(super) const FORKED: u8 = 1 << 1;
    /// The operator archived it. Kept, and out of the default list.
    pub(super) const ARCHIVED: u8 = 1 << 2;
    /// A process was running when this row was last written, and its identity follows.
    pub(super) const LIVE: u8 = 1 << 3;
}

/// A process runtrol believed was serving this session.
///
/// The tick count is a reuse guard, and it is not optional. Operating systems reissue process identifiers,
/// so a stored identifier alone can name somebody else's process after a restart, and acting on that would
/// mean runtrol signalling a program it never started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveProcess {
    /// The process identifier.
    pub pid: u32,
    /// When that process started, in whatever unit the platform counts.
    pub start_ticks: u64,
}

/// Everything runtrol stores about one session.
///
/// Roughly 144 bytes typically, 172 with a live process, 188 with a fork parent. The contract is about 200
/// bytes per session and the arithmetic is in the encoder below rather than in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// Which coding CLI owns it.
    pub provider: ProviderId,
    /// That CLI's own identifier for it.
    pub native: NativeSessionId,
    /// Where it works.
    pub cwd: AbsPath,
    /// A name the operator gave it.
    pub label: Option<Box<str>>,
    /// When runtrol first saw it.
    pub created_at: WallMs,
    /// When runtrol last saw it.
    pub last_seen_at: WallMs,
    /// Pinned to the top of the list.
    pub pinned: bool,
    /// Archived: kept, and out of the default list.
    pub archived: bool,
    /// The session this was forked from.
    pub forked_from: Option<SessionId>,
    /// The process serving it, when one was.
    pub live: Option<LiveProcess>,
}

impl SessionRow {
    /// Encode the row for storage.
    ///
    /// # Errors
    ///
    /// [`StoreError::Codec`] when a field is longer than its length prefix can describe. Refused rather than
    /// truncated: a silently shortened working directory would point runtrol at a different place on disk.
    pub fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(200);
        out.push(SCHEMA_VERSION);

        write_short(&mut out, "provider", self.provider.as_str())?;
        write_short(&mut out, "native id", self.native.as_str())?;
        write_long(&mut out, "working directory", self.cwd.as_str())?;
        write_short(&mut out, "label", self.label.as_deref().unwrap_or(""))?;

        out.extend_from_slice(&self.created_at.as_millis().to_le_bytes());
        out.extend_from_slice(&self.last_seen_at.as_millis().to_le_bytes());

        let mut flags = 0_u8;
        if self.pinned {
            flags |= flag::PINNED;
        }
        if self.archived {
            flags |= flag::ARCHIVED;
        }
        if self.forked_from.is_some() {
            flags |= flag::FORKED;
        }
        if self.live.is_some() {
            flags |= flag::LIVE;
        }
        out.push(flags);

        // Optional tails, in flag-bit order. The order is part of the format.
        if let Some(parent) = self.forked_from {
            out.extend_from_slice(parent.as_bytes());
        }
        if let Some(live) = self.live {
            out.extend_from_slice(&live.pid.to_le_bytes());
            out.extend_from_slice(&live.start_ticks.to_le_bytes());
        }

        Ok(out)
    }

    /// Decode a stored row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Codec`] naming the field the decoder stopped at. A row written by a different schema
    /// version is refused here as well as at the table level, because a single row can outlive a migration
    /// that forgot it.
    pub fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        let mut cursor = Cursor::new(bytes);

        let version = cursor.byte("row version")?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::Codec {
                field: "row version",
                why: "written by a different schema version than this build understands",
            });
        }

        let provider =
            ProviderId::parse(cursor.short_text("provider")?).map_err(|_| StoreError::Codec {
                field: "provider",
                why: "not a valid provider identifier",
            })?;
        let native = NativeSessionId::new(cursor.short_text("native id")?).map_err(|_| {
            StoreError::Codec {
                field: "native id",
                why: "not a valid native session identifier",
            }
        })?;
        let cwd = AbsPath::new(cursor.long_text("working directory")?).map_err(|_| {
            StoreError::Codec {
                field: "working directory",
                why: "not an absolute path",
            }
        })?;
        let label_text = cursor.short_text("label")?;
        let label = if label_text.is_empty() {
            None
        } else {
            Some(Box::from(label_text))
        };

        let created_at = WallMs::from_millis(cursor.u64("created at")?);
        let last_seen_at = WallMs::from_millis(cursor.u64("last seen at")?);
        let flags = cursor.byte("flags")?;

        let forked_from = if flags & flag::FORKED == 0 {
            None
        } else {
            Some(SessionId::from_bytes(cursor.sixteen("fork parent")?))
        };
        let live = if flags & flag::LIVE == 0 {
            None
        } else {
            Some(LiveProcess {
                pid: cursor.u32("process id")?,
                start_ticks: cursor.u64("process start ticks")?,
            })
        };

        if !cursor.is_finished() {
            // Extra bytes mean this row was written by something that knew more than this build does, and the
            // table-level version check should already have refused it. Refusing again rather than ignoring
            // the tail: silently dropping fields is how a session loses its fork parent.
            return Err(StoreError::Codec {
                field: "end of row",
                why: "trailing bytes this build does not understand",
            });
        }

        Ok(Self {
            provider,
            native,
            cwd,
            label,
            created_at,
            last_seen_at,
            pinned: flags & flag::PINNED != 0,
            archived: flags & flag::ARCHIVED != 0,
            forked_from,
            live,
        })
    }
}

/// Append text behind a one-byte length.
fn write_short(out: &mut Vec<u8>, field: &'static str, text: &str) -> Result<(), StoreError> {
    let len = u8::try_from(text.len()).map_err(|_| StoreError::Codec {
        field,
        why: "longer than 255 bytes, which this field cannot describe",
    })?;
    out.push(len);
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

/// Append text behind a two-byte length.
fn write_long(out: &mut Vec<u8>, field: &'static str, text: &str) -> Result<(), StoreError> {
    let len = u16::try_from(text.len()).map_err(|_| StoreError::Codec {
        field,
        why: "longer than 65535 bytes, which this field cannot describe",
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

/// Reads a row front to back, refusing to run off the end.
///
/// Every read goes through this rather than through indexing, so a truncated row produces a named field in an
/// error instead of a panic in the middle of listing the operator's sessions.
struct Cursor<'a> {
    /// The row.
    bytes: &'a [u8],
    /// How far in the reader has got.
    at: usize,
}

impl<'a> Cursor<'a> {
    /// Start at the beginning.
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Whether every byte has been read.
    const fn is_finished(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// Take `count` bytes.
    fn take(&mut self, field: &'static str, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self.at.checked_add(count).ok_or(StoreError::Codec {
            field,
            why: "a length that overflows the row",
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })?;
        self.at = end;
        Ok(slice)
    }

    /// Take one byte.
    fn byte(&mut self, field: &'static str) -> Result<u8, StoreError> {
        let slice = self.take(field, 1)?;
        slice.first().copied().ok_or(StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })
    }

    /// Take a fixed-width little-endian number.
    fn u32(&mut self, field: &'static str) -> Result<u32, StoreError> {
        let slice = self.take(field, 4)?;
        let array: [u8; 4] = slice.try_into().map_err(|_| StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })?;
        Ok(u32::from_le_bytes(array))
    }

    /// Take a fixed-width little-endian number.
    fn u64(&mut self, field: &'static str) -> Result<u64, StoreError> {
        let slice = self.take(field, 8)?;
        let array: [u8; 8] = slice.try_into().map_err(|_| StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })?;
        Ok(u64::from_le_bytes(array))
    }

    /// Take sixteen bytes, for an identifier.
    fn sixteen(&mut self, field: &'static str) -> Result<[u8; 16], StoreError> {
        let slice = self.take(field, 16)?;
        slice.try_into().map_err(|_| StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })
    }

    /// Take text from behind a one-byte length.
    fn short_text(&mut self, field: &'static str) -> Result<&'a str, StoreError> {
        let len = usize::from(self.byte(field)?);
        self.text(field, len)
    }

    /// Take text from behind a two-byte length.
    fn long_text(&mut self, field: &'static str) -> Result<&'a str, StoreError> {
        let slice = self.take(field, 2)?;
        let array: [u8; 2] = slice.try_into().map_err(|_| StoreError::Codec {
            field,
            why: "the row ends before this field does",
        })?;
        let len = usize::from(u16::from_le_bytes(array));
        self.text(field, len)
    }

    /// Take `len` bytes and require them to be text.
    fn text(&mut self, field: &'static str, len: usize) -> Result<&'a str, StoreError> {
        let slice = self.take(field, len)?;
        core::str::from_utf8(slice).map_err(|_| StoreError::Codec {
            field,
            why: "not valid UTF-8",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path shape for whichever platform the test is running on.
    fn a_path() -> AbsPath {
        let text = if cfg!(windows) {
            r"C:\Users\me\projects\app"
        } else {
            "/home/me/projects/app"
        };
        AbsPath::new(text).expect("a valid absolute path")
    }

    fn a_row() -> SessionRow {
        SessionRow {
            provider: ProviderId::parse("codex").expect("valid provider id"),
            native: NativeSessionId::new("0199c0de-1234-7000-8000-abcdef012345")
                .expect("valid native id"),
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
    fn a_plain_row_round_trips() {
        let row = a_row();
        let encoded = row.encode().expect("encodable");
        assert_eq!(SessionRow::decode(&encoded).expect("decodable"), row);
    }

    #[test]
    fn every_optional_field_round_trips() {
        let mut row = a_row();
        row.label = Some(Box::from("the interesting one"));
        row.pinned = true;
        row.archived = true;
        row.forked_from = Some(SessionId::now());
        row.live = Some(LiveProcess {
            pid: 4242,
            start_ticks: 987_654_321,
        });

        let encoded = row.encode().expect("encodable");
        assert_eq!(SessionRow::decode(&encoded).expect("decodable"), row);
    }

    #[test]
    fn the_row_stays_inside_its_size_contract() {
        // The memory and storage contract is roughly 200 bytes per session. The number lives here, next to
        // the encoder that determines it, rather than in a document that can drift away from the code.
        let plain = a_row().encode().expect("encodable");
        assert!(
            plain.len() <= 200,
            "a plain row grew to {} bytes",
            plain.len()
        );

        let mut fullest = a_row();
        fullest.label = Some(Box::from("a label"));
        fullest.forked_from = Some(SessionId::now());
        fullest.live = Some(LiveProcess {
            pid: u32::MAX,
            start_ticks: u64::MAX,
        });
        let heavy = fullest.encode().expect("encodable");
        assert!(
            heavy.len() <= 240,
            "the fullest row grew to {} bytes",
            heavy.len()
        );
    }

    #[test]
    fn a_truncated_row_names_the_field_it_stopped_at() {
        // A row that fails to decode must not panic in the middle of listing sessions, and the message has to
        // say where it stopped or nobody can act on it.
        let encoded = a_row().encode().expect("encodable");
        for cut in 1..encoded.len() {
            let partial = encoded.get(..cut).expect("a prefix of the row");
            match SessionRow::decode(partial) {
                Err(StoreError::Codec { field, .. }) => assert!(!field.is_empty()),
                Err(other) => panic!("expected a codec error, got {other:?}"),
                Ok(_) => panic!("a row cut at {cut} bytes decoded as if it were whole"),
            }
        }
    }

    #[test]
    fn trailing_bytes_are_refused_rather_than_ignored() {
        // Ignoring a tail is how a session silently loses its fork parent when an older build reads a newer
        // row.
        let mut encoded = a_row().encode().expect("encodable");
        encoded.push(0);
        assert!(matches!(
            SessionRow::decode(&encoded),
            Err(StoreError::Codec {
                field: "end of row",
                ..
            })
        ));
    }

    #[test]
    fn a_row_from_another_schema_version_is_refused() {
        let mut encoded = a_row().encode().expect("encodable");
        if let Some(first) = encoded.first_mut() {
            *first = SCHEMA_VERSION.wrapping_add(1);
        }
        assert!(matches!(
            SessionRow::decode(&encoded),
            Err(StoreError::Codec {
                field: "row version",
                ..
            })
        ));
    }

    #[test]
    fn a_field_longer_than_its_prefix_is_refused_rather_than_truncated() {
        // A silently shortened working directory points runtrol at a different place on disk.
        let mut row = a_row();
        row.label = Some(Box::from("x".repeat(usize::from(u8::MAX) + 1)));
        assert!(matches!(
            row.encode(),
            Err(StoreError::Codec { field: "label", .. })
        ));
    }

    #[test]
    fn the_golden_row_decodes_exactly() {
        // A checked-in byte array with its expected decode. Any change to the layout that does not also bump
        // the schema version turns this red, which is the whole reason the encoding is written by hand.
        //
        // Two fixtures rather than one, because the only platform-varying field is the working directory:
        // what counts as an absolute path differs, and a single fixture would have to pick a path that one
        // platform refuses. Everything before and after that field is byte-identical in both, which is what
        // the fixture is actually pinning.
        #[cfg(windows)]
        const GOLDEN: &[u8] = &[
            1, // row version
            5, b'c', b'o', b'd', b'e', b'x', // provider
            4, b't', b'h', b'-', b'1', // native id
            7, 0, b'C', b':', b'\\', b'w', b'o', b'r',
            b'k', // working directory, two-byte length
            0,    // label, empty
            0x40, 0xE2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // created at, 123456
            0x80, 0xC4, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, // last seen at, 246912
            0x01, // flags: pinned
        ];
        #[cfg(windows)]
        const GOLDEN_CWD: &str = r"C:\work";

        #[cfg(unix)]
        const GOLDEN: &[u8] = &[
            1, // row version
            5, b'c', b'o', b'd', b'e', b'x', // provider
            4, b't', b'h', b'-', b'1', // native id
            5, 0, b'/', b'w', b'o', b'r', b'k', // working directory, two-byte length
            0,    // label, empty
            0x40, 0xE2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // created at, 123456
            0x80, 0xC4, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, // last seen at, 246912
            0x01, // flags: pinned
        ];
        #[cfg(unix)]
        const GOLDEN_CWD: &str = "/work";

        let row = SessionRow::decode(GOLDEN).expect("the golden row must decode");
        assert_eq!(row.provider.as_str(), "codex");
        assert_eq!(row.native.as_str(), "th-1");
        assert_eq!(row.cwd.as_str(), GOLDEN_CWD);
        assert_eq!(row.label, None);
        assert_eq!(row.created_at.as_millis(), 123_456);
        assert_eq!(row.last_seen_at.as_millis(), 246_912);
        assert!(row.pinned);
        assert!(!row.archived);
        assert_eq!(row.forked_from, None);
        assert_eq!(row.live, None);

        // And the other direction: encoding what was decoded reproduces the same bytes.
        assert_eq!(row.encode().expect("encodable"), GOLDEN);
    }
}
