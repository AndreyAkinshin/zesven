//! Memory tracking and allocation management for streaming operations.
//!
//! This module provides [`MemoryTracker`] for monitoring and enforcing
//! memory limits during streaming decompression.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Error, Result};

/// Memory usage tracker for streaming operations.
///
/// This tracker monitors memory allocations and enforces limits to prevent
/// excessive memory usage during decompression. It uses atomic operations
/// for thread-safe tracking.
///
/// # Example
///
/// ```rust
/// use zesven::streaming::MemoryTracker;
///
/// // Create a tracker with a 64 MiB limit
/// let tracker = MemoryTracker::new(64 * 1024 * 1024);
///
/// // Allocate memory (returns guard that releases on drop)
/// let guard = tracker.allocate(1024)?;
///
/// // Check current usage
/// println!("Current usage: {} bytes", tracker.current_usage());
///
/// // Memory is automatically released when guard is dropped
/// drop(guard);
/// # Ok::<(), zesven::Error>(())
/// ```
#[derive(Debug)]
pub struct MemoryTracker {
    current_usage: AtomicUsize,
    peak_usage: AtomicUsize,
    limit: usize,
}

impl MemoryTracker {
    /// Creates a new memory tracker with the specified limit.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum memory in bytes that can be allocated.
    pub fn new(limit: usize) -> Self {
        Self {
            current_usage: AtomicUsize::new(0),
            peak_usage: AtomicUsize::new(0),
            limit,
        }
    }

    /// Creates an unlimited memory tracker.
    ///
    /// This tracker will never fail allocations due to limits.
    pub fn unlimited() -> Self {
        Self::new(usize::MAX)
    }

    /// Returns the memory limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the current memory usage.
    pub fn current_usage(&self) -> usize {
        self.current_usage.load(Ordering::SeqCst)
    }

    /// Returns the peak memory usage since tracker creation.
    pub fn peak_usage(&self) -> usize {
        self.peak_usage.load(Ordering::SeqCst)
    }

    /// Returns the remaining available memory.
    pub fn available(&self) -> usize {
        self.limit.saturating_sub(self.current_usage())
    }

    /// Checks if the specified amount can be allocated without exceeding limits.
    pub fn can_allocate(&self, bytes: usize) -> bool {
        self.current_usage() + bytes <= self.limit
    }

    /// Allocates the specified amount of memory.
    ///
    /// Returns a [`MemoryGuard`] that automatically releases the memory
    /// when dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the allocation would exceed the memory limit.
    pub fn allocate(&self, bytes: usize) -> Result<MemoryGuard<'_>> {
        // Try to atomically increment usage
        loop {
            let current = self.current_usage.load(Ordering::SeqCst);
            let new_usage = current.checked_add(bytes).ok_or_else(|| {
                Error::ResourceLimitExceeded(format!(
                    "Memory allocation overflow: {} + {} bytes",
                    current, bytes
                ))
            })?;

            if new_usage > self.limit {
                return Err(Error::ResourceLimitExceeded(format!(
                    "Memory limit exceeded: {} + {} = {} bytes (limit: {} bytes)",
                    current, bytes, new_usage, self.limit
                )));
            }

            // Try to update atomically
            if self
                .current_usage
                .compare_exchange(current, new_usage, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                // Update peak usage
                self.peak_usage.fetch_max(new_usage, Ordering::SeqCst);

                return Ok(MemoryGuard {
                    tracker: self,
                    bytes,
                });
            }
            // If compare_exchange failed, another thread modified it; retry
        }
    }

    /// Tries to allocate memory without returning an error.
    ///
    /// Returns `None` if the allocation would exceed limits.
    pub fn try_allocate(&self, bytes: usize) -> Option<MemoryGuard<'_>> {
        self.allocate(bytes).ok()
    }

    /// Allocates up to the specified amount, returning how much was allocated.
    ///
    /// This will allocate as much as possible without exceeding the limit.
    pub fn allocate_up_to(&self, bytes: usize) -> (MemoryGuard<'_>, usize) {
        let available = self.available();
        let to_allocate = bytes.min(available);

        if to_allocate == 0 {
            return (
                MemoryGuard {
                    tracker: self,
                    bytes: 0,
                },
                0,
            );
        }

        match self.allocate(to_allocate) {
            Ok(guard) => (guard, to_allocate),
            Err(_) => {
                // Race condition - try with what's available
                let available = self.available();
                if available > 0 {
                    match self.allocate(available) {
                        Ok(guard) => (guard, available),
                        Err(_) => (
                            MemoryGuard {
                                tracker: self,
                                bytes: 0,
                            },
                            0,
                        ),
                    }
                } else {
                    (
                        MemoryGuard {
                            tracker: self,
                            bytes: 0,
                        },
                        0,
                    )
                }
            }
        }
    }

    /// Resets the tracker to zero usage.
    ///
    /// This should only be used when all allocations have been released.
    /// Using this while guards are still active may cause memory tracking
    /// to become inaccurate.
    pub fn reset(&self) {
        self.current_usage.store(0, Ordering::SeqCst);
    }

    /// Resets peak usage tracking.
    pub fn reset_peak(&self) {
        self.peak_usage
            .store(self.current_usage(), Ordering::SeqCst);
    }

    // Internal method to release memory (called by MemoryGuard)
    fn release(&self, bytes: usize) {
        self.current_usage.fetch_sub(bytes, Ordering::SeqCst);
    }
}

