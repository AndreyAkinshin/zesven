//! Parallel frame-based compression/decompression.
//!
//! This module provides frame-based compression that enables parallel processing
//! by splitting data into independently-compressed frames. Each frame can be
//! compressed or decompressed in parallel.
//!
//! # Frame Format
//!
//! ```text
//! +------------------+
//! | Frame Header     |  <- Magic + frame count + frame index
//! +------------------+
//! | Frame 0 data     |  <- Compressed frame data
//! +------------------+
//! | Frame 1 data     |
//! +------------------+
//! | ...              |
//! +------------------+
//! ```
//!
//! Each frame index entry contains:
//! - Compressed size (varint)
//! - Uncompressed size (varint)
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::codec::parallel_frames::{ParallelFrameEncoder, ParallelFrameDecoder, FrameCodec};
//!
//! // Compress data into frames
//! let encoder = ParallelFrameEncoder::new(FrameCodec::Lzma2, 5)
//!     .frame_size(1024 * 1024);  // 1 MB frames
//!
//! let result = encoder.compress(&large_data)?;
//! println!("Compressed {} frames", result.frame_count);
//!
//! // Decompress frames in parallel
//! let decoder = ParallelFrameDecoder::new();
//! let decompressed = decoder.decompress(&result.data)?;
//! ```

#[allow(unused_imports)]
use std::io::{self, Read, Write};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Magic bytes for parallel frame format.
pub const FRAME_MAGIC: &[u8; 4] = b"PF7Z";

/// Codec to use for frame compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameCodec {
    /// LZMA2 compression.
    #[cfg(feature = "lzma")]
    Lzma2,
    /// Zstd compression.
    #[cfg(feature = "zstd")]
    Zstd,
    /// LZ4 compression (fast).
    #[cfg(feature = "lz4")]
    Lz4,
    /// Brotli compression.
    #[cfg(feature = "brotli")]
    Brotli,
    /// No compression (copy).
    Copy,
}

impl FrameCodec {
    /// Returns a human-readable name for this codec.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "lzma")]
            Self::Lzma2 => "LZMA2",
            #[cfg(feature = "zstd")]
            Self::Zstd => "Zstd",
            #[cfg(feature = "lz4")]
            Self::Lz4 => "LZ4",
            #[cfg(feature = "brotli")]
            Self::Brotli => "Brotli",
            Self::Copy => "Copy",
        }
    }

    /// Compress a single frame.
    #[allow(unused_variables)]
    fn compress_frame(&self, data: &[u8], level: i32) -> io::Result<Vec<u8>> {
        match self {
            #[cfg(feature = "lzma")]
            Self::Lzma2 => {
                use lzma_rust2::{Lzma2Options, Lzma2Writer};
                let mut output = Vec::new();
                let options = Lzma2Options::with_preset(level as u32);
                let mut writer = Lzma2Writer::new(&mut output, options);
                writer.write_all(data)?;
                writer.finish()?;
                Ok(output)
            }
            #[cfg(feature = "zstd")]
            Self::Zstd => zstd::encode_all(data, level).map_err(io::Error::other),
            #[cfg(feature = "lz4")]
            Self::Lz4 => {
                use lz4_flex::frame::FrameEncoder;
                let mut output = Vec::new();
                let mut encoder = FrameEncoder::new(&mut output);
                encoder.write_all(data)?;
                encoder.finish()?;
                Ok(output)
            }
            #[cfg(feature = "brotli")]
            Self::Brotli => {
                let mut output = Vec::new();
                let mut encoder =
                    brotli::CompressorWriter::new(&mut output, 4096, level as u32, 22);
                encoder.write_all(data)?;
                drop(encoder);
                Ok(output)
            }
            Self::Copy => Ok(data.to_vec()),
        }
    }

    /// Decompress a single frame.
    fn decompress_frame(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            #[cfg(feature = "lzma")]
            Self::Lzma2 => {
                use lzma_rust2::Lzma2Reader;
                let mut output = Vec::new();
                // Use max dictionary size (LZMA2 dictionaries are limited to ~4GB)
                let mut reader = Lzma2Reader::new(std::io::Cursor::new(data), u32::MAX, None);
                reader.read_to_end(&mut output)?;
                Ok(output)
            }
            #[cfg(feature = "zstd")]
            Self::Zstd => zstd::decode_all(data).map_err(io::Error::other),
            #[cfg(feature = "lz4")]
            Self::Lz4 => {
                use lz4_flex::frame::FrameDecoder;
                let mut output = Vec::new();
                let mut decoder = FrameDecoder::new(std::io::Cursor::new(data));
                decoder.read_to_end(&mut output)?;
                Ok(output)
            }
            #[cfg(feature = "brotli")]
            Self::Brotli => {
                let mut output = Vec::new();
                let mut decoder = brotli::Decompressor::new(std::io::Cursor::new(data), 4096);
                decoder.read_to_end(&mut output)?;
                Ok(output)
            }
            Self::Copy => Ok(data.to_vec()),
        }
    }
}

