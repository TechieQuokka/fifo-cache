//! LRU cache implementation for benchmarking baseline.
//!
//! This is a standard Least Recently Used (LRU) cache implementation provided
//! as a baseline for comparing S3-FIFO and SIEVE performance.
//!
//! ## Why Include LRU?
//!
//! The S3-FIFO and SIEVE papers compare against LRU as the primary baseline.
//! Including an LRU implementation allows users to:
//!
//! 1. Benchmark against the same baseline used in the papers
//! 2. Verify the claimed performance improvements
//! 3. Choose the right algorithm for their workload
//!
//! ## Performance Characteristics
//!
//! | Operation | Complexity | Notes |
//! |-----------|------------|-------|
//! | get()     | O(1)       | Requires lock for list manipulation |
//! | insert()  | O(1)       | Requires lock for list manipulation |
//! | evict()   | O(1)       | Always evicts tail |
//!
//! ## Known Limitations
//!
//! - Every cache hit requires moving the entry to the head (lock contention)
//! - Performance degrades significantly under high concurrency (4+ threads)
//! - The S3-FIFO paper shows LRU throughput drops from 8M to 2M ops/sec at 16 threads

use std::collections::HashMap;
use std::hash::Hash;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ahash::RandomState;

use crate::cache::Cache;
use crate::config::CacheConfig;
use crate::stats::CacheStats;

/// Doubly-linked list node for LRU.
struct LruNode<K, V> {
    /// The cache key.
    key: K,

    /// The cached value.
    value: V,

    /// Pointer to previous node (toward MRU).
    prev: Option<NonNull<LruNode<K, V>>>,

    /// Pointer to next node (toward LRU).
    next: Option<NonNull<LruNode<K, V>>>,
}

impl<K, V> LruNode<K, V> {
    fn new(key: K, value: V) -> Self {
        Self {
            key,
            value,
            prev: None,
            next: None,
        }
    }
}

/// Internal state protected by a single mutex.
/// This simplifies the locking model and prevents race conditions.
struct LruState<K, V> {
    /// Fast key lookup: key -> node pointer.
    index: HashMap<K, NonNull<LruNode<K, V>>, RandomState>,

    /// Head of linked list (most recently used).
    head: Option<NonNull<LruNode<K, V>>>,

    /// Tail of linked list (least recently used).
    tail: Option<NonNull<LruNode<K, V>>>,
}

/// Standard LRU cache implementation.
///
/// Provided as a baseline for benchmarking against S3-FIFO and SIEVE.
/// For production use, prefer `SieveCache` or `S3FifoCache`.
///
/// # Example
///
/// ```rust
/// use fifo_cache::{Cache, LruCache};
///
/// let cache = LruCache::new(100);
///
/// cache.insert("key1".to_string(), 42);
/// assert_eq!(cache.get(&"key1".to_string()), Some(42));
/// ```
///
/// # Performance Warning
///
/// LRU has significant lock contention under concurrent workloads.
/// The S3-FIFO paper demonstrates 6× lower throughput compared to
/// FIFO-based algorithms at 16 threads.
pub struct LruCache<K, V> {
    /// All mutable state protected by a single mutex.
    /// This is simpler and safer than fine-grained locking.
    state: Mutex<LruState<K, V>>,

    /// Current number of entries.
    size: AtomicUsize,

    /// Maximum capacity.
    capacity: usize,

    /// Statistics.
    stats: CacheStats,
}

// Safety: LruCache is Send + Sync because all shared state is protected by Mutex
unsafe impl<K: Send, V: Send> Send for LruCache<K, V> {}
unsafe impl<K: Send + Sync, V: Send + Sync> Sync for LruCache<K, V> {}

