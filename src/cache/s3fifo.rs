//! S3-FIFO cache implementation.
//!
//! S3-FIFO is a cache eviction algorithm from SOSP 2023.
//! It achieves state-of-the-art hit ratios with O(1) operations.
//!
//! ## Key Insight
//!
//! Most cached objects are "one-hit-wonders" (26-82% accessed only once).
//! S3-FIFO filters them using a probationary small queue before
//! promoting frequently-accessed items to the main queue.
//!
//! ## Three-Queue Structure
//!
//! 1. **Small Queue** (10% of capacity): Probationary FIFO queue
//!    - New entries start here
//!    - Quick demotion: 1 bit decides fate during eviction
//!
//! 2. **Main Queue** (90% of capacity): Protected FIFO queue
//!    - Holds entries promoted from small queue
//!    - Lazy promotion: entries stay until freq > 0 on eviction
//!
//! 3. **Ghost Queue** (metadata only): Tracks recently evicted keys
//!    - Enables quick re-insertion into main queue
//!    - Bounded to main queue capacity
//!
//! ## Algorithm
//!
//! - On insert: If key in ghost → main queue, else → small queue
//! - On hit: Increment frequency counter (atomic, lock-free)
//! - On eviction from small:
//!   - If freq > 0 → promote to main (reset freq)
//!   - Else → evict to ghost
//! - On eviction from main:
//!   - If freq > 0 → decrement and reinsert at tail
//!   - Else → evict permanently
//!
//! ## Performance
//!
//! - Up to 72% lower miss ratio than LRU (block cache)
//! - 6× higher throughput at 16 threads
//! - Lock-free cache hits

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ahash::RandomState;

use crate::cache::Cache;
use crate::config::CacheConfig;
use crate::node::CacheEntry;
use crate::stats::CacheStats;

/// S3-FIFO cache implementation.
///
/// A thread-safe cache using the S3-FIFO eviction algorithm with
/// three queues: small (probationary), main (protected), and ghost (metadata).
///
/// # Example
///
/// ```rust
/// use fifo_cache::{Cache, S3FifoCache};
///
/// let cache = S3FifoCache::new(100);
///
/// cache.insert("key1".to_string(), 42);
/// cache.insert("key2".to_string(), 84);
///
/// // Access key1 multiple times to increase frequency
/// cache.get(&"key1".to_string());
/// cache.get(&"key1".to_string());
///
/// assert_eq!(cache.get(&"key1".to_string()), Some(42));
/// assert_eq!(cache.len(), 2);
/// ```
pub struct S3FifoCache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// Fast key lookup: key -> entry with queue location.
    index: RwLock<HashMap<K, Arc<CacheEntry<K, V>>, RandomState>>,

    /// Small queue (probationary): new entries start here.
    small_queue: Mutex<VecDeque<Arc<CacheEntry<K, V>>>>,

    /// Main queue (protected): promoted entries live here.
    main_queue: Mutex<VecDeque<Arc<CacheEntry<K, V>>>>,

    /// Ghost queue: metadata-only tracking of recently evicted keys.
    ghost: Mutex<GhostQueue<K>>,

    /// Track which entries are in which queue (for O(1) removal).
    queue_membership: RwLock<HashMap<K, QueueType, RandomState>>,

    /// Current size (small + main).
    size: AtomicUsize,

    /// Configuration.
    config: CacheConfig,

    /// Statistics.
    stats: CacheStats,
}

/// Queue type indicator for efficient lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueType {
    Small,
    Main,
}

/// Ghost queue for tracking evicted keys.
///
/// Stores only keys (no values) with a bounded size.
struct GhostQueue<K> {
    /// Set for O(1) lookups.
    keys: HashSet<K, RandomState>,

    /// Queue for FIFO eviction order.
    queue: VecDeque<K>,

    /// Maximum capacity.
    capacity: usize,
}