/// Information about a single frame.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    /// Offset of this frame in the compressed data.
    pub offset: u64,
    /// Compressed size of this frame.
    pub compressed_size: u64,
    /// Uncompressed size of this frame.
    pub uncompressed_size: u64,
}

/// Index of frames in a parallel frame archive.
#[derive(Debug, Clone)]
pub struct FrameIndex {
    /// Codec used for compression.
    pub codec: u8,
    /// Compression level used.
    pub level: i32,
    /// Information about each frame.
    pub frames: Vec<FrameInfo>,
}

impl FrameIndex {
    /// Returns the total compressed size.
    pub fn total_compressed_size(&self) -> u64 {
        self.frames.iter().map(|f| f.compressed_size).sum()
    }

    /// Returns the total uncompressed size.
    pub fn total_uncompressed_size(&self) -> u64 {
        self.frames.iter().map(|f| f.uncompressed_size).sum()
    }

    /// Returns the compression ratio (compressed / uncompressed).
    pub fn compression_ratio(&self) -> f64 {
        let uncompressed = self.total_uncompressed_size();
        if uncompressed == 0 {
            1.0
        } else {
            self.total_compressed_size() as f64 / uncompressed as f64
        }
    }
}

/// Result of parallel frame compression.
#[derive(Debug)]
pub struct FrameCompressionResult {
    /// Compressed data including header and all frames.
    pub data: Vec<u8>,
    /// Frame index information.
    pub index: FrameIndex,
    /// Number of frames.
    pub frame_count: usize,
    /// Total uncompressed size.
    pub uncompressed_size: u64,
    /// Total compressed size (data only, excluding header).
    pub compressed_size: u64,
}

impl FrameCompressionResult {
    /// Returns the compression ratio.
    pub fn compression_ratio(&self) -> f64 {
        if self.uncompressed_size == 0 {
            1.0
        } else {
            self.compressed_size as f64 / self.uncompressed_size as f64
        }
    }

    /// Returns the space savings as a percentage (0.0-1.0).
    pub fn space_savings(&self) -> f64 {
        1.0 - self.compression_ratio()
    }
}

/// Encoder for parallel frame compression.
#[derive(Debug, Clone)]
pub struct ParallelFrameEncoder {
    codec: FrameCodec,
    level: i32,
    frame_size: usize,
}

impl ParallelFrameEncoder {
    /// Creates a new parallel frame encoder.
    pub fn new(codec: FrameCodec, level: i32) -> Self {
        Self {
            codec,
            level: level.clamp(1, 22),
            frame_size: 4 * 1024 * 1024, // 4 MB default
        }
    }

    /// Sets the frame size in bytes.
    pub fn frame_size(mut self, size: usize) -> Self {
        self.frame_size = size.max(1024); // Minimum 1 KB
        self
    }

