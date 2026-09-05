//! A finite terminal operation outlives its caller until process and workspace ownership settle.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Composed;

pub(crate) struct TerminalOperation(Arc<Composed>);

impl TerminalOperation {
    pub(crate) fn begin(composed: &Arc<Composed>) -> Self {
        composed.terminal_operations.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(composed))
    }
}

impl Drop for TerminalOperation {
    fn drop(&mut self) {
        self.0.terminal_operations.fetch_sub(1, Ordering::AcqRel);
        self.0.terminal_closed.notify_one();
    }
}

/// Dropping a queued open cancels it. Once process creation begins, its owner finishes registration.
pub(super) struct LaunchCaller(pub(super) Arc<AtomicBool>);

impl LaunchCaller {
    pub(super) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

impl Drop for LaunchCaller {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
