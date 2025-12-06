//! Cache trait and implementations.
//!
//! This module provides:
//! - [`Cache`]: The core trait defining cache operations
//! - [`SieveCache`]: SIEVE algorithm implementation (NSDI 2024)
//! - [`S3FifoCache`]: S3-FIFO algorithm implementation (SOSP 2023)
//! - [`LruCache`]: LRU baseline implementation (for benchmarks)

mod lru;
mod s3fifo;
mod sieve;

pub use lru::LruCache;
pub use s3fifo::S3FifoCache;
pub use sieve::SieveCache;

use std::hash::Hash;

use crate::stats::CacheStats;

/// Core cache trait defining the interface for all cache implementations.
///
/// This trait is algorithm-agnostic and can be implemented by any cache
/// eviction algorithm. Both `SieveCache` and `S3FifoCache` implement this trait.
///
/// # Thread Safety
///
/// All implementations are required to be `Send + Sync`, enabling safe
/// concurrent access from multiple threads.
///
/// # Example
///
/// ```rust
/// use fifo_cache::{Cache, CacheConfig};
///
/// // Works with any Cache implementation
/// fn use_cache<C: Cache<String, i32>>(cache: &C) {
///     cache.insert("answer".to_string(), 42);
///
///     if let Some(value) = cache.get(&"answer".to_string()) {
///         println!("The answer is {}", value);
///     }
///
///     println!("Hit ratio: {:.2}%", cache.stats().hit_ratio() * 100.0);
/// }
/// ```
pub trait Cache<K, V>: Send + Sync
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// Retrieve a value from the cache.
    ///
    /// This operation updates the access metadata:
    /// - S3-FIFO: Increments the frequency counter
    /// - SIEVE: Sets the visited flag
    ///
    /// # Arguments
    ///
    /// * `key` - The key to look up.
    ///
    /// # Returns
    ///
    /// - `Some(value)` if the key exists (records a hit)
    /// - `None` if the key does not exist (records a miss)
    ///
    /// # Performance
    ///
    /// O(1) average case. For hot entries, this is lock-free (atomic only).
    fn get(&self, key: &K) -> Option<V>;

    /// Insert or update a key-value pair.
    ///
    /// If the key already exists, the value is updated.
    /// If the cache is at capacity, entries may be evicted.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to insert.
    /// * `value` - The value to associate with the key.
    ///
    /// # Returns
    ///
    /// - `Some(old_value)` if the key already existed
    /// - `None` if this is a new key
    ///
    /// # Performance
    ///
    /// O(1) amortized. May trigger eviction.
    fn insert(&self, key: K, value: V) -> Option<V>;

    /// Remove a key-value pair from the cache.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to remove.
    ///
    /// # Returns
    ///
    /// - `Some(value)` if the key existed
    /// - `None` if the key did not exist
    fn remove(&self, key: &K) -> Option<V>;

    /// Check if a key exists in the cache.
    ///
    /// This operation does NOT update access metadata or statistics.
    /// Use `get()` if you want to update metadata.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to check.
    ///
    /// # Returns
    ///
    /// `true` if the key exists, `false` otherwise.
    fn contains(&self, key: &K) -> bool;

    /// Get the current number of entries in the cache.
    fn len(&self) -> usize;

    /// Check if the cache is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the maximum capacity of the cache.
    fn capacity(&self) -> usize;

    /// Check if the cache is at capacity.
    fn is_full(&self) -> bool {
        self.len() >= self.capacity()
    }

    /// Remove all entries from the cache.
    ///
    /// Statistics are preserved. Use `stats().reset()` to clear them.
    fn clear(&self);

    /// Get a reference to the cache statistics.
    ///
    /// Statistics include:
    /// - Hit count
    /// - Miss count
    /// - Insertion count
    /// - Eviction count
    fn stats(&self) -> &CacheStats;

    // === Extension methods with default implementations ===

    /// Get a value or insert a default.
    ///
    /// If the key exists, returns the existing value.
    /// Otherwise, inserts the result of `default()` and returns it.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to look up or insert.
    /// * `default` - Function to compute the default value.
    ///
    /// # Returns
    ///
    /// The existing or newly inserted value.
    fn get_or_insert_with<F>(&self, key: K, default: F) -> V
    where
        F: FnOnce() -> V,
    {
        if let Some(value) = self.get(&key) {
            return value;
        }

        let value = default();
        self.insert(key, value.clone());
        value
    }

    /// Peek at a value without updating access metadata.
    ///
    /// Unlike `get()`, this does not affect hit/miss statistics
    /// or eviction priority. Useful for inspection.
    ///
    /// Default implementation calls `get()`. Override for algorithms
    /// where this distinction matters.
    fn peek(&self, key: &K) -> Option<V> {
        // Default: same as get. Implementations can override.
        self.get(key)
    }
}
