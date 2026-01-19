//! Enhanced progress reporting for archive operations.
//!
//! This module provides extended progress callbacks with support for:
//! - Cancellation signaling (return false to abort)
//! - Compression ratio tracking
//! - ETA calculation
//! - Rate limiting callbacks to reduce overhead
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::progress::{ProgressReporter, StatisticsProgress};
//! use zesven::{Archive, ExtractOptions};
//!
//! let progress = StatisticsProgress::new();
//! let options = ExtractOptions::new().progress(progress);
//! archive.extract("./output", (), &options)?;
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// IEC byte unit: 1 KiB = 1024 bytes.
pub const BYTES_KIB: u64 = 1024;
/// IEC byte unit: 1 MiB = 1024 KiB.
pub const BYTES_MIB: u64 = 1024 * BYTES_KIB;
/// IEC byte unit: 1 GiB = 1024 MiB.
pub const BYTES_GIB: u64 = 1024 * BYTES_MIB;

// Floating point versions for formatting calculations
const BYTES_KB: f64 = 1024.0;
const BYTES_MB: f64 = BYTES_KB * 1024.0;
const BYTES_GB: f64 = BYTES_MB * 1024.0;

/// Progress reporting trait for archive operations.
///
/// This trait supports:
/// - Cancellation by returning `false` from `on_progress`
/// - Compression ratio reporting
/// - Archive-level totals
/// - Entry-level progress tracking
pub trait ProgressReporter: Send {
    /// Called at the start with the total bytes to process.
    ///
    /// This is called once before extraction begins.
    fn on_total(&mut self, total_bytes: u64) {
        let _ = total_bytes;
    }

    /// Called periodically during operation.
    ///
    /// Returns `true` to continue or `false` to request cancellation.
    fn on_progress(&mut self, bytes_processed: u64, total_bytes: u64) -> bool {
        let _ = (bytes_processed, total_bytes);
        true
    }

    /// Called when compression/decompression ratio changes significantly.
    ///
    /// Useful for displaying compression efficiency.
    fn on_ratio(&mut self, input_bytes: u64, output_bytes: u64) {
        let _ = (input_bytes, output_bytes);
    }

    /// Called when starting to process a new entry.
    ///
    /// Note: Archives contain "entries" which may be files or directories.
    fn on_entry_start(&mut self, entry_name: &str, size: u64) {
        let _ = (entry_name, size);
    }

    /// Called when entry processing completes.
    ///
    /// Note: Archives contain "entries" which may be files or directories.
    fn on_entry_complete(&mut self, entry_name: &str, success: bool) {
        let _ = (entry_name, success);
    }

    /// Called when a password is needed.
    ///
    /// Return `Some(password)` to provide a password, or `None` to abort.
    fn on_password_needed(&mut self) -> Option<String> {
        None
    }

    /// Called on any warning during processing.
    fn on_warning(&mut self, message: &str) {
        let _ = message;
    }

    /// Checks if cancellation has been requested.
    ///
    /// This is called before processing each entry to allow early termination
    /// without waiting for the next `on_progress` callback.
    ///
    /// Default implementation returns `false` (no cancellation).
    fn should_cancel(&self) -> bool {
        false
    }
}

/// Progress state with timing and rate calculation.
#[derive(Debug, Clone)]
pub struct ProgressState {
    /// Total bytes to process.
    pub total_bytes: u64,
    /// Bytes processed so far.
    pub processed_bytes: u64,
    /// Compressed/packed bytes (for ratio calculation).
    pub packed_bytes: u64,
    /// Current entry being processed (may be a file or directory).
    pub current_entry: Option<String>,
    /// Number of entries processed.
    pub entries_processed: usize,
    /// Total number of entries.
    pub entries_total: usize,
    /// Processing start time.
    pub start_time: Instant,
    /// Time of last update.
    pub last_update: Instant,
}

impl Default for ProgressState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            total_bytes: 0,
            processed_bytes: 0,
            packed_bytes: 0,
            current_entry: None,
            entries_processed: 0,
            entries_total: 0,
            start_time: now,
            last_update: now,
        }
    }
}

