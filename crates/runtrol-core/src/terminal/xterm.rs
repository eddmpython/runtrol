//! Answering the questions a CLI asks its terminal, the way xterm answers them.
//!
//! Measured 2026-08-25 on a real pseudo terminal: Claude Code sends XTVERSION and a cursor position report
//! request at start and draws nothing until both are answered; Codex and Grok ask the cursor. The host, not
//! a viewer, answers, so the CLI's screen exists before any viewer attaches and so two viewers do not
//! answer twice. The answers are xterm 378's, which every one of these CLIs was built against.
//!
//! A question is answered where it stands in the stream. The bytes before it reach the screen model first,
//! the answer observes the cursor they leave, and the bytes after it are applied afterwards; a cursor report
//! therefore names the cursor at the question, not at the end of whatever read happened to contain it. A
//! question split across two reads is answered once, when its last byte arrives, and only the unfinished
//! question itself is carried between reads.
//!
//! This is terminal protocol, not conversation: a query is a fixed byte sequence and its answer is a fixed
//! byte sequence. Nothing here reads what the CLI drew.

/// The longest unfinished question carried to the next read. A terminfo request (`ESC P + q <names> ESC \`)
/// is the one query without a fixed length; nothing a CLI asks runs past this, and a control string that does
/// is drawing, not a question.
const CARRY_LIMIT: usize = 128;

/// The scan state between reads: the unfinished question the previous read ended inside, if any.
#[derive(Debug, Default)]
pub struct QueryCarry {
    tail: Vec<u8>,
}

impl QueryCarry {
    /// Apply `chunk` to the screen in stream order, answering each question where it stands.
    ///
    /// `apply` receives the bytes up to and including the next question, applies them to the screen, and
    /// returns the cursor they leave, zero-based `(row, col)`; the answer to that question observes exactly
    /// that cursor. Every byte of `chunk` reaches `apply` exactly once, an unfinished tail included, so the
    /// screen never waits for the next read. The answers owed for this read come back in the order asked.
    pub fn answer_in_order(
        &mut self,
        chunk: &[u8],
        mut apply: impl FnMut(&[u8]) -> (u16, u16),
    ) -> Vec<u8> {
        let mut window = std::mem::take(&mut self.tail);
        let carried = window.len();
        window.extend_from_slice(chunk);
        let mut answers = Vec::new();
        let mut applied = 0usize;
        let mut at = 0usize;
        loop {
            match next_query(&window, at) {
                Next::Complete { query, end } => {
                    // A question finished by this read ends inside `chunk`; the carried bytes were applied last time.
                    let upto = end.saturating_sub(carried);
                    let cursor = apply(chunk.get(applied..upto).unwrap_or(&[]));
                    answer(query, cursor, &mut answers);
                    applied = upto;
                    at = end;
                }
                Next::Unfinished { start } => {
                    self.tail = window.get(start..).unwrap_or(&[]).to_vec();
                    break;
                }
                Next::None => break,
            }
        }
        if applied < chunk.len() {
            apply(chunk.get(applied..).unwrap_or(&[]));
        }
        answers
    }
}

/// What a CLI can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Query {
    /// `ESC [ > 0 q` or `ESC [ > q`: which terminal is this.
    Version,
    /// `ESC [ ? <mode> $ p`: is this private mode set.
    ModeReport(u32),
    /// `ESC [ 6 n`: where is the cursor.
    CursorPosition,
    /// `ESC [ 5 n`: are you ok.
    Status,
    /// `ESC [ c` / `ESC [ 0 c`: primary device attributes.
    PrimaryAttributes,
    /// `ESC [ > c` / `ESC [ > 0 c`: secondary device attributes.
    SecondaryAttributes,
    /// `ESC ] 10 ; ? BEL|ST` and `ESC ] 11 ; ? BEL|ST`: foreground and background colours.
    Colour(u8),
    /// `ESC P + q ... ESC \`: a terminfo capability.
    Capability,
    /// `ESC [ ? u`: the kitty keyboard protocol flags.
    KeyboardFlags,
}

/// The questions with one fixed spelling.
const LITERALS: [(&[u8], Query); 9] = [
    (b"[>0q", Query::Version),
    (b"[>q", Query::Version),
    (b"[6n", Query::CursorPosition),
    (b"[5n", Query::Status),
    (b"[0c", Query::PrimaryAttributes),
    (b"[c", Query::PrimaryAttributes),
    (b"[>0c", Query::SecondaryAttributes),
    (b"[>c", Query::SecondaryAttributes),
    (b"[?u", Query::KeyboardFlags),
];

/// A private mode number has at most this many digits; a longer run of digits is not a mode report.
const MODE_DIGITS: usize = 5;

/// What the next escape at or after `from` turns out to be.
enum Next {
    Complete {
        query: Query,
        end: usize,
    },
    /// The window ends inside a question that began at `start`.
    Unfinished {
        start: usize,
    },
    None,
}

