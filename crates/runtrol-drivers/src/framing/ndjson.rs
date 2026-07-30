//! One JSON object per line, read from a child's output under a bound.
//!
//! # The bound is measured, and the number that felt right was wrong
//!
//! Before measuring, one mebibyte looked generous for a line of JSON. It is not. Scanned across the
//! transcripts on this machine, 4,578,197 lines in 1,043 files:
//!
//! | | bytes |
//! |---|---:|
//! | median | 816 |
//! | 99th percentile | 36,506 |
//! | 99.9th percentile | 192,741 |
//! | 99.99th percentile | 1,098,350 |
//! | largest observed | **10,651,365** |
//!
//! One thousand and fifty-eight lines were at or above a mebibyte. A bound there would have refused real
//! traffic and detached working sessions, and the largest lines are exactly the ones an operator would miss: a
//! message with pasted content, and a compaction, which by its nature carries a summary of everything before
//! it.
//!
//! So the bound is sixteen mebibytes: above the largest line anyone here has produced, with headroom, and
//! still a bound. What it is for is a CLI that has stopped emitting newlines at all, or one printing a binary
//! file into its own output stream. Without it that is unbounded memory; with it, it is one refusal.
//!
//! # Why a big line cannot inflate the reader
//!
//! The reader owns a fixed buffer and never a line buffer. Each line is read into an allocation of its own that
//! lives exactly as long as somebody holds the line. A ten-megabyte compaction is therefore a spike and not a
//! new resting size, which matters because eight sessions each keeping a ten-megabyte buffer would be more
//! than twice the daemon's whole memory ceiling.
//!
//! # Why lines come out as shared bytes
//!
//! Because the payloads inside them are relayed without being copied. A slice of a line becomes an opaque
//! payload that shares the line's allocation, so fanning one event out to a phone, a terminal and a window
//! costs three pointers. Handing out a `String` per line would put a copy of every message on that path.

use bytes::Bytes;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};

/// The longest line runtrol will assemble.
///
/// Derived from measurement rather than taste: see the module documentation. Above the largest line observed on
/// this machine, and low enough that a CLI which never emits a newline is one refusal instead of an unbounded
/// allocation.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How much of the child's output the reader holds between lines.
///
/// Constant, whatever the lines turn out to be. Chosen against the measured distribution: it holds the 99th
/// percentile line whole, and anything larger is a transient allocation rather than a permanent one.
pub const READ_BUFFER: usize = 64 * 1024;

/// A line could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LineError {
    /// A line went past [`MAX_LINE`] without ending.
    ///
    /// A protocol violation, and the session detaches. Either the provider is emitting something that is not a
    /// line of JSON, or it has stopped emitting newlines, and neither is something to work around.
    #[error("a line reached {bytes} bytes without ending, and the limit is {max}")]
    TooLong {
        /// How much had been read when the limit was passed.
        bytes: usize,
        /// The limit.
        max: usize,
    },

    /// The reader already refused a line and will not continue.
    ///
    /// Reading on after a refusal would hand out the tail of the refused line as though it were a line of its
    /// own, which is worse than the refusal: it is a fragment of a message presented as a message.
    #[error("this reader refused a line and cannot continue")]
    Poisoned,

    /// Reading from the child failed.
    #[error("cannot read from the child: {detail}")]
    Io {
        /// What the operating system said.
        detail: String,
    },
}

/// Lines from a child's output.
#[derive(Debug)]
pub struct Lines<R> {
    /// The child's stream, with a buffer of a fixed size.
    reader: BufReader<R>,
    /// The size that buffer was given.
    ///
    /// Recorded because the reader does not report it, and it is the number the memory claim rests on: the
    /// reader's own size must be a constant this file chose and never a function of the line bound.
    capacity: usize,
    /// Set once a line has been refused. Nothing more comes out after that.
    poisoned: bool,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    /// Read lines from `source`.
    pub fn new(source: R) -> Self {
        Self {
            reader: BufReader::with_capacity(READ_BUFFER, source),
            capacity: READ_BUFFER,
            poisoned: false,
        }
    }