impl Default for MemoryTracker {
    fn default() -> Self {
        // Default to 64 MiB limit
        Self::new(64 * 1024 * 1024)
    }
}

/// RAII guard that releases memory when dropped.
///
/// This guard is returned by [`MemoryTracker::allocate`] and ensures
/// that allocated memory is properly tracked and released.
#[derive(Debug)]
pub struct MemoryGuard<'a> {
    tracker: &'a MemoryTracker,
    bytes: usize,
}

impl<'a> MemoryGuard<'a> {
    /// Returns the number of bytes held by this guard.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Consumes this guard without releasing the memory.
    ///
    /// This is useful when transferring ownership of the allocation.
    /// The caller is responsible for ensuring the memory is eventually released.
    pub fn forget(self) -> usize {
        let bytes = self.bytes;
        std::mem::forget(self);
        bytes
    }
}

impl Drop for MemoryGuard<'_> {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.tracker.release(self.bytes);
        }
    }
}

/// A tracked allocation that owns a `Vec<u8>`.
///
/// This combines memory tracking with actual buffer allocation,
/// ensuring the memory accounting stays in sync with real allocations.
#[derive(Debug)]
pub struct TrackedBuffer<'a> {
    data: Vec<u8>,
    _guard: MemoryGuard<'a>,
}

impl<'a> TrackedBuffer<'a> {
    /// Creates a new tracked buffer with the specified capacity.
    pub fn new(tracker: &'a MemoryTracker, capacity: usize) -> Result<Self> {
        let guard = tracker.allocate(capacity)?;
        let data = Vec::with_capacity(capacity);
        Ok(Self {
            data,
            _guard: guard,
        })
    }

    /// Creates a new tracked buffer filled with zeros.
    pub fn zeroed(tracker: &'a MemoryTracker, size: usize) -> Result<Self> {
        let guard = tracker.allocate(size)?;
        let data = vec![0u8; size];
        Ok(Self {
            data,
            _guard: guard,
        })
    }

    /// Returns a reference to the underlying data.
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Returns a mutable reference to the underlying data.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Returns the length of the buffer.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Consumes the buffer and returns the underlying Vec.
    ///
    /// Note: The memory will still be tracked until the guard is dropped.
    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

impl AsRef<[u8]> for TrackedBuffer<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl AsMut<[u8]> for TrackedBuffer<'_> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl std::ops::Deref for TrackedBuffer<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl std::ops::DerefMut for TrackedBuffer<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_basic() {
        let tracker = MemoryTracker::new(1024);

