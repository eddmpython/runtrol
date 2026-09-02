//! A child that is not a process of ours at all: some other window owns the terminal and feeds this host the
//! raw bytes it observed (`docs/terminalSurface.md`, observed mirror). The host reads it like any child, so
//! viewers, the projector and the sidebar row apply unchanged; what differs is that input has nowhere to go.

use std::io::{Read, Write};
use std::sync::{PoisonError, RwLock};

use runtrol_childproc::SpawnError;
use runtrol_childproc::pty::TerminalRead;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender, channel};

/// Chunks the feeder may run ahead of the reader before a feed is refused. The reader thread hands each
/// chunk to the raw lane without waiting on any viewer, so the queue only fills when the machine is stalled.
const FEED_QUEUE: usize = 256;

/// The feeder's side of an observed mirror: the bytes go in here and come out of the host's reader.
#[derive(Debug)]
pub struct FedChild {
    /// The pid the owner window reported for the observed shell, or zero when it reported none.
    pid: u32,
    feed: RwLock<Option<Sender<Vec<u8>>>>,
    reader: RwLock<Option<Receiver<Vec<u8>>>>,
    exit: RwLock<Option<i32>>,
}

/// Why a chunk was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedError {
    /// The feed was ended, by its owner or by a stop.
    Ended,
    /// The feeder ran [`FEED_QUEUE`] chunks ahead of the reader; nothing was dropped, the chunk is refused.
    Behind,
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ended => "the observed mirror has ended",
            Self::Behind => "the observed mirror is behind its feed",
        })
    }
}

impl std::error::Error for FeedError {}

impl FedChild {
    pub(super) fn new(pid: u32) -> Self {
        let (feed, reader) = channel(FEED_QUEUE);
        Self {
            pid,
            feed: RwLock::new(Some(feed)),
            reader: RwLock::new(Some(reader)),
            exit: RwLock::new(None),
        }
    }

    pub(super) const fn pid(&self) -> u32 {
        self.pid
    }

    /// The host's reader: taken once, blocking on the feed until it ends.
    pub(super) fn reader(&self) -> Result<Box<dyn TerminalRead>, SpawnError> {
        let receiver = self
            .reader
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(SpawnError::Pty {
                doing: "reading an observed mirror",
                detail: "its reader was already taken".to_owned(),
            })?;
        Ok(Box::new(FeedReader {
            receiver,
            pending: Vec::new(),
            at: 0,
        }))
    }

    /// The host's writer. The only writer of an observed mirror is the host's own query answering, and the
    /// owner's real terminal already answered those queries; the daemon refuses viewer input before it gets here.
    pub(super) fn writer() -> Box<dyn Write + Send> {
        Box::new(AnsweredElsewhere)
    }

    /// One chunk from the owner, exactly as observed.
    ///
    /// # Errors
    ///
    /// [`FeedError::Ended`] after [`Self::end`] or a stop; [`FeedError::Behind`] when the reader is
    /// [`FEED_QUEUE`] chunks behind.
    pub fn feed(&self, bytes: Vec<u8>) -> Result<(), FeedError> {
        let feed = self.feed.read().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = feed.as_ref() else {
            return Err(FeedError::Ended);
        };
        match sender.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(FeedError::Behind),
            Err(TrySendError::Closed(_)) => Err(FeedError::Ended),
        }
    }

    /// The observed command ended: the reader sees end of stream after the last fed chunk, and the host
    /// reports `exit_code` (a missing one as -1, the same as a process the platform lost) once it has drained.
    pub fn end(&self, exit_code: Option<i32>) {
        let mut exit = self.exit.write().unwrap_or_else(PoisonError::into_inner);
        if exit.is_none() {
            *exit = Some(exit_code.unwrap_or(-1));
        }
        drop(exit);
        drop(
            self.feed
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }

    pub(super) fn try_wait(&self) -> Option<i32> {
        *self.exit.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// A stop of an observed mirror ends the feed; the owner's process is not ours to end.
    pub(super) fn kill(&self) {
        self.end(None);
    }
}

/// The host's end of the feed, blocking like a pipe.
struct FeedReader {
    receiver: Receiver<Vec<u8>>,
    pending: Vec<u8>,
    at: usize,
}

impl FeedReader {
    fn remaining(&self) -> usize {
        self.pending.len().saturating_sub(self.at)
    }
}

impl Read for FeedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining() == 0 {
            let Some(chunk) = self.receiver.blocking_recv() else {
                return Ok(0);
            };
            self.pending = chunk;
            self.at = 0;
        }
        let rest = self.pending.get(self.at..).unwrap_or(&[]);
        let count = rest.len().min(buffer.len());
        if let (Some(target), Some(source)) = (buffer.get_mut(..count), rest.get(..count)) {
            target.copy_from_slice(source);
        }
        self.at += count;
        Ok(count)
    }
}

impl TerminalRead for FeedReader {
    fn available(&mut self) -> usize {
        if self.remaining() == 0
            && let Ok(chunk) = self.receiver.try_recv()
        {
            self.pending = chunk;
            self.at = 0;
        }
        self.remaining()
    }
}

/// Where the host's query answers go on an observed mirror: nowhere. The owner window's terminal answered the
/// provider's queries itself; a second answer from here would reach nothing, and accepting it keeps the host's
/// writer path healthy instead of turning every answered query into a lost-writer close.
struct AnsweredElsewhere;

impl Write for AnsweredElsewhere {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fed_bytes_come_out_of_the_reader_in_order_and_the_end_reports_the_exit_code() {
        let child = FedChild::new(7);
        let mut reader = child.reader().expect("first reader");
        assert!(child.reader().is_err(), "the reader is taken once");
        child.feed(b"ab".to_vec()).expect("first chunk");
        child.feed(b"cd".to_vec()).expect("second chunk");
        assert_eq!(child.try_wait(), None);
        let mut buffer = [0_u8; 3];
        assert_eq!(reader.read(&mut buffer).expect("read"), 2);
        assert_eq!(buffer.get(..2), Some(b"ab".as_slice()));
        assert_eq!(reader.available(), 2);
        assert_eq!(reader.read(&mut buffer).expect("read"), 2);
        assert_eq!(buffer.get(..2), Some(b"cd".as_slice()));
        child.end(Some(3));
        assert_eq!(reader.read(&mut buffer).expect("end"), 0);
        assert_eq!(child.try_wait(), Some(3));
        assert_eq!(child.feed(b"late".to_vec()), Err(FeedError::Ended));
        assert_eq!(child.pid(), 7);
        assert_eq!(
            FedChild::writer()
                .write(b"\x1b[0n")
                .expect("answer accepted"),
            4
        );
    }

    #[test]
    fn a_feeder_far_ahead_of_the_reader_is_refused_without_losing_what_was_accepted() {
        let child = FedChild::new(0);
        let mut reader = child.reader().expect("reader");
        for _ in 0..FEED_QUEUE {
            child.feed(vec![1]).expect("fits");
        }
        assert_eq!(child.feed(vec![2]), Err(FeedError::Behind));
        let mut buffer = [0_u8; 1];
        assert_eq!(reader.read(&mut buffer).expect("read"), 1);
        child.feed(vec![2]).expect("room again");
        child.kill();
        assert_eq!(child.try_wait(), Some(-1));
    }
}