    /// How big the reader's own buffer is.
    ///
    /// A constant, and deliberately not a function of [`MAX_LINE`]. That is what keeps a ten-megabyte line from
    /// becoming a ten-megabyte resting size: the big allocation belongs to the line and goes when the line does,
    /// while the reader stays the size it always was.
    #[must_use]
    pub const fn reader_capacity(&self) -> usize {
        self.capacity
    }

    /// How many of the child's bytes are sitting in the reader right now.
    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.reader.buffer().len()
    }

    /// The next line, or `None` at the end of the stream.
    ///
    /// Blank lines are skipped. A line with nothing on it is the absence of a record rather than a broken one,
    /// and handing it on would make every caller parse nothing and report a failure.
    ///
    /// A trailing carriage return is removed. One of these CLIs runs on a platform whose conventions add it,
    /// and a carriage return inside a JSON document is not something to pass on and hope.
    ///
    /// # Errors
    ///
    /// [`LineError::TooLong`] when a line passes [`MAX_LINE`], [`LineError::Poisoned`] on any call after that,
    /// [`LineError::Io`] when the child's stream fails.
    pub async fn next(&mut self) -> Result<Option<Bytes>, LineError> {
        loop {
            let Some(raw) = self.read_one().await? else {
                return Ok(None);
            };
            let (start, end) = trimmed_bounds(&raw);
            if start == end {
                continue;
            }
            // Sliced rather than copied: the line's allocation is what every payload inside it will share.
            return Ok(Some(Bytes::from(raw).slice(start..end)));
        }
    }

    /// One line's bytes, up to but not including its newline.
    async fn read_one(&mut self) -> Result<Option<Vec<u8>>, LineError> {
        if self.poisoned {
            return Err(LineError::Poisoned);
        }

        // A fresh allocation per line, sized by the line. The reader keeps no line buffer, so the largest line
        // ever seen does not become the size of every session's reader from then on.
        let mut line: Vec<u8> = Vec::new();

        loop {
            // The borrow of the reader's buffer ends with this block, so what happens next may take the reader
            // mutably again. Deciding inside and acting outside is what keeps that legal without a copy.
            let step = {
                let available = self
                    .reader
                    .fill_buf()
                    .await
                    .map_err(|error| LineError::Io {
                        detail: error.to_string(),
                    })?;

                if available.is_empty() {
                    Step::Ended
                } else {
                    match available.iter().position(|byte| *byte == b'\n') {
                        None => {
                            let taken = available.len();
                            if line.len().saturating_add(taken) > MAX_LINE {
                                Step::TooLong(line.len().saturating_add(taken))
                            } else {
                                line.extend_from_slice(available);
                                Step::Continues { consume: taken }
                            }
                        }
                        Some(at) => match available.get(..at) {
                            Some(head) => {
                                if line.len().saturating_add(head.len()) > MAX_LINE {
                                    Step::TooLong(line.len().saturating_add(head.len()))
                                } else {
                                    line.extend_from_slice(head);
                                    // The newline is consumed with the line, so the next call starts at the
                                    // next record rather than on an empty one.
                                    Step::Complete {
                                        consume: at.saturating_add(1),
                                    }
                                }
                            }
                            // `position` returned this index, so the slice is there. Reporting the impossible as
                            // a read failure keeps a supervisor supervising.
                            None => Step::Impossible,
                        },
                    }
                }
            };

            match step {
                // End of the stream. A last line without a newline is still a line: a child that was killed
                // mid-write produced something, and dropping it would lose the reason it stopped.
                Step::Ended => {
                    return if line.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(line))
                    };
                }
                Step::TooLong(bytes) => return Err(self.refuse(bytes)),
                Step::Continues { consume } => self.reader.consume(consume),
                Step::Complete { consume } => {
                    self.reader.consume(consume);
                    return Ok(Some(line));
                }
                Step::Impossible => {
                    return Err(LineError::Io {
                        detail: "the child's buffer changed shape while being read".to_owned(),
                    });
                }
            }
        }
    }

    /// Refuse a line and stop reading for good.
    fn refuse(&mut self, bytes: usize) -> LineError {
        self.poisoned = true;
        LineError::TooLong {
            bytes,
            max: MAX_LINE,
        }
    }
}