impl<K, V> LruCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create a new LRU cache with the specified capacity.
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
        assert!(capacity > 0, "{}", crate::config::messages::ZERO_CAPACITY);
        Self::with_config(CacheConfig::new(capacity))
    }

    /// Create a new LRU cache with the given configuration.
    #[must_use]
    pub fn with_config(config: CacheConfig) -> Self {
        let capacity = config.capacity();
        let initial_cap = config.initial_capacity_hint();
        Self {
            state: Mutex::new(LruState {
                index: HashMap::with_capacity_and_hasher(initial_cap, RandomState::new()),
                head: None,
                tail: None,
            }),
            size: AtomicUsize::new(0),
            capacity,
            stats: CacheStats::new(),
        }
    }

    /// Insert a node at the head of the list.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock and ensure `node_ptr` is valid.
    unsafe fn insert_at_head_locked(
        state: &mut LruState<K, V>,
        node_ptr: NonNull<LruNode<K, V>>,
    ) {
        // SAFETY: Caller guarantees node_ptr is valid and we hold the lock
        unsafe {
            let node = node_ptr.as_ptr();

            (*node).next = state.head;
            (*node).prev = None;

            if let Some(old_head) = state.head {
                (*old_head.as_ptr()).prev = Some(node_ptr);
            }

            state.head = Some(node_ptr);

            if state.tail.is_none() {
                state.tail = Some(node_ptr);
            }
        }
    }

    /// Unlink a node from the list.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock and ensure `node_ptr` is valid and in the list.
    unsafe fn unlink_node_locked(state: &mut LruState<K, V>, node_ptr: NonNull<LruNode<K, V>>) {
        // SAFETY: Caller guarantees node_ptr is valid, in the list, and we hold the lock
        unsafe {
            let node = node_ptr.as_ptr();
            let prev = (*node).prev;
            let next = (*node).next;

            match prev {
                Some(prev_ptr) => {
                    (*prev_ptr.as_ptr()).next = next;
                }
                None => {
                    state.head = next;
                }
            }

            match next {
                Some(next_ptr) => {
                    (*next_ptr.as_ptr()).prev = prev;
                }
                None => {
                    state.tail = prev;
                }
            }

            (*node).prev = None;
            (*node).next = None;
        }
    }

    /// Move a node to the head of the list.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock and ensure `node_ptr` is valid and in the list.
    unsafe fn move_to_head_locked(state: &mut LruState<K, V>, node_ptr: NonNull<LruNode<K, V>>) {
        // SAFETY: Caller guarantees node_ptr is valid, in the list, and we hold the lock
        unsafe {
            let node = node_ptr.as_ptr();

            // If already at head, nothing to do
            if (*node).prev.is_none() {
                return;
            }

            // Unlink from current position
            Self::unlink_node_locked(state, node_ptr);

            // Insert at head
            Self::insert_at_head_locked(state, node_ptr);
        }
    }

    /// Evict the least recently used entry (tail).
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock.
    unsafe fn evict_tail_locked(&self, state: &mut LruState<K, V>) {
        if let Some(tail_ptr) = state.tail {
            // SAFETY: tail_ptr is valid since it's in state.tail, and we hold the lock
            unsafe {
                let node = tail_ptr.as_ptr();
                let key = (*node).key.clone();

                // Remove from index
                state.index.remove(&key);

                // Unlink from list
                Self::unlink_node_locked(state, tail_ptr);

                // Deallocate
                let _ = Box::from_raw(tail_ptr.as_ptr());
            }

            self.size.fetch_sub(1, Ordering::Relaxed);
            self.stats.record_eviction();
        }
    }
}

impl<K, V> Cache<K, V> for LruCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn get(&self, key: &K) -> Option<V> {
        let mut state = self.state.lock().unwrap();

        match state.index.get(key).copied() {
            Some(node_ptr) => {
                // SAFETY: We hold the lock, so the node is valid
                let value = unsafe {
                    let node = node_ptr.as_ptr();
                    let value = (*node).value.clone();

                    // Move to head
                    Self::move_to_head_locked(&mut state, node_ptr);

                    value
                };

                drop(state);
                self.stats.record_hit();
                Some(value)
            }
            None => {
                drop(state);
                self.stats.record_miss();
                None
            }
        }
    }

    fn insert(&self, key: K, value: V) -> Option<V> {
        let mut state = self.state.lock().unwrap();

        // Check if key already exists
        if let Some(&node_ptr) = state.index.get(&key) {
            // SAFETY: We hold the lock
            let old_value = unsafe {
                let node = node_ptr.as_ptr();
                let old_value = (*node).value.clone();

                // Remove old entry
                state.index.remove(&key);
                Self::unlink_node_locked(&mut state, node_ptr);
                let _ = Box::from_raw(node_ptr.as_ptr());

                old_value
            };

            self.size.fetch_sub(1, Ordering::Relaxed);

            // Create new entry
            let new_node = Box::new(LruNode::new(key.clone(), value));
            let new_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(new_node)) };

            state.index.insert(key, new_ptr);
            unsafe { Self::insert_at_head_locked(&mut state, new_ptr) };
            self.size.fetch_add(1, Ordering::Relaxed);
            self.stats.record_insertion();

            return Some(old_value);
        }

        // Create new node
        let node = Box::new(LruNode::new(key.clone(), value));
        let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        // Insert into index and list
        state.index.insert(key, node_ptr);
        unsafe { Self::insert_at_head_locked(&mut state, node_ptr) };
        self.size.fetch_add(1, Ordering::Relaxed);
        self.stats.record_insertion();

        // Evict if over capacity
        while self.size.load(Ordering::Relaxed) > self.capacity {
            unsafe { self.evict_tail_locked(&mut state) };
        }

        None
    }

    fn remove(&self, key: &K) -> Option<V> {
        let mut state = self.state.lock().unwrap();

        let node_ptr = state.index.remove(key)?;

        // SAFETY: We hold the lock and just removed from index
        let value = unsafe {
            let node = node_ptr.as_ptr();
            let value = (*node).value.clone();

            Self::unlink_node_locked(&mut state, node_ptr);
            let _ = Box::from_raw(node_ptr.as_ptr());

            value
        };

        self.size.fetch_sub(1, Ordering::Relaxed);
        Some(value)
    }

    fn contains(&self, key: &K) -> bool {
        let state = self.state.lock().unwrap();
        state.index.contains_key(key)
    }

    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn clear(&self) {
        let mut state = self.state.lock().unwrap();

        // Deallocate all nodes
        for (_, node_ptr) in state.index.drain() {
            unsafe {
                let _ = Box::from_raw(node_ptr.as_ptr());
            }
        }

        state.head = None;
        state.tail = None;
        self.size.store(0, Ordering::Relaxed);
    }

    fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn peek(&self, key: &K) -> Option<V> {
        let state = self.state.lock().unwrap();

        match state.index.get(key) {
            Some(&node_ptr) => {
                // Peek: Do NOT move to head
                let value = unsafe { (*node_ptr.as_ptr()).value.clone() };
                Some(value)
            }
            None => None,
        }
    }
}

