use crate::{CARRY_LIMIT, QUERY_TERMINATOR, QuerySpan, scan_next};

/// One viewer's bounded presentation filter. The Runtime wire remains untouched.
///
/// Only queries answered by the host leave the rendered stream. User input requires no reply classifier.
#[derive(Debug, Default)]
pub struct QueryFilter {
    tail: Vec<u8>,
}

impl QueryFilter {
    /// Consume host-owned queries before a renderer can produce duplicate replies.
    #[must_use]
    pub fn filter(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(bytes);
        let mut out = Vec::with_capacity(window.len());
        let mut at = 0;
        loop {
            match scan_next(&window, at) {
                QuerySpan::Complete { start, end, .. } => {
                    out.extend_from_slice(window.get(at..start).unwrap_or(&[]));
                    // Deleting the ESC would join an earlier unfinished CSI with later drawing. ST also
                    // completes earlier OSC/DCS strings just as the query's ESC did, without a reply.
                    out.extend_from_slice(QUERY_TERMINATOR);
                    at = end;
                }
                QuerySpan::Unfinished { start } => {
                    out.extend_from_slice(window.get(at..start).unwrap_or(&[]));
                    self.tail = window.get(start..).unwrap_or(&[]).to_vec();
                    debug_assert!(self.tail.len() <= CARRY_LIMIT);
                    return out;
                }
                QuerySpan::None => {
                    out.extend_from_slice(window.get(at..).unwrap_or(&[]));
                    return out;
                }
            }
        }
    }

    /// Finish an output stream before exit, a replacement checkpoint, or reattachment.
    /// An unfinished candidate was never proved to be a query and remains literal output.
    #[must_use]
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tail)
    }
}