impl ProgressState {
    /// Creates a new progress state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the completion percentage (0.0 - 100.0).
    pub fn percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.processed_bytes as f64 / self.total_bytes as f64) * 100.0
        }
    }

    /// Returns the compression ratio (packed / unpacked).
    pub fn compression_ratio(&self) -> f64 {
        if self.processed_bytes == 0 {
            1.0
        } else {
            self.packed_bytes as f64 / self.processed_bytes as f64
        }
    }

    /// Returns elapsed time since start.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Returns the processing rate in bytes per second.
    pub fn bytes_per_second(&self) -> f64 {
        let elapsed = self.elapsed().as_secs_f64();
        if elapsed < 0.001 {
            0.0
        } else {
            self.processed_bytes as f64 / elapsed
        }
    }

    /// Returns estimated time remaining.
    pub fn eta(&self) -> Option<Duration> {
        let rate = self.bytes_per_second();
        if rate < 1.0 || self.processed_bytes >= self.total_bytes {
            return None;
        }
        let remaining = self.total_bytes - self.processed_bytes;
        let seconds = remaining as f64 / rate;
        Some(Duration::from_secs_f64(seconds))
    }

    /// Formats the rate as a human-readable string using IEC units.
    pub fn format_rate(&self) -> String {
        let rate = self.bytes_per_second();
        format_bytes_per_second_iec(rate)
    }

    /// Formats the ETA as a human-readable string.
    pub fn format_eta(&self) -> String {
        match self.eta() {
            Some(duration) => format_duration(duration),
            None => "unknown".to_string(),
        }
    }
}

/// A progress reporter that does nothing (null object pattern).
#[derive(Debug, Default, Clone)]
pub struct NoProgress;

impl ProgressReporter for NoProgress {}

/// A progress reporter that collects statistics.
#[derive(Debug, Default, Clone)]
pub struct StatisticsProgress {
    /// The progress state.
    pub state: ProgressState,
    /// Whether cancellation was requested.
    pub cancelled: bool,
    /// Warnings collected.
    pub warnings: Vec<String>,
}

impl StatisticsProgress {
    /// Creates a new statistics progress reporter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the collected state.
    pub fn state(&self) -> &ProgressState {
        &self.state
    }
}

impl ProgressReporter for StatisticsProgress {
    fn on_total(&mut self, total_bytes: u64) {
        self.state.total_bytes = total_bytes;
    }

    fn on_progress(&mut self, bytes_processed: u64, _total_bytes: u64) -> bool {
        self.state.processed_bytes = bytes_processed;
        self.state.last_update = Instant::now();
        !self.cancelled
    }

    fn on_ratio(&mut self, _input_bytes: u64, output_bytes: u64) {
        self.state.packed_bytes = output_bytes;
    }

    fn on_entry_start(&mut self, entry_name: &str, _size: u64) {
        self.state.current_entry = Some(entry_name.to_string());
    }

    fn on_entry_complete(&mut self, _entry_name: &str, _success: bool) {
        self.state.entries_processed += 1;
        self.state.current_entry = None;
    }

    fn on_warning(&mut self, message: &str) {
        self.warnings.push(message.to_string());
    }

    fn should_cancel(&self) -> bool {
        self.cancelled
    }
}

/// A progress reporter that rate-limits callbacks.
///
/// Useful for reducing overhead when progress is reported very frequently.
pub struct ThrottledProgress<P> {
    inner: P,
    min_interval: Duration,
    last_callback: Instant,
    last_bytes: u64,
}

impl<P: ProgressReporter> ThrottledProgress<P> {
    /// Creates a new throttled progress reporter.
    ///
    /// `min_interval` is the minimum time between progress callbacks.
    pub fn new(inner: P, min_interval: Duration) -> Self {
        Self {
            inner,
            min_interval,
            last_callback: Instant::now(),
            last_bytes: 0,
        }
    }

    /// Creates with default 100ms interval.
    pub fn default_interval(inner: P) -> Self {
        Self::new(inner, Duration::from_millis(100))
    }

    /// Returns the inner reporter.
    pub fn into_inner(self) -> P {
        self.inner
    }
}

impl<P: ProgressReporter> ProgressReporter for ThrottledProgress<P> {
    fn on_total(&mut self, total_bytes: u64) {
        self.inner.on_total(total_bytes);
    }

    fn on_progress(&mut self, bytes_processed: u64, total_bytes: u64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_callback);

        // Always call on completion
        if bytes_processed >= total_bytes || elapsed >= self.min_interval {
            self.last_callback = now;
            self.last_bytes = bytes_processed;
            self.inner.on_progress(bytes_processed, total_bytes)
        } else {
            true
        }
    }

    fn on_ratio(&mut self, input_bytes: u64, output_bytes: u64) {
        self.inner.on_ratio(input_bytes, output_bytes);
    }

    fn on_entry_start(&mut self, entry_name: &str, size: u64) {
        self.inner.on_entry_start(entry_name, size);
    }

    fn on_entry_complete(&mut self, entry_name: &str, success: bool) {
        self.inner.on_entry_complete(entry_name, success);
    }

    fn on_password_needed(&mut self) -> Option<String> {
        self.inner.on_password_needed()
    }

    fn on_warning(&mut self, message: &str) {
        self.inner.on_warning(message);
    }

    fn should_cancel(&self) -> bool {
        self.inner.should_cancel()
    }
}

