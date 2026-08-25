//! Answering the questions a CLI asks its terminal, the way xterm answers them.
//!
//! Measured 2026-08-25 on a real pseudo terminal: Claude Code sends XTVERSION and a cursor position report
//! request at start and draws nothing until both are answered; Codex and Grok ask the cursor. The host, not
//! a viewer, answers, so the CLI's screen exists before any viewer attaches and so two viewers do not
//! answer twice. The answers are xterm 378's, which every one of these CLIs was built against.
//!
//! This is terminal protocol, not conversation: a query is a fixed byte sequence and its answer is a fixed
//! byte sequence. Nothing here reads what the CLI drew.

/// A query may be split across two reads. The tail of the previous chunk is kept so a sequence that
/// straddles the boundary is still seen exactly once.
const CARRY_BYTES: usize = 32;

/// The scan state between chunks.
#[derive(Debug, Default)]
pub struct QueryCarry {
    tail: Vec<u8>,
}

impl QueryCarry {
    /// The answers owed for this chunk, in the order the CLI asked.
    ///
    /// `cursor` is the screen's cursor after this chunk was applied, zero-based `(row, col)`.
    pub fn answers(&mut self, chunk: &[u8], cursor: (u16, u16)) -> Vec<u8> {
        let mut window = std::mem::take(&mut self.tail);
        let carried = window.len();
        window.extend_from_slice(chunk);
        let mut answers = Vec::new();
        let mut at = 0usize;
        while let Some(found) = find_query(&window, at) {
            // A query that ends inside the carried tail was complete last time and was answered then.
            if found.end > carried {
                answer(found.query, cursor, &mut answers);
            }
            at = found.end;
        }
        let keep = window.len().saturating_sub(CARRY_BYTES);
        self.tail = window.get(keep..).unwrap_or(&[]).to_vec();
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

struct Found {
    query: Query,
    end: usize,
}

/// The next query at or after `from`.
fn find_query(window: &[u8], from: usize) -> Option<Found> {
    let mut at = from;
    while at < window.len() {
        let Some(&byte) = window.get(at) else { break };
        if byte != 0x1b {
            at += 1;
            continue;
        }
        if let Some((query, end)) = query_at(window, at) {
            return Some(Found { query, end });
        }
        at += 1;
    }
    None
}

/// The query beginning at `start` (an ESC), and where it ends, if it is one.
fn query_at(window: &[u8], start: usize) -> Option<(Query, usize)> {
    let rest = window.get(start + 1..)?;
    let end = |len: usize| start + 1 + len;
    for (literal, query) in [
        (&b"[>0q"[..], Query::Version),
        (&b"[>q"[..], Query::Version),
        (&b"[6n"[..], Query::CursorPosition),
        (&b"[5n"[..], Query::Status),
        (&b"[0c"[..], Query::PrimaryAttributes),
        (&b"[c"[..], Query::PrimaryAttributes),
        (&b"[>0c"[..], Query::SecondaryAttributes),
        (&b"[>c"[..], Query::SecondaryAttributes),
        (&b"[?u"[..], Query::KeyboardFlags),
    ] {
        if rest.starts_with(literal) {
            return Some((query, end(literal.len())));
        }
    }
    if let Some(after) = rest.strip_prefix(b"[?") {
        let digits = after
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0 && after.get(digits..digits + 2) == Some(b"$p") {
            let mode = after.get(..digits)?.iter().try_fold(0u32, |value, byte| {
                value
                    .checked_mul(10)?
                    .checked_add(u32::from(byte.wrapping_sub(b'0')))
            })?;
            return Some((Query::ModeReport(mode), end(2 + digits + 2)));
        }
    }
    for (prefix, index) in [(&b"]10;?"[..], 10u8), (&b"]11;?"[..], 11u8)] {
        if let Some(after) = rest.strip_prefix(prefix) {
            let terminator = if after.starts_with(b"\x07") {
                1
            } else if after.starts_with(b"\x1b\\") {
                2
            } else {
                return None;
            };
            return Some((Query::Colour(index), end(prefix.len() + terminator)));
        }
    }
    if let Some(after) = rest.strip_prefix(b"P+q") {
        let terminator = after.windows(2).position(|pair| pair == b"\x1b\\")?;
        return Some((Query::Capability, end(3 + terminator + 2)));
    }
    None
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

    #[test]
    fn the_start_up_questions_get_xterm_answers_in_order() {
        let mut carry = QueryCarry::default();
        let answers = carry.answers(b"\x1b[>0q\x1b[?2026$p\x1b[6n", (4, 9));
        assert_eq!(
            answers,
            b"\x1bP>|xterm(378)\x1b\\\x1b[?2026;2$y\x1b[5;10R".to_vec()
        );
    }

    #[test]
    fn a_question_split_across_two_chunks_is_answered_once() {
        let mut carry = QueryCarry::default();
        let first = carry.answers(b"hello \x1b[>", (0, 0));
        assert!(first.is_empty());
        let second = carry.answers(b"0q world", (0, 0));
        assert_eq!(second, b"\x1bP>|xterm(378)\x1b\\".to_vec());
        let third = carry.answers(b" more", (0, 0));
        assert!(third.is_empty(), "the carried tail is not answered again");
    }

    #[test]
    fn drawing_that_merely_resembles_a_question_is_left_alone() {
        let mut carry = QueryCarry::default();
        assert!(
            carry
                .answers(b"\x1b[2J\x1b[H\x1b[?25l\x1b[1;1H", (0, 0))
                .is_empty()
        );
        assert!(carry.answers(b"\x1b[?1049h\x1b[?2004h", (0, 0)).is_empty());
    }

    #[test]
    fn colour_and_capability_questions_are_answered_and_consumed_whole() {
        let mut carry = QueryCarry::default();
        let answers = carry.answers(b"\x1b]11;?\x07\x1bP+q524742\x1b\\\x1b[?u", (0, 0));
        assert_eq!(
            answers,
            b"\x1b]11;rgb:0000/0000/0000\x1b\\\x1bP0+r\x1b\\\x1b[?0u".to_vec()
        );
    }
}
