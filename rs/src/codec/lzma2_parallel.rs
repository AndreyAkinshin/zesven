//! Parallel LZMA2 encoder for improved compression speed.
//!
//! This module provides a block-based parallel LZMA2 encoder that can achieve
//! 2-4x speedup on multi-core systems while producing fully compatible output.
//!
//! # How it works
//!
//! The encoder splits input data into independent blocks, compresses each block
//! in parallel using multiple threads, then concatenates the results into a
//! valid LZMA2 stream.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::codec::lzma2_parallel::{ParallelLzma2Encoder, ParallelLzma2Options};
//!
//! let options = ParallelLzma2Options::default()
//!     .level(6)
//!     .threads(4);
//!
//! let data = b"Hello, World! ".repeat(10000);
//! let compressed = ParallelLzma2Encoder::compress(&data, &options)?;
//! ```

use std::io::{self, Write};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use super::Encoder;
use super::lzma::{Lzma2Encoder, Lzma2EncoderOptions, encode_lzma2_dict_size};
use crate::{Error, Result};

/// Default block size for parallel compression (4 MB).
pub const DEFAULT_BLOCK_SIZE: usize = 4 * 1024 * 1024;

/// Minimum block size (64 KB).
pub const MIN_BLOCK_SIZE: usize = 64 * 1024;

/// Options for parallel LZMA2 encoding.
#[derive(Debug, Clone)]
pub struct ParallelLzma2Options {
    /// Compression level (0-9, default 6).
    pub level: u32,
    /// Dictionary size in bytes (optional, derived from level if None).
    pub dict_size: Option<u32>,
    /// Number of threads to use (None = auto-detect).
    pub threads: Option<usize>,
    /// Block size for parallel compression.
    pub block_size: usize,
}

impl Default for ParallelLzma2Options {
    fn default() -> Self {
        Self {
            level: 6,
            dict_size: None,
            threads: None,
            block_size: DEFAULT_BLOCK_SIZE,
        }
    }
}

impl ParallelLzma2Options {
    /// Creates new options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the compression level (0-9).
    pub fn level(mut self, level: u32) -> Self {
        self.level = level.min(9);
        self
    }

    /// Sets a custom dictionary size.
    pub fn dict_size(mut self, size: u32) -> Self {
        self.dict_size = Some(size);
        self
    }

    /// Sets the number of threads to use.
    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = Some(threads.max(1));
        self
    }

    /// Sets the block size for parallel compression.
    pub fn block_size(mut self, size: usize) -> Self {
        self.block_size = size.max(MIN_BLOCK_SIZE);
        self
    }

    /// Returns the effective dictionary size for this configuration.
    pub fn effective_dict_size(&self) -> u32 {
        self.dict_size.unwrap_or(
            // Default dictionary sizes by level (similar to 7-Zip)
            match self.level {
                0 => 64 * 1024,        // 64 KB
                1 => 256 * 1024,       // 256 KB
                2 => 1024 * 1024,      // 1 MB
                3 => 2 * 1024 * 1024,  // 2 MB
                4 => 4 * 1024 * 1024,  // 4 MB
                5 => 8 * 1024 * 1024,  // 8 MB
                6 => 16 * 1024 * 1024, // 16 MB
                7 => 32 * 1024 * 1024, // 32 MB
                8 => 64 * 1024 * 1024, // 64 MB
                _ => 64 * 1024 * 1024, // 64 MB (level 9)
            },
        )
    }

    /// Returns the effective number of threads.
    pub fn effective_threads(&self) -> usize {
        self.threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
    }

    /// Returns LZMA2 properties (1 byte: encoded dict size).
    pub fn properties(&self) -> Vec<u8> {
        vec![encode_lzma2_dict_size(self.effective_dict_size())]
    }

    /// Converts to serial encoder options.
    fn to_serial_options(&self) -> Lzma2EncoderOptions {
        let mut opts = Lzma2EncoderOptions::with_preset(self.level);
        if let Some(dict_size) = self.dict_size {
            opts = opts.with_dict_size(dict_size);
        }
        opts
    }
}

