//! Safe supervisor-side cancellation state. Renderer checkpoints are a separate API.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cloneable cancellation flag for one serial session's generated tiles.
///
/// Set from a native supervisor thread. Reset only after the cancelled request
/// has completed; the synchronous WASI command has no shared-memory flag bridge.
/// Checking this flag does not interrupt an executing renderer operation.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// Request cancellation. Repeated calls are idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Reset after acknowledgement, before submitting a new generation.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