/// What one pass over the child's buffer decided.
///
/// A value rather than a branch taken on the spot, so the decision can be made while the reader's buffer is
/// borrowed and acted on after that borrow has ended.
enum Step {
    /// The stream is over.
    Ended,
    /// The line passed the bound at this many bytes.
    TooLong(usize),
    /// Everything available was part of the line, and there is more to come.
    Continues {
        /// How much of the reader's buffer was taken.
        consume: usize,
    },
    /// The line ended.
    Complete {
        /// How much of the reader's buffer was taken, including the newline.
        consume: usize,
    },
    /// A slice that had just been located was not there.
    Impossible,
}

/// Where a line's content starts and ends, with surrounding whitespace excluded.
///
/// Offsets rather than a slice, so the caller can take them from the line's own shared allocation instead of
/// copying the middle of it out.
fn trimmed_bounds(line: &[u8]) -> (usize, usize) {
    let mut start = 0;
    let mut end = line.len();
    while start < end && line.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    while end > start
        && end
            .checked_sub(1)
            .and_then(|last| line.get(last))
            .is_some_and(u8::is_ascii_whitespace)
    {
        end -= 1;
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read every line from bytes that behave like a child's output.
    async fn all(source: &[u8]) -> Result<Vec<String>, LineError> {
        let mut lines = Lines::new(std::io::Cursor::new(source.to_vec()));
        let mut found = Vec::new();
        while let Some(line) = lines.next().await? {
            found.push(String::from_utf8_lossy(&line).into_owned());
        }
        Ok(found)
    }

    #[tokio::test]
    async fn one_object_per_line() {
        let read = all(b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n")
            .await
            .expect("readable");
        assert_eq!(read, vec![r#"{"a":1}"#, r#"{"b":2}"#, r#"{"c":3}"#]);
    }

    #[tokio::test]
    async fn a_last_line_without_a_newline_is_still_a_line() {
        // A child that was killed mid-write produced something, and it is usually the reason it stopped.
        let read = all(b"{\"a\":1}\n{\"partial\":true}")
            .await
            .expect("readable");
        assert_eq!(read, vec![r#"{"a":1}"#, r#"{"partial":true}"#]);
    }

    #[tokio::test]
    async fn a_carriage_return_does_not_travel_with_the_line() {
        // One of these CLIs runs on a platform whose conventions add it, and a carriage return inside a JSON
        // document is not something to pass on and hope.
        let read = all(b"{\"a\":1}\r\n{\"b\":2}\r\n").await.expect("readable");
        assert_eq!(read, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[tokio::test]
    async fn a_blank_line_is_the_absence_of_a_record() {
        // Handing it on would make every caller parse nothing and report a failure.
        let read = all(b"\n{\"a\":1}\n\n   \n{\"b\":2}\n\n")
            .await
            .expect("readable");
        assert_eq!(read, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[tokio::test]
    async fn nothing_at_all_is_the_end_and_not_an_error() {
        assert_eq!(all(b"").await.expect("readable"), Vec::<String>::new());
        assert_eq!(
            all(b"\n\n\n").await.expect("readable"),
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn a_line_larger_than_anything_measured_is_still_read() {
        // The bound exists for a CLI that stopped emitting newlines, not to police real traffic. Measured, the
        // largest line on this machine is 10,651,365 bytes, and refusing that would detach a working session on
        // every compaction.
        let big = 11 * 1024 * 1024;
        let mut source = Vec::with_capacity(big + 16);
        source.extend_from_slice(br#"{"type":"compacted","text":""#);
        source.resize(big, b'x');
        source.extend_from_slice(b"\"}\n");

        let mut lines = Lines::new(std::io::Cursor::new(source));
        let line = lines
            .next()
            .await
            .expect("a line this size is real traffic")
            .expect("a line is there");
        assert!(line.len() > 10_651_365, "{} bytes", line.len());
    }

    #[tokio::test]
    async fn a_line_that_never_ends_is_refused_by_name() {
        // The only thing the bound is for. Without it this is memory with no limit.
        let source = vec![b'x'; MAX_LINE + 1];
        let mut lines = Lines::new(std::io::Cursor::new(source));

        match lines.next().await {
            Err(LineError::TooLong { bytes, max }) => {
                assert_eq!(max, MAX_LINE);
                assert!(bytes > MAX_LINE, "{bytes}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_reader_that_refused_a_line_never_hands_out_its_tail() {
        // Continuing would present a fragment of a message as a message, which is worse than the refusal.
        let mut source = vec![b'x'; MAX_LINE + 1];
        source.extend_from_slice(b"\n{\"after\":true}\n");
        let mut lines = Lines::new(std::io::Cursor::new(source));

        assert!(matches!(lines.next().await, Err(LineError::TooLong { .. })));
        assert!(
            matches!(lines.next().await, Err(LineError::Poisoned)),
            "the reader must stay refused"
        );
    }

    #[tokio::test]
    async fn a_huge_line_does_not_become_the_readers_resting_size() {
        // Eight sessions each keeping a ten-megabyte buffer would be more than twice the daemon's whole
        // ceiling. The reader owns a fixed buffer and no line buffer, which is what makes the spike a spike.
        let big = 4 * 1024 * 1024;
        let mut source = Vec::with_capacity(big + 32);
        source.resize(big, b'x');
        source.extend_from_slice(b"\n{\"small\":1}\n");

        let mut lines = Lines::new(std::io::Cursor::new(source));
        let first = lines.next().await.expect("readable").expect("a line");
        assert!(first.len() >= big);

        assert_eq!(
            lines.reader_capacity(),
            READ_BUFFER,
            "the reader's own size must be the constant this file chose"
        );
        assert!(
            lines.reader_capacity() < MAX_LINE / 64,
            "the reader's size must not be a function of the line bound"
        );
        assert!(
            lines.buffered_bytes() <= lines.reader_capacity(),
            "the reader is holding {} bytes after a {big} byte line",
            lines.buffered_bytes()
        );

        let second = lines.next().await.expect("readable").expect("a line");
        assert_eq!(&*second, br#"{"small":1}"#);
    }

    #[tokio::test]
    async fn a_line_arriving_in_pieces_is_assembled() {
        // A pipe hands over whatever has arrived, which is not a line. Every real read is this case.
        struct Dribble {
            chunks: Vec<Vec<u8>>,
            at: usize,
        }

        impl tokio::io::AsyncRead for Dribble {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                match self.chunks.get(self.at) {
                    None => std::task::Poll::Ready(Ok(())),
                    Some(chunk) => {
                        buf.put_slice(chunk);
                        self.at += 1;
                        std::task::Poll::Ready(Ok(()))
                    }
                }
            }
        }

        let dribble = Dribble {
            chunks: vec![
                b"{\"a\"".to_vec(),
                b":1}\n{\"b\"".to_vec(),
                b":2}\n".to_vec(),
            ],
            at: 0,
        };

        let mut lines = Lines::new(dribble);
        let mut read = Vec::new();
        while let Some(line) = lines.next().await.expect("readable") {
            read.push(String::from_utf8_lossy(&line).into_owned());
        }
        assert_eq!(read, vec![r#"{"a":1}"#, r#"{"b":2}"#]);
    }

    #[tokio::test]
    async fn a_payload_can_be_borrowed_out_of_a_line_even_after_it_was_trimmed() {
        // The reason lines come out as shared bytes rather than as strings. What this checks is the part framing
        // owns: that trimming leaves the line's own allocation intact, so a payload inside it is still a slice
        // the seam will accept. That a slice costs nothing to relay is `Opaque`'s property and is proved where
        // that type lives, against a parent buffer the test built itself; asserting it here against a slice of
        // the returned value would compare the value to itself.
        let mut lines = Lines::new(std::io::Cursor::new(
            b"  {\"type\":\"assistant\",\"text\":\"hello\"}  \r\n".to_vec(),
        ));
        let line = lines.next().await.expect("readable").expect("a line");
        assert_eq!(&*line, br#"{"type":"assistant","text":"hello"}"#);

        let text = core::str::from_utf8(&line).expect("ascii");
        let inner = text
            .split_once(r#""text":"#)
            .map(|(_, tail)| tail)
            .expect("a payload is there");
        let payload = runtrol_provider::Opaque::borrowed_from(&line, inner)
            .expect("a slice of a trimmed line is still inside that line");
        assert_eq!(payload.as_str(), r#""hello"}"#);
    }
}
