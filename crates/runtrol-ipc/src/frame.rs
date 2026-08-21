//! runtrol's own frames, between runtrol's own processes.
//!
//! # Why this wire has a compatibility story at all
//!
//! An update replaces the one executable atomically, and the daemon started from the old image keeps serving until
//! its hot sessions reach zero. So a command run from the new image meets an older daemon on a real machine, for as
//! long as somebody is mid-session. That is not an edge case, it is a designed window: the alternative is killing a
//! running agent to install something. So the version is on the wire and a side that does not know it **says so by
//! name** instead of reading the bytes with the wrong meaning.
//!
//! # A length prefix is a promise somebody else writes
//!
//! The bytes that say how long a frame is arrive from another process. They are checked against the bound **before**
//! anything is reserved, because the alternative is that whoever writes the prefix decides how much memory the reader
//! allocates. That is the whole reason this file is not four lines long.
//!
//! # Where the bound comes from
//!
//! The largest thing that crosses this wire is one event, and an event's payload is a slice of a line a provider
//! wrote. Measured across 4,578,197 transcript lines on this machine, the largest was 10,651,365 bytes, and the
//! transport that reads them refuses anything past 16 MiB. So a frame has to be able to carry 16 MiB plus its own
//! envelope, and [`MAX_FRAME`] is that sum rather than a number that felt right.
//!
//! The two bounds are in two crates that cannot see each other (the wire does not depend on a driver, by design), so
//! the arithmetic between them is asserted by an audit gate that can see both.

use bytes::{BufMut as _, Bytes};

/// The wire format this build speaks.
///
/// One byte, sent once when a connection opens. Not per frame: a version on every frame would pay for the whole
/// conversation to answer a question that is settled at hello, and it would let one connection change meaning
/// halfway through, which nothing should be able to do.
pub const WIRE_VERSION: u8 = 26;

/// How many bytes of payload one frame may carry.
///
/// Derived rather than chosen: 16 MiB is what the transport reading a provider's output will pass along, and 64 KiB is
/// room for the envelope runtrol wraps it in. See the module notes for the measurement behind the first number.
pub const MAX_FRAME: usize = 16 * 1024 * 1024 + 64 * 1024;

/// How many bytes carry the length.
const HEADER: usize = 4;

/// A frame could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// A frame is longer than this build will carry.
    ///
    /// Checked against the prefix before anything is reserved. A reader that trusted the prefix would let the other
    /// side decide how much memory it allocates, and the other side is another process.
    #[error("a frame of {bytes} bytes is past the limit of {max}")]
    TooLarge {
        /// How long the frame claims to be.
        bytes: usize,
        /// The limit.
        max: usize,
    },

    /// The other side speaks a wire format this build does not.
    ///
    /// Named rather than guessed at. The alternative is reading somebody else's bytes with this build's meaning, which
    /// produces a session list that is wrong rather than an error that is actionable.
    #[error("the other side speaks wire format {theirs} and this build speaks {ours}")]
    WrongVersion {
        /// What they said.
        theirs: u8,
        /// What this build speaks.
        ours: u8,
    },
}

/// Write one frame.
///
/// The length and the payload go into the buffer together, so a frame cannot be half written into it.
///
/// # Errors
///
/// [`FrameError::TooLarge`] when the payload is past [`MAX_FRAME`]. Refused here rather than at the reader, because a
/// writer that emitted a frame nobody can read has already spent the bytes.
pub fn encode(payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let length = payload.len();
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge {
            bytes: length,
            max: MAX_FRAME,
        });
    }
    // The cast cannot lose anything: the check above bounds the length far below what four bytes hold.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "length <= MAX_FRAME, which is well under u32::MAX"
    )]
    let header = length as u32;

    out.reserve(HEADER.saturating_add(length));
    out.put_u32(header);
    out.put_slice(payload);
    Ok(())
}

/// What reading a buffer produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decoded {
    /// A whole frame, and how much of the buffer it used.
    Frame {
        /// The payload, sharing the buffer rather than copied out of it.
        payload: Bytes,
        /// How many bytes to drop from the front before reading again.
        consumed: usize,
    },
    /// Not enough has arrived yet.
    ///
    /// Carries how much is needed in total, so a reader waits for that much instead of waking on every byte.
    NeedMore {
        /// How many bytes the buffer needs before a frame can be read.
        at_least: usize,
    },
}

