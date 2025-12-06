//! Cache statistics with atomic counters.
//!
//! Provides thread-safe hit/miss tracking with minimal overhead.
//! Uses relaxed ordering since exact counts are not critical.

use std::sync::atomic::{AtomicU64, Ordering};

/// Cache statistics snapshot.
///
/// Contains point-in-time counters for cache operations.
/// All counters are updated atomically and are thread-safe.
///
/// # Example
///
/// ```rust
/// use fifo_cache::{Cache, CacheConfig};
///
/// let cache = CacheConfig::new(100).build_sieve::<u64, u64>().unwrap();
///
/// // Perform some operations
/// cache.insert(1, 100);
/// cache.get(&1);
/// cache.get(&2); // miss
///
/// // Check statistics
/// let stats = cache.stats();
/// println!("Hit ratio: {:.2}%", stats.hit_ratio() * 100.0);
/// println!("Total hits: {}", stats.hits());
/// println!("Total misses: {}", stats.misses());
/// ```
#[derive(Debug, Default)]
pub struct CacheStats {
    /// Number of cache hits.
    hits: AtomicU64,

    /// Number of cache misses.
    misses: AtomicU64,

    /// Number of insertions.
    insertions: AtomicU64,

    /// Number of evictions.
    evictions: AtomicU64,
}

impl CacheStats {
    /// Create a new statistics tracker with all counters at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // === Recording operations ===

    /// Record a cache hit.
    ///
    /// Called when `get()` finds the requested key.
    #[inline]
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss.
    ///
    /// Called when `get()` does not find the requested key.
    #[inline]
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an insertion.
    ///
    /// Called when a new key-value pair is added.
    #[inline]
    pub fn record_insertion(&self) {
        self.insertions.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an eviction.
    ///
    /// Called when an entry is removed to make room for a new one.
    #[inline]
    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    // === Getters ===

    /// Get the total number of cache hits.
    #[must_use]
    #[inline]
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Get the total number of cache misses.
    #[must_use]
    #[inline]
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Get the total number of insertions.
    #[must_use]
    #[inline]
    pub fn insertions(&self) -> u64 {
        self.insertions.load(Ordering::Relaxed)
    }

    /// Get the total number of evictions.
    #[must_use]
    #[inline]
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    // === Computed metrics ===

    /// Calculate the cache hit ratio.
    ///
    /// Returns a value between 0.0 and 1.0.
    /// Returns 0.0 if no accesses have been made.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fifo_cache::CacheStats;
    ///
    /// let stats = CacheStats::new();
    /// // Simulate 3 hits and 1 miss
    /// stats.record_hit();
    /// stats.record_hit();
    /// stats.record_hit();
    /// stats.record_miss();
    ///
    /// assert!((stats.hit_ratio() - 0.75).abs() < 0.001);
    /// ```
    #[must_use]
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits();
        let total = hits + self.misses();
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Calculate the cache miss ratio.
    ///
    /// Returns a value between 0.0 and 1.0.
    /// Returns 0.0 if no accesses have been made.
    #[must_use]
    pub fn miss_ratio(&self) -> f64 {
        1.0 - self.hit_ratio()
    }

    /// Get the total number of accesses (hits + misses).
    #[must_use]
    pub fn total_accesses(&self) -> u64 {
        self.hits() + self.misses()
    }

    /// Reset all counters to zero.
    ///
    /// Useful for measuring statistics over a specific time period.
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.insertions.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
    }
}

impl Clone for CacheStats {
    /// Create a snapshot of the current statistics.
    ///
    /// Note: This is a point-in-time snapshot. Values may have changed
    /// by the time the clone is returned.
    fn clone(&self) -> Self {
        Self {
            hits: AtomicU64::new(self.hits()),
            misses: AtomicU64::new(self.misses()),
            insertions: AtomicU64::new(self.insertions()),
            evictions: AtomicU64::new(self.evictions()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let stats = CacheStats::new();
        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
        assert_eq!(stats.insertions(), 0);
        assert_eq!(stats.evictions(), 0);
        assert!((stats.hit_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recording() {
        let stats = CacheStats::new();

        stats.record_hit();
        stats.record_hit();
        stats.record_miss();
        stats.record_insertion();
        stats.record_eviction();
        stats.record_eviction();

        assert_eq!(stats.hits(), 2);
        assert_eq!(stats.misses(), 1);
        assert_eq!(stats.insertions(), 1);
        assert_eq!(stats.evictions(), 2);
    }

    #[test]
    fn test_hit_ratio() {
        let stats = CacheStats::new();

        // 75% hit ratio
        stats.record_hit();
        stats.record_hit();
        stats.record_hit();
        stats.record_miss();

        assert!((stats.hit_ratio() - 0.75).abs() < 0.001);
        assert!((stats.miss_ratio() - 0.25).abs() < 0.001);
        assert_eq!(stats.total_accesses(), 4);
    }

    #[test]
    fn test_reset() {
        let stats = CacheStats::new();
        stats.record_hit();
        stats.record_miss();
        stats.record_insertion();
        stats.record_eviction();

        stats.reset();

        assert_eq!(stats.hits(), 0);
        assert_eq!(stats.misses(), 0);
        assert_eq!(stats.insertions(), 0);
        assert_eq!(stats.evictions(), 0);
    }

    #[test]
    fn test_clone() {
        let stats = CacheStats::new();
        stats.record_hit();
        stats.record_hit();
        stats.record_miss();

        let snapshot = stats.clone();

        // Modify original
        stats.record_hit();

        // Snapshot should be unchanged
        assert_eq!(snapshot.hits(), 2);
        assert_eq!(stats.hits(), 3);
    }
}