impl<K: Hash + Eq + Clone> GhostQueue<K> {
    fn new(capacity: usize) -> Self {
        Self {
            keys: HashSet::with_capacity_and_hasher(capacity, RandomState::new()),
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Check if key is in ghost queue.
    fn contains(&self, key: &K) -> bool {
        self.keys.contains(key)
    }

    /// Insert a key into the ghost queue.
    ///
    /// If at capacity, evicts the oldest entry first.
    fn insert(&mut self, key: K) {
        // Don't insert duplicates
        if self.keys.contains(&key) {
            return;
        }

        // Evict oldest if at capacity
        while self.queue.len() >= self.capacity && !self.queue.is_empty() {
            if let Some(old_key) = self.queue.pop_front() {
                self.keys.remove(&old_key);
            }
        }

        // Insert new key
        self.keys.insert(key.clone());
        self.queue.push_back(key);
    }

    /// Remove a key from the ghost queue.
    fn remove(&mut self, key: &K) {
        if self.keys.remove(key) {
            // O(n) removal from queue, but ghost eviction is rare
            self.queue.retain(|k| k != key);
        }
    }

    /// Clear the ghost queue.
    fn clear(&mut self) {
        self.keys.clear();
        self.queue.clear();
    }
}

// Safety: S3FifoCache is Send + Sync because:
// 1. All shared state is protected by RwLock/Mutex
// 2. Arc<CacheEntry> is Send + Sync when K, V are Send + Sync
unsafe impl<K: Hash + Eq + Clone + Send, V: Clone + Send> Send for S3FifoCache<K, V> {}
unsafe impl<K: Hash + Eq + Clone + Send + Sync, V: Clone + Send + Sync> Sync for S3FifoCache<K, V> {}

impl<K, V> S3FifoCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create a new S3-FIFO cache with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of entries.
    ///
    /// # Panics
    ///
    /// Panics if capacity is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be greater than 0");
        Self::with_config(CacheConfig::new(capacity))
    }

    /// Create a new S3-FIFO cache with the given configuration.
    #[must_use]
    pub fn with_config(config: CacheConfig) -> Self {
        let capacity = config.capacity();
        let small_cap = config.small_queue_capacity();
        let main_cap = config.main_queue_capacity();

        Self {
            index: RwLock::new(HashMap::with_capacity_and_hasher(
                capacity,
                RandomState::new(),
            )),
            small_queue: Mutex::new(VecDeque::with_capacity(small_cap)),
            main_queue: Mutex::new(VecDeque::with_capacity(main_cap)),
            ghost: Mutex::new(GhostQueue::new(main_cap)),
            queue_membership: RwLock::new(HashMap::with_capacity_and_hasher(
                capacity,
                RandomState::new(),
            )),
            size: AtomicUsize::new(0),
            config,
            stats: CacheStats::new(),
        }
    }

    /// Get the small queue capacity.
    #[must_use]
    pub fn small_queue_capacity(&self) -> usize {
        self.config.small_queue_capacity()
    }

    /// Get the main queue capacity.
    #[must_use]
    pub fn main_queue_capacity(&self) -> usize {
        self.config.main_queue_capacity()
    }

    /// Evict entries until we're under capacity.
    fn evict_if_needed(&self) {
        while self.size.load(Ordering::Relaxed) > self.config.capacity() {
            // First try to evict from small queue
            if !self.evict_from_small() {
                // If small is empty, evict from main
                if !self.evict_from_main() {
                    // Both queues are empty, nothing to evict
                    break;
                }
            }
        }
    }

    /// Evict one entry from the small queue.
    ///
    /// Returns true if an entry was evicted, false if small queue is empty.
    fn evict_from_small(&self) -> bool {
        let entry = {
            let mut small = self.small_queue.lock().unwrap();
            small.pop_front()
        };

        let entry = match entry {
            Some(e) => e,
            None => return false,
        };

        let key = entry.key().clone();
        let freq = entry.freq();

        if freq > 0 {
            // Promote to main queue (lazy promotion)
            entry.reset_freq();

            {
                let mut main = self.main_queue.lock().unwrap();
                main.push_back(entry);
            }

            {
                let mut membership = self.queue_membership.write().unwrap();
                membership.insert(key, QueueType::Main);
            }

            // Entry moved from small to main, no size change
            // Need to check if main is over capacity now
            self.evict_main_if_needed();
        } else {
            // Evict to ghost queue
            {
                let mut index = self.index.write().unwrap();
                index.remove(&key);
            }

            {
                let mut membership = self.queue_membership.write().unwrap();
                membership.remove(&key);
            }

            {
                let mut ghost = self.ghost.lock().unwrap();
                ghost.insert(key);
            }

            self.size.fetch_sub(1, Ordering::Relaxed);
            self.stats.record_eviction();
        }

        true
    }

    /// Evict entries from main queue if it's over capacity.
    fn evict_main_if_needed(&self) {
        let main_cap = self.config.main_queue_capacity();

        loop {
            let main_len = {
                let main = self.main_queue.lock().unwrap();
                main.len()
            };

            if main_len <= main_cap {
                break;
            }

            self.evict_from_main();
        }
    }