/// A thread-safe progress reporter using atomics.
///
/// Allows progress to be monitored from another thread.
#[derive(Debug)]
pub struct AtomicProgress {
    total_bytes: AtomicU64,
    processed_bytes: AtomicU64,
    packed_bytes: AtomicU64,
    cancelled: AtomicBool,
    start_time: Instant,
}

impl Default for AtomicProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicProgress {
    /// Creates a new atomic progress reporter.
    pub fn new() -> Self {
        Self {
            total_bytes: AtomicU64::new(0),
            processed_bytes: AtomicU64::new(0),
            packed_bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            start_time: Instant::now(),
        }
    }

    /// Creates a shared atomic progress reporter.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Returns total bytes to process.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    /// Returns processed bytes.
    pub fn processed_bytes(&self) -> u64 {
        self.processed_bytes.load(Ordering::Relaxed)
    }

    /// Returns packed/compressed bytes.
    pub fn packed_bytes(&self) -> u64 {
        self.packed_bytes.load(Ordering::Relaxed)
    }

    /// Returns whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Returns completion percentage (0.0 - 100.0).
    pub fn percentage(&self) -> f64 {
        let total = self.total_bytes();
        if total == 0 {
            0.0
        } else {
            (self.processed_bytes() as f64 / total as f64) * 100.0
        }
    }

    /// Returns elapsed time since creation.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Returns processing rate in bytes per second.
    pub fn bytes_per_second(&self) -> f64 {
        let elapsed = self.elapsed().as_secs_f64();
        if elapsed < 0.001 {
            0.0
        } else {
            self.processed_bytes() as f64 / elapsed
        }
    }
}

impl ProgressReporter for AtomicProgress {
    fn on_total(&mut self, total_bytes: u64) {
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
    }

    fn on_progress(&mut self, bytes_processed: u64, _total_bytes: u64) -> bool {
        self.processed_bytes
            .store(bytes_processed, Ordering::Relaxed);
        !self.is_cancelled()
    }

    fn on_ratio(&mut self, _input_bytes: u64, output_bytes: u64) {
        self.packed_bytes.store(output_bytes, Ordering::Relaxed);
    }

    fn should_cancel(&self) -> bool {
        self.is_cancelled()
    }
}

/// Progress reporter for shared `Arc<AtomicProgress>`.
impl ProgressReporter for Arc<AtomicProgress> {
    fn on_total(&mut self, total_bytes: u64) {
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
    }

    fn on_progress(&mut self, bytes_processed: u64, _total_bytes: u64) -> bool {
        self.processed_bytes
            .store(bytes_processed, Ordering::Relaxed);
        !self.is_cancelled()
    }

    fn on_ratio(&mut self, _input_bytes: u64, output_bytes: u64) {
        self.packed_bytes.store(output_bytes, Ordering::Relaxed);
    }

    fn should_cancel(&self) -> bool {
        self.is_cancelled()
    }
}

/// A progress reporter that calls a closure.
pub struct ClosureProgress<F> {
    callback: F,
}

