//! Brotli compression codec.
//!
//! This module provides Brotli compression support for 7z archives.
//! Brotli is a compression algorithm developed by Google, optimized for web content.
//!
//! # Skippable Frame Support
//!
//! This decoder supports the zstdmt skippable frame format used by 7-Zip forks
//! (7-Zip-zstd, NanaZip). These tools wrap Brotli compressed data in skippable
//! frames with the following 16-byte header:
//!
//! ```text
//! ┌────────────────┬────────────────┬────────────────┬────────────────┬────────────────┐
//! │ Magic (4)      │ Frame Size (4) │ Compressed     │ Brotli Magic   │ Uncompressed   │
//! │ 0x184D2A50     │ = 8            │ Size (4)       │ 0x5242 "BR"(2) │ Hint (2)       │
//! └────────────────┴────────────────┴────────────────┴────────────────┴────────────────┘
//! ```
//!
//! The "Uncompressed Hint" field is a 2-byte value representing the uncompressed
//! size in 64KB units. This field is currently ignored by the decoder but could
//! be used for pre-allocation optimization in the future.
//!
//! Multiple frames may be concatenated. The decoder automatically detects
//! and handles both standard Brotli streams and zstdmt skippable frames.

use std::io::{self, Read, Write};

use brotli::CompressorWriter;
use brotli::Decompressor;
use brotli::enc::BrotliEncoderParams;

use super::skippable_frame::{self, FrameReader, MAX_HEADER_SIZE, SKIPPABLE_FRAME_MAGIC};
use super::{Decoder, Encoder, method};

/// Header size for Brotli skippable frames.
const BROTLI_HEADER_SIZE: usize = 16;

/// Expected frame_size field value for Brotli skippable frames.
/// This indicates the metadata section is 8 bytes (compressed_size + brotli_magic + hint).
const BROTLI_FRAME_SIZE: u32 = 8;

/// Brotli magic identifier in skippable frame ("BR" in little-endian).
const BROTLI_MAGIC: u16 = 0x5242;

/// Default buffer size for Brotli decompressor.
const BUFFER_SIZE: usize = 4096;

/// Brotli decoder with zstdmt skippable frame support.
///
/// Automatically detects whether the input uses standard Brotli format
/// or the zstdmt skippable frame wrapper, and decodes accordingly.
pub struct BrotliDecoder<R: Read> {
    inner: Option<Decompressor<FrameReader<R, BROTLI_HEADER_SIZE>>>,
    buffer_size: usize,
}

impl<R: Read> std::fmt::Debug for BrotliDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrotliDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BrotliDecoder<R> {
    /// Creates a new Brotli decoder.
    ///
    /// The decoder will automatically detect if the input uses zstdmt
    /// skippable frames and handle them appropriately.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the initial header fails.
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut header = [0u8; MAX_HEADER_SIZE];
        let header_read =
            skippable_frame::read_full_or_eof(&mut input, &mut header[..BROTLI_HEADER_SIZE])?;

        if header_read == 0 {
            // Empty input
            return Ok(Self {
                inner: None,
                buffer_size: BUFFER_SIZE,
            });
        }

        let frame_reader = if header_read == BROTLI_HEADER_SIZE {
            if let Some(compressed_size) =
                validate_brotli_header(&header[..BROTLI_HEADER_SIZE].try_into().unwrap())
            {
                FrameReader::new_skippable(input, compressed_size)
            } else {
                FrameReader::new_standard(input, header, header_read)
            }
        } else {
            // Partial header - treat as standard stream
            FrameReader::new_standard(input, header, header_read)
        };

        let decompressor = Decompressor::new(frame_reader, BUFFER_SIZE);
        Ok(Self {
            inner: Some(decompressor),
            buffer_size: BUFFER_SIZE,
        })
    }
}

impl<R: Read + Send> Read for BrotliDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(inner) = &mut self.inner else {
            return Ok(0);
        };

        match inner.read(buf) {
            Ok(0) => {
                // Current frame exhausted, try to read next frame
                let frame_reader = inner.get_mut();
                match frame_reader.try_read_next_frame(validate_brotli_header)? {
                    Some(_compressed_size) => {
                        // Reset decompressor for new frame
                        let reader = std::mem::replace(frame_reader, FrameReader::Empty);
                        let mut new_decompressor = Decompressor::new(reader, self.buffer_size);
                        let result = new_decompressor.read(buf);
                        self.inner = Some(new_decompressor);
                        result
                    }
                    None => {
                        self.inner = None;
                        Ok(0)
                    }
                }
            }
            result => result,
        }
    }
}

impl<R: Read + Send> Decoder for BrotliDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BROTLI
    }
}

/// Validates a Brotli skippable frame header.
///
/// Returns `Some(compressed_size)` if the header is valid, `None` otherwise.
///
/// The header must have:
/// - Magic: 0x184D2A50
/// - Frame size: 8
/// - Brotli magic: 0x5242 ("BR")
///
/// The uncompressed hint field (bytes 14-15) is read but currently ignored.
fn validate_brotli_header(header: &[u8; BROTLI_HEADER_SIZE]) -> Option<u32> {
    let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let frame_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let brotli_magic = u16::from_le_bytes([header[12], header[13]]);
    // Note: header[14..16] contains uncompressed_hint (in 64KB units), currently unused

    if magic == SKIPPABLE_FRAME_MAGIC
        && frame_size == BROTLI_FRAME_SIZE
        && brotli_magic == BROTLI_MAGIC
    {
        Some(u32::from_le_bytes([
            header[8], header[9], header[10], header[11],
        ]))
    } else {
        None
    }
}

