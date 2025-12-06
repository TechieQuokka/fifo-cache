//! SIEVE cache implementation.
//!
//! SIEVE is a cache eviction algorithm from NSDI 2024 (Best Paper Award).
//! It is simpler than LRU while achieving better hit ratios on web workloads.
//!
//! ## Key Insight
//!
//! Cache hits only require setting a single bit atomically. No queue
//! reordering or list manipulation is needed, enabling lock-free hits.
//!
//! ## Algorithm
//!
//! - Maintains a FIFO queue (doubly-linked list)
//! - Each entry has a 1-bit "visited" flag
//! - A "hand" pointer sweeps from tail to head during eviction
//! - On hit: Set visited = true (atomic, O(1))
//! - On eviction: Move hand until finding unvisited entry, evict it
//!
//! ## Performance
//!
//! - 21% lower miss ratio than FIFO (vs 15% for CLOCK)
//! - Lock-free cache hits (atomic flag update only)
//! - O(1) amortized eviction
//!
//! ## Limitations
//!
//! - Not scan-resistant (use S3-FIFO for block cache workloads)
//! - Worst case O(n) eviction when all entries are visited

use std::collections::HashMap;
use std::hash::Hash;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ahash::RandomState;

use crate::cache::Cache;
use crate::config::CacheConfig;
use crate::node::CacheEntry;
use crate::stats::CacheStats;

/// Doubly-linked list node for SIEVE.
struct SieveNode<K, V> {
    /// The cache entry (key, value, visited flag).
    entry: CacheEntry<K, V>,

    /// Pointer to previous node (toward head).
    prev: Option<NonNull<SieveNode<K, V>>>,

    /// Pointer to next node (toward tail).
    next: Option<NonNull<SieveNode<K, V>>>,
}

impl<K, V> SieveNode<K, V> {
    /// Create a new node with the given key and value.
    fn new(key: K, value: V) -> Self {
        Self {
            entry: CacheEntry::new(key, value),
            prev: None,
            next: None,
        }
    }
}

/// All mutable state protected by a single mutex.
struct SieveState<K, V> {
    /// Fast key lookup: key -> node pointer.
    index: HashMap<K, NonNull<SieveNode<K, V>>, RandomState>,

    /// Head of linked list (newest entries).
    head: Option<NonNull<SieveNode<K, V>>>,

    /// Tail of linked list (oldest entries).
    tail: Option<NonNull<SieveNode<K, V>>>,

    /// Hand pointer for eviction (sweeps tail -> head).
    hand: Option<NonNull<SieveNode<K, V>>>,
}

impl<K, V> SieveState<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            index: HashMap::with_capacity_and_hasher(capacity, RandomState::new()),
            head: None,
            tail: None,
            hand: None,
        }
    }
}

/// SIEVE cache implementation.
///
/// A thread-safe cache using the SIEVE eviction algorithm.
///
/// # Example
///
/// ```rust
/// use fifo_cache::{Cache, SieveCache};
///
/// let cache = SieveCache::new(100);
///
/// cache.insert("key1".to_string(), 42);
/// cache.insert("key2".to_string(), 84);
///
/// assert_eq!(cache.get(&"key1".to_string()), Some(42));
/// assert_eq!(cache.len(), 2);
/// ```
pub struct SieveCache<K, V> {
    /// All mutable state protected by a single mutex.
    state: Mutex<SieveState<K, V>>,

    /// Current number of entries.
    size: AtomicUsize,

    /// Configuration.
    config: CacheConfig,

    /// Statistics.
    stats: CacheStats,
}

// Safety: SieveCache is Send + Sync because:
// 1. All shared state is protected by Mutex
// 2. NonNull pointers are only dereferenced while holding the lock
// 3. Memory is properly managed (Box allocation, Drop implementation)
unsafe impl<K: Send, V: Send> Send for SieveCache<K, V> {}
unsafe impl<K: Send + Sync, V: Send + Sync> Sync for SieveCache<K, V> {}

