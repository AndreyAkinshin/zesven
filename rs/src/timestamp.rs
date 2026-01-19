//! High-precision timestamp handling.
//!
//! This module provides the [`Timestamp`] type for working with file timestamps
//! stored in 7z archives. Timestamps are stored as Windows FILETIME values,
//! which provide 100-nanosecond precision.
//!
//! # Precision
//!
//! The 7z archive format stores timestamps as Windows FILETIME values:
//! - 64-bit unsigned integer
//! - Counts 100-nanosecond intervals since January 1, 1601 (UTC)
//! - Maximum precision: 100 nanoseconds (not true nanoseconds)
//!
//! When converting to other time representations, be aware of precision loss:
//! - Unix timestamps (seconds): loses sub-second precision
//! - Unix timestamps with microseconds: loses 100ns precision
//! - Unix timestamps with nanoseconds: preserves full precision (as multiples of 100ns)
//!
//! # Example
//!
//! ```rust
//! use zesven::Timestamp;
//!
//! // Create from raw FILETIME
//! let ts = Timestamp::from_filetime(132456789012345678);
//!
//! // Convert to various formats
//! let unix_secs = ts.as_unix_secs();
//! println!("Unix seconds: {}", unix_secs);
//!
//! // Get sub-second components
//! println!("100-nanosecond intervals in second: {}", ts.sub_second_100ns());
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Windows FILETIME epoch: January 1, 1601 (UTC)
/// Difference from Unix epoch (January 1, 1970) in 100-nanosecond intervals.
const FILETIME_UNIX_DIFF: u64 = 116444736000000000;

/// Number of 100-nanosecond intervals per second.
const INTERVALS_PER_SECOND: u64 = 10_000_000;

/// Number of 100-nanosecond intervals per millisecond.
const INTERVALS_PER_MILLI: u64 = 10_000;

/// Number of 100-nanosecond intervals per microsecond.
const INTERVALS_PER_MICRO: u64 = 10;

/// A high-precision timestamp from a 7z archive.
///
/// Wraps a Windows FILETIME value (100-nanosecond intervals since January 1, 1601)
/// and provides various conversion methods while preserving the original precision.
///
/// # Precision
///
/// FILETIME precision is 100 nanoseconds, not 1 nanosecond. When converting to
/// nanoseconds, values will always be multiples of 100.
///
/// # Example
///
/// ```rust
/// use zesven::Timestamp;
/// use std::time::SystemTime;
///
/// // Unix epoch in FILETIME format
/// let unix_epoch_filetime = 116444736000000000u64;
/// let ts = Timestamp::from_filetime(unix_epoch_filetime);
///
/// assert_eq!(ts.as_unix_secs(), 0);
/// assert_eq!(ts.as_system_time(), SystemTime::UNIX_EPOCH);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Raw FILETIME value (100-nanosecond intervals since 1601-01-01)
    filetime: u64,
}

impl Timestamp {
    /// Creates a timestamp from a raw Windows FILETIME value.
    ///
    /// FILETIME counts 100-nanosecond intervals since January 1, 1601 (UTC).
    #[inline]
    pub const fn from_filetime(filetime: u64) -> Self {
        Self { filetime }
    }

    /// Creates a timestamp from Unix seconds (since January 1, 1970).
    ///
    /// Returns `None` if the timestamp would overflow.
    pub fn from_unix_secs(secs: i64) -> Option<Self> {
        if secs < 0 {
            // Handle times before Unix epoch
            let neg_secs = (-secs) as u64;
            let neg_intervals = neg_secs.checked_mul(INTERVALS_PER_SECOND)?;
            FILETIME_UNIX_DIFF
                .checked_sub(neg_intervals)
                .map(Self::from_filetime)
        } else {
            let intervals = (secs as u64).checked_mul(INTERVALS_PER_SECOND)?;
            FILETIME_UNIX_DIFF
                .checked_add(intervals)
                .map(Self::from_filetime)
        }
    }

    /// Creates a timestamp from Unix seconds and nanoseconds.
    ///
    /// Note: Only 100-nanosecond precision is preserved. The nanoseconds value
    /// is rounded down to the nearest 100ns.
    pub fn from_unix_secs_nanos(secs: i64, nanos: u32) -> Option<Self> {
        // Convert nanos to 100ns intervals (truncating)
        let nano_intervals = (nanos as u64) / 100;

        if secs < 0 {
            let neg_secs = (-secs) as u64;
            let neg_intervals = neg_secs.checked_mul(INTERVALS_PER_SECOND)?;
            let base = FILETIME_UNIX_DIFF.checked_sub(neg_intervals)?;
            // For negative times, we subtract the remaining nanosecond portion
            if nano_intervals > 0 {
                base.checked_add(nano_intervals).map(Self::from_filetime)
            } else {
                Some(Self::from_filetime(base))
            }
        } else {
            let sec_intervals = (secs as u64).checked_mul(INTERVALS_PER_SECOND)?;
            let base = FILETIME_UNIX_DIFF.checked_add(sec_intervals)?;
            base.checked_add(nano_intervals).map(Self::from_filetime)
        }
    }