/// Result of parallel compression.
#[derive(Debug, Clone)]
pub struct Lzma2CompressionResult {
    /// The compressed data.
    pub data: Vec<u8>,
    /// Number of blocks compressed.
    pub blocks: usize,
    /// Original uncompressed size.
    pub uncompressed_size: u64,
    /// Compressed size.
    pub compressed_size: u64,
}

impl Lzma2CompressionResult {
    /// Returns the compression ratio (compressed / uncompressed).
    pub fn ratio(&self) -> f64 {
        if self.uncompressed_size == 0 {
            1.0
        } else {
            self.compressed_size as f64 / self.uncompressed_size as f64
        }
    }

    /// Returns the space savings as a percentage.
    pub fn space_savings(&self) -> f64 {
        1.0 - self.ratio()
    }
}

/// Parallel LZMA2 encoder.
///
/// This encoder splits data into blocks and compresses them in parallel,
/// producing output compatible with standard LZMA2 decoders.
#[derive(Debug)]
pub struct ParallelLzma2Encoder {
    options: ParallelLzma2Options,
}

impl ParallelLzma2Encoder {
    /// Creates a new parallel encoder with the given options.
    pub fn new(options: ParallelLzma2Options) -> Self {
        Self { options }
    }

    /// Creates a new encoder with default options.
    pub fn with_defaults() -> Self {
        Self::new(ParallelLzma2Options::default())
    }

    /// Returns the LZMA2 properties for this encoder.
    pub fn properties(&self) -> Vec<u8> {
        self.options.properties()
    }

    /// Compresses data using parallel block encoding.
    ///
    /// This is the main entry point for parallel compression.
    #[cfg(feature = "parallel")]
    pub fn compress(&self, data: &[u8]) -> Result<Lzma2CompressionResult> {
        if data.is_empty() {
            return Ok(Lzma2CompressionResult {
                data: vec![0x00], // LZMA2 end marker
                blocks: 0,
                uncompressed_size: 0,
                compressed_size: 1,
            });
        }

        // Split into blocks
        let blocks: Vec<&[u8]> = data.chunks(self.options.block_size).collect();
        let num_blocks = blocks.len();

        // Compress blocks in parallel
        let serial_opts = self.options.to_serial_options();
        let compressed_blocks: Vec<Result<Vec<u8>>> = blocks
            .par_iter()
            .map(|block| compress_block(block, &serial_opts))
            .collect();

        // Check for errors and concatenate results
        let mut result = Vec::new();
        for block_result in compressed_blocks {
            let block_data = block_result?;
            result.extend_from_slice(&block_data);
        }

        // Add LZMA2 end marker
        result.push(0x00);

        Ok(Lzma2CompressionResult {
            compressed_size: result.len() as u64,
            data: result,
            blocks: num_blocks,
            uncompressed_size: data.len() as u64,
        })
    }

    /// Compresses data using serial encoding (fallback when parallel is disabled).
    #[cfg(not(feature = "parallel"))]
    pub fn compress(&self, data: &[u8]) -> Result<Lzma2CompressionResult> {
        // Fall back to serial compression
        let serial_opts = self.options.to_serial_options();
        let mut compressed = Vec::new();
        {
            let mut encoder =
                Lzma2Encoder::new(std::io::Cursor::new(&mut compressed), &serial_opts);
            encoder.write_all(data).map_err(Error::Io)?;
            Box::new(encoder).finish().map_err(Error::Io)?;
        }

        Ok(Lzma2CompressionResult {
            compressed_size: compressed.len() as u64,
            data: compressed,
            blocks: 1,
            uncompressed_size: data.len() as u64,
        })
    }

    /// Convenience function to compress data with default options.
    pub fn compress_default(data: &[u8]) -> Result<Vec<u8>> {
        let encoder = Self::with_defaults();
        let result = encoder.compress(data)?;
        Ok(result.data)
    }