        assert_eq!(tracker.limit(), 1024);
        assert_eq!(tracker.current_usage(), 0);
        assert_eq!(tracker.peak_usage(), 0);
        assert_eq!(tracker.available(), 1024);
    }

    #[test]
    fn test_allocate_success() {
        let tracker = MemoryTracker::new(1024);

        let guard = tracker.allocate(512).unwrap();
        assert_eq!(tracker.current_usage(), 512);
        assert_eq!(tracker.available(), 512);
        assert_eq!(guard.bytes(), 512);

        drop(guard);
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn test_allocate_exceeds_limit() {
        let tracker = MemoryTracker::new(1024);

        let result = tracker.allocate(2048);
        assert!(result.is_err());
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn test_multiple_allocations() {
        let tracker = MemoryTracker::new(1024);

        let guard1 = tracker.allocate(256).unwrap();
        assert_eq!(tracker.current_usage(), 256);

        let guard2 = tracker.allocate(256).unwrap();
        assert_eq!(tracker.current_usage(), 512);

        drop(guard1);
        assert_eq!(tracker.current_usage(), 256);

        drop(guard2);
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn test_peak_usage() {
        let tracker = MemoryTracker::new(1024);

        let guard1 = tracker.allocate(300).unwrap();
        let guard2 = tracker.allocate(400).unwrap();
        assert_eq!(tracker.peak_usage(), 700);

        drop(guard1);
        assert_eq!(tracker.current_usage(), 400);
        assert_eq!(tracker.peak_usage(), 700); // Peak unchanged

        let guard3 = tracker.allocate(500).unwrap();
        assert_eq!(tracker.peak_usage(), 900);

        drop(guard2);
        drop(guard3);
    }

    #[test]
    fn test_can_allocate() {
        let tracker = MemoryTracker::new(1024);

        assert!(tracker.can_allocate(512));
        assert!(tracker.can_allocate(1024));
        assert!(!tracker.can_allocate(2048));

        let _guard = tracker.allocate(512).unwrap();
        assert!(tracker.can_allocate(512));
        assert!(!tracker.can_allocate(1024));
    }

    #[test]
    fn test_try_allocate() {
        let tracker = MemoryTracker::new(1024);

        let guard = tracker.try_allocate(512);
        assert!(guard.is_some());

        let guard2 = tracker.try_allocate(1024);
        assert!(guard2.is_none());

        drop(guard);
    }

    #[test]
    fn test_allocate_up_to() {
        let tracker = MemoryTracker::new(1024);

        let (guard1, amount1) = tracker.allocate_up_to(2048);
        assert_eq!(amount1, 1024);
        assert_eq!(tracker.current_usage(), 1024);

        let (guard2, amount2) = tracker.allocate_up_to(512);
        assert_eq!(amount2, 0);

        drop(guard1);
        drop(guard2);
    }

    #[test]
    fn test_guard_forget() {
        let tracker = MemoryTracker::new(1024);

        let guard = tracker.allocate(256).unwrap();
        let bytes = guard.forget();
        assert_eq!(bytes, 256);
        assert_eq!(tracker.current_usage(), 256); // Not released

        // Manually reset (normally would need to track this)
        tracker.reset();
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn test_tracked_buffer() {
        let tracker = MemoryTracker::new(1024);

        let buffer = TrackedBuffer::new(&tracker, 256).unwrap();
        assert_eq!(buffer.capacity(), 256);
        assert_eq!(tracker.current_usage(), 256);

        drop(buffer);
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn test_tracked_buffer_zeroed() {
        let tracker = MemoryTracker::new(1024);

        let buffer = TrackedBuffer::zeroed(&tracker, 128).unwrap();
        assert_eq!(buffer.len(), 128);
        assert!(buffer.iter().all(|&b| b == 0));
        assert_eq!(tracker.current_usage(), 128);
    }

    #[test]
    fn test_unlimited_tracker() {
        let tracker = MemoryTracker::unlimited();

        let guard = tracker.allocate(1024 * 1024 * 1024).unwrap();
        assert_eq!(guard.bytes(), 1024 * 1024 * 1024);
    }

    #[test]
    fn test_reset_peak() {
        let tracker = MemoryTracker::new(1024);

        let guard = tracker.allocate(500).unwrap();
        assert_eq!(tracker.peak_usage(), 500);

        drop(guard);
        assert_eq!(tracker.peak_usage(), 500);

        tracker.reset_peak();
        assert_eq!(tracker.peak_usage(), 0);
    }
}