impl<F> ClosureProgress<F>
where
    F: FnMut(u64, u64) -> bool + Send,
{
    /// Creates a progress reporter from a closure.
    ///
    /// The closure receives (bytes_processed, total_bytes) and returns
    /// `true` to continue or `false` to cancel.
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F> ProgressReporter for ClosureProgress<F>
where
    F: FnMut(u64, u64) -> bool + Send,
{
    fn on_progress(&mut self, bytes_processed: u64, total_bytes: u64) -> bool {
        (self.callback)(bytes_processed, total_bytes)
    }
}

/// Creates a closure-based progress reporter.
pub fn progress_fn<F>(f: F) -> ClosureProgress<F>
where
    F: FnMut(u64, u64) -> bool + Send,
{
    ClosureProgress::new(f)
}

/// Formats bytes per second as a human-readable string using IEC units.
///
/// Uses 1024-based calculation with correct IEC labels (KiB/s, MiB/s, GiB/s).
pub fn format_bytes_per_second_iec(rate: f64) -> String {
    if rate < BYTES_KB {
        format!("{:.0} B/s", rate)
    } else if rate < BYTES_MB {
        format!("{:.1} KiB/s", rate / BYTES_KB)
    } else if rate < BYTES_GB {
        format!("{:.1} MiB/s", rate / BYTES_MB)
    } else {
        format!("{:.1} GiB/s", rate / BYTES_GB)
    }
}

/// Formats a duration as a human-readable string.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Formats bytes as a human-readable string using IEC units (KiB, MiB, GiB).
///
/// This version uses the technically correct IEC binary prefixes that explicitly
/// indicate binary (1024-based) measurements.
///
/// # Examples
///
/// ```rust
/// use zesven::progress::format_bytes_iec;
///
/// assert_eq!(format_bytes_iec(0), "0 B");
/// assert_eq!(format_bytes_iec(512), "512 B");
/// assert_eq!(format_bytes_iec(1024), "1.0 KiB");
/// assert_eq!(format_bytes_iec(1536), "1.5 KiB");
/// assert_eq!(format_bytes_iec(1048576), "1.0 MiB");
/// ```
pub fn format_bytes_iec(bytes: u64) -> String {
    let bytes_f64 = bytes as f64;
    if bytes_f64 < BYTES_KB {
        format!("{} B", bytes)
    } else if bytes_f64 < BYTES_MB {
        format!("{:.1} KiB", bytes_f64 / BYTES_KB)
    } else if bytes_f64 < BYTES_GB {
        format!("{:.1} MiB", bytes_f64 / BYTES_MB)
    } else {
        format!("{:.1} GiB", bytes_f64 / BYTES_GB)
    }
}

/// Formats bytes as a human-readable string using IEC units.
///
/// This version accepts `usize` for convenience when working with memory sizes.
/// See [`format_bytes_iec`] for the `u64` version.
pub fn format_bytes_iec_usize(bytes: usize) -> String {
    format_bytes_iec(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_state_percentage() {
        let mut state = ProgressState::new();
        state.total_bytes = 100;
        state.processed_bytes = 25;
        assert!((state.percentage() - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_progress_state_compression_ratio() {
        let mut state = ProgressState::new();
        state.processed_bytes = 1000;
        state.packed_bytes = 500;
        assert!((state.compression_ratio() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_no_progress() {
        let mut progress = NoProgress;
        assert!(progress.on_progress(50, 100));
    }

    #[test]
    fn test_statistics_progress() {
        let mut progress = StatisticsProgress::new();
        progress.on_total(1000);
        progress.on_entry_start("test.txt", 500);
        progress.on_progress(250, 1000);
        progress.on_entry_complete("test.txt", true);

        assert_eq!(progress.state().total_bytes, 1000);
        assert_eq!(progress.state().processed_bytes, 250);
        assert_eq!(progress.state().entries_processed, 1);
    }

    #[test]
    fn test_throttled_progress() {
        let inner = StatisticsProgress::new();
        let mut throttled = ThrottledProgress::new(inner, Duration::from_millis(10));

        throttled.on_total(100);
        assert!(throttled.on_progress(10, 100));

        // Should be throttled
        assert!(throttled.on_progress(20, 100));

        // Wait and should pass through
        std::thread::sleep(Duration::from_millis(15));
        assert!(throttled.on_progress(30, 100));
    }

    #[test]
    fn test_atomic_progress() {
        let progress = AtomicProgress::shared();
        let mut reporter: Arc<AtomicProgress> = Arc::clone(&progress);

        reporter.on_total(1000);
        reporter.on_progress(500, 1000);

        assert_eq!(progress.total_bytes(), 1000);
        assert_eq!(progress.processed_bytes(), 500);
        assert!((progress.percentage() - 50.0).abs() < 0.001);

        progress.cancel();
        assert!(!reporter.on_progress(600, 1000));
    }

    #[test]
    fn test_closure_progress() {
        let mut count = 0;
        let mut progress = progress_fn(|bytes, total| {
            count += 1;
            bytes < total
        });

        assert!(progress.on_progress(50, 100));
        assert!(progress.on_progress(99, 100));
        assert!(!progress.on_progress(100, 100));
        assert_eq!(count, 3);
    }

    #[test]
    fn test_format_bytes_per_second() {
        // Test the IEC version (correct labels)
        assert_eq!(format_bytes_per_second_iec(500.0), "500 B/s");
        assert_eq!(format_bytes_per_second_iec(1500.0), "1.5 KiB/s");
        assert_eq!(format_bytes_per_second_iec(1500.0 * 1024.0), "1.5 MiB/s");
        assert_eq!(
            format_bytes_per_second_iec(1500.0 * 1024.0 * 1024.0),
            "1.5 GiB/s"
        );
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3700)), "1h 1m");
    }

    #[test]
    fn test_format_bytes() {
        // Test the IEC version (correct labels)
        assert_eq!(format_bytes_iec(500), "500 B");
        assert_eq!(format_bytes_iec(1500), "1.5 KiB");
        assert_eq!(format_bytes_iec(1500 * 1024), "1.5 MiB");
        assert_eq!(format_bytes_iec(1500 * 1024 * 1024), "1.5 GiB");
    }

    #[test]
    fn test_progress_state_empty() {
        let state = ProgressState::new();
        assert_eq!(state.percentage(), 0.0);
        assert_eq!(state.compression_ratio(), 1.0);
    }

    #[test]
    fn test_statistics_cancellation() {
        let mut progress = StatisticsProgress::new();
        assert!(progress.on_progress(50, 100));

        progress.cancelled = true;
        assert!(!progress.on_progress(75, 100));
    }
}
