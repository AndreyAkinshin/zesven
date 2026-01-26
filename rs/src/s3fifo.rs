use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::{borrow::Borrow, collections::VecDeque, hash::Hash};

/// Tracks which queue a key belongs to for O(1) lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueId {
    Small,
    Main,
}

/// A FIFO-ordered ghost list that supports O(1) random access and removal.
/// Insertion has (because of evictions) mostly O(1), but has the worst case of
/// O(n) if all items of a queue are tombstones.
struct GhostList<K> {
    map: HashSet<K>,
    queue: VecDeque<K>,
    capacity: usize,
}

impl<K: Clone + Eq + Hash> GhostList<K> {
    fn new(capacity: usize) -> Self {
        Self {
            map: HashSet::new(),
            queue: VecDeque::new(),
            capacity,
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.contains(key)
    }

    fn insert(&mut self, key: K) {
        if self.map.contains(&key) {
            return;
        }

        while self.len() >= self.capacity {
            self.evict_oldest();
        }

        self.map.insert(key.clone());
        self.queue.push_front(key);
    }

    fn clear(&mut self) {
        self.map.clear();
        self.queue.clear();
    }

    fn remove<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        // We only remove the item from the lookup map. This means we create a tombstone
        // in the queue, so we need to occasionally remove the tombstones.
        self.map.remove(key);
    }

    fn evict_oldest(&mut self) -> Option<K> {
        while let Some(key) = self.queue.pop_back() {
            if self.map.contains(&key) {
                self.map.remove(&key);
                return Some(key);
            }
        }
        None
    }
}

struct ValueEntry<V> {
    value: V,
    freq: u8,
}

impl<V> ValueEntry<V> {
    fn new(value: V) -> Self {
        Self { value, freq: 0 }
    }
}

/// A cache that holds a certain number of values and uses the S3-FIFO cache strategy.
///
/// Based on "FIFO queues are all you need for cache eviction" (2023) by Juncheng Yang,
/// Yazhuo Zhang, Ziyue Qiu, Yao Yue and Rashmi Vinayak.
///
/// https://dl.acm.org/doi/10.1145/3600006.3613147
pub(crate) struct S3FifoCache<K, V> {
    values: HashMap<K, ValueEntry<V>>,
    /// Tracks which queue each key belongs to for O(1) lookup in pop().
    queue_map: HashMap<K, QueueId>,

    small_fifo: VecDeque<K>,
    main_fifo: VecDeque<K>,
    ghost: GhostList<K>,

    small_len: usize,
    small_capacity: usize,
    main_len: usize,
    main_capacity: usize,
    capacity: usize,
}

impl<K: Clone + Eq + Hash, V> S3FifoCache<K, V> {
    /// Creates a new cache that holds at most `capacity` values.
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        let capacity = capacity.get();

        // Small FIFO gets 10% of capacity (minimum 1, but ensure main has at least 1)
        // For capacity=1, use small=0 so everything goes directly to main
        let small_capacity = if capacity == 1 {
            0
        } else {
            std::cmp::max(1, capacity / 10)
        };

