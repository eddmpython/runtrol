//! Opt-in, body-free read and publication observations for native transport measurements.
//!
//! This module is absent from shipping builds. The attempt supplies its own monotonic clock and bounded,
//! nonblocking sink. Neither a public frame nor a provider byte is changed or retained here.

use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use runtrol_childproc::pty::TerminalRead;

static OBSERVER: OnceLock<Arc<dyn Observer>> = OnceLock::new();
static NEXT_TERMINAL: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
mod tests;

/// One host object, independent of PID reuse. The attempt additionally proves the native PID birth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// The process containing this observer.
    pub host_pid: u32,
    /// Monotonic identity within this host process.
    pub terminal: u64,
    /// The exact child whose reader this host owns.
    pub child_pid: u32,
}

/// A structural boundary. Byte counts refer to raw bytes, never decoded characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// Hosting began before its reader started.
    Hosted,
    /// A successful nonempty OS read returned, before any coalescing wait.
    Read {
        /// Ordinal of this successful read, starting at one.
        ordinal: u64,
        /// Bytes returned by this read.
        bytes: usize,
        /// Total bytes returned by this reader through this boundary.
        through: u64,
    },
    /// A raw publication was assigned its ordinary public output sequence.
    Published {
        /// Existing output sequence, with no private field added to the wire.
        sequence: u64,
        /// Bytes in this unchanged publication.
        bytes: usize,
        /// Total raw bytes published by this terminal.
        through: u64,
    },
    /// One completed atomic checkpoint and live subscription. Public view counters start at one here.
    Attached {
        /// Completed attachment ordinal for this exact host object.
        ordinal: u64,
        /// Existing raw sequence of the first live chunk after the checkpoint.
        next_sequence: u64,
    },
}

/// One timestamp in the attempt's explicitly selected clock domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// Exact host object identity.
    pub identity: Identity,
    /// Clock value captured before constructing this observation.
    pub tick: u64,
    /// Successful read or raw publication metadata.
    pub boundary: Boundary,
}

/// A measurement-only clock and bounded sink. Callbacks must not block, panic, or inspect output bodies.
pub trait Observer: Send + Sync {
    /// Return a monotonic clock value. Native Windows attempts use QPC for every observation surface.
    fn tick(&self) -> u64;
    /// Accept only this structural record. Overflow must invalidate the attempt instead of losing evidence.
    fn record(&self, observation: Observation);
}

/// Install one observer before opening any terminals in this measurement process.
///
/// The observer remains installed for the finite attempt process lifetime. Replacing it would make read
/// offsets from one observer appear to belong to another and is deliberately refused.
///
/// # Errors
///
/// Returns the unused observer if a previous installation already owns this process.
pub fn install(observer: Arc<dyn Observer>) -> Result<(), Arc<dyn Observer>> {
    OBSERVER.set(observer)
}

pub(super) struct Trace {
    identity: Identity,
    observer: Arc<dyn Observer>,
    published_bytes: AtomicU64,
    attachments: AtomicU64,
}

impl Trace {
    pub(super) fn new(child_pid: u32) -> Option<Arc<Self>> {
        let observer = Arc::clone(OBSERVER.get()?);
        let tick = observer.tick();
        let trace = Arc::new(Self {
            identity: Identity {
                host_pid: std::process::id(),
                terminal: NEXT_TERMINAL.fetch_add(1, Ordering::Relaxed),
                child_pid,
            },
            observer,
            published_bytes: AtomicU64::new(0),
            attachments: AtomicU64::new(0),
        });
        trace.record(tick, Boundary::Hosted);
        Some(trace)
    }

    fn record(&self, tick: u64, boundary: Boundary) {
        self.observer.record(Observation {
            identity: self.identity,
            tick,
            boundary,
        });
    }

    pub(super) fn publish(&self, sequence: u64, bytes: usize) {
        let tick = self.observer.tick();
        let through = self
            .published_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed)
            + bytes as u64;
        self.record(
            tick,
            Boundary::Published {
                sequence,
                bytes,
                through,
            },
        );
    }

    pub(super) fn attached(&self, next_sequence: u64) {
        self.record(
            self.observer.tick(),
            Boundary::Attached {
                ordinal: self.attachments.fetch_add(1, Ordering::Relaxed) + 1,
                next_sequence,
            },
        );
    }
}

pub(super) fn reader(
    inner: Box<dyn TerminalRead>,
    trace: Option<&Arc<Trace>>,
) -> Box<dyn TerminalRead> {
    let Some(trace) = trace else {
        return inner;
    };
    Box::new(ObservedReader {
        inner,
        trace: Arc::clone(trace),
        ordinal: 0,
        through: 0,
    })
}

struct ObservedReader {
    inner: Box<dyn TerminalRead>,
    trace: Arc<Trace>,
    ordinal: u64,
    through: u64,
}

impl Read for ObservedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(bytes)?;
        let tick = self.trace.observer.tick();
        if count > 0 {
            self.ordinal += 1;
            self.through += count as u64;
            self.trace.record(
                tick,
                Boundary::Read {
                    ordinal: self.ordinal,
                    bytes: count,
                    through: self.through,
                },
            );
        }
        Ok(count)
    }
}

impl TerminalRead for ObservedReader {
    fn available(&mut self) -> usize {
        self.inner.available()
    }
}
