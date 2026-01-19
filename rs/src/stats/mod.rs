//! Read statistics tracking for archive operations.
//!
//! This module provides wrappers that track I/O operations during archive
//! reading, useful for performance analysis and debugging.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A single read operation record.
#[derive(Debug, Clone)]
pub struct ReadOp {
    /// Offset where the read started.
    pub offset: u64,
    /// Number of bytes requested.
    pub requested: usize,
    /// Number of bytes actually read.
    pub actual: usize,
    /// Duration of the read operation.
    pub duration_us: u64,
}

/// A single seek operation record.
#[derive(Debug, Clone)]
pub struct SeekOp {
    /// Position before seek.
    pub from: u64,
    /// Position after seek.
    pub to: u64,
    /// Seek type.
    pub seek_from: SeekKind,
    /// Duration of the seek operation.
    pub duration_us: u64,
}

/// Type of seek operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekKind {
    /// Seek from start of file.
    Start,
    /// Seek from end of file.
    End,
    /// Seek from current position.
    Current,
}

impl From<&SeekFrom> for SeekKind {
    fn from(seek: &SeekFrom) -> Self {
        match seek {
            SeekFrom::Start(_) => SeekKind::Start,
            SeekFrom::End(_) => SeekKind::End,
            SeekFrom::Current(_) => SeekKind::Current,
        }
    }
}

/// Aggregated read statistics.
#[derive(Debug, Clone, Default)]
pub struct ReadStats {
    /// Total bytes read.
    pub bytes_read: u64,
    /// Total number of read operations.
    pub read_count: u64,
    /// Total number of seek operations.
    pub seek_count: u64,
    /// Total time spent in read operations (microseconds).
    pub read_time_us: u64,
    /// Total time spent in seek operations (microseconds).
    pub seek_time_us: u64,
    /// Individual read operations (if detailed tracking enabled).
    pub read_ops: Vec<ReadOp>,
    /// Individual seek operations (if detailed tracking enabled).
    pub seek_ops: Vec<SeekOp>,
    /// Current position in stream.
    pub position: u64,
}

impl ReadStats {
    /// Returns the average read size in bytes.
    pub fn avg_read_size(&self) -> f64 {
        if self.read_count == 0 {
            0.0
        } else {
            self.bytes_read as f64 / self.read_count as f64
        }
    }

    /// Returns the average read duration in microseconds.
    pub fn avg_read_time_us(&self) -> f64 {
        if self.read_count == 0 {
            0.0
        } else {
            self.read_time_us as f64 / self.read_count as f64
        }
    }

    /// Returns the average seek duration in microseconds.
    pub fn avg_seek_time_us(&self) -> f64 {
        if self.seek_count == 0 {
            0.0
        } else {
            self.seek_time_us as f64 / self.seek_count as f64
        }
    }

    /// Returns the total I/O time in microseconds.
    pub fn total_io_time_us(&self) -> u64 {
        self.read_time_us + self.seek_time_us
    }

    /// Returns the throughput in bytes per second.
    pub fn throughput_bytes_per_sec(&self) -> f64 {
        if self.read_time_us == 0 {
            0.0
        } else {
            (self.bytes_read as f64 * 1_000_000.0) / self.read_time_us as f64
        }
    }

    /// Clears all statistics.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Merges another ReadStats into this one.
    pub fn merge(&mut self, other: &ReadStats) {
        self.bytes_read += other.bytes_read;
        self.read_count += other.read_count;
        self.seek_count += other.seek_count;
        self.read_time_us += other.read_time_us;
        self.seek_time_us += other.seek_time_us;
        self.read_ops.extend(other.read_ops.iter().cloned());
        self.seek_ops.extend(other.seek_ops.iter().cloned());
    }
}

/// Configuration for statistics tracking.
#[derive(Debug, Clone, Default)]
pub struct StatsConfig {
    /// Track individual operations (more memory, more detail).
    pub detailed: bool,
}

impl StatsConfig {
    /// Enable detailed operation tracking.
    pub fn detailed() -> Self {
        Self { detailed: true }
    }

    /// Enable summary-only tracking (lower memory overhead).
    pub fn summary_only() -> Self {
        Self { detailed: false }
    }
}

