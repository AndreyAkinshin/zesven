//! Write trait sink support for streaming extraction.
//!
//! This module provides utilities for extracting entries to various
//! Write implementations, including bounded and hashing sinks.

use std::io::{self, Write};

use crate::Result;

/// Trait for extracting entries to Write sinks.
///
/// This trait provides methods for extracting archive entries directly
/// to any type implementing `Write`, with optional progress callbacks.
pub trait ExtractToSink<R> {
    /// Extracts an entry to any Write implementation.
    fn extract_to<W: Write>(&mut self, entry_index: usize, sink: &mut W) -> Result<u64>;

    /// Extracts an entry with progress callback.
    fn extract_to_with_progress<W, F>(
        &mut self,
        entry_index: usize,
        sink: &mut W,
        on_progress: F,
    ) -> Result<u64>
    where
        W: Write,
        F: FnMut(u64, u64); // (bytes_written, total_bytes)
}

/// In-memory Vec sink with size limit.
///
/// This sink collects data into a Vec while enforcing a maximum size limit,
/// useful for extracting entries with memory constraints.
#[derive(Debug)]
pub struct BoundedVecSink {
    data: Vec<u8>,
    max_size: usize,
    bytes_written: u64,
}

impl BoundedVecSink {
    /// Creates a new bounded Vec sink with the specified maximum size.
    pub fn new(max_size: usize) -> Self {
        Self {
            data: Vec::new(),
            max_size,
            bytes_written: 0,
        }
    }

    /// Creates a new bounded Vec sink with pre-allocated capacity.
    pub fn with_capacity(capacity: usize, max_size: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity.min(max_size)),
            max_size,
            bytes_written: 0,
        }
    }

    /// Returns the collected data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Consumes the sink and returns the collected data.
    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns the maximum size.
    pub fn max_size(&self) -> usize {
        self.max_size
    }

    /// Returns the remaining capacity.
    pub fn remaining(&self) -> usize {
        self.max_size.saturating_sub(self.data.len())
    }

    /// Clears the collected data.
    pub fn clear(&mut self) {
        self.data.clear();
        self.bytes_written = 0;
    }
}