impl<K, V> Drop for LruCache<K, V> {
    fn drop(&mut self) {
        // We have exclusive access in drop, so no need for locks
        if let Ok(state) = self.state.get_mut() {
            for (_, node_ptr) in state.index.drain() {
                unsafe {
                    let _ = Box::from_raw(node_ptr.as_ptr());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let cache: LruCache<String, i32> = LruCache::new(100);
        assert_eq!(cache.capacity(), 100);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    #[should_panic(expected = "capacity must be greater than 0")]
    fn test_zero_capacity() {
        let _: LruCache<String, i32> = LruCache::new(0);
    }

    #[test]
    fn test_insert_and_get() {
        let cache = LruCache::new(10);

        assert!(cache.insert("a".to_string(), 1).is_none());
        assert!(cache.insert("b".to_string(), 2).is_none());

        assert_eq!(cache.get(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"b".to_string()), Some(2));
        assert_eq!(cache.get(&"c".to_string()), None);

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_update() {
        let cache = LruCache::new(10);

        cache.insert("a".to_string(), 1);
        assert_eq!(cache.insert("a".to_string(), 100), Some(1));
        assert_eq!(cache.get(&"a".to_string()), Some(100));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_remove() {
        let cache = LruCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);

        assert_eq!(cache.remove(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.len(), 1);

        assert_eq!(cache.remove(&"c".to_string()), None);
    }

    #[test]
    fn test_lru_eviction() {
        let cache = LruCache::new(3);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        // Access 'a' to make it recently used
        cache.get(&"a".to_string());

        // Insert 'd' - should evict 'b' (least recently used)
        cache.insert("d".to_string(), 4);

        assert!(cache.contains(&"a".to_string())); // Recently accessed
        assert!(!cache.contains(&"b".to_string())); // Evicted (LRU)
        assert!(cache.contains(&"c".to_string()));
        assert!(cache.contains(&"d".to_string()));
    }

    #[test]
    fn test_clear() {
        let cache = LruCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.get(&"a".to_string()), None);
    }

    #[test]
    fn test_peek() {
        let cache = LruCache::new(3);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        // Peek 'a' - should NOT move it to head
        assert_eq!(cache.peek(&"a".to_string()), Some(1));

        // Insert 'd' - should evict 'a' since peek didn't update recency
        cache.insert("d".to_string(), 4);

        assert!(!cache.contains(&"a".to_string())); // Evicted
    }

    #[test]
    fn test_stats() {
        use crate::config::defaults::FLOAT_TOLERANCE;

        let cache = LruCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.get(&"a".to_string()); // hit
        cache.get(&"b".to_string()); // miss
        cache.get(&"b".to_string()); // miss

        let stats = cache.stats();
        assert_eq!(stats.hits(), 1);
        assert_eq!(stats.misses(), 2);
        assert_eq!(stats.insertions(), 1);

        let expected_ratio = 1.0 / 3.0;
        assert!((stats.hit_ratio() - expected_ratio).abs() < FLOAT_TOLERANCE);
    }
}
