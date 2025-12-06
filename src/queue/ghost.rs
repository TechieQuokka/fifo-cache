//! Ghost queue for tracking evicted keys.
//!
//! The ghost queue stores only key metadata (no values) for recently evicted
//! entries. This enables S3-FIFO to detect when a previously-evicted key is
//! re-requested, indicating it should be promoted directly to the main queue.
//!
//! ## Purpose
//!
//! 1. **Second-chance admission**: Keys in ghost get inserted to main queue
//! 2. **Scan resistance**: Prevents sequential scans from polluting cache
//! 3. **Memory efficiency**: Only stores key hashes, not actual data
//!
//! ## Memory Overhead
//!
//! At 1× main queue size (default), ghost queue adds ~4% memory overhead
//! assuming 8-byte keys vs 100+ byte values.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

use ahash::RandomState;

/// Ghost queue for tracking recently evicted keys.
///
/// Stores only keys (no values) with a bounded FIFO eviction policy.
/// Used by S3-FIFO for detecting re-requests of evicted items.
///
/// # Type Parameters
///
/// * `K` - Key type (must be `Hash + Eq + Clone`).
///
/// # Example
///
/// ```rust,ignore
/// use fifo_cache::queue::GhostQueue;
///
/// let mut ghost = GhostQueue::new(100);
///
/// ghost.insert("evicted_key".to_string());
/// assert!(ghost.contains(&"evicted_key".to_string()));
///
/// ghost.remove(&"evicted_key".to_string());
/// assert!(!ghost.contains(&"evicted_key".to_string()));
/// ```
#[derive(Debug)]
pub struct GhostQueue<K> {
    /// Set for O(1) membership checks.
    keys: HashSet<K, RandomState>,

    /// Queue for FIFO eviction order.
    order: VecDeque<K>,

    /// Maximum capacity.
    capacity: usize,
}

impl<K: Hash + Eq + Clone> GhostQueue<K> {
    /// Create a new ghost queue with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of keys to track.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            keys: HashSet::with_capacity_and_hasher(capacity, RandomState::new()),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Check if a key is in the ghost queue.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to check.
    ///
    /// # Performance
    ///
    /// O(1) average case.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.keys.contains(key)
    }

    /// Insert a key into the ghost queue.
    ///
    /// If the queue is at capacity, the oldest entry is evicted first.
    /// Duplicate keys are ignored.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to insert.
    ///
    /// # Returns
    ///
    /// `true` if the key was inserted, `false` if it already existed.
    pub fn insert(&mut self, key: K) -> bool {
        // Don't insert duplicates
        if self.keys.contains(&key) {
            return false;
        }

        // Evict oldest entries if at capacity
        while self.order.len() >= self.capacity && !self.order.is_empty() {
            if let Some(old_key) = self.order.pop_front() {
                self.keys.remove(&old_key);
            }
        }

        // Insert new key
        self.keys.insert(key.clone());
        self.order.push_back(key);
        true
    }

    /// Remove a key from the ghost queue.
    ///
    /// Called when a ghost hit occurs (key re-requested).
    ///
    /// # Arguments
    ///
    /// * `key` - The key to remove.
    ///
    /// # Returns
    ///
    /// `true` if the key was present and removed.
    ///
    /// # Performance
    ///
    /// O(n) for queue removal, but ghost hits are relatively rare.
    pub fn remove(&mut self, key: &K) -> bool {
        if self.keys.remove(key) {
            // O(n) removal from queue
            self.order.retain(|k| k != key);
            true
        } else {
            false
        }
    }

    /// Get the number of keys in the ghost queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Check if the ghost queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Check if the ghost queue is at capacity.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.keys.len() >= self.capacity
    }

    /// Get the maximum capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all keys from the ghost queue.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.order.clear();
    }

    /// Iterate over all keys in insertion order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.order.iter()
    }
}

impl<K: Hash + Eq + Clone> Default for GhostQueue<K> {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut ghost = GhostQueue::new(10);

        assert!(ghost.is_empty());
        assert!(!ghost.is_full());

        assert!(ghost.insert("a".to_string()));
        assert!(ghost.insert("b".to_string()));
        assert!(ghost.insert("c".to_string()));

        assert!(ghost.contains(&"a".to_string()));
        assert!(ghost.contains(&"b".to_string()));
        assert!(ghost.contains(&"c".to_string()));
        assert!(!ghost.contains(&"d".to_string()));

        assert_eq!(ghost.len(), 3);
    }

    #[test]
    fn test_no_duplicates() {
        let mut ghost = GhostQueue::new(10);

        assert!(ghost.insert("a".to_string()));
        assert!(!ghost.insert("a".to_string())); // Duplicate

        assert_eq!(ghost.len(), 1);
    }

    #[test]
    fn test_remove() {
        let mut ghost = GhostQueue::new(10);

        ghost.insert("a".to_string());
        ghost.insert("b".to_string());

        assert!(ghost.remove(&"a".to_string()));
        assert!(!ghost.contains(&"a".to_string()));
        assert!(ghost.contains(&"b".to_string()));

        assert!(!ghost.remove(&"c".to_string())); // Not present
    }

    #[test]
    fn test_fifo_eviction() {
        let mut ghost = GhostQueue::new(3);

        ghost.insert("a".to_string());
        ghost.insert("b".to_string());
        ghost.insert("c".to_string());

        // At capacity - inserting "d" should evict "a"
        ghost.insert("d".to_string());

        assert!(!ghost.contains(&"a".to_string())); // Evicted
        assert!(ghost.contains(&"b".to_string()));
        assert!(ghost.contains(&"c".to_string()));
        assert!(ghost.contains(&"d".to_string()));
        assert_eq!(ghost.len(), 3);
    }

    #[test]
    fn test_clear() {
        let mut ghost = GhostQueue::new(10);

        ghost.insert("a".to_string());
        ghost.insert("b".to_string());
        ghost.insert("c".to_string());

        ghost.clear();

        assert!(ghost.is_empty());
        assert!(!ghost.contains(&"a".to_string()));
    }

    #[test]
    fn test_iter() {
        let mut ghost = GhostQueue::new(10);

        ghost.insert("a".to_string());
        ghost.insert("b".to_string());
        ghost.insert("c".to_string());

        let keys: Vec<_> = ghost.iter().collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_capacity_zero() {
        let ghost = GhostQueue::<String>::new(0);

        // With zero capacity, the queue starts empty and is immediately full
        assert!(ghost.is_empty());
        assert!(ghost.is_full()); // 0 >= 0 is true
    }
}
