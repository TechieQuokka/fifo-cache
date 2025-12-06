//! Basic FIFO queue implementation.
//!
//! A simple first-in-first-out queue using `VecDeque` for O(1) push/pop operations.
//! This is used as a building block for cache implementations.

use std::collections::VecDeque;
use std::sync::Arc;

/// A basic FIFO queue with configurable capacity.
///
/// Provides standard queue operations with O(1) amortized complexity.
/// Used by S3-FIFO for its small and main queues.
///
/// # Type Parameters
///
/// * `T` - The type of elements stored in the queue.
///
/// # Example
///
/// ```rust,ignore
/// use fifo_cache::queue::FifoQueue;
///
/// let mut queue = FifoQueue::new(100);
/// queue.push_back(1);
/// queue.push_back(2);
/// assert_eq!(queue.pop_front(), Some(1));
/// ```
#[derive(Debug)]
pub struct FifoQueue<T> {
    /// The underlying deque storage.
    inner: VecDeque<T>,

    /// Maximum capacity (0 = unlimited).
    capacity: usize,
}

impl<T> FifoQueue<T> {
    /// Create a new FIFO queue with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of elements. Use 0 for unlimited.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity.min(1024)), // Don't over-allocate
            capacity,
        }
    }

    /// Create a new FIFO queue with no capacity limit.
    #[must_use]
    pub fn unbounded() -> Self {
        Self::new(0)
    }

    /// Push an element to the back of the queue.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to push.
    ///
    /// # Returns
    ///
    /// `true` if the element was added, `false` if the queue is at capacity.
    pub fn push_back(&mut self, value: T) -> bool {
        if self.capacity > 0 && self.inner.len() >= self.capacity {
            return false;
        }
        self.inner.push_back(value);
        true
    }

    /// Push an element to the back, evicting the front if at capacity.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to push.
    ///
    /// # Returns
    ///
    /// The evicted element if the queue was at capacity.
    pub fn push_back_evict(&mut self, value: T) -> Option<T> {
        let evicted = if self.capacity > 0 && self.inner.len() >= self.capacity {
            self.inner.pop_front()
        } else {
            None
        };
        self.inner.push_back(value);
        evicted
    }

    /// Pop an element from the front of the queue.
    ///
    /// # Returns
    ///
    /// The front element, or `None` if the queue is empty.
    pub fn pop_front(&mut self) -> Option<T> {
        self.inner.pop_front()
    }

    /// Peek at the front element without removing it.
    ///
    /// # Returns
    ///
    /// A reference to the front element, or `None` if empty.
    pub fn front(&self) -> Option<&T> {
        self.inner.front()
    }

    /// Peek at the back element without removing it.
    ///
    /// # Returns
    ///
    /// A reference to the back element, or `None` if empty.
    pub fn back(&self) -> Option<&T> {
        self.inner.back()
    }

    /// Get the number of elements in the queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Check if the queue is at capacity.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.capacity > 0 && self.inner.len() >= self.capacity
    }

    /// Get the maximum capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all elements from the queue.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Iterate over elements without removing them.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inner.iter()
    }

    /// Get the underlying deque as a mutable reference.
    ///
    /// Use with caution - this bypasses capacity checks.
    pub fn inner_mut(&mut self) -> &mut VecDeque<T> {
        &mut self.inner
    }
}

impl<T: PartialEq> FifoQueue<T> {
    /// Remove all elements matching a predicate.
    ///
    /// # Arguments
    ///
    /// * `predicate` - Function that returns true for elements to remove.
    pub fn retain<F>(&mut self, predicate: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.inner.retain(predicate);
    }

    /// Check if the queue contains an element.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to search for.
    ///
    /// # Performance
    ///
    /// O(n) linear search.
    pub fn contains(&self, value: &T) -> bool {
        self.inner.contains(value)
    }
}

impl<T: Clone> FifoQueue<Arc<T>> {
    /// Find an element by key and return a clone.
    ///
    /// # Arguments
    ///
    /// * `predicate` - Function that returns true for the desired element.
    ///
    /// # Returns
    ///
    /// A clone of the Arc if found.
    pub fn find<F>(&self, predicate: F) -> Option<Arc<T>>
    where
        F: Fn(&Arc<T>) -> bool,
    {
        self.inner.iter().find(|e| predicate(e)).cloned()
    }
}

impl<T> Default for FifoQueue<T> {
    fn default() -> Self {
        Self::unbounded()
    }
}

impl<T> IntoIterator for FifoQueue<T> {
    type Item = T;
    type IntoIter = std::collections::vec_deque::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<T> FromIterator<T> for FifoQueue<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let inner: VecDeque<T> = iter.into_iter().collect();
        let len = inner.len();
        Self {
            inner,
            capacity: len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut queue = FifoQueue::new(10);

        assert!(queue.is_empty());
        assert!(!queue.is_full());

        queue.push_back(1);
        queue.push_back(2);
        queue.push_back(3);

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.front(), Some(&1));
        assert_eq!(queue.back(), Some(&3));

        assert_eq!(queue.pop_front(), Some(1));
        assert_eq!(queue.pop_front(), Some(2));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_capacity_limit() {
        let mut queue = FifoQueue::new(3);

        assert!(queue.push_back(1));
        assert!(queue.push_back(2));
        assert!(queue.push_back(3));
        assert!(!queue.push_back(4)); // At capacity
        assert!(queue.is_full());

        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn test_push_back_evict() {
        let mut queue = FifoQueue::new(3);

        queue.push_back(1);
        queue.push_back(2);
        queue.push_back(3);

        let evicted = queue.push_back_evict(4);
        assert_eq!(evicted, Some(1));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.front(), Some(&2));
    }

    #[test]
    fn test_unbounded() {
        let mut queue = FifoQueue::<i32>::unbounded();

        for i in 0..1000 {
            assert!(queue.push_back(i));
        }

        assert_eq!(queue.len(), 1000);
        assert!(!queue.is_full());
    }

    #[test]
    fn test_clear() {
        let mut queue = FifoQueue::new(10);
        queue.push_back(1);
        queue.push_back(2);

        queue.clear();

        assert!(queue.is_empty());
    }

    #[test]
    fn test_retain() {
        let mut queue = FifoQueue::new(10);
        queue.push_back(1);
        queue.push_back(2);
        queue.push_back(3);
        queue.push_back(4);

        queue.retain(|x| x % 2 == 0);

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop_front(), Some(2));
        assert_eq!(queue.pop_front(), Some(4));
    }

    #[test]
    fn test_iter() {
        let mut queue = FifoQueue::new(10);
        queue.push_back(1);
        queue.push_back(2);
        queue.push_back(3);

        let sum: i32 = queue.iter().sum();
        assert_eq!(sum, 6);
    }

    #[test]
    fn test_from_iter() {
        let queue: FifoQueue<i32> = vec![1, 2, 3].into_iter().collect();
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.capacity(), 3);
    }
}