    /// Creates a timestamp from a `SystemTime`.
    pub fn from_system_time(time: SystemTime) -> Option<Self> {
        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                Self::from_unix_secs_nanos(duration.as_secs() as i64, duration.subsec_nanos())
            }
            Err(e) => {
                let duration = e.duration();
                Self::from_unix_secs_nanos(-(duration.as_secs() as i64), duration.subsec_nanos())
            }
        }
    }

    /// Returns the raw Windows FILETIME value.
    ///
    /// This is the original value with maximum precision preserved.
    #[inline]
    pub const fn as_filetime(&self) -> u64 {
        self.filetime
    }

    /// Returns the timestamp as Unix seconds.
    ///
    /// Returns negative values for timestamps before January 1, 1970 (Unix epoch).
    /// Sub-second precision is truncated (rounded towards negative infinity for
    /// pre-epoch dates).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use zesven::Timestamp;
    ///
    /// // Unix epoch
    /// let ts = Timestamp::from_unix_secs(0).unwrap();
    /// assert_eq!(ts.as_unix_secs(), 0);
    ///
    /// // Before Unix epoch
    /// let ts = Timestamp::from_unix_secs(-3600).unwrap();
    /// assert_eq!(ts.as_unix_secs(), -3600);
    /// ```
    pub fn as_unix_secs(&self) -> i64 {
        if self.filetime >= FILETIME_UNIX_DIFF {
            let intervals = self.filetime - FILETIME_UNIX_DIFF;
            (intervals / INTERVALS_PER_SECOND) as i64
        } else {
            // Before Unix epoch - return negative
            let intervals = FILETIME_UNIX_DIFF - self.filetime;
            let secs = intervals / INTERVALS_PER_SECOND;
            // Round up for negative values if there's a remainder
            let extra = if intervals % INTERVALS_PER_SECOND > 0 {
                1
            } else {
                0
            };
            -((secs + extra) as i64)
        }
    }

    /// Returns the timestamp as Unix milliseconds.
    ///
    /// Preserves millisecond precision. For times before Unix epoch,
    /// returns negative values.
    pub fn as_unix_millis(&self) -> i64 {
        if self.filetime >= FILETIME_UNIX_DIFF {
            let intervals = self.filetime - FILETIME_UNIX_DIFF;
            (intervals / INTERVALS_PER_MILLI) as i64
        } else {
            let intervals = FILETIME_UNIX_DIFF - self.filetime;
            -((intervals / INTERVALS_PER_MILLI) as i64)
        }
    }

    /// Returns the timestamp as Unix microseconds.
    ///
    /// Preserves microsecond precision. For times before Unix epoch,
    /// returns negative values.
    pub fn as_unix_micros(&self) -> i64 {
        if self.filetime >= FILETIME_UNIX_DIFF {
            let intervals = self.filetime - FILETIME_UNIX_DIFF;
            (intervals / INTERVALS_PER_MICRO) as i64
        } else {
            let intervals = FILETIME_UNIX_DIFF - self.filetime;
            -((intervals / INTERVALS_PER_MICRO) as i64)
        }
    }

    /// Returns the timestamp as Unix nanoseconds.
    ///
    /// Note: Values are always multiples of 100 due to FILETIME precision.
    /// For times before Unix epoch, returns negative values.
    pub fn as_unix_nanos(&self) -> i128 {
        if self.filetime >= FILETIME_UNIX_DIFF {
            let intervals = self.filetime - FILETIME_UNIX_DIFF;
            (intervals as i128) * 100
        } else {
            let intervals = FILETIME_UNIX_DIFF - self.filetime;
            -((intervals as i128) * 100)
        }
    }

    /// Converts to a `SystemTime`.
    ///
    /// Preserves full 100-nanosecond precision.
    pub fn as_system_time(&self) -> SystemTime {
        if self.filetime >= FILETIME_UNIX_DIFF {
            let intervals = self.filetime - FILETIME_UNIX_DIFF;
            let secs = intervals / INTERVALS_PER_SECOND;
            let sub_intervals = intervals % INTERVALS_PER_SECOND;
            let nanos = (sub_intervals * 100) as u32;
            UNIX_EPOCH + Duration::new(secs, nanos)
        } else {
            let intervals = FILETIME_UNIX_DIFF - self.filetime;
            let secs = intervals / INTERVALS_PER_SECOND;
            let sub_intervals = intervals % INTERVALS_PER_SECOND;
            let nanos = (sub_intervals * 100) as u32;
            UNIX_EPOCH - Duration::new(secs, nanos)
        }
    }

    /// Returns the sub-second portion as 100-nanosecond intervals (0-9999999).
    ///
    /// This provides the maximum precision available in the FILETIME format.
    #[inline]
    pub fn sub_second_100ns(&self) -> u32 {
        (self.filetime % INTERVALS_PER_SECOND) as u32
    }

    /// Returns the sub-second portion as nanoseconds (0-999999900).
    ///
    /// Note: Values are always multiples of 100 due to FILETIME precision.
    #[inline]
    pub fn sub_second_nanos(&self) -> u32 {
        ((self.filetime % INTERVALS_PER_SECOND) * 100) as u32
    }

    /// Returns the sub-second portion as microseconds (0-999999).
    #[inline]
    pub fn sub_second_micros(&self) -> u32 {
        ((self.filetime % INTERVALS_PER_SECOND) / INTERVALS_PER_MICRO) as u32
    }

    /// Returns the sub-second portion as milliseconds (0-999).
    #[inline]
    pub fn sub_second_millis(&self) -> u32 {
        ((self.filetime % INTERVALS_PER_SECOND) / INTERVALS_PER_MILLI) as u32
    }

    /// Returns true if this timestamp is before the Unix epoch.
    #[inline]
    pub fn is_before_unix_epoch(&self) -> bool {
        self.filetime < FILETIME_UNIX_DIFF
    }

    /// Returns true if this timestamp is at or after the Unix epoch.
    #[inline]
    pub fn is_at_or_after_unix_epoch(&self) -> bool {
        self.filetime >= FILETIME_UNIX_DIFF
    }
}

