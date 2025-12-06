//! # fifo-cache
//!
//! High-performance cache implementations based on cutting-edge research:
//!
//! - **S3-FIFO**: Simple, Scalable FIFO-based cache (SOSP 2023)
//! - **SIEVE**: Simpler than LRU eviction algorithm (NSDI 2024 Best Paper)
//!
//! ## Key Features
//!
//! - **Lock-free cache hits**: Both algorithms only update atomic metadata on hits
//! - **6× throughput vs LRU**: Scales linearly to 16+ threads
//! - **Up to 72% lower miss ratio**: Validated on 6,594 production traces
//!
//! ## Quick Start
//!
//! ```rust
//! use fifo_cache::{Cache, CacheConfig};
//!
//! // Create a SIEVE cache (simpler, great for web workloads)
//! let cache = CacheConfig::new(10_000)
//!     .build_sieve::<String, Vec<u8>>()
//!     .unwrap();
//!
//! cache.insert("key".to_string(), vec![1, 2, 3]);
//! assert_eq!(cache.get(&"key".to_string()), Some(vec![1, 2, 3]));
//!
//! // Check hit ratio
//! println!("Hit ratio: {:.2}%", cache.stats().hit_ratio() * 100.0);
//! ```
//!
//! ## Algorithm Selection
//!
//! | Workload | Recommended | Reason |
//! |----------|-------------|--------|
//! | Web/CDN cache | SIEVE | High temporal locality |
//! | Block storage | S3-FIFO | Scan resistance |
//! | Key-value store | Either | Zipfian distribution |
//! | Database buffer | S3-FIFO | Mixed access patterns |
//!
//! ## References
//!
//! - [S3-FIFO Paper (SOSP 2023)](https://jasony.me/publication/sosp23-s3fifo.pdf)
//! - [SIEVE Paper (NSDI 2024)](https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo)

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]

mod cache;
mod config;
mod error;
mod node;
mod stats;

// Public API re-exports
pub use cache::{Cache, S3FifoCache, SieveCache};
pub use config::CacheConfig;
pub use error::{CacheError, Result};
pub use stats::CacheStats;