    /// Compresses data into parallel frames.
    #[cfg(feature = "parallel")]
    pub fn compress(&self, data: &[u8]) -> io::Result<FrameCompressionResult> {
        if data.is_empty() {
            return Ok(self.create_empty_result());
        }

        // Split data into chunks
        let chunks: Vec<&[u8]> = data.chunks(self.frame_size).collect();
        let frame_count = chunks.len();

        // Compress chunks in parallel
        let compressed_frames: Vec<io::Result<Vec<u8>>> = chunks
            .par_iter()
            .map(|chunk| self.codec.compress_frame(chunk, self.level))
            .collect();

        // Collect results, propagating any errors
        let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frame_count);
        for result in compressed_frames {
            frames.push(result?);
        }

        self.build_result(data, &chunks, frames)
    }

    /// Compresses data into parallel frames (single-threaded fallback).
    #[cfg(not(feature = "parallel"))]
    pub fn compress(&self, data: &[u8]) -> io::Result<FrameCompressionResult> {
        if data.is_empty() {
            return Ok(self.create_empty_result());
        }

        // Split data into chunks
        let chunks: Vec<&[u8]> = data.chunks(self.frame_size).collect();
        let frame_count = chunks.len();

        // Compress chunks sequentially
        let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frame_count);
        for chunk in &chunks {
            frames.push(self.codec.compress_frame(chunk, self.level)?);
        }

        self.build_result(data, &chunks, frames)
    }

    fn create_empty_result(&self) -> FrameCompressionResult {
        let mut data = Vec::new();
        data.extend_from_slice(FRAME_MAGIC);
        data.push(self.codec_id());
        data.push(self.level as u8);
        write_varint(&mut data, 0); // 0 frames

        FrameCompressionResult {
            data,
            index: FrameIndex {
                codec: self.codec_id(),
                level: self.level,
                frames: Vec::new(),
            },
            frame_count: 0,
            uncompressed_size: 0,
            compressed_size: 0,
        }
    }

    fn build_result(
        &self,
        original_data: &[u8],
        chunks: &[&[u8]],
        compressed_frames: Vec<Vec<u8>>,
    ) -> io::Result<FrameCompressionResult> {
        let frame_count = compressed_frames.len();
        let uncompressed_size = original_data.len() as u64;
        let compressed_size: u64 = compressed_frames.iter().map(|f| f.len() as u64).sum();

        // Build header
        let mut data = Vec::new();
        data.extend_from_slice(FRAME_MAGIC);
        data.push(self.codec_id());
        data.push(self.level as u8);
        write_varint(&mut data, frame_count as u64);

        // Write frame index
        let mut frame_infos = Vec::with_capacity(frame_count);
        let mut offset = 0u64;

        for (i, frame) in compressed_frames.iter().enumerate() {
            let compressed_len = frame.len() as u64;
            let uncompressed_len = chunks[i].len() as u64;

            write_varint(&mut data, compressed_len);
            write_varint(&mut data, uncompressed_len);

            frame_infos.push(FrameInfo {
                offset,
                compressed_size: compressed_len,
                uncompressed_size: uncompressed_len,
            });

            offset += compressed_len;
        }

        // Write frame data
        for frame in compressed_frames {
            data.extend_from_slice(&frame);
        }

        Ok(FrameCompressionResult {
            data,
            index: FrameIndex {
                codec: self.codec_id(),
                level: self.level,
                frames: frame_infos,
            },
            frame_count,
            uncompressed_size,
            compressed_size,
        })
    }

    fn codec_id(&self) -> u8 {
        match self.codec {
            #[cfg(feature = "lzma")]
            FrameCodec::Lzma2 => 0x21,
            #[cfg(feature = "zstd")]
            FrameCodec::Zstd => 0x01,
            #[cfg(feature = "lz4")]
            FrameCodec::Lz4 => 0x04,
            #[cfg(feature = "brotli")]
            FrameCodec::Brotli => 0x02,
            FrameCodec::Copy => 0x00,
        }
    }
}

/// Decoder for parallel frame decompression.
#[derive(Debug, Clone, Default)]
pub struct ParallelFrameDecoder;