impl Default for Timestamp {
    /// Returns the Unix epoch (January 1, 1970).
    fn default() -> Self {
        Self::from_filetime(FILETIME_UNIX_DIFF)
    }
}

impl From<u64> for Timestamp {
    fn from(filetime: u64) -> Self {
        Self::from_filetime(filetime)
    }
}

impl From<Timestamp> for u64 {
    fn from(ts: Timestamp) -> u64 {
        ts.filetime
    }
}

impl From<Timestamp> for SystemTime {
    fn from(ts: Timestamp) -> SystemTime {
        ts.as_system_time()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unix_epoch() {
        let ts = Timestamp::from_filetime(FILETIME_UNIX_DIFF);
        assert_eq!(ts.as_unix_secs(), 0);
        assert_eq!(ts.as_unix_millis(), 0);
        assert_eq!(ts.as_unix_micros(), 0);
        assert_eq!(ts.as_unix_nanos(), 0);
        assert_eq!(ts.as_system_time(), UNIX_EPOCH);
        assert!(!ts.is_before_unix_epoch());
        assert!(ts.is_at_or_after_unix_epoch());
    }

    #[test]
    fn test_sub_second_precision() {
        // Unix epoch + 1.5 seconds (5000000 100-ns intervals)
        let ts = Timestamp::from_filetime(FILETIME_UNIX_DIFF + 15_000_000);

        assert_eq!(ts.as_unix_secs(), 1);
        assert_eq!(ts.sub_second_100ns(), 5_000_000);
        assert_eq!(ts.sub_second_nanos(), 500_000_000);
        assert_eq!(ts.sub_second_micros(), 500_000);
        assert_eq!(ts.sub_second_millis(), 500);
    }

    #[test]
    fn test_from_unix_secs() {
        let ts = Timestamp::from_unix_secs(0).unwrap();
        assert_eq!(ts.as_filetime(), FILETIME_UNIX_DIFF);

        let ts = Timestamp::from_unix_secs(1).unwrap();
        assert_eq!(ts.as_filetime(), FILETIME_UNIX_DIFF + INTERVALS_PER_SECOND);

        // Negative (before Unix epoch)
        let ts = Timestamp::from_unix_secs(-1).unwrap();
        assert_eq!(ts.as_filetime(), FILETIME_UNIX_DIFF - INTERVALS_PER_SECOND);
    }

    #[test]
    fn test_from_unix_secs_nanos() {
        // 1 second + 500ms
        let ts = Timestamp::from_unix_secs_nanos(1, 500_000_000).unwrap();
        assert_eq!(ts.as_unix_secs(), 1);
        assert_eq!(ts.sub_second_millis(), 500);
    }

    #[test]
    fn test_roundtrip_system_time() {
        let original = UNIX_EPOCH + Duration::new(1234567890, 123_456_700);
        let ts = Timestamp::from_system_time(original).unwrap();
        let recovered = ts.as_system_time();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_100ns_precision() {
        // Create timestamp with specific 100ns value
        let ts = Timestamp::from_filetime(FILETIME_UNIX_DIFF + 123);

        // Should preserve the 100ns precision
        assert_eq!(ts.sub_second_100ns(), 123);
        assert_eq!(ts.sub_second_nanos(), 12300);
    }

    #[test]
    fn test_before_unix_epoch() {
        // 1 day before Unix epoch
        let day_in_intervals = 24 * 60 * 60 * INTERVALS_PER_SECOND;
        let ts = Timestamp::from_filetime(FILETIME_UNIX_DIFF - day_in_intervals);

        assert!(ts.is_before_unix_epoch());
        assert_eq!(ts.as_unix_secs(), -86400);
    }

    #[test]
    fn test_conversions() {
        let ts: Timestamp = 132456789012345678u64.into();
        let back: u64 = ts.into();
        assert_eq!(back, 132456789012345678);
    }

    #[test]
    fn test_default() {
        let ts = Timestamp::default();
        assert_eq!(ts.as_unix_secs(), 0);
    }
}