    /// Convenience function to compress data with a specific level.
    pub fn compress_level(data: &[u8], level: u32) -> Result<Vec<u8>> {
        let encoder = Self::new(ParallelLzma2Options::default().level(level));
        let result = encoder.compress(data)?;
        Ok(result.data)
    }
}

/// Compresses a single block using LZMA2.
///
/// The output is a valid LZMA2 block that can be concatenated with other blocks.
fn compress_block(data: &[u8], options: &Lzma2EncoderOptions) -> Result<Vec<u8>> {
    let mut compressed = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut compressed);
        let mut encoder = Lzma2Encoder::new(cursor, options);
        encoder.write_all(data).map_err(Error::Io)?;
        // Don't call finish - we want raw LZMA2 blocks without end marker
        // The encoder will flush on drop, but we need proper termination
    }

    // Use a separate approach: compress to buffer with finish
    let mut compressed = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut compressed);
        let mut encoder = Lzma2Encoder::new(cursor, options);
        encoder.write_all(data).map_err(Error::Io)?;
        Box::new(encoder).finish().map_err(Error::Io)?;
    }

    // Remove the trailing end marker (0x00) since we'll add one at the end
    if compressed.last() == Some(&0x00) {
        compressed.pop();
    }

    Ok(compressed)
}

/// Streaming parallel LZMA2 encoder.
///
/// This encoder buffers data and compresses it in blocks when the buffer
/// reaches the block size threshold.
#[cfg(feature = "parallel")]
pub struct StreamingParallelLzma2Encoder<W: Write + Send> {
    output: W,
    options: ParallelLzma2Options,
    buffer: Vec<u8>,
    total_written: u64,
}

#[cfg(feature = "parallel")]
impl<W: Write + Send> StreamingParallelLzma2Encoder<W> {
    /// Creates a new streaming parallel encoder.
    pub fn new(output: W, options: ParallelLzma2Options) -> Self {
        Self {
            output,
            options,
            buffer: Vec::new(),
            total_written: 0,
        }
    }

    /// Returns LZMA2 properties for this encoder.
    pub fn properties(&self) -> Vec<u8> {
        self.options.properties()
    }

    /// Flushes the internal buffer, compressing and writing data.
    fn flush_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let encoder = ParallelLzma2Encoder::new(self.options.clone());
        let result = encoder
            .compress(&self.buffer)
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Write compressed data (without end marker for streaming)
        let data_without_marker = if result.data.last() == Some(&0x00) {
            &result.data[..result.data.len() - 1]
        } else {
            &result.data
        };

        self.output.write_all(data_without_marker)?;
        self.total_written += data_without_marker.len() as u64;
        self.buffer.clear();

        Ok(())
    }

    /// Finishes encoding and writes any remaining data.
    pub fn finish(mut self) -> io::Result<W> {
        self.flush_buffer()?;
        // Write end marker
        self.output.write_all(&[0x00])?;
        Ok(self.output)
    }

    /// Returns the total bytes written so far.
    pub fn bytes_written(&self) -> u64 {
        self.total_written
    }
}

