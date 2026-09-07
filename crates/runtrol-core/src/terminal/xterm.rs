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

use runtrol_terminal_protocol::{Query, QuerySpan as Next, scan_next as next_query};

#[cfg(test)]
use runtrol_terminal_protocol::CARRY_LIMIT;

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
                Next::Complete { query, end, .. } => {
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