fn next_query(window: &[u8], from: usize) -> Next {
    let mut at = from;
    while at < window.len() {
        if window.get(at) != Some(&0x1b) {
            at += 1;
            continue;
        }
        match query_at(window, at) {
            Scan::Complete(query, end) => return Next::Complete { query, end },
            Scan::Unfinished if window.len() - at <= CARRY_LIMIT => {
                return Next::Unfinished { start: at };
            }
            Scan::Unfinished | Scan::Other => at += 1,
        }
    }
    Next::None
}

/// What begins at `start` (an ESC).
enum Scan {
    /// A whole question, and where it ends.
    Complete(Query, usize),
    /// The window ends before the sequence could be told apart from a question.
    Unfinished,
    /// Not a question.
    Other,
}

fn query_at(window: &[u8], start: usize) -> Scan {
    let rest = window.get(start + 1..).unwrap_or(&[]);
    if rest.is_empty() {
        return Scan::Unfinished;
    }
    let end = |len: usize| start + 1 + len;
    let mut unfinished = false;
    for (literal, query) in LITERALS {
        if rest.starts_with(literal) {
            return Scan::Complete(query, end(literal.len()));
        }
        unfinished |= literal.starts_with(rest);
    }
    if let Some(after) = rest.strip_prefix(b"[?") {
        return mode_report(after, unfinished).map_or(Scan::Other, |scan| match scan {
            Scan::Complete(query, len) => Scan::Complete(query, end(2 + len)),
            other => other,
        });
    }
    for (prefix, index) in [(&b"]10;?"[..], 10u8), (&b"]11;?"[..], 11u8)] {
        if let Some(after) = rest.strip_prefix(prefix) {
            return match after {
                [] | [0x1b] => Scan::Unfinished,
                [0x07, ..] => Scan::Complete(Query::Colour(index), end(prefix.len() + 1)),
                [0x1b, b'\\', ..] => Scan::Complete(Query::Colour(index), end(prefix.len() + 2)),
                _ => Scan::Other,
            };
        }
        unfinished |= prefix.starts_with(rest);
    }
    if let Some(after) = rest.strip_prefix(b"P+q") {
        return match after.windows(2).position(|pair| pair == b"\x1b\\") {
            Some(terminator) => Scan::Complete(Query::Capability, end(3 + terminator + 2)),
            None => Scan::Unfinished,
        };
    }
    unfinished |= b"P+q".starts_with(rest);
    if unfinished {
        Scan::Unfinished
    } else {
        Scan::Other
    }
}

/// `ESC [ ?` was read; `after` is what follows. A mode report's length here counts from after the `?`.
fn mode_report(after: &[u8], literal_unfinished: bool) -> Option<Scan> {
    let digits = after
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits > MODE_DIGITS {
        return None;
    }
    if digits == 0 {
        // `ESC [ ?` alone may still become `ESC [ ? u` or a mode report on the next read.
        return (after.is_empty() && literal_unfinished).then_some(Scan::Unfinished);
    }
    match after.get(digits..) {
        Some([] | [b'$']) => Some(Scan::Unfinished),
        Some([b'$', b'p', ..]) => {
            let mode = after.get(..digits)?.iter().try_fold(0u32, |value, byte| {
                value
                    .checked_mul(10)?
                    .checked_add(u32::from(byte.wrapping_sub(b'0')))
            })?;
            Some(Scan::Complete(Query::ModeReport(mode), digits + 2))
        }
        _ => None,
    }
}

/// Whether this host treats the private mode as set. Synchronized output (2026) is what Claude asks about;
/// reporting it as reset but recognized keeps the CLI on the plain path this host renders correctly.
const fn mode_value(mode: u32) -> u8 {
    match mode {
        2026 => 2,
        _ => 0,
    }
}

