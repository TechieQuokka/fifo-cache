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
use std::sync::{Mutex, RwLock};

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
    /// Fast key lookup: key -> node pointer.
    index: RwLock<HashMap<K, NonNull<SieveNode<K, V>>, RandomState>>,

    /// Head of linked list (newest entries).
    head: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// Tail of linked list (oldest entries).
    tail: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// Hand pointer for eviction (sweeps tail -> head).
    hand: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// Current number of entries.
    size: AtomicUsize,

    /// Configuration.
    config: CacheConfig,

    /// Statistics.
    stats: CacheStats,
}

// Safety: SieveCache is Send + Sync because:
// 1. All shared state is protected by RwLock/Mutex
// 2. NonNull pointers are only dereferenced while holding appropriate locks
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
        assert!(capacity > 0, "capacity must be greater than 0");
        Self::with_config(CacheConfig::new(capacity))
    }

    /// Create a new SIEVE cache with the given configuration.
    #[must_use]
    pub fn with_config(config: CacheConfig) -> Self {
        let initial_cap = config.initial_capacity_hint();
        Self {
            index: RwLock::new(HashMap::with_capacity_and_hasher(
                initial_cap,
                RandomState::new(),
            )),
            head: Mutex::new(None),
            tail: Mutex::new(None),
            hand: Mutex::new(None),
            size: AtomicUsize::new(0),
            config,
            stats: CacheStats::new(),
        }
    }

    /// Insert a node at the head of the list.
    ///
    /// # Safety
    ///
    /// Caller must ensure `node_ptr` is valid and not already in the list.
    fn insert_at_head(&self, node_ptr: NonNull<SieveNode<K, V>>) {
        let mut head_guard = self.head.lock().unwrap();
        let mut tail_guard = self.tail.lock().unwrap();

        // SAFETY: We have exclusive access via locks, and node_ptr is valid
        unsafe {
            let node = node_ptr.as_ptr();

            // Link to current head
            (*node).next = *head_guard;
            (*node).prev = None;

            // Update old head's prev pointer
            if let Some(old_head) = *head_guard {
                (*old_head.as_ptr()).prev = Some(node_ptr);
            }

            // Update head
            *head_guard = Some(node_ptr);

            // If list was empty, also set tail
            if tail_guard.is_none() {
                *tail_guard = Some(node_ptr);
            }
        }
    }

    /// Remove a node from the list.
    ///
    /// # Safety
    ///
    /// Caller must ensure `node_ptr` is valid and currently in the list.
    fn unlink_node(&self, node_ptr: NonNull<SieveNode<K, V>>) {
        let mut head_guard = self.head.lock().unwrap();
        let mut tail_guard = self.tail.lock().unwrap();
        let mut hand_guard = self.hand.lock().unwrap();

        // SAFETY: We have exclusive access via locks, and node_ptr is valid
        unsafe {
            let node = node_ptr.as_ptr();
            let prev = (*node).prev;
            let next = (*node).next;

            // If hand points to this node, move it
            if *hand_guard == Some(node_ptr) {
                // Move hand to prev, or to tail if at head
                *hand_guard = prev.or(*tail_guard);
                // If we're removing the only entry, hand becomes None
                if *hand_guard == Some(node_ptr) {
                    *hand_guard = None;
                }
            }

            // Update prev node's next pointer
            match prev {
                Some(prev_ptr) => {
                    (*prev_ptr.as_ptr()).next = next;
                }
                None => {
                    // This was the head
                    *head_guard = next;
                }
            }

            // Update next node's prev pointer
            match next {
                Some(next_ptr) => {
                    (*next_ptr.as_ptr()).prev = prev;
                }
                None => {
                    // This was the tail
                    *tail_guard = prev;
                }
            }
        }
    }

    /// Evict one entry using the SIEVE algorithm.
    ///
    /// The hand sweeps from tail toward head, resetting visited flags
    /// until it finds an unvisited entry to evict.
    fn evict_one(&self) {
        // Get current hand position, initializing to tail if needed
        let evict_ptr = {
            let mut hand_guard = self.hand.lock().unwrap();

            // Initialize hand to tail if not set
            if hand_guard.is_none() {
                let tail_guard = self.tail.lock().unwrap();
                *hand_guard = *tail_guard;
            }

            // Find an unvisited entry
            loop {
                let current_ptr = match *hand_guard {
                    Some(ptr) => ptr,
                    None => return, // Empty cache
                };

                // SAFETY: We hold the hand lock and current_ptr is valid
                let is_visited = unsafe { (*current_ptr.as_ptr()).entry.is_visited() };

                if is_visited {
                    // Clear visited flag and move hand toward head
                    unsafe {
                        (*current_ptr.as_ptr()).entry.clear_visited();
                        let prev = (*current_ptr.as_ptr()).prev;
                        *hand_guard = prev;

                        // Wrap around to tail if we reach head
                        if hand_guard.is_none() {
                            let tail_guard = self.tail.lock().unwrap();
                            *hand_guard = *tail_guard;
                        }
                    }
                } else {
                    // Found an unvisited entry - evict it
                    // Move hand first, then return the pointer to evict
                    unsafe {
                        let prev = (*current_ptr.as_ptr()).prev;
                        *hand_guard = prev;

                        if hand_guard.is_none() {
                            let tail_guard = self.tail.lock().unwrap();
                            *hand_guard = *tail_guard;
                            // Don't point hand at the node we're about to remove
                            if *hand_guard == Some(current_ptr) {
                                *hand_guard = None;
                            }
                        }
                    }
                    break current_ptr;
                }
            }
        };

        // Get the key before removing from index
        let key = unsafe { (*evict_ptr.as_ptr()).entry.key().clone() };

        // Remove from index
        {
            let mut index_guard = self.index.write().unwrap();
            index_guard.remove(&key);
        }

        // Remove from list
        self.unlink_node(evict_ptr);

        // Deallocate the node
        // SAFETY: We've removed all references to this node
        unsafe {
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
        let index_guard = self.index.read().unwrap();

        match index_guard.get(key) {
            Some(&node_ptr) => {
                // SIEVE: Just set visited flag - NO list modification!
                // SAFETY: node_ptr is valid while we hold the index read lock
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
        // Check if key already exists
        {
            let index_guard = self.index.read().unwrap();
            if let Some(&node_ptr) = index_guard.get(&key) {
                // Update existing entry
                // SAFETY: node_ptr is valid while we hold the index read lock
                let old_value = unsafe { (*node_ptr.as_ptr()).entry.value().clone() };

                // For simplicity, we remove and re-insert
                // A more optimized version could update in place
                drop(index_guard);
                self.remove(&key);
                self.insert(key, value);
                return Some(old_value);
            }
        }

        // Create new node
        let node = Box::new(SieveNode::new(key.clone(), value));
        // SAFETY: Box::into_raw returns a valid pointer
        let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        // Insert into index
        {
            let mut index_guard = self.index.write().unwrap();
            index_guard.insert(key, node_ptr);
        }

        // Insert at head of list
        self.insert_at_head(node_ptr);
        self.size.fetch_add(1, Ordering::Relaxed);
        self.stats.record_insertion();

        // Evict if over capacity
        while self.size.load(Ordering::Relaxed) > self.config.capacity() {
            self.evict_one();
        }

        None
    }

    fn remove(&self, key: &K) -> Option<V> {
        // Remove from index
        let node_ptr = {
            let mut index_guard = self.index.write().unwrap();
            index_guard.remove(key)?
        };

        // Get value before deallocating
        // SAFETY: node_ptr is valid, we just removed it from the index
        let value = unsafe { (*node_ptr.as_ptr()).entry.value().clone() };

        // Remove from list
        self.unlink_node(node_ptr);

        // Deallocate
        // SAFETY: We've removed all references to this node
        unsafe {
            let _ = Box::from_raw(node_ptr.as_ptr());
        }

        self.size.fetch_sub(1, Ordering::Relaxed);
        Some(value)
    }

    fn contains(&self, key: &K) -> bool {
        let index_guard = self.index.read().unwrap();
        index_guard.contains_key(key)
    }

    fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    fn capacity(&self) -> usize {
        self.config.capacity()
    }

    fn clear(&self) {
        // Remove all nodes
        let mut index_guard = self.index.write().unwrap();
        let mut head_guard = self.head.lock().unwrap();
        let mut tail_guard = self.tail.lock().unwrap();
        let mut hand_guard = self.hand.lock().unwrap();

        // Deallocate all nodes
        for (_, node_ptr) in index_guard.drain() {
            // SAFETY: We have exclusive access to all data structures
            unsafe {
                let _ = Box::from_raw(node_ptr.as_ptr());
            }
        }

        *head_guard = None;
        *tail_guard = None;
        *hand_guard = None;
        self.size.store(0, Ordering::Relaxed);
    }

    fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn peek(&self, key: &K) -> Option<V> {
        let index_guard = self.index.read().unwrap();

        match index_guard.get(key) {
            Some(&node_ptr) => {
                // Peek: Do NOT update visited flag
                // SAFETY: node_ptr is valid while we hold the index read lock
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
        if let Ok(index) = self.index.get_mut() {
            for (_, node_ptr) in index.drain() {
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
        let cache = SieveCache::new(10);

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
        let cache = SieveCache::new(10);

        let value = cache.get_or_insert_with("a".to_string(), || 42);
        assert_eq!(value, 42);

        let value = cache.get_or_insert_with("a".to_string(), || 100);
        assert_eq!(value, 42); // Should return existing value
    }
}