/// A reader wrapper that tracks I/O statistics.
///
/// Wraps any `Read + Seek` type and records all read and seek operations.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::stats::{StatsReader, StatsConfig};
/// use std::fs::File;
///
/// let file = File::open("archive.7z")?;
/// let (reader, stats) = StatsReader::new(file, StatsConfig::default());
///
/// // Use reader for archive operations...
///
/// // Later, check statistics
/// let stats = stats.lock().unwrap();
/// println!("Read {} bytes in {} operations", stats.bytes_read, stats.read_count);
/// ```
pub struct StatsReader<R> {
    inner: R,
    stats: Arc<Mutex<ReadStats>>,
    config: StatsConfig,
    position: u64,
}

impl<R> StatsReader<R> {
    /// Creates a new StatsReader wrapping the given reader.
    ///
    /// Returns the reader and a shared reference to the statistics.
    pub fn new(inner: R, config: StatsConfig) -> (Self, Arc<Mutex<ReadStats>>) {
        let stats = Arc::new(Mutex::new(ReadStats::default()));
        let reader = Self {
            inner,
            stats: Arc::clone(&stats),
            config,
            position: 0,
        };
        (reader, stats)
    }

    /// Creates a StatsReader with existing shared stats.
    ///
    /// Useful for tracking multiple readers together.
    pub fn with_shared_stats(inner: R, stats: Arc<Mutex<ReadStats>>, config: StatsConfig) -> Self {
        Self {
            inner,
            stats,
            config,
            position: 0,
        }
    }

    /// Returns a reference to the underlying reader.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Returns a mutable reference to the underlying reader.
    pub fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Consumes the wrapper and returns the underlying reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Returns a clone of the statistics Arc.
    pub fn stats(&self) -> Arc<Mutex<ReadStats>> {
        Arc::clone(&self.stats)
    }

    /// Takes a snapshot of the current statistics.
    pub fn snapshot(&self) -> ReadStats {
        self.stats.lock().unwrap().clone()
    }
}

impl<R: Read> Read for StatsReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = Instant::now();
        let result = self.inner.read(buf);
        let duration = start.elapsed();

        if let Ok(n) = &result {
            let mut stats = self.stats.lock().unwrap();
            let duration_us = duration.as_micros() as u64;

            if self.config.detailed {
                stats.read_ops.push(ReadOp {
                    offset: self.position,
                    requested: buf.len(),
                    actual: *n,
                    duration_us,
                });
            }

            stats.bytes_read += *n as u64;
            stats.read_count += 1;
            stats.read_time_us += duration_us;
            stats.position = self.position + *n as u64;

            self.position += *n as u64;
        }

        result
    }
}

impl<R: Seek> Seek for StatsReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let start = Instant::now();
        let from_pos = self.position;
        let result = self.inner.seek(pos);
        let duration = start.elapsed();

        if let Ok(new_pos) = &result {
            let mut stats = self.stats.lock().unwrap();
            let duration_us = duration.as_micros() as u64;

            if self.config.detailed {
                stats.seek_ops.push(SeekOp {
                    from: from_pos,
                    to: *new_pos,
                    seek_from: SeekKind::from(&pos),
                    duration_us,
                });
            }

            stats.seek_count += 1;
            stats.seek_time_us += duration_us;
            stats.position = *new_pos;

            self.position = *new_pos;
        }

        result
    }
}

/// Extension trait for creating stats-wrapped readers easily.
pub trait WithStats: Sized {
    /// Wraps self with statistics tracking.
    fn with_stats(self, config: StatsConfig) -> (StatsReader<Self>, Arc<Mutex<ReadStats>>);

    /// Wraps self with default (summary-only) statistics tracking.
    fn with_stats_default(self) -> (StatsReader<Self>, Arc<Mutex<ReadStats>>) {
        self.with_stats(StatsConfig::default())
    }

    /// Wraps self with detailed statistics tracking.
    fn with_detailed_stats(self) -> (StatsReader<Self>, Arc<Mutex<ReadStats>>) {
        self.with_stats(StatsConfig::detailed())
    }
}

impl<R: Read + Seek> WithStats for R {
    fn with_stats(self, config: StatsConfig) -> (StatsReader<Self>, Arc<Mutex<ReadStats>>) {
        StatsReader::new(self, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_stats_reader_read() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let (mut reader, stats) = StatsReader::new(cursor, StatsConfig::default());

        let mut buf = [0u8; 5];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"Hello");

        let stats = stats.lock().unwrap();
        assert_eq!(stats.bytes_read, 5);
        assert_eq!(stats.read_count, 1);
        assert_eq!(stats.position, 5);
    }

    #[test]
    fn test_stats_reader_seek() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let (mut reader, stats) = StatsReader::new(cursor, StatsConfig::default());

