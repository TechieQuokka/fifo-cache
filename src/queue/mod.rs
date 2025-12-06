//! Queue abstractions for cache implementations.
//!
//! This module provides:
//! - [`FifoQueue`]: Basic FIFO queue with O(1) operations
//! - [`GhostQueue`]: Metadata-only queue for tracking evicted keys

mod fifo;
mod ghost;

pub use fifo::FifoQueue;
pub use ghost::GhostQueue;