impl ParallelFrameDecoder {
    /// Creates a new parallel frame decoder.
    pub fn new() -> Self {
        Self
    }

    /// Decompresses parallel frames back to original data.
    #[cfg(feature = "parallel")]
    pub fn decompress(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        let (index, frame_data) = self.parse_header(data)?;

        if index.frames.is_empty() {
            return Ok(Vec::new());
        }

        let codec = self.codec_from_id(index.codec)?;

        // Extract individual frame data
        let mut frame_slices: Vec<&[u8]> = Vec::with_capacity(index.frames.len());
        let mut pos = 0usize;

        for frame_info in &index.frames {
            let end = pos + frame_info.compressed_size as usize;
            if end > frame_data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame data truncated",
                ));
            }
            frame_slices.push(&frame_data[pos..end]);
            pos = end;
        }

        // Decompress frames in parallel
        let decompressed_frames: Vec<io::Result<Vec<u8>>> = frame_slices
            .par_iter()
            .map(|frame| codec.decompress_frame(frame))
            .collect();

        // Collect results and concatenate
        let mut output = Vec::with_capacity(index.total_uncompressed_size() as usize);
        for result in decompressed_frames {
            output.extend_from_slice(&result?);
        }

        Ok(output)
    }

    /// Decompresses parallel frames (single-threaded fallback).
    #[cfg(not(feature = "parallel"))]
    pub fn decompress(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        let (index, frame_data) = self.parse_header(data)?;

        if index.frames.is_empty() {
            return Ok(Vec::new());
        }

        let codec = self.codec_from_id(index.codec)?;

        // Decompress frames sequentially
        let mut output = Vec::with_capacity(index.total_uncompressed_size() as usize);
        let mut pos = 0usize;

        for frame_info in &index.frames {
            let end = pos + frame_info.compressed_size as usize;
            if end > frame_data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame data truncated",
                ));
            }
            let decompressed = codec.decompress_frame(&frame_data[pos..end])?;
            output.extend_from_slice(&decompressed);
            pos = end;
        }

        Ok(output)
    }

    fn parse_header<'a>(&self, data: &'a [u8]) -> io::Result<(FrameIndex, &'a [u8])> {
        if data.len() < 6 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "data too short for header",
            ));
        }

        // Verify magic
        if &data[0..4] != FRAME_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid frame magic",
            ));
        }

        let codec_id = data[4];
        let level = data[5] as i32;
        let mut pos = 6;

        // Read frame count
        let (frame_count, bytes_read) = read_varint(&data[pos..])?;
        pos += bytes_read;

        // Read frame index
        let mut frames = Vec::with_capacity(frame_count as usize);
        let mut offset = 0u64;

        for _ in 0..frame_count {
            let (compressed_size, bytes1) = read_varint(&data[pos..])?;
            pos += bytes1;
            let (uncompressed_size, bytes2) = read_varint(&data[pos..])?;
            pos += bytes2;

            frames.push(FrameInfo {
                offset,
                compressed_size,
                uncompressed_size,
            });
            offset += compressed_size;
        }

        let index = FrameIndex {
            codec: codec_id,
            level,
            frames,
        };

        Ok((index, &data[pos..]))
    }

    fn codec_from_id(&self, id: u8) -> io::Result<FrameCodec> {
        match id {
            #[cfg(feature = "lzma")]
            0x21 => Ok(FrameCodec::Lzma2),
            #[cfg(feature = "zstd")]
            0x01 => Ok(FrameCodec::Zstd),
            #[cfg(feature = "lz4")]
            0x04 => Ok(FrameCodec::Lz4),
            #[cfg(feature = "brotli")]
            0x02 => Ok(FrameCodec::Brotli),
            0x00 => Ok(FrameCodec::Copy),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown codec ID: 0x{:02X}", id),
            )),
        }
    }
}