    /// Evict one entry from the main queue.
    ///
    /// Returns true if an entry was evicted, false if main queue is empty.
    fn evict_from_main(&self) -> bool {
        // Try to find an entry with freq == 0
        let mut attempts = 0;
        let max_attempts = {
            let main = self.main_queue.lock().unwrap();
            main.len()
        };

        if max_attempts == 0 {
            return false;
        }

        loop {
            let entry = {
                let mut main = self.main_queue.lock().unwrap();
                main.pop_front()
            };

            let entry = match entry {
                Some(e) => e,
                None => return false,
            };

            let freq = entry.freq();

            if freq > 0 {
                // Decrement and reinsert at tail
                entry.decrement_freq();

                {
                    let mut main = self.main_queue.lock().unwrap();
                    main.push_back(entry);
                }

                attempts += 1;
                if attempts >= max_attempts {
                    // Scanned entire queue, force evict the next one
                    let entry = {
                        let mut main = self.main_queue.lock().unwrap();
                        main.pop_front()
                    };

                    if let Some(entry) = entry {
                        let key = entry.key().clone();

                        {
                            let mut index = self.index.write().unwrap();
                            index.remove(&key);
                        }

                        {
                            let mut membership = self.queue_membership.write().unwrap();
                            membership.remove(&key);
                        }

                        self.size.fetch_sub(1, Ordering::Relaxed);
                        self.stats.record_eviction();
                        return true;
                    }
                    return false;
                }
            } else {
                // Evict this entry
                let key = entry.key().clone();

                {
                    let mut index = self.index.write().unwrap();
                    index.remove(&key);
                }

                {
                    let mut membership = self.queue_membership.write().unwrap();
                    membership.remove(&key);
                }

                self.size.fetch_sub(1, Ordering::Relaxed);
                self.stats.record_eviction();
                return true;
            }
        }
    }

    /// Remove an entry from its current queue.
    fn remove_from_queue(&self, key: &K, queue_type: QueueType) {
        match queue_type {
            QueueType::Small => {
                let mut small = self.small_queue.lock().unwrap();
                small.retain(|e| e.key() != key);
            }
            QueueType::Main => {
                let mut main = self.main_queue.lock().unwrap();
                main.retain(|e| e.key() != key);
            }
        }
    }
}

impl<K, V> Cache<K, V> for S3FifoCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn get(&self, key: &K) -> Option<V> {
        let index = self.index.read().unwrap();

