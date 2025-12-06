//! Cache entry with atomic metadata.
//!
//! The `CacheEntry` struct stores key-value pairs with metadata that can be
//! atomically updated without locks. This enables lock-free cache hits.
//!
//! ## Metadata Usage
//!
//! - **S3-FIFO**: 2-bit frequency counter (values 0-3)
//! - **SIEVE**: 1-bit visited flag (0 or 1)
//!
//! Both use `AtomicU8` for platform compatibility and atomic operations.

use std::sync::atomic::{AtomicU8, Ordering};

// SIEVE visited flag values
const VISITED: u8 = 1;
const NOT_VISITED: u8 = 0;

/// A cache entry containing a key-value pair and atomic metadata.
///
/// The metadata byte is used differently by each algorithm:
/// - S3-FIFO uses it as a 2-bit frequency counter (0-3)
/// - SIEVE uses it as a 1-bit visited flag (0 or 1)
///
/// All metadata operations use relaxed ordering since:
/// 1. The counters are advisory (slight inaccuracy is acceptable)
/// 2. This is the hot path - performance is critical
/// 3. No happens-before relationships depend on these values
pub struct CacheEntry<K, V> {
    /// The cache key.
    key: K,

    /// The cached value.
    value: V,

    /// Atomic metadata byte.
    ///
    /// - S3-FIFO: frequency counter (0-3)
    /// - SIEVE: visited flag (0 or 1)
    metadata: AtomicU8,
}

impl<K, V> CacheEntry<K, V> {
    /// Create a new cache entry with metadata initialized to 0.
    #[must_use]
    pub fn new(key: K, value: V) -> Self {
        Self {
            key,
            value,
            metadata: AtomicU8::new(0),
        }
    }

    /// Create a new cache entry with the specified initial metadata.
    #[must_use]
    pub fn with_metadata(key: K, value: V, metadata: u8) -> Self {
        Self {
            key,
            value,
            metadata: AtomicU8::new(metadata),
        }
    }

    /// Get a reference to the key.
    #[must_use]
    #[inline]
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Get a reference to the value.
    #[must_use]
    #[inline]
    pub fn value(&self) -> &V {
        &self.value
    }

    /// Get the raw metadata value.
    #[must_use]
    #[inline]
    pub fn metadata(&self) -> u8 {
        self.metadata.load(Ordering::Relaxed)
    }

    // =========================================================================
    // S3-FIFO Operations (2-bit frequency counter)
    // =========================================================================

    /// Maximum frequency value (2 bits = 0-3).
    ///
    /// From paper analysis: 2 bits is sufficient; more bits provide
    /// negligible improvement while increasing memory overhead.
    ///
    /// This value is sourced from `config::defaults::MAX_FREQUENCY`.
    pub const MAX_FREQ: u8 = crate::config::defaults::MAX_FREQUENCY;

    /// Increment the frequency counter, capped at `MAX_FREQ`.
    ///
    /// This is the only operation needed on cache hit for S3-FIFO.
    /// Uses compare-and-swap to atomically increment without exceeding max.
    ///
    /// # Performance
    ///
    /// O(1) atomic operation. No locks required.
    #[inline]
    pub fn increment_freq(&self) {
        // Attempt to increment, but don't exceed MAX_FREQ
        let _ = self.metadata.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v < Self::MAX_FREQ {
                Some(v + 1)
            } else {
                None // Already at max, no update needed
            }
        });
    }

    /// Increment the frequency counter with a custom maximum.
    ///
    /// Allows overriding the default maximum for experimentation.
    #[inline]
    pub fn increment_freq_with_max(&self, max_freq: u8) {
        let _ = self.metadata.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v < max_freq {
                Some(v + 1)
            } else {
                None
            }
        });
    }

    /// Decrement the frequency counter, saturating at 0.
    ///
    /// Used during S3-FIFO main queue eviction.
    /// Returns the value before decrementing.
    #[inline]
    pub fn decrement_freq(&self) -> u8 {
        self.metadata
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            })
            .unwrap_or(0)
    }

    /// Get the current frequency value.
    #[must_use]
    #[inline]
    pub fn freq(&self) -> u8 {
        self.metadata.load(Ordering::Relaxed)
    }

    /// Reset the frequency counter to 0.
    ///
    /// Used when promoting an entry from small to main queue.
    #[inline]
    pub fn reset_freq(&self) {
        self.metadata.store(0, Ordering::Relaxed);
    }

    /// Set the frequency to a specific value.
    #[inline]
    pub fn set_freq(&self, freq: u8) {
        self.metadata.store(freq, Ordering::Relaxed);
    }

    // =========================================================================
    // SIEVE Operations (1-bit visited flag)
    // =========================================================================

    /// Mark the entry as visited.
    ///
    /// This is the only operation needed on cache hit for SIEVE.
    /// Subsequent hits on an already-visited entry are no-ops.
    ///
    /// # Performance
    ///
    /// O(1) atomic store. No locks required.
    #[inline]
    pub fn mark_visited(&self) {
        self.metadata.store(VISITED, Ordering::Relaxed);
    }

    /// Clear the visited flag.
    ///
    /// Used during SIEVE eviction when the hand passes over a visited entry.
    #[inline]
    pub fn clear_visited(&self) {
        self.metadata.store(NOT_VISITED, Ordering::Relaxed);
    }

    /// Check if the entry has been visited.
    #[must_use]
    #[inline]
    pub fn is_visited(&self) -> bool {
        self.metadata.load(Ordering::Relaxed) != NOT_VISITED
    }
}