/// Writes a variable-length integer.
fn write_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Reads a variable-length integer.
fn read_varint(data: &[u8]) -> io::Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut bytes_read = 0;

    for &byte in data {
        bytes_read += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, bytes_read));
        }
        shift += 7;
        if shift >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "varint too large",
            ));
        }
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "incomplete varint",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        let test_values = [0, 1, 127, 128, 255, 256, 16383, 16384, u64::MAX];

        for &value in &test_values {
            let mut encoded = Vec::new();
            write_varint(&mut encoded, value);
            let (decoded, _) = read_varint(&encoded).unwrap();
            assert_eq!(value, decoded, "varint roundtrip failed for {}", value);
        }
    }

    #[test]
    fn test_frame_codec_copy() {
        let data = b"Hello, World!";
        let codec = FrameCodec::Copy;

        let compressed = codec.compress_frame(data, 0).unwrap();
        assert_eq!(compressed, data);

        let decompressed = codec.decompress_frame(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_empty_compression() {
        let encoder = ParallelFrameEncoder::new(FrameCodec::Copy, 1);
        let result = encoder.compress(&[]).unwrap();

        assert_eq!(result.frame_count, 0);
        assert_eq!(result.uncompressed_size, 0);
        assert_eq!(result.compressed_size, 0);

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_copy_roundtrip() {
        // Use larger data to create multiple frames (minimum frame size is 1024)
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Copy, 1).frame_size(1024);

        let result = encoder.compress(&data).unwrap();
        assert!(
            result.frame_count > 1,
            "Expected multiple frames, got {}",
            result.frame_count
        );

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compression_result_metrics() {
        let data = vec![0u8; 10000];
        let encoder = ParallelFrameEncoder::new(FrameCodec::Copy, 1).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert_eq!(result.frame_count, 10);
        assert_eq!(result.uncompressed_size, 10000);
        assert_eq!(result.compressed_size, 10000); // Copy doesn't compress
        assert!((result.compression_ratio() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_frame_index_metrics() {
        let index = FrameIndex {
            codec: 0x00,
            level: 1,
            frames: vec![
                FrameInfo {
                    offset: 0,
                    compressed_size: 50,
                    uncompressed_size: 100,
                },
                FrameInfo {
                    offset: 50,
                    compressed_size: 50,
                    uncompressed_size: 100,
                },
            ],
        };

        assert_eq!(index.total_compressed_size(), 100);
        assert_eq!(index.total_uncompressed_size(), 200);
        assert!((index.compression_ratio() - 0.5).abs() < 0.001);
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_lzma2_roundtrip() {
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Lzma2, 1).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert!(
            result.frame_count >= 5,
            "Expected >=5 frames, got {}",
            result.frame_count
        );

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn test_zstd_roundtrip() {
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Zstd, 3).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert!(
            result.frame_count >= 5,
            "Expected >=5 frames, got {}",
            result.frame_count
        );

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(feature = "lz4")]
    #[test]
    fn test_lz4_roundtrip() {
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Lz4, 1).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert!(
            result.frame_count >= 5,
            "Expected >=5 frames, got {}",
            result.frame_count
        );

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(feature = "brotli")]
    #[test]
    fn test_brotli_roundtrip() {
        let data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Brotli, 4).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert!(
            result.frame_count >= 5,
            "Expected >=5 frames, got {}",
            result.frame_count
        );

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_large_data() {
        // Test with larger data to ensure multiple frames work correctly
        let data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let encoder = ParallelFrameEncoder::new(FrameCodec::Copy, 1).frame_size(1000);

        let result = encoder.compress(&data).unwrap();
        assert_eq!(result.frame_count, 10);

        let decoder = ParallelFrameDecoder::new();
        let decompressed = decoder.decompress(&result.data).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_invalid_magic() {
        let data = b"XXXX\x00\x01\x00";
        let decoder = ParallelFrameDecoder::new();
        let result = decoder.decompress(data);
        assert!(result.is_err());
    }

    #[test]
    fn test_truncated_header() {
        let data = b"PF7Z";
        let decoder = ParallelFrameDecoder::new();
        let result = decoder.decompress(data);
        assert!(result.is_err());
    }
}