/// Read one frame from the front of `buffer`.
///
/// # Errors
///
/// [`FrameError::TooLarge`] when the length prefix claims more than this build will carry. Reported before anything is
/// reserved, which is the point: the prefix came from another process.
pub fn decode(buffer: &Bytes) -> Result<Decoded, FrameError> {
    let Some(header) = buffer.get(..HEADER) else {
        return Ok(Decoded::NeedMore { at_least: HEADER });
    };
    let length = read_u32(header);

    // Checked before the buffer is asked for anything. A reader that reserved first would let whoever wrote the prefix
    // decide how much memory it holds.
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge {
            bytes: length,
            max: MAX_FRAME,
        });
    }

    let total = HEADER.saturating_add(length);
    if buffer.len() < total {
        return Ok(Decoded::NeedMore { at_least: total });
    }
    Ok(Decoded::Frame {
        payload: buffer.slice(HEADER..total),
        consumed: total,
    })
}

/// The length out of a four byte header.
fn read_u32(header: &[u8]) -> usize {
    let mut value: u32 = 0;
    for byte in header.iter().take(HEADER) {
        value = (value << 8) | u32::from(*byte);
    }
    value as usize
}

/// Check the version the other side announced.
///
/// # Errors
///
/// [`FrameError::WrongVersion`] naming both sides. An operator whose command surface and daemon disagree needs to know
/// which one to update, and that is only answerable if both numbers are in the message.
pub const fn check_version(theirs: u8) -> Result<(), FrameError> {
    if theirs == WIRE_VERSION {
        Ok(())
    } else {
        Err(FrameError::WrongVersion {
            theirs,
            ours: WIRE_VERSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payload: &[u8]) -> Bytes {
        let mut out = Vec::new();
        encode(payload, &mut out).expect("a small payload is writable");
        Bytes::from(out)
    }

    #[test]
    fn a_frame_written_here_reads_back_here() {
        for payload in [&b""[..], b"x", b"{\"a\":1}", &[0xFF; 1024][..]] {
            let buffer = framed(payload);
            match decode(&buffer).expect("readable") {
                Decoded::Frame {
                    payload: read,
                    consumed,
                } => {
                    assert_eq!(&*read, payload);
                    assert_eq!(consumed, buffer.len());
                }
                other @ Decoded::NeedMore { .. } => panic!("expected a whole frame, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_payload_shares_the_buffer_rather_than_being_copied_out_of_it() {
        // Every event that reaches a watcher crosses this wire. Copying each one here would put a copy of every
        // message on the path this whole design exists to keep free of copies.
        let buffer = framed(b"{\"text\":\"hello\"}");
        let start = buffer.as_ptr() as usize;
        match decode(&buffer).expect("readable") {
            Decoded::Frame { payload, .. } => {
                let at = payload.as_ptr() as usize;
                assert!(
                    at > start && at < start + buffer.len(),
                    "the payload was copied out of the buffer instead of shared with it"
                );
            }
            other @ Decoded::NeedMore { .. } => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn a_length_prefix_claiming_more_than_this_build_carries_is_refused_before_anything_is_reserved()
     {
        // The load-bearing one. The prefix comes from another process, and a reader that trusted it would let that
        // process decide how much memory this one allocates. Four bytes are enough to ask for four gigabytes.
        let mut hostile = Vec::new();
        hostile.extend_from_slice(&u32::MAX.to_be_bytes());
        // Nothing follows. A reader that reserved on the prefix would already have allocated by now.
        let buffer = Bytes::from(hostile);

        match decode(&buffer) {
            Err(FrameError::TooLarge { bytes, max }) => {
                assert_eq!(max, MAX_FRAME);
                assert!(bytes > max, "{bytes}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_nobody_could_read_is_refused_at_the_writer() {
        // Refused before the bytes are spent. A writer that emitted one would have paid for a frame that can only be
        // thrown away.
        let mut out = Vec::new();
        let error = encode(&vec![0; MAX_FRAME + 1], &mut out)
            .expect_err("a payload past the limit must be refused");
        assert!(matches!(error, FrameError::TooLarge { .. }));
        assert!(out.is_empty(), "and nothing was written");
    }

    #[test]
    fn the_bound_is_big_enough_for_the_largest_line_a_provider_has_produced_here() {
        // Measured across 4,578,197 transcript lines on this machine: the largest was 10,651,365 bytes, and the
        // transport reading them refuses anything past 16 MiB. A wire that could not carry that would drop a real
        // message at the last hop.
        const LARGEST_OBSERVED_LINE: usize = 10_651_365;
        const TRANSPORT_LINE_BOUND: usize = 16 * 1024 * 1024;

        // Checked at compile time, because both sides are constants: a build whose wire cannot carry what its
        // transport passes along should not exist, rather than fail a test somebody might skip.
        const {
            assert!(MAX_FRAME > LARGEST_OBSERVED_LINE);
            assert!(
                MAX_FRAME > TRANSPORT_LINE_BOUND,
                "a frame has to carry the biggest line the transport will pass along, plus an envelope"
            );
        }
    }

    #[test]
    fn a_reader_is_told_how_much_more_to_wait_for() {
        // A socket hands over whatever has arrived, which is not a frame. Waking on every byte would be the reader
        // doing the operating system's job badly.
        let whole = framed(&[7; 500]);

        assert_eq!(
            decode(&Bytes::new()).expect("readable"),
            Decoded::NeedMore { at_least: HEADER },
            "with nothing at all, the header is what is needed"
        );

        for taken in [1, HEADER - 1] {
            assert_eq!(
                decode(&whole.slice(..taken)).expect("readable"),
                Decoded::NeedMore { at_least: HEADER },
                "a partial header asks for the header"
            );
        }

        match decode(&whole.slice(..HEADER + 10)).expect("readable") {
            Decoded::NeedMore { at_least } => assert_eq!(
                at_least,
                whole.len(),
                "once the header is there, the reader knows exactly how much the frame needs"
            ),
            other @ Decoded::Frame { .. } => panic!("expected a wait, got {other:?}"),
        }
    }

    #[test]
    fn frames_back_to_back_are_read_one_at_a_time() {
        let mut out = Vec::new();
        encode(b"first", &mut out).expect("writable");
        encode(b"second", &mut out).expect("writable");
        let mut buffer = Bytes::from(out);

        let mut read = Vec::new();
        while !buffer.is_empty() {
            match decode(&buffer).expect("readable") {
                Decoded::Frame { payload, consumed } => {
                    read.push(String::from_utf8_lossy(&payload).into_owned());
                    buffer = buffer.slice(consumed..);
                }
                other @ Decoded::NeedMore { .. } => panic!("expected a frame, got {other:?}"),
            }
        }
        assert_eq!(read, vec!["first", "second"]);
    }

    #[test]
    fn an_empty_frame_is_a_frame() {
        // A command with no arguments is a real thing to send, and treating its frame as "nothing arrived" would make
        // the reader wait forever for a frame that is already there.
        let buffer = framed(b"");
        match decode(&buffer).expect("readable") {
            Decoded::Frame { payload, consumed } => {
                assert!(payload.is_empty());
                assert_eq!(consumed, HEADER);
            }
            other @ Decoded::NeedMore { .. } => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn a_side_that_speaks_a_different_wire_format_is_told_which_one_to_update() {
        // A daemon from the replaced image serves until its sessions end, so this happens on real machines. An
        // operator needs to know which of the two is behind, and that is only answerable if both numbers are in
        // the message.
        assert_eq!(check_version(WIRE_VERSION), Ok(()));

        match check_version(WIRE_VERSION + 1) {
            Err(FrameError::WrongVersion { theirs, ours }) => {
                assert_eq!(theirs, WIRE_VERSION + 1);
                assert_eq!(ours, WIRE_VERSION);
            }
            other => panic!("expected a refusal naming both sides, got {other:?}"),
        }

        let another = WIRE_VERSION.checked_add(2).expect("fixture version fits");
        let message = check_version(another)
            .expect_err("a version this build does not speak")
            .to_string();
        assert!(message.contains(&another.to_string()), "{message}");
        assert!(message.contains(&WIRE_VERSION.to_string()), "{message}");
    }

    #[test]
    fn the_length_is_read_the_same_way_on_every_machine() {
        // Two runtrol processes on one machine today, and the same format is what a phone will read tomorrow. A
        // native-endian prefix would make the wire mean different things on different hardware.
        let buffer = framed(b"abc");
        assert_eq!(
            buffer.get(..HEADER),
            Some(&[0, 0, 0, 3][..]),
            "the length is big endian, which is what a wire format means by network order"
        );
    }
}