impl<K: Clone, V: Clone> Clone for CacheEntry<K, V> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            value: self.value.clone(),
            metadata: AtomicU8::new(self.metadata.load(Ordering::Relaxed)),
        }
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for CacheEntry<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheEntry")
            .field("key", &self.key)
            .field("value", &self.value)
            .field("metadata", &self.metadata.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_entry() {
        let entry = CacheEntry::new("key", 42);
        assert_eq!(entry.key(), &"key");
        assert_eq!(entry.value(), &42);
        assert_eq!(entry.metadata(), 0);
    }

    #[test]
    fn test_with_metadata() {
        let entry = CacheEntry::with_metadata("key", 42, 2);
        assert_eq!(entry.metadata(), 2);
    }

    // S3-FIFO tests

    #[test]
    fn test_freq_increment() {
        let entry = CacheEntry::new("key", 42);

        entry.increment_freq();
        assert_eq!(entry.freq(), 1);

        entry.increment_freq();
        assert_eq!(entry.freq(), 2);

        entry.increment_freq();
        assert_eq!(entry.freq(), 3);

        // Should not exceed MAX_FREQ
        entry.increment_freq();
        assert_eq!(entry.freq(), 3);
    }

    #[test]
    fn test_freq_decrement() {
        let entry = CacheEntry::new("key", 42);
        entry.set_freq(3);

        assert_eq!(entry.decrement_freq(), 3); // Returns old value
        assert_eq!(entry.freq(), 2);

        assert_eq!(entry.decrement_freq(), 2);
        assert_eq!(entry.freq(), 1);

        assert_eq!(entry.decrement_freq(), 1);
        assert_eq!(entry.freq(), 0);

        // Should not go below 0
        assert_eq!(entry.decrement_freq(), 0);
        assert_eq!(entry.freq(), 0);
    }

    #[test]
    fn test_freq_reset() {
        let entry = CacheEntry::new("key", 42);
        entry.set_freq(3);
        entry.reset_freq();
        assert_eq!(entry.freq(), 0);
    }

    #[test]
    fn test_custom_max_freq() {
        let entry = CacheEntry::new("key", 42);

        // Use custom max of 7
        for _ in 0..10 {
            entry.increment_freq_with_max(7);
        }
        assert_eq!(entry.freq(), 7);
    }

    // SIEVE tests

    #[test]
    fn test_visited_flag() {
        let entry = CacheEntry::new("key", 42);

        assert!(!entry.is_visited());

        entry.mark_visited();
        assert!(entry.is_visited());

        entry.clear_visited();
        assert!(!entry.is_visited());
    }

    #[test]
    fn test_multiple_marks() {
        let entry = CacheEntry::new("key", 42);

        // Multiple marks should be idempotent
        entry.mark_visited();
        entry.mark_visited();
        entry.mark_visited();

        assert!(entry.is_visited());
        assert_eq!(entry.metadata(), 1);
    }

    #[test]
    fn test_clone() {
        let entry = CacheEntry::new("key".to_string(), vec![1, 2, 3]);
        entry.set_freq(2);

        let cloned = entry.clone();
        assert_eq!(cloned.key(), entry.key());
        assert_eq!(cloned.value(), entry.value());
        assert_eq!(cloned.freq(), entry.freq());
    }

    #[test]
    fn test_debug() {
        let entry = CacheEntry::new("key", 42);
        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("CacheEntry"));
        assert!(debug_str.contains("key"));
        assert!(debug_str.contains("42"));
    }
}