        reader.seek(SeekFrom::Start(7)).unwrap();

        let stats = stats.lock().unwrap();
        assert_eq!(stats.seek_count, 1);
        assert_eq!(stats.position, 7);
    }

    #[test]
    fn test_stats_reader_detailed() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let (mut reader, stats) = StatsReader::new(cursor, StatsConfig::detailed());

        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();
        reader.seek(SeekFrom::Start(7)).unwrap();
        reader.read_exact(&mut buf).unwrap();

        let stats = stats.lock().unwrap();
        assert_eq!(stats.read_ops.len(), 2);
        assert_eq!(stats.seek_ops.len(), 1);

        assert_eq!(stats.read_ops[0].offset, 0);
        assert_eq!(stats.read_ops[0].actual, 5);

        assert_eq!(stats.seek_ops[0].from, 5);
        assert_eq!(stats.seek_ops[0].to, 7);
    }

    #[test]
    fn test_stats_calculations() {
        let stats = ReadStats {
            bytes_read: 1000,
            read_count: 10,
            read_time_us: 100,
            seek_count: 5,
            seek_time_us: 50,
            ..Default::default()
        };

        assert_eq!(stats.avg_read_size(), 100.0);
        assert_eq!(stats.avg_read_time_us(), 10.0);
        assert_eq!(stats.avg_seek_time_us(), 10.0);
        assert_eq!(stats.total_io_time_us(), 150);
        assert_eq!(stats.throughput_bytes_per_sec(), 10_000_000.0);
    }

    #[test]
    fn test_stats_merge() {
        let mut stats1 = ReadStats {
            bytes_read: 100,
            read_count: 5,
            seek_count: 2,
            read_time_us: 50,
            seek_time_us: 20,
            ..Default::default()
        };

        let stats2 = ReadStats {
            bytes_read: 200,
            read_count: 10,
            seek_count: 3,
            read_time_us: 100,
            seek_time_us: 30,
            ..Default::default()
        };

        stats1.merge(&stats2);

        assert_eq!(stats1.bytes_read, 300);
        assert_eq!(stats1.read_count, 15);
        assert_eq!(stats1.seek_count, 5);
        assert_eq!(stats1.read_time_us, 150);
        assert_eq!(stats1.seek_time_us, 50);
    }

    #[test]
    fn test_with_stats_trait() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let (mut reader, stats) = cursor.with_stats_default();

        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();

        let stats = stats.lock().unwrap();
        assert_eq!(stats.bytes_read, 5);
    }

    #[test]
    fn test_stats_snapshot() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let (mut reader, _) = StatsReader::new(cursor, StatsConfig::default());

        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();

        let snapshot = reader.snapshot();
        assert_eq!(snapshot.bytes_read, 5);

        // Read more - snapshot should be unchanged
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(snapshot.bytes_read, 5); // Still 5 from snapshot

        let new_snapshot = reader.snapshot();
        assert_eq!(new_snapshot.bytes_read, 10); // New snapshot reflects all reads
    }

    #[test]
    fn test_into_inner() {
        let data = b"Hello";
        let cursor = Cursor::new(data.to_vec());
        let (reader, _) = StatsReader::new(cursor, StatsConfig::default());

        let inner = reader.into_inner();
        assert_eq!(inner.into_inner(), data.to_vec());
    }

    #[test]
    fn test_seek_kind() {
        assert_eq!(SeekKind::from(&SeekFrom::Start(0)), SeekKind::Start);
        assert_eq!(SeekKind::from(&SeekFrom::End(0)), SeekKind::End);
        assert_eq!(SeekKind::from(&SeekFrom::Current(0)), SeekKind::Current);
    }

    #[test]
    fn test_empty_stats() {
        let stats = ReadStats::default();
        assert_eq!(stats.avg_read_size(), 0.0);
        assert_eq!(stats.avg_read_time_us(), 0.0);
        assert_eq!(stats.avg_seek_time_us(), 0.0);
        assert_eq!(stats.throughput_bytes_per_sec(), 0.0);
    }

    #[test]
    fn test_stats_clear() {
        let mut stats = ReadStats {
            bytes_read: 100,
            read_count: 5,
            seek_count: 2,
            ..Default::default()
        };

        stats.clear();
        assert_eq!(stats.bytes_read, 0);
        assert_eq!(stats.read_count, 0);
        assert_eq!(stats.seek_count, 0);
    }
}
