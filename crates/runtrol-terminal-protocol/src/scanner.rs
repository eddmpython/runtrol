use crate::{CARRY_LIMIT, Query, Scan, query_at};

/// Structural position of the next host-owned query in an output window.
pub enum QuerySpan {
    /// A complete query and its exact byte span.
    Complete {
        /// Query family and structural parameter.
        query: Query,
        /// First query byte.
        start: usize,
        /// Exclusive end of the query.
        end: usize,
    },
    /// A candidate whose complete spelling is not available yet.
    Unfinished {
        /// First byte of the bounded unfinished candidate.
        start: usize,
    },
    /// No host-owned query remains in the window.
    None,
}

/// Scan a window without interpreting unknown VT sequences or control-string contents.
/// Callers carry an unfinished candidate, at most [`CARRY_LIMIT`] bytes, into the next window.
/// ESC starts a new sequence, including inside a control string.
#[must_use]
pub fn scan_next(window: &[u8], from: usize) -> QuerySpan {
    let mut at = from;
    while let Some(&byte) = window.get(at) {
        if byte != 0x1b {
            at += 1;
            continue;
        }
        match query_at(window, at) {
            Scan::Complete(query, end) if end - at <= CARRY_LIMIT => {
                return QuerySpan::Complete {
                    query,
                    start: at,
                    end,
                };
            }
            Scan::Unfinished if window.len() - at <= CARRY_LIMIT => {
                return QuerySpan::Unfinished { start: at };
            }
            Scan::Complete(..) | Scan::Unfinished | Scan::Other => at += 1,
        }
    }
    QuerySpan::None
}