/// Brotli encoder options.
#[derive(Debug, Clone)]
pub struct BrotliEncoderOptions {
    /// Compression quality (0-11, default 4).
    pub quality: u32,
    /// LG window size (10-24, default 22).
    pub lg_window_size: u32,
}

impl Default for BrotliEncoderOptions {
    fn default() -> Self {
        Self {
            quality: 4,
            lg_window_size: 22,
        }
    }
}

/// Brotli encoder.
pub struct BrotliEncoder<W: Write> {
    inner: CompressorWriter<W>,
}

impl<W: Write + Send> BrotliEncoder<W> {
    /// Creates a new Brotli encoder.
    pub fn new(output: W, options: &BrotliEncoderOptions) -> Self {
        let params = BrotliEncoderParams {
            quality: options.quality as i32,
            lgwin: options.lg_window_size as i32,
            ..Default::default()
        };
        Self {
            inner: CompressorWriter::with_params(output, BUFFER_SIZE, &params),
        }
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        Ok(self.inner.into_inner())
    }
}

impl<W: Write + Send> Write for BrotliEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BrotliEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BROTLI
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        let _ = self.inner.into_inner();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let original = b"Hello, World! This is a test of Brotli compression.";

        // Compress
        let mut compressed = Vec::new();
        {
            let mut encoder = BrotliEncoder::new(&mut compressed, &BrotliEncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = BrotliDecoder::new(Cursor::new(compressed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn method_id() {
        // Compress some data to create valid brotli stream
        let mut compressed = Vec::new();
        {
            let mut encoder = BrotliEncoder::new(&mut compressed, &BrotliEncoderOptions::default());
            encoder.write_all(b"test").unwrap();
            encoder.try_finish().unwrap();
        }

        let decoder = BrotliDecoder::new(Cursor::new(compressed)).unwrap();
        assert_eq!(decoder.method_id(), method::BROTLI);
    }

    #[test]
    fn empty_input() {
        let mut decoder = BrotliDecoder::new(Cursor::new(Vec::new())).unwrap();
        let mut output = Vec::new();
        let n = decoder.read_to_end(&mut output).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn skippable_frame_single() {
        // Create a valid skippable frame with Brotli data inside
        let original = b"test data for skippable frame";

        // First compress with standard Brotli
        let mut brotli_data = Vec::new();
        {
            let mut encoder =
                BrotliEncoder::new(&mut brotli_data, &BrotliEncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Wrap in skippable frame
        let mut framed = Vec::new();
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&BROTLI_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(brotli_data.len() as u32).to_le_bytes());
        framed.extend_from_slice(&BROTLI_MAGIC.to_le_bytes());
        framed.extend_from_slice(&0u16.to_le_bytes()); // hint (unused)
        framed.extend_from_slice(&brotli_data);

        // Decode
        let mut decoder = BrotliDecoder::new(Cursor::new(framed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn skippable_frame_multi() {
        let data1 = b"first frame data";
        let data2 = b"second frame data";

        // Compress both
        let mut brotli_data1 = Vec::new();
        {
            let mut encoder =
                BrotliEncoder::new(&mut brotli_data1, &BrotliEncoderOptions::default());
            encoder.write_all(data1).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut brotli_data2 = Vec::new();
        {
            let mut encoder =
                BrotliEncoder::new(&mut brotli_data2, &BrotliEncoderOptions::default());
            encoder.write_all(data2).unwrap();
            encoder.try_finish().unwrap();
        }

        // Create two concatenated skippable frames
        let mut framed = Vec::new();
        // Frame 1
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&BROTLI_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(brotli_data1.len() as u32).to_le_bytes());
        framed.extend_from_slice(&BROTLI_MAGIC.to_le_bytes());
        framed.extend_from_slice(&0u16.to_le_bytes());
        framed.extend_from_slice(&brotli_data1);
        // Frame 2
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&BROTLI_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(brotli_data2.len() as u32).to_le_bytes());
        framed.extend_from_slice(&BROTLI_MAGIC.to_le_bytes());
        framed.extend_from_slice(&0u16.to_le_bytes());
        framed.extend_from_slice(&brotli_data2);

        // Decode
        let mut decoder = BrotliDecoder::new(Cursor::new(framed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(data1);
        expected.extend_from_slice(data2);
        assert_eq!(decompressed, expected);
    }

    #[test]
    fn skippable_frame_empty_payload() {
        // Skippable frame with compressed_size = 0
        let mut framed = Vec::new();
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&BROTLI_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&0u32.to_le_bytes()); // empty payload
        framed.extend_from_slice(&BROTLI_MAGIC.to_le_bytes());
        framed.extend_from_slice(&0u16.to_le_bytes());

        let mut decoder = BrotliDecoder::new(Cursor::new(framed)).unwrap();
        let mut output = Vec::new();
        // This will likely fail when trying to decode empty Brotli data
        // which is expected - empty compressed_size is unusual
        let _ = decoder.read_to_end(&mut output);
    }
}
