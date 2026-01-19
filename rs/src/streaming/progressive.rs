//! Progressive reader for streaming data with progress tracking.
//!
//! This module provides [`ProgressiveReader`] which wraps a reader and
//! provides progress information during reads.

use std::io::{self, Read};

/// A reader wrapper that tracks read progress.
///
/// This wrapper keeps track of how many bytes have been read, allowing
/// for progress reporting during streaming operations.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::ProgressiveReader;
/// use std::io::Read;
///
/// let data = vec![0u8; 1000];
/// let mut reader = ProgressiveReader::new(&data[..], 1000);
///
/// let mut buf = [0u8; 100];
/// reader.read(&mut buf)?;
///
/// println!("Progress: {:.1}%", reader.progress() * 100.0);
/// ```
#[derive(Debug)]
pub struct ProgressiveReader<R> {
    inner: R,
    total_size: u64,
    bytes_read: u64,
}

impl<R: Read> ProgressiveReader<R> {
    /// Creates a new progressive reader.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying reader
    /// * `total_size` - The expected total size (for progress calculation)
    pub fn new(inner: R, total_size: u64) -> Self {
        Self {
            inner,
            total_size,
            bytes_read: 0,
        }
    }

    /// Returns the number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the expected total size.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns the remaining bytes to read.
    pub fn remaining(&self) -> u64 {
        self.total_size.saturating_sub(self.bytes_read)
    }

    /// Returns the progress as a fraction (0.0 to 1.0).
    pub fn progress(&self) -> f64 {
        if self.total_size == 0 {
            1.0
        } else {
            self.bytes_read as f64 / self.total_size as f64
        }
    }

    /// Returns the progress as a percentage (0.0 to 100.0).
    pub fn progress_percent(&self) -> f64 {
        self.progress() * 100.0
    }

    /// Returns true if all expected bytes have been read.
    pub fn is_complete(&self) -> bool {
        self.bytes_read >= self.total_size
    }

    /// Consumes the reader and returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Returns a reference to the inner reader.
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Returns a mutable reference to the inner reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Resets the bytes read counter.
    pub fn reset(&mut self) {
        self.bytes_read = 0;
    }

    /// Updates the total size.
    pub fn set_total_size(&mut self, size: u64) {
        self.total_size = size;
    }
}

impl<R: Read> Read for ProgressiveReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

/// A progressive reader with a callback for progress updates.
///
/// This variant calls a user-provided callback after each read operation,
/// allowing for real-time progress tracking.
pub struct ProgressiveReaderWithCallback<R, F> {
    inner: R,
    total_size: u64,
    bytes_read: u64,
    callback: F,
    /// Minimum bytes between callback invocations
    callback_threshold: u64,
    /// Bytes read since last callback
    bytes_since_callback: u64,
}

impl<R: Read, F: FnMut(u64, u64)> ProgressiveReaderWithCallback<R, F> {
    /// Creates a new progressive reader with callback.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying reader
    /// * `total_size` - The expected total size
    /// * `callback` - Called with (bytes_read, total_size) after reads
    pub fn new(inner: R, total_size: u64, callback: F) -> Self {
        Self {
            inner,
            total_size,
            bytes_read: 0,
            callback,
            callback_threshold: 0,
            bytes_since_callback: 0,
        }
    }

    /// Sets the minimum bytes between callback invocations.
    ///
    /// This can be used to reduce callback overhead when reading
    /// many small chunks.
    pub fn with_threshold(mut self, threshold: u64) -> Self {
        self.callback_threshold = threshold;
        self
    }

    /// Returns the number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the expected total size.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns the progress as a fraction (0.0 to 1.0).
    pub fn progress(&self) -> f64 {
        if self.total_size == 0 {
            1.0
        } else {
            self.bytes_read as f64 / self.total_size as f64
        }
    }
}

impl<R: Read, F: FnMut(u64, u64)> Read for ProgressiveReaderWithCallback<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        self.bytes_since_callback += n as u64;

        // Call callback if threshold is met or if this is the last read
        if self.bytes_since_callback >= self.callback_threshold || n == 0 {
            (self.callback)(self.bytes_read, self.total_size);
            self.bytes_since_callback = 0;
        }

        Ok(n)
    }
}

/// A bounded reader that limits the number of bytes read.
///
/// This reader wraps another reader and limits the total bytes that
/// can be read, useful for reading fixed-size entries from a stream.
#[derive(Debug)]
pub struct BoundedReader<R> {
    inner: R,
    limit: u64,
    bytes_read: u64,
}