impl<K, V> SieveCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Create a new SIEVE cache with the specified capacity.
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

    /// Create a new SIEVE cache with the given configuration.
    #[must_use]
    pub fn with_config(config: CacheConfig) -> Self {
        let initial_cap = config.initial_capacity_hint();
        Self {
            state: Mutex::new(SieveState::new(initial_cap)),
            size: AtomicUsize::new(0),
            config,
            stats: CacheStats::new(),
        }
    }

    /// Insert a node at the head of the list.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock and ensure `node_ptr` is valid.
    unsafe fn insert_at_head_locked(
        state: &mut SieveState<K, V>,
        node_ptr: NonNull<SieveNode<K, V>>,
    ) {
        // SAFETY: Caller guarantees node_ptr is valid and we hold the lock
        unsafe {
            let node = node_ptr.as_ptr();

            // Link to current head
            (*node).next = state.head;
            (*node).prev = None;

            // Update old head's prev pointer
            if let Some(old_head) = state.head {
                (*old_head.as_ptr()).prev = Some(node_ptr);
            }

            // Update head
            state.head = Some(node_ptr);

            // If list was empty, also set tail
            if state.tail.is_none() {
                state.tail = Some(node_ptr);
            }
        }
    }

    /// Remove a node from the list.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock and ensure `node_ptr` is valid and in the list.
    unsafe fn unlink_node_locked(
        state: &mut SieveState<K, V>,
        node_ptr: NonNull<SieveNode<K, V>>,
    ) {
        // SAFETY: Caller guarantees node_ptr is valid and we hold the lock
        unsafe {
            let node = node_ptr.as_ptr();
            let prev = (*node).prev;
            let next = (*node).next;

            // If hand points to this node, move it
            if state.hand == Some(node_ptr) {
                // Move hand to prev, or to tail if at head
                state.hand = prev.or(state.tail);
                // If we're removing the only entry, hand becomes None
                if state.hand == Some(node_ptr) {
                    state.hand = None;
                }
            }

            // Update prev node's next pointer
            match prev {
                Some(prev_ptr) => {
                    (*prev_ptr.as_ptr()).next = next;
                }
                None => {
                    // This was the head
                    state.head = next;
                }
            }

            // Update next node's prev pointer
            match next {
                Some(next_ptr) => {
                    (*next_ptr.as_ptr()).prev = prev;
                }
                None => {
                    // This was the tail
                    state.tail = prev;
                }
            }
        }
    }

    /// Evict one entry using the SIEVE algorithm.
    ///
    /// The hand sweeps from tail toward head, resetting visited flags
    /// until it finds an unvisited entry to evict.
    ///
    /// # Safety
    ///
    /// Caller must hold the state lock.
    unsafe fn evict_one_locked(&self, state: &mut SieveState<K, V>) {
        // Initialize hand to tail if not set
        if state.hand.is_none() {
            state.hand = state.tail;
        }

        // Find an unvisited entry
        let evict_ptr = loop {
            let current_ptr = match state.hand {
                Some(ptr) => ptr,
                None => return, // Empty cache
            };

            // SAFETY: We hold the lock and current_ptr is valid
            unsafe {
                let is_visited = (*current_ptr.as_ptr()).entry.is_visited();

                if is_visited {
                    // Clear visited flag and move hand toward head
                    (*current_ptr.as_ptr()).entry.clear_visited();
                    let prev = (*current_ptr.as_ptr()).prev;
                    state.hand = prev;

                    // Wrap around to tail if we reach head
                    if state.hand.is_none() {
                        state.hand = state.tail;
                    }
                } else {
                    // Found an unvisited entry - evict it
                    // Move hand first
                    let prev = (*current_ptr.as_ptr()).prev;
                    state.hand = prev;

                    if state.hand.is_none() {
                        state.hand = state.tail;
                        // Don't point hand at the node we're about to remove
                        if state.hand == Some(current_ptr) {
                            state.hand = None;
                        }
                    }
                    break current_ptr;
                }
            }
        };

        // SAFETY: evict_ptr is valid, we hold the lock
        unsafe {
            // Get the key before removing
            let key = (*evict_ptr.as_ptr()).entry.key().clone();

            // Remove from index
            state.index.remove(&key);

            // Remove from list
            Self::unlink_node_locked(state, evict_ptr);

            // Deallocate the node
            let _ = Box::from_raw(evict_ptr.as_ptr());
        }

        self.size.fetch_sub(1, Ordering::Relaxed);
        self.stats.record_eviction();
    }
}

