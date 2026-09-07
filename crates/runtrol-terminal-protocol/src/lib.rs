//! Mechanical VT query grammar shared by the Runtime host and its presentation adapters.
//! No process, transport, clock, provider name, or conversation interpretation lives here.

mod filter;
mod scanner;

pub use filter::QueryFilter;
pub use scanner::{QuerySpan, scan_next};

#[cfg(test)]
mod tests;

/// Maximum unfinished VT query retained between output chunks.
pub const CARRY_LIMIT: usize = 128;

/// Non-drawing replacement that preserves a query's leading ESC completion of previous VT state.
/// CAN would abort OSC/DCS dispatch that the original ESC completes, so presentation uses ST.
pub const QUERY_TERMINATOR: &[u8] = b"\x1b\\";

/// What a CLI can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
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
        let terminator = after.iter().position(|byte| *byte == 0x1b);
        let body = terminator.map_or(after, |at| after.get(..at).unwrap_or(&[]));
        if !body
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() || *byte == b';')
        {
            return Scan::Other;
        }
        return match terminator.and_then(|at| after.get(at..).map(|suffix| (at, suffix))) {
            Some((at, [0x1b, b'\\', ..])) => Scan::Complete(Query::Capability, end(3 + at + 2)),
            None | Some((_, [0x1b])) => Scan::Unfinished,
            Some(_) => Scan::Other,
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