fn answer(query: Query, cursor: (u16, u16), out: &mut Vec<u8>) {
    match query {
        Query::Version => out.extend_from_slice(b"\x1bP>|xterm(378)\x1b\\"),
        Query::ModeReport(mode) => {
            out.extend_from_slice(format!("\x1b[?{mode};{}$y", mode_value(mode)).as_bytes());
        }
        Query::CursorPosition => {
            let (row, col) = cursor;
            out.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        }
        Query::Status => out.extend_from_slice(b"\x1b[0n"),
        Query::PrimaryAttributes => out.extend_from_slice(b"\x1b[?62;22c"),
        Query::SecondaryAttributes => out.extend_from_slice(b"\x1b[>41;378;0c"),
        Query::Colour(10) => out.extend_from_slice(b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
        Query::Colour(_) => out.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
        Query::Capability => out.extend_from_slice(b"\x1bP0+r\x1b\\"),
        Query::KeyboardFlags => out.extend_from_slice(b"\x1b[?0u"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen that only counts: its cursor column is the number of bytes applied so far, so a cursor
    /// report proves which bytes had reached the screen when the question was answered.
    struct Counting {
        applied: Vec<u8>,
    }

    impl Counting {
        fn run(&mut self, carry: &mut QueryCarry, chunk: &[u8]) -> Vec<u8> {
            carry.answer_in_order(chunk, |bytes| {
                self.applied.extend_from_slice(bytes);
                (
                    0,
                    u16::try_from(self.applied.len()).expect("a short fixture"),
                )
            })
        }
    }

    /// Every question this host answers, between drawing, with a cursor report early and late.
    const PIECES: [&[u8]; 17] = [
        b"\x1b[2J\x1b[H",
        b"\x1b[>0q",
        b"\x1b[?2026$p",
        b"one ",
        b"\x1b[6n",
        b"\x1b[5n",
        b"\x1b[c",
        b"\x1b[0c",
        b"\x1b[>c",
        b"\x1b[>0c",
        b"\x1b]10;?\x07",
        b"\x1b]11;?\x1b\\",
        b"\x1bP+q524742\x1b\\",
        b"\x1b[?u",
        b"two \x1b[6n",
        b"\x1b[>q",
        b" end",
    ];

    fn fixture() -> Vec<u8> {
        PIECES.concat()
    }

    /// The answers owed for the fixture, with each cursor report naming the bytes up to its own question.
    fn expected() -> Vec<u8> {
        let through =
            |piece: usize| -> usize { PIECES.iter().take(piece + 1).map(|p| p.len()).sum() };
        let cursor = |piece: usize| format!("\x1b[1;{}R", through(piece) + 1);
        [
            b"\x1bP>|xterm(378)\x1b\\".to_vec(),
            b"\x1b[?2026;2$y".to_vec(),
            cursor(4).into_bytes(),
            b"\x1b[0n".to_vec(),
            b"\x1b[?62;22c".to_vec(),
            b"\x1b[?62;22c".to_vec(),
            b"\x1b[>41;378;0c".to_vec(),
            b"\x1b[>41;378;0c".to_vec(),
            b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
            b"\x1b]11;rgb:0000/0000/0000\x1b\\".to_vec(),
            b"\x1bP0+r\x1b\\".to_vec(),
            b"\x1b[?0u".to_vec(),
            cursor(14).into_bytes(),
            b"\x1bP>|xterm(378)\x1b\\".to_vec(),
        ]
        .concat()
    }

    #[test]
    fn every_question_is_answered_once_where_it_stands_however_the_reads_split() {
        let fixture = fixture();
        let expected = expected();
        for split in 0..=fixture.len() {
            let mut carry = QueryCarry::default();
            let mut screen = Counting {
                applied: Vec::new(),
            };
            let (first, second) = fixture.split_at(split);
            let mut answers = screen.run(&mut carry, first);
            answers.extend(screen.run(&mut carry, second));
            assert_eq!(answers, expected, "split at {split}");
            assert_eq!(screen.applied, fixture, "split at {split}");
        }
        let mut carry = QueryCarry::default();
        let mut screen = Counting {
            applied: Vec::new(),
        };
        let answers: Vec<u8> = fixture
            .iter()
            .flat_map(|byte| screen.run(&mut carry, std::slice::from_ref(byte)))
            .collect();
        assert_eq!(answers, expected, "one byte per read");
        assert_eq!(screen.applied, fixture);
    }

    #[test]
    fn a_cursor_report_names_the_cursor_at_the_question_on_a_real_screen() {
        let mut parser = vt100::Parser::new(30, 100, 0);
        let mut carry = QueryCarry::default();
        let answers = carry.answer_in_order(b"abc\x1b[6n\r\nxy\x1b[6n done", |bytes| {
            parser.process(bytes);
            parser.screen().cursor_position()
        });
        assert_eq!(answers, b"\x1b[1;4R\x1b[2;3R".to_vec());
        assert_eq!(parser.screen().contents(), "abc\nxy done");
    }

    #[test]
    fn drawing_that_merely_resembles_a_question_is_left_alone() {
        let mut carry = QueryCarry::default();
        let mut screen = Counting {
            applied: Vec::new(),
        };
        for chunk in [
            &b"\x1b[2J\x1b[H\x1b[?25l\x1b[1;1H"[..],
            b"\x1b[?1049h\x1b[?2004h",
            b"\x1b]0;a title\x07\x1b[?12;25h",
        ] {
            assert!(screen.run(&mut carry, chunk).is_empty());
            assert!(carry.tail.is_empty(), "nothing of {chunk:?} is carried");
        }
    }

    #[test]
    fn a_control_string_that_runs_past_the_carry_is_drawing_not_a_question() {
        let mut carry = QueryCarry::default();
        let mut screen = Counting {
            applied: Vec::new(),
        };
        let mut long = b"\x1bP+q".to_vec();
        long.extend(std::iter::repeat_n(b'5', CARRY_LIMIT));
        assert!(screen.run(&mut carry, &long).is_empty());
        assert!(carry.tail.is_empty());
        assert!(screen.run(&mut carry, b"\x1b\\").is_empty());
        assert_eq!(
            screen.applied.len(),
            long.len() + 2,
            "every byte still reached the screen"
        );
    }
}