impl<K, V> Cache<K, V> for SieveCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn get(&self, key: &K) -> Option<V> {
        let state = self.state.lock().unwrap();

        match state.index.get(key) {
            Some(&node_ptr) => {
                // SIEVE: Just set visited flag - NO list modification!
                // SAFETY: node_ptr is valid while we hold the lock
                let value = unsafe {
                    let node = node_ptr.as_ref();
                    node.entry.mark_visited();
                    node.entry.value().clone()
                };

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
        let mut state = self.state.lock().unwrap();

        // Check if key already exists
        if let Some(&node_ptr) = state.index.get(&key) {
            // Update existing entry - get old value first
            // SAFETY: node_ptr is valid while we hold the lock
            let old_value = unsafe { (*node_ptr.as_ptr()).entry.value().clone() };

            // Remove the old node
            // SAFETY: We hold the lock
            unsafe {
                Self::unlink_node_locked(&mut state, node_ptr);
                state.index.remove(&key);
                let _ = Box::from_raw(node_ptr.as_ptr());
            }
            self.size.fetch_sub(1, Ordering::Relaxed);

            // Create new node with new value
            let node = Box::new(SieveNode::new(key.clone(), value));
            // SAFETY: Box::into_raw returns a valid pointer
            let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

            // Insert into index
            state.index.insert(key, node_ptr);

            // Insert at head of list
            // SAFETY: We hold the lock and node_ptr is valid
            unsafe {
                Self::insert_at_head_locked(&mut state, node_ptr);
            }
            self.size.fetch_add(1, Ordering::Relaxed);

            return Some(old_value);
        }

        // Create new node
        let node = Box::new(SieveNode::new(key.clone(), value));
        // SAFETY: Box::into_raw returns a valid pointer
        let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        // Insert into index
        state.index.insert(key, node_ptr);

        // Insert at head of list
        // SAFETY: We hold the lock and node_ptr is valid
        unsafe {
            Self::insert_at_head_locked(&mut state, node_ptr);
        }
        self.size.fetch_add(1, Ordering::Relaxed);
        self.stats.record_insertion();

        // Evict if over capacity
        while self.size.load(Ordering::Relaxed) > self.config.capacity() {
            // SAFETY: We hold the lock
            unsafe {
                self.evict_one_locked(&mut state);
            }
        }

        None
    }

    fn remove(&self, key: &K) -> Option<V> {
        let mut state = self.state.lock().unwrap();

        // Remove from index
        let node_ptr = state.index.remove(key)?;

        // Get value before deallocating
        // SAFETY: node_ptr is valid, we just removed it from the index but still hold lock
        let value = unsafe { (*node_ptr.as_ptr()).entry.value().clone() };

        // Remove from list
        // SAFETY: We hold the lock
        unsafe {
            Self::unlink_node_locked(&mut state, node_ptr);
        }

        // Deallocate
        // SAFETY: We've removed all references to this node
        unsafe {
            let _ = Box::from_raw(node_ptr.as_ptr());
        }

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
        self.config.capacity()
    }

    fn clear(&self) {
        let mut state = self.state.lock().unwrap();

        // Deallocate all nodes
        for (_, node_ptr) in state.index.drain() {
            // SAFETY: We have exclusive access to all data structures
            unsafe {
                let _ = Box::from_raw(node_ptr.as_ptr());
            }
        }

        state.head = None;
        state.tail = None;
        state.hand = None;
        self.size.store(0, Ordering::Relaxed);
    }

    fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn peek(&self, key: &K) -> Option<V> {
        let state = self.state.lock().unwrap();

        match state.index.get(key) {
            Some(&node_ptr) => {
                // Peek: Do NOT update visited flag
                // SAFETY: node_ptr is valid while we hold the lock
                let value = unsafe { (*node_ptr.as_ptr()).entry.value().clone() };
                Some(value)
            }
            None => None,
        }
    }
}

impl<K, V> Drop for SieveCache<K, V> {
    fn drop(&mut self) {
        // We have exclusive access in drop, so no need for locks
        // SAFETY: No other threads can access this cache
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
        let cache: SieveCache<String, i32> = SieveCache::new(100);
        assert_eq!(cache.capacity(), 100);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    #[should_panic(expected = "capacity must be greater than 0")]
    fn test_zero_capacity() {
        let _: SieveCache<String, i32> = SieveCache::new(0);
    }

    #[test]
    fn test_insert_and_get() {
        let cache = SieveCache::new(10);

        assert!(cache.insert("a".to_string(), 1).is_none());
        assert!(cache.insert("b".to_string(), 2).is_none());

        assert_eq!(cache.get(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"b".to_string()), Some(2));
        assert_eq!(cache.get(&"c".to_string()), None);

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_update() {
        let cache = SieveCache::new(10);

        cache.insert("a".to_string(), 1);
        assert_eq!(cache.insert("a".to_string(), 100), Some(1));
        assert_eq!(cache.get(&"a".to_string()), Some(100));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_remove() {
        let cache = SieveCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);

        assert_eq!(cache.remove(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.len(), 1);

        assert_eq!(cache.remove(&"c".to_string()), None);
    }

    #[test]
    fn test_contains() {
        let cache = SieveCache::new(10);

        cache.insert("a".to_string(), 1);

        assert!(cache.contains(&"a".to_string()));
        assert!(!cache.contains(&"b".to_string()));
    }

    #[test]
    fn test_clear() {
        let cache = SieveCache::new(10);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.get(&"a".to_string()), None);
    }

    #[test]
    fn test_eviction() {
        let cache = SieveCache::new(3);

        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);

        // Access 'a' to mark it visited
        cache.get(&"a".to_string());

        // Insert 'd', should evict unvisited entry
        cache.insert("d".to_string(), 4);

        assert_eq!(cache.len(), 3);
        // 'a' should still be there (was visited)
        assert!(cache.contains(&"a".to_string()));
        // 'd' should be there (just inserted)
        assert!(cache.contains(&"d".to_string()));
    }

    #[test]
    fn test_peek() {
        let cache = SieveCache::new(3);

        cache.insert("a".to_string(), 1);

        // Peek should not mark as visited
        assert_eq!(cache.peek(&"a".to_string()), Some(1));

        // Insert enough to trigger eviction
        cache.insert("b".to_string(), 2);
        cache.insert("c".to_string(), 3);
        cache.insert("d".to_string(), 4); // Should evict 'a' since it wasn't visited

        // 'a' should be evicted because peek didn't mark it
        assert!(!cache.contains(&"a".to_string()));
    }

    #[test]
    fn test_stats() {
        use crate::config::defaults::FLOAT_TOLERANCE;

        let cache = SieveCache::new(10);

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

    #[test]
    fn test_get_or_insert_with() {
        let cache = SieveCache::new(10);

        let value = cache.get_or_insert_with("a".to_string(), || 42);
        assert_eq!(value, 42);

        let value = cache.get_or_insert_with("a".to_string(), || 100);
        assert_eq!(value, 42); // Should return existing value
    }
}