impl Write for BoundedVecSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let remaining = self.remaining();
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "bounded sink size limit reached",
            ));
        }

        let to_write = buf.len().min(remaining);
        self.data.extend_from_slice(&buf[..to_write]);
        self.bytes_written += to_write as u64;

        if to_write < buf.len() {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "bounded sink size limit reached",
            ))
        } else {
            Ok(to_write)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Streaming hash computation sink.
///
/// This sink computes a CRC32 hash while discarding the data,
/// useful for verifying entry integrity without storing the data.
#[derive(Debug)]
pub struct Crc32Sink {
    hasher: crc32fast::Hasher,
    bytes_processed: u64,
}

impl Crc32Sink {
    /// Creates a new CRC32 hashing sink.
    pub fn new() -> Self {
        Self {
            hasher: crc32fast::Hasher::new(),
            bytes_processed: 0,
        }
    }

    /// Returns the computed CRC32 value.
    pub fn finalize(self) -> u32 {
        self.hasher.finalize()
    }

    /// Returns the current CRC32 value without consuming the sink.
    pub fn crc(&self) -> u32 {
        self.hasher.clone().finalize()
    }

    /// Returns the number of bytes processed.
    pub fn bytes_processed(&self) -> u64 {
        self.bytes_processed
    }

    /// Resets the hasher.
    pub fn reset(&mut self) {
        self.hasher = crc32fast::Hasher::new();
        self.bytes_processed = 0;
    }
}

impl Default for Crc32Sink {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Crc32Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.update(buf);
        self.bytes_processed += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Null sink for skip operations.
///
/// This sink discards all data written to it, equivalent to `io::sink()`
/// but with byte counting.
#[derive(Debug, Default)]
pub struct NullSink {
    bytes_discarded: u64,
}

impl NullSink {
    /// Creates a new null sink.
    pub fn new() -> Self {
        Self { bytes_discarded: 0 }
    }

    /// Returns the number of bytes discarded.
    pub fn bytes_discarded(&self) -> u64 {
        self.bytes_discarded
    }

    /// Resets the byte counter.
    pub fn reset(&mut self) {
        self.bytes_discarded = 0;
    }
}

impl Write for NullSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes_discarded += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Counting sink that wraps another Write and counts bytes.
#[derive(Debug)]
pub struct CountingSink<W> {
    inner: W,
    bytes_written: u64,
}

impl<W: Write> CountingSink<W> {
    /// Creates a new counting sink wrapping the given writer.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Returns a mutable reference to the inner writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consumes the sink and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Resets the byte counter.
    pub fn reset_counter(&mut self) {
        self.bytes_written = 0;
    }
}

impl<W: Write> Write for CountingSink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Progress-reporting sink that calls a callback periodically.
pub struct ProgressSink<W, F> {
    inner: W,
    callback: F,
    bytes_written: u64,
    total_bytes: Option<u64>,
    /// Bytes written since last callback
    bytes_since_callback: u64,
    /// Callback threshold (minimum bytes between callbacks)
    threshold: u64,
}

impl<W: Write, F: FnMut(u64, Option<u64>)> ProgressSink<W, F> {
    /// Creates a new progress sink.
    pub fn new(inner: W, callback: F) -> Self {
        Self {
            inner,
            callback,
            bytes_written: 0,
            total_bytes: None,
            bytes_since_callback: 0,
            threshold: 0,
        }
    }

    /// Sets the expected total bytes for progress calculation.
    pub fn with_total(mut self, total: u64) -> Self {
        self.total_bytes = Some(total);
        self
    }

    /// Sets the callback threshold (minimum bytes between callbacks).
    pub fn with_threshold(mut self, threshold: u64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns the progress as a fraction (0.0 to 1.0), if total is known.
    pub fn progress(&self) -> Option<f64> {
        self.total_bytes.map(|total| {
            if total == 0 {
                1.0
            } else {
                self.bytes_written as f64 / total as f64
            }
        })
    }

    /// Consumes the sink and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write, F: FnMut(u64, Option<u64>)> Write for ProgressSink<W, F> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n as u64;
        self.bytes_since_callback += n as u64;

        if self.bytes_since_callback >= self.threshold {
            (self.callback)(self.bytes_written, self.total_bytes);
            self.bytes_since_callback = 0;
        }

        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Tee sink that writes to two writers simultaneously.
pub struct TeeSink<W1, W2> {
    sink1: W1,
    sink2: W2,
    bytes_written: u64,
}

impl<W1: Write, W2: Write> TeeSink<W1, W2> {
    /// Creates a new tee sink.
    pub fn new(sink1: W1, sink2: W2) -> Self {
        Self {
            sink1,
            sink2,
            bytes_written: 0,
        }
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Consumes the sink and returns both inner writers.
    pub fn into_inner(self) -> (W1, W2) {
        (self.sink1, self.sink2)
    }
}

impl<W1: Write, W2: Write> Write for TeeSink<W1, W2> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Write to first sink
        let n1 = self.sink1.write(buf)?;

        // Write the same amount to second sink
        self.sink2.write_all(&buf[..n1])?;

        self.bytes_written += n1 as u64;
        Ok(n1)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink1.flush()?;
        self.sink2.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_vec_sink() {
        let mut sink = BoundedVecSink::new(100);

        sink.write_all(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(sink.data(), &[1, 2, 3, 4, 5]);
        assert_eq!(sink.bytes_written(), 5);
        assert_eq!(sink.remaining(), 95);
    }

    #[test]
    fn test_bounded_vec_sink_limit() {
        let mut sink = BoundedVecSink::new(5);

        let result = sink.write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(result.is_err());
        assert_eq!(sink.data().len(), 5);
    }

    #[test]
    fn test_crc32_sink() {
        let mut sink = Crc32Sink::new();

        sink.write_all(b"hello world").unwrap();

        let crc = sink.finalize();
        assert_eq!(crc, crc32fast::hash(b"hello world"));
    }

    #[test]
    fn test_null_sink() {
        let mut sink = NullSink::new();

        sink.write_all(&[0u8; 1000]).unwrap();
        assert_eq!(sink.bytes_discarded(), 1000);
    }

    #[test]
    fn test_counting_sink() {
        let mut sink = CountingSink::new(Vec::new());

        sink.write_all(&[1, 2, 3]).unwrap();
        sink.write_all(&[4, 5, 6, 7]).unwrap();

        assert_eq!(sink.bytes_written(), 7);
        assert_eq!(sink.into_inner(), vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_progress_sink() {
        let mut progress_values = Vec::new();

        {
            let callback = |bytes: u64, _total: Option<u64>| {
                progress_values.push(bytes);
            };

            let mut sink = ProgressSink::new(Vec::new(), callback);

            sink.write_all(&[1, 2, 3]).unwrap();
            sink.write_all(&[4, 5, 6]).unwrap();
        }

        assert_eq!(progress_values, vec![3, 6]);
    }

    #[test]
    fn test_tee_sink() {
        let mut sink = TeeSink::new(Vec::new(), Vec::new());

        sink.write_all(&[1, 2, 3]).unwrap();

        let (v1, v2) = sink.into_inner();
        assert_eq!(v1, vec![1, 2, 3]);
        assert_eq!(v2, vec![1, 2, 3]);
    }

    #[test]
    fn test_crc32_sink_reset() {
        let mut sink = Crc32Sink::new();

        sink.write_all(b"test").unwrap();
        let crc1 = sink.crc();

        sink.reset();
        assert_eq!(sink.bytes_processed(), 0);

        sink.write_all(b"test").unwrap();
        let crc2 = sink.finalize();

        assert_eq!(crc1, crc2);
    }
}
