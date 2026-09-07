#![cfg(test)]

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::{SyncSender, sync_channel};

use super::*;
use crate::terminal::{PtySize, Terminal};

struct Capture {
    clock: AtomicU64,
    records: SyncSender<Observation>,
    overflow: AtomicBool,
}

impl Observer for Capture {
    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    fn record(&self, observation: Observation) {
        if observation.identity.child_pid == u32::MAX && self.records.try_send(observation).is_err()
        {
            // The observer outlives this one fixture. Any overflow before its assertions invalidates it.
            self.overflow.store(true, Ordering::Relaxed);
        }
    }
}

#[tokio::test]
async fn successful_reads_map_by_offsets_to_unchanged_publications_before_completion() {
    let (records, received) = sync_channel(64);
    let capture = Arc::new(Capture {
        clock: AtomicU64::new(1),
        records,
        overflow: AtomicBool::new(false),
    });
    assert!(install(capture.clone()).is_ok());
    let terminal =
        Terminal::fed(u32::MAX, PtySize { cols: 80, rows: 24 }).expect("the fixture host starts");
    let mut view = terminal.attach().await;
    let first = b"\x1b[31mA\x1b[0m";
    let second = "\r\n\u{AC00}".as_bytes();
    terminal
        .feed(first.to_vec())
        .expect("first bytes are accepted");
    terminal
        .feed(second.to_vec())
        .expect("second bytes are accepted");
    terminal.end_feed(Some(0)).expect("the finite source ends");
    let mut publications = Vec::new();
    let mut actual = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while actual.len() < first.len() + second.len() {
            let chunk = view
                .live
                .recv()
                .await
                .expect("all fixture output stays live");
            actual.extend_from_slice(&chunk.bytes);
            publications.push((chunk.sequence, chunk.bytes.len()));
        }
        while view.exited.borrow().exit_code.is_none() {
            view.exited
                .changed()
                .await
                .expect("the fixture completes after its authority drains");
        }
    })
    .await
    .expect("the fixture finishes within its finite lifetime");
    assert_eq!(actual, [first.as_slice(), second].concat());
    let observations: Vec<_> = received.try_iter().collect();
    let owner = observations.first().expect("hosting was observed").identity;
    assert!(observations.iter().all(|record| record.identity == owner));
    let mut read_through = 0;
    let mut read_ordinal = 0;
    let mut published = Vec::new();
    let mut publish_through = 0;
    let mut attachments = 0;
    for observation in observations {
        match observation.boundary {
            Boundary::Hosted => {}
            Boundary::Attached {
                ordinal,
                next_sequence,
            } => {
                attachments += 1;
                assert_eq!(ordinal, attachments);
                assert_eq!(
                    next_sequence, 1,
                    "the live fixture starts at this observed boundary"
                );
            }
            Boundary::Read {
                ordinal,
                bytes,
                through,
            } => {
                read_ordinal += 1;
                read_through += bytes as u64;
                assert_eq!((ordinal, through), (read_ordinal, read_through));
            }
            Boundary::Published {
                sequence,
                bytes,
                through,
            } => {
                publish_through += bytes as u64;
                assert_eq!(through, publish_through);
                assert!(
                    through <= read_through,
                    "publication cannot precede the reads it carries"
                );
                published.push((sequence, bytes));
            }
        }
    }
    assert_eq!(
        read_ordinal, 2,
        "each successful source read is observed before coalescing"
    );
    assert_eq!(
        published, publications,
        "the observer follows real public sequences and lengths"
    );
    assert_eq!(read_through, publish_through);
    assert_eq!(attachments, 1);
    assert!(!capture.overflow.load(Ordering::Relaxed));
}