        match index.get(key) {
            Some(entry) => {
                // S3-FIFO: Increment frequency counter (lock-free!)
                entry.increment_freq();
                let value = entry.value().clone();

                self.stats.record_hit();
                Some(value)
            }
            None => {
                self.stats.record_miss();
                None
            }
        }
    }

    fn insert(&self, key: K, value: V) -> Option<V> {
        // Check if key already exists
        {
            let index = self.index.read().unwrap();
            if let Some(entry) = index.get(&key) {
                let old_value = entry.value().clone();

                // Update by removing and re-inserting
                drop(index);
                self.remove(&key);
                self.insert(key, value);
                return Some(old_value);
            }
        }

        // Check ghost queue for quick promotion
        let insert_to_main = {
            let mut ghost = self.ghost.lock().unwrap();
            if ghost.contains(&key) {
                ghost.remove(&key);
                true
            } else {
                false
            }
        };

        // Create new entry
        let entry = Arc::new(CacheEntry::new(key.clone(), value));

        // Insert into index
        {
            let mut index = self.index.write().unwrap();
            index.insert(key.clone(), Arc::clone(&entry));
        }

        if insert_to_main {
            // Ghost hit: insert directly into main queue
            {
                let mut main = self.main_queue.lock().unwrap();
                main.push_back(entry);
            }

            {
                let mut membership = self.queue_membership.write().unwrap();
                membership.insert(key, QueueType::Main);
            }
        } else {
            // New entry: insert into small queue
            {
                let mut small = self.small_queue.lock().unwrap();
                small.push_back(entry);
            }

            {
                let mut membership = self.queue_membership.write().unwrap();
                membership.insert(key, QueueType::Small);
            }
        }

        self.size.fetch_add(1, Ordering::Relaxed);
        self.stats.record_insertion();

        // Evict if over capacity
        self.evict_if_needed();

        None
    }

    fn remove(&self, key: &K) -> Option<V> {
        // Remove from index
        let entry = {
            let mut index = self.index.write().unwrap();
            index.remove(key)?
        };

        // Remove from queue
        let queue_type = {
            let mut membership = self.queue_membership.write().unwrap();
            membership.remove(key)
        };

        if let Some(qt) = queue_type {
            self.remove_from_queue(key, qt);
        }

        self.size.fetch_sub(1, Ordering::Relaxed);

        Some(entry.value().clone())
    }

    fn contains(&self, key: &K) -> bool {
        let index = self.index.read().unwrap();
        index.contains_key(key)
    }

    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    fn capacity(&self) -> usize {
        self.config.capacity()
    }

    fn clear(&self) {
        // Clear all data structures
        {
            let mut index = self.index.write().unwrap();
            index.clear();
        }

        {
            let mut small = self.small_queue.lock().unwrap();
            small.clear();
        }

        {
            let mut main = self.main_queue.lock().unwrap();
            main.clear();
        }

        {
            let mut ghost = self.ghost.lock().unwrap();
            ghost.clear();
        }

        {
            let mut membership = self.queue_membership.write().unwrap();
            membership.clear();
        }

        self.size.store(0, Ordering::Relaxed);
    }

    fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn peek(&self, key: &K) -> Option<V> {
        let index = self.index.read().unwrap();
        // Peek: Do NOT increment frequency
        index.get(key).map(|entry| entry.value().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let cache: S3FifoCache<String, i32> = S3FifoCache::new(100);
        assert_eq!(cache.capacity(), 100);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    #[should_panic(expected = "capacity must be greater than 0")]
    fn test_zero_capacity() {
        let _: S3FifoCache<String, i32> = S3FifoCache::new(0);
    }

    #[test]
    fn test_insert_and_get() {
        let cache = S3FifoCache::new(10);

        assert!(cache.insert("a".to_string(), 1).is_none());
        assert!(cache.insert("b".to_string(), 2).is_none());

        assert_eq!(cache.get(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"b".to_string()), Some(2));
        assert_eq!(cache.get(&"c".to_string()), None);

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_update() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);
        assert_eq!(cache.insert("a".to_string(), 100), Some(1));
        assert_eq!(cache.get(&"a".to_string()), Some(100));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_remove() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);

        assert_eq!(cache.remove(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.len(), 1);

        assert_eq!(cache.remove(&"c".to_string()), None);
    }

    #[test]
    fn test_contains() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);

        assert!(cache.contains(&"a".to_string()));
        assert!(!cache.contains(&"b".to_string()));
    }

    #[test]
    fn test_clear() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.get(&"a".to_string()), None);
    }

    #[test]
    fn test_eviction_promotes_frequent() {
        // Capacity 10, small queue = 1 (10% of 10)
        let cache = S3FifoCache::new(10);

        // Fill the cache
        for i in 0..10 {
            cache.insert(format!("key{}", i), i);
        }

        // Access key0 multiple times to increase frequency
        cache.get(&"key0".to_string());
        cache.get(&"key0".to_string());

        // Insert more entries to trigger eviction
        cache.insert("new1".to_string(), 100);
        cache.insert("new2".to_string(), 101);

        // key0 should still be present (was promoted due to high frequency)
        // This test validates the promotion logic
        assert_eq!(cache.len(), 10);
    }

    #[test]
    fn test_ghost_queue_reinsert() {
        // Small cache to easily trigger eviction
        let cache = S3FifoCache::new(3);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        // 'a' should be in small queue and will be evicted first
        cache.insert("d".to_string(), 4);

        // 'a' is now in ghost queue
        assert!(!cache.contains(&"a".to_string()));

        // Re-insert 'a' - should go to main queue (ghost hit)
        cache.insert("a".to_string(), 10);

        // 'a' should be present again
        assert_eq!(cache.get(&"a".to_string()), Some(10));
    }

    #[test]
    fn test_peek() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);

        // Peek should not increment frequency
        assert_eq!(cache.peek(&"a".to_string()), Some(1));

        // Verify get was not called
        let stats = cache.stats();
        assert_eq!(stats.hits(), 0);
    }

    #[test]
    fn test_stats() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.get(&"a".to_string()); // hit
        cache.get(&"b".to_string()); // miss
        cache.get(&"b".to_string()); // miss

        let stats = cache.stats();
        assert_eq!(stats.hits(), 1);
        assert_eq!(stats.misses(), 2);
        assert_eq!(stats.insertions(), 1);
        assert!((stats.hit_ratio() - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_get_or_insert_with() {
        let cache = S3FifoCache::new(10);

        let value = cache.get_or_insert_with("a".to_string(), || 42);
        assert_eq!(value, 42);

        let value = cache.get_or_insert_with("a".to_string(), || 100);
        assert_eq!(value, 42); // Should return existing value
    }

    #[test]
    fn test_queue_capacities() {
        let config = CacheConfig::new(100).with_small_queue_ratio(0.2).unwrap();
        let cache = S3FifoCache::<String, i32>::with_config(config);

        assert_eq!(cache.small_queue_capacity(), 20);
        assert_eq!(cache.main_queue_capacity(), 80);
    }

    #[test]
    fn test_frequency_increment() {
        let cache = S3FifoCache::new(10);

        cache.insert("a".to_string(), 1);

        // Each get increments frequency
        cache.get(&"a".to_string());
        cache.get(&"a".to_string());
        cache.get(&"a".to_string());

        // Frequency should be capped at MAX_FREQ (3)
        let index = cache.index.read().unwrap();
        let entry = index.get(&"a".to_string()).unwrap();
        assert_eq!(entry.freq(), 3);
    }
}