impl<R> BoundedReader<R> {
    /// Creates a new bounded reader with the specified limit.
    pub fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            limit,
            bytes_read: 0,
        }
    }

    /// Returns the number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the remaining bytes that can be read.
    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.bytes_read)
    }

    /// Returns true if the limit has been reached.
    pub fn is_exhausted(&self) -> bool {
        self.bytes_read >= self.limit
    }

    /// Consumes the reader and returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.remaining() as usize;
        if remaining == 0 {
            return Ok(0);
        }

        let to_read = buf.len().min(remaining);
        let n = self.inner.read(&mut buf[..to_read])?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

/// A chained reader that reads from multiple readers in sequence.
pub struct ChainedReader<I> {
    readers: I,
    current: Option<Box<dyn Read + Send>>,
}

impl<I: Iterator<Item = Box<dyn Read + Send>>> ChainedReader<I> {
    /// Creates a new chained reader from an iterator of readers.
    pub fn new(mut readers: I) -> Self {
        let current = readers.next();
        Self { readers, current }
    }
}

impl<I: Iterator<Item = Box<dyn Read + Send>>> Read for ChainedReader<I> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let reader = match &mut self.current {
                Some(r) => r,
                None => return Ok(0),
            };

            let n = reader.read(buf)?;
            if n > 0 {
                return Ok(n);
            }

            // Current reader exhausted, try next
            self.current = self.readers.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_progressive_reader_basic() {
        let data = vec![0u8; 1000];
        let mut reader = ProgressiveReader::new(Cursor::new(data), 1000);

        assert_eq!(reader.bytes_read(), 0);
        assert_eq!(reader.total_size(), 1000);
        assert!((reader.progress() - 0.0).abs() < f64::EPSILON);

        let mut buf = [0u8; 100];
        let _ = reader.read(&mut buf).unwrap();

        assert_eq!(reader.bytes_read(), 100);
        assert!((reader.progress() - 0.1).abs() < f64::EPSILON);
        assert!((reader.progress_percent() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_progressive_reader_complete() {
        let data = vec![0u8; 100];
        let mut reader = ProgressiveReader::new(Cursor::new(data), 100);

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();

        assert!(reader.is_complete());
        assert_eq!(reader.bytes_read(), 100);
        assert!((reader.progress() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_progressive_reader_zero_size() {
        let reader = ProgressiveReader::new(Cursor::new(Vec::new()), 0);
        assert!((reader.progress() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_progressive_reader_with_callback() {
        let data = vec![0u8; 100];
        let mut progress_updates = Vec::new();

        {
            let callback = |bytes: u64, total: u64| {
                progress_updates.push((bytes, total));
            };

            let mut reader = ProgressiveReaderWithCallback::new(Cursor::new(data), 100, callback);

            let mut buf = [0u8; 25];
            for _ in 0..4 {
                let _ = reader.read(&mut buf).unwrap();
            }
        }

        assert_eq!(progress_updates.len(), 4);
        assert_eq!(progress_updates[0], (25, 100));
        assert_eq!(progress_updates[3], (100, 100));
    }

    #[test]
    fn test_bounded_reader() {
        let data = vec![1u8; 1000];
        let mut reader = BoundedReader::new(Cursor::new(data), 100);

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();

        assert_eq!(buf.len(), 100);
        assert!(reader.is_exhausted());
    }

    #[test]
    fn test_bounded_reader_partial() {
        let data = vec![1u8; 1000];
        let mut reader = BoundedReader::new(Cursor::new(data), 100);

        let mut buf = [0u8; 30];
        let _ = reader.read(&mut buf).unwrap();

        assert_eq!(reader.bytes_read(), 30);
        assert_eq!(reader.remaining(), 70);
        assert!(!reader.is_exhausted());
    }

    #[test]
    fn test_chained_reader() {
        let r1: Box<dyn Read + Send> = Box::new(Cursor::new(vec![1u8; 10]));
        let r2: Box<dyn Read + Send> = Box::new(Cursor::new(vec![2u8; 10]));
        let r3: Box<dyn Read + Send> = Box::new(Cursor::new(vec![3u8; 10]));

        let readers = vec![r1, r2, r3].into_iter();
        let mut chained = ChainedReader::new(readers);

        let mut buf = Vec::new();
        chained.read_to_end(&mut buf).unwrap();

        assert_eq!(buf.len(), 30);
        assert!(buf[..10].iter().all(|&b| b == 1));
        assert!(buf[10..20].iter().all(|&b| b == 2));
        assert!(buf[20..].iter().all(|&b| b == 3));
    }
}