        Self {
            values: HashMap::new(),
            queue_map: HashMap::new(),
            main_fifo: VecDeque::new(),
            small_fifo: VecDeque::new(),
            ghost: GhostList::new(capacity - small_capacity),
            small_len: 0,
            small_capacity,
            main_len: 0,
            main_capacity: capacity - small_capacity,
            capacity,
        }
    }

    /// Returns the current length of all values inside the cache.
    #[inline(always)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.small_len + self.main_len
    }

    /// Returns the maximal amount of values this cache can hold.
    #[inline(always)]
    #[must_use]
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns true if the cache contains no elements.
    #[inline(always)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Clears the cache, removing all key-value pairs.
    pub(crate) fn clear(&mut self) {
        self.values.clear();
        self.queue_map.clear();
        self.small_fifo.clear();
        self.main_fifo.clear();
        self.ghost.clear();
        self.small_len = 0;
        self.main_len = 0;
    }

    /// Removes and returns the value for the given key from the cache,
    /// or `None` if it does not exist.
    ///
    /// Note: The key remains in its queue as a tombstone until naturally evicted.
    /// This is acceptable because the queue_map and values HashMap are the source
    /// of truth for membership, and tombstones are skipped during eviction.
    pub(crate) fn pop<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let (owned_key, entry) = self.values.remove_entry(key)?;

        // Use queue_map for O(1) lookup instead of O(n) VecDeque::contains()
        if let Some(queue_id) = self.queue_map.remove::<K>(&owned_key) {
            match queue_id {
                QueueId::Small => self.small_len -= 1,
                QueueId::Main => self.main_len -= 1,
            }
        }

        Some(entry.value)
    }

    /// Inserts the value for the given key into the cache.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        if let Some(entry) = self.values.get_mut(&key) {
            entry.value = value;
            return;
        }

        // Determine target queue: main if in ghost list or if small queue is disabled
        let insert_to_main = self.ghost.contains(&key) || self.small_capacity == 0;

        if insert_to_main {
            self.ghost.remove(&key);

            while self.main_len >= self.main_capacity {
                self.evict_m();
            }
            self.queue_map.insert(key.clone(), QueueId::Main);
            self.main_fifo.push_front(key.clone());
            self.main_len += 1;
        } else {
            while self.small_len >= self.small_capacity {
                self.evict_s();
            }
            self.queue_map.insert(key.clone(), QueueId::Small);
            self.small_fifo.push_front(key.clone());
            self.small_len += 1;
        }

        self.values.insert(key, ValueEntry::new(value));
    }

    /// Returns a reference of the given cached value.
    #[must_use]
    pub(crate) fn get<Q>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.values.get_mut(key).map(|value_entry| {
            value_entry.freq = std::cmp::min(value_entry.freq + 1, 3);
            &value_entry.value
        })
    }

    fn evict_s(&mut self) {
        while let Some(tail_key) = self.small_fifo.pop_back() {
            // Skip tombstones (keys that were removed via pop() but still in queue)
            let Some(tail) = self.values.get(&tail_key) else {
                continue;
            };

            self.small_len -= 1;

            if tail.freq > 1 {
                // Promote to main queue
                while self.main_len >= self.main_capacity {
                    self.evict_m();
                }

                self.queue_map.insert(tail_key.clone(), QueueId::Main);
                self.main_fifo.push_back(tail_key);
                self.main_len += 1;

                return;
            } else {
                // Evict to ghost list
                self.queue_map.remove(&tail_key);
                self.values.remove(&tail_key);
                self.ghost.insert(tail_key);

                return;
            }
        }
    }

    fn evict_m(&mut self) {
        while let Some(tail_key) = self.main_fifo.pop_back() {
            // Skip tombstones (keys that were removed via pop() but still in queue)
            let Some(tail) = self.values.get_mut(&tail_key) else {
                continue;
            };

            self.main_len -= 1;

            if tail.freq > 0 {
                // Re-insert at front with decremented frequency
                self.main_len += 1;
                tail.freq = tail.freq.saturating_sub(1);
                self.main_fifo.push_front(tail_key);
            } else {
                // Evict completely (not to ghost - main queue items don't go to ghost)
                self.queue_map.remove(&tail_key);
                self.values.remove(&tail_key);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ghost_list_basic_operations() {
        let mut ghost = GhostList::new(3);

        assert_eq!(ghost.len(), 0);
        assert!(!ghost.contains("key1"));

        ghost.insert("key1".to_string());
        assert_eq!(ghost.len(), 1);
        assert!(ghost.contains("key1"));

        ghost.insert("key2".to_string());
        ghost.insert("key3".to_string());
        assert_eq!(ghost.len(), 3);

        ghost.insert("key1".to_string());
        assert_eq!(ghost.len(), 3);

        ghost.remove("key2");
        assert_eq!(ghost.len(), 2);
        assert!(!ghost.contains("key2"));
        assert!(ghost.contains("key1"));
        assert!(ghost.contains("key3"));
    }

    #[test]
    fn test_ghost_list_fifo_eviction() {
        let mut ghost = GhostList::new(2);

        ghost.insert("first".to_string());
        ghost.insert("second".to_string());
        assert_eq!(ghost.len(), 2);

        ghost.insert("third".to_string());
        assert_eq!(ghost.len(), 2);
        assert!(!ghost.contains("first"));
        assert!(ghost.contains("second"));
        assert!(ghost.contains("third"));

        ghost.insert("fourth".to_string());
        assert_eq!(ghost.len(), 2);
        assert!(!ghost.contains("second"));
        assert!(ghost.contains("third"));
        assert!(ghost.contains("fourth"));
    }

    #[test]
    fn test_ghost_list_evict_oldest_with_tombstones() {
        let mut ghost = GhostList::new(3);

        ghost.insert("a".to_string());
        ghost.insert("b".to_string());
        ghost.insert("c".to_string());
        ghost.insert("d".to_string());

        assert_eq!(ghost.len(), 3);
        assert!(!ghost.contains("a"));

        ghost.remove("b");
        ghost.remove("c");
        assert_eq!(ghost.len(), 1);

        // Now evict_oldest should skip tombstones and evict 'd'.
        let evicted = ghost.evict_oldest();
        assert_eq!(evicted, Some("d".to_string()));
        assert_eq!(ghost.len(), 0);
        assert_eq!(ghost.queue.len(), 0);
    }

    #[test]
    fn test_basic_insertion_and_retrieval() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        let key1 = "test_key_1".to_string();
        let data1 = 500;

        cache.insert(key1.clone(), data1);
        assert_eq!(cache.len(), 1);

        let retrieved = cache.get(&key1);
        assert!(retrieved.is_some());
        assert_eq!(*retrieved.unwrap(), data1);
    }

    #[test]
    fn test_multiple_insertions() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        for i in 0..50 {
            let key = format!("key_{i}");
            let data = 100;
            cache.insert(key.clone(), data);
        }

        assert_eq!(cache.len(), 10);

        for i in 0..10 {
            let key = format!("key_{i}");
            let data = 100;
            cache.insert(key.clone(), data);
        }

        assert_eq!(cache.len(), 20);

        // Ghosts are promoted to main FIFO.
        for i in 0..10 {
            let key = format!("key_{i}");
            assert!(cache.get(&key).is_some(), "Key {key} should be present");
        }

        // The last batch of one hits are still in small FIFO.
        for i in 40..50 {
            let key = format!("key_{i}");
            assert!(cache.get(&key).is_some(), "Key {key} should be present");
        }
    }

    #[test]
    fn test_cache_eviction_by_len() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        for i in 0..20 {
            let key = format!("key_{i}");
            let data = 100;
            cache.insert(key.clone(), data);
        }

        assert_eq!(cache.len(), 10);

        for i in 10..20 {
            let key = format!("key_{i}");
            assert!(cache.get(&key).is_some(), "Key {key} should be present");
        }
    }

    #[test]
    fn test_overwrite_existing_key() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        let key = "overwrite_test".to_string();

        let data1 = 1000;
        cache.insert(key.clone(), data1);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&key), Some(&1000));

        let data2 = 1500;
        cache.insert(key.clone(), data2);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&key), Some(&1500));
    }

    #[test]
    fn test_is_empty() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        assert!(cache.is_empty());

        cache.insert("key".to_string(), 42);
        assert!(!cache.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        cache.insert("key1".to_string(), 1);
        cache.insert("key2".to_string(), 2);
        assert_eq!(cache.len(), 2);

        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert!(cache.get(&"key1".to_string()).is_none());
    }

    #[test]
    fn test_pop() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        cache.insert("key".to_string(), 42);
        assert_eq!(cache.len(), 1);

        let value = cache.pop("key");
        assert_eq!(value, Some(42));
        assert_eq!(cache.len(), 0);
        assert!(cache.get(&"key".to_string()).is_none());

        let value = cache.pop("nonexistent");
        assert_eq!(value, None);
    }

    #[test]
    fn test_capacity_one() {
        // With capacity=1, small_capacity=0, main_capacity=1
        // Everything goes directly to main queue
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(1).unwrap());

        assert_eq!(cache.capacity(), 1);
        assert!(cache.is_empty());

        cache.insert("key1".to_string(), 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&"key1".to_string()), Some(&1));

        // Inserting second key should evict first
        cache.insert("key2".to_string(), 2);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&"key1".to_string()).is_none());
        assert_eq!(cache.get(&"key2".to_string()), Some(&2));
    }

    #[test]
    fn test_insert_after_clear() {
        // Use larger capacity so small queue can hold multiple items
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        cache.insert("key1".to_string(), 1);
        cache.insert("key2".to_string(), 2);
        cache.clear();

        // Re-insert same keys after clear
        cache.insert("key1".to_string(), 10);
        cache.insert("key2".to_string(), 20);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"key1".to_string()), Some(&10));
        assert_eq!(cache.get(&"key2".to_string()), Some(&20));
    }

    #[test]
    fn test_pop_then_reinsert() {
        let mut cache: S3FifoCache<String, u64> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        cache.insert("key".to_string(), 1);
        let _ = cache.pop("key");
        assert!(cache.is_empty());

        // Re-insert after pop
        cache.insert("key".to_string(), 2);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&"key".to_string()), Some(&2));
    }

    #[test]
    fn test_queue_consistency_after_operations() {
        // Use larger capacity (100) so small_capacity=10 can hold our test items
        let mut cache: S3FifoCache<i32, i32> = S3FifoCache::new(NonZeroUsize::new(100).unwrap());

        // Fill small queue with 5 items
        for i in 0..5 {
            cache.insert(i, i * 10);
        }
        assert_eq!(cache.len(), 5);

        // Access some items to increase frequency (for potential promotion)
        cache.get(&0);
        cache.get(&0);
        cache.get(&2);
        cache.get(&2);

        // Pop some items
        cache.pop(&1);
        cache.pop(&3);

        // Verify remaining items are accessible
        assert_eq!(cache.get(&0), Some(&0));
        assert_eq!(cache.get(&2), Some(&20));
        assert_eq!(cache.get(&4), Some(&40));
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&3).is_none());

        // Verify length is correct
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn test_ghost_promotion() {
        // With capacity 10, small_capacity = 1, main_capacity = 9
        let mut cache: S3FifoCache<i32, i32> = S3FifoCache::new(NonZeroUsize::new(10).unwrap());

        // Insert and evict key 0 to put it in ghost list
        cache.insert(0, 0);
        cache.insert(1, 1); // Evicts 0 to ghost (small queue can only hold 1)

        assert!(cache.get(&0).is_none());

        // Re-insert key 0 - should be promoted to main queue from ghost
        cache.insert(0, 100);
        assert_eq!(cache.get(&0), Some(&100));

        // Both keys should now be in cache (1 in small, 0 in main)
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_capacity_two() {
        // With capacity=2, small_capacity=1, main_capacity=1
        let mut cache: S3FifoCache<i32, i32> = S3FifoCache::new(NonZeroUsize::new(2).unwrap());

        cache.insert(0, 0);
        cache.insert(1, 1); // Evicts 0 to ghost
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&0).is_none());

        // Re-insert 0 (promoted from ghost to main)
        cache.insert(0, 100);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&0), Some(&100));
        assert_eq!(cache.get(&1), Some(&1));
    }
}