#[cfg(feature = "parallel")]
impl<W: Write + Send> Write for StreamingParallelLzma2Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Flush when buffer exceeds threshold
        if self.buffer.len() >= self.options.block_size * 2 {
            self.flush_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Don't flush buffer on regular flush - only on finish
        self.output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::lzma::Lzma2Decoder;
    use std::io::Read;

    #[test]
    fn test_parallel_options_default() {
        let opts = ParallelLzma2Options::default();
        assert_eq!(opts.level, 6);
        assert!(opts.dict_size.is_none());
        assert!(opts.threads.is_none());
        assert_eq!(opts.block_size, DEFAULT_BLOCK_SIZE);
    }

    #[test]
    fn test_parallel_options_builder() {
        let opts = ParallelLzma2Options::new()
            .level(9)
            .dict_size(32 * 1024 * 1024)
            .threads(8)
            .block_size(8 * 1024 * 1024);

        assert_eq!(opts.level, 9);
        assert_eq!(opts.dict_size, Some(32 * 1024 * 1024));
        assert_eq!(opts.threads, Some(8));
        assert_eq!(opts.block_size, 8 * 1024 * 1024);
    }

    #[test]
    fn test_effective_dict_size() {
        let opts = ParallelLzma2Options::new().level(5);
        assert_eq!(opts.effective_dict_size(), 8 * 1024 * 1024);

        let opts_custom = ParallelLzma2Options::new().dict_size(1024 * 1024);
        assert_eq!(opts_custom.effective_dict_size(), 1024 * 1024);
    }

    #[test]
    fn test_parallel_compression_empty() {
        let encoder = ParallelLzma2Encoder::with_defaults();
        let result = encoder.compress(&[]).unwrap();
        assert_eq!(result.blocks, 0);
        assert_eq!(result.uncompressed_size, 0);
        assert_eq!(result.data, vec![0x00]); // End marker only
    }

    #[test]
    fn test_parallel_compression_small() {
        let data = b"Hello, World!";
        let encoder = ParallelLzma2Encoder::new(ParallelLzma2Options::new().level(0));
        let result = encoder.compress(data).unwrap();

        assert_eq!(result.blocks, 1);
        assert_eq!(result.uncompressed_size, data.len() as u64);
        assert!(result.compressed_size > 0);
    }

    #[test]
    fn test_parallel_compression_roundtrip() {
        let data = b"Hello, World! This is a test. ".repeat(1000);
        let encoder = ParallelLzma2Encoder::new(ParallelLzma2Options::new().level(1));

        let result = encoder.compress(&data).unwrap();
        let props = encoder.properties();

        // Decompress
        let cursor = std::io::Cursor::new(&result.data);
        let mut decoder = Lzma2Decoder::new(cursor, &props).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_parallel_compression_large() {
        // Create data larger than block size
        let data = vec![0u8; 5 * 1024 * 1024]; // 5 MB
        let encoder = ParallelLzma2Encoder::new(
            ParallelLzma2Options::new().level(0).block_size(1024 * 1024), // 1 MB blocks
        );

        let result = encoder.compress(&data).unwrap();

        // Should have multiple blocks
        assert!(result.blocks >= 5);
        assert_eq!(result.uncompressed_size, data.len() as u64);
    }

    #[test]
    fn test_compress_default() {
        let data = b"Test data for compression";
        let compressed = ParallelLzma2Encoder::compress_default(data).unwrap();
        assert!(!compressed.is_empty());
    }

    #[test]
    fn test_compress_level() {
        let data = b"Test data for compression with level";
        let compressed = ParallelLzma2Encoder::compress_level(data, 3).unwrap();
        assert!(!compressed.is_empty());
    }

    #[test]
    fn test_compression_result_metrics() {
        let result = Lzma2CompressionResult {
            data: vec![],
            blocks: 10,
            uncompressed_size: 1000,
            compressed_size: 500,
        };

        assert!((result.ratio() - 0.5).abs() < 0.001);
        assert!((result.space_savings() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_compression_result_empty() {
        let result = Lzma2CompressionResult {
            data: vec![],
            blocks: 0,
            uncompressed_size: 0,
            compressed_size: 0,
        };

        assert!((result.ratio() - 1.0).abs() < 0.001);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn test_streaming_encoder() {
        use std::io::Cursor;

        let mut output = Vec::new();
        let opts = ParallelLzma2Options::new().level(0).block_size(1024);

        {
            let cursor = Cursor::new(&mut output);
            let mut encoder = StreamingParallelLzma2Encoder::new(cursor, opts.clone());

            // Write some data
            encoder.write_all(b"Hello, World!").unwrap();
            encoder.finish().unwrap();
        }

        // Verify we can decompress
        let cursor = Cursor::new(&output);
        let props = opts.properties();
        let mut decoder = Lzma2Decoder::new(cursor, &props).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, b"Hello, World!");
    }

    #[test]
    fn test_properties() {
        let opts = ParallelLzma2Options::new().level(6);
        let props = opts.properties();

        assert_eq!(props.len(), 1);
        // Level 6 = 16MB dict = prop value around 24
        assert!(props[0] > 0);
    }
}
