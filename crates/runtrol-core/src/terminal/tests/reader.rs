use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{COALESCE_WAIT, RING_CHUNKS, TerminalRead, mpsc, read_terminal};

struct DelayedAvailability {
    contents: &'static [u8],
    probes: Arc<AtomicUsize>,
}

impl Read for DelayedAvailability {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.contents.read(buffer)
    }
}

impl TerminalRead for DelayedAvailability {
    fn available(&mut self) -> usize {
        self.probes.fetch_add(1, Ordering::Relaxed);
        // A delayed OS query consumes the entire collection budget without reporting more bytes.
        // Requested sleep durations cannot account for this actual elapsed time.
        std::thread::sleep(COALESCE_WAIT * 2);
        0
    }
}

#[test]
fn a_delayed_availability_query_cannot_restart_the_output_collection_budget() {
    let probes = Arc::new(AtomicUsize::new(0));
    let reader = DelayedAvailability {
        contents: b"echo",
        probes: Arc::clone(&probes),
    };
    let (sent, mut received) = mpsc::channel(RING_CHUNKS);
    read_terminal(Box::new(reader), &sent);
    drop(sent);
    assert!(
        matches!(received.blocking_recv(), Some(Ok(chunk)) if chunk.as_ref() == b"echo"),
        "the already accepted echo must be published unchanged"
    );
    assert!(received.blocking_recv().is_none());
    assert_eq!(
        probes.load(Ordering::Relaxed),
        1,
        "once collection time has elapsed, publish instead of scheduling more waits"
    );
}
