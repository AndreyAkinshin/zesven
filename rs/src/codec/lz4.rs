//! LZ4 compression codec.
//!
//! This module provides LZ4 compression support for 7z archives.
//! LZ4 is a fast compression algorithm optimized for speed.
//!
//! # Skippable Frame Support
//!
//! This decoder supports the zstdmt skippable frame format used by 7-Zip forks
//! (7-Zip-zstd, NanaZip). These tools wrap LZ4 compressed data in skippable
//! frames with the following 12-byte header:
//!
//! ```text
//! ┌────────────────┬────────────────┬────────────────┐
//! │ Magic (4)      │ Frame Size (4) │ Compressed     │
//! │ 0x184D2A50     │ = 4            │ Size (4)       │
//! └────────────────┴────────────────┴────────────────┘
//! ```
//!
//! Multiple frames may be concatenated. The decoder automatically detects
//! and handles both standard LZ4 frames and zstdmt skippable frames.

use std::io::{self, Read, Write};

use lz4_flex::frame::{FrameDecoder, FrameEncoder};

use super::skippable_frame::{self, FrameReader, MAX_HEADER_SIZE, SKIPPABLE_FRAME_MAGIC};
use super::{Decoder, Encoder, method};

/// Header size for LZ4 skippable frames.
const LZ4_HEADER_SIZE: usize = 12;

/// Expected frame_size field value for LZ4 skippable frames.
/// This indicates the metadata section contains only the compressed size (4 bytes).
const LZ4_FRAME_SIZE: u32 = 4;

/// LZ4 decoder with zstdmt skippable frame support.
///
/// Automatically detects whether the input uses standard LZ4 frame format
/// or the zstdmt skippable frame wrapper, and decodes accordingly.
pub struct Lz4Decoder<R: Read> {
    inner: Option<FrameDecoder<FrameReader<R, LZ4_HEADER_SIZE>>>,
}

impl<R: Read> std::fmt::Debug for Lz4Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lz4Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> Lz4Decoder<R> {
    /// Creates a new LZ4 decoder.
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
            skippable_frame::read_full_or_eof(&mut input, &mut header[..LZ4_HEADER_SIZE])?;

        if header_read == 0 {
            // Empty input
            return Ok(Self { inner: None });
        }

        let frame_reader = if header_read == LZ4_HEADER_SIZE {
            if let Some(compressed_size) =
                validate_lz4_header(&header[..LZ4_HEADER_SIZE].try_into().unwrap())
            {
                FrameReader::new_skippable(input, compressed_size)
            } else {
                FrameReader::new_standard(input, header, header_read)
            }
        } else {
            // Partial header - treat as standard stream
            FrameReader::new_standard(input, header, header_read)
        };

        let decoder = FrameDecoder::new(frame_reader);
        Ok(Self {
            inner: Some(decoder),
        })
    }
}

impl<R: Read + Send> Read for Lz4Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(inner) = &mut self.inner else {
            return Ok(0);
        };

        match inner.read(buf) {
            Ok(0) => {
                // Current frame exhausted, try to read next frame
                let frame_reader = inner.get_mut();
                match frame_reader.try_read_next_frame(validate_lz4_header)? {
                    Some(_compressed_size) => {
                        // Reset decoder for new frame
                        let reader = std::mem::replace(frame_reader, FrameReader::Empty);
                        let mut new_decoder = FrameDecoder::new(reader);
                        let result = new_decoder.read(buf);
                        self.inner = Some(new_decoder);
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

impl<R: Read + Send> Decoder for Lz4Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ4
    }
}

/// Validates an LZ4 skippable frame header.
///
/// Returns `Some(compressed_size)` if the header is valid, `None` otherwise.
fn validate_lz4_header(header: &[u8; LZ4_HEADER_SIZE]) -> Option<u32> {
    let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let frame_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

    if magic == SKIPPABLE_FRAME_MAGIC && frame_size == LZ4_FRAME_SIZE {
        Some(u32::from_le_bytes([
            header[8], header[9], header[10], header[11],
        ]))
    } else {
        None
    }
}

/// LZ4 encoder options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lz4EncoderOptions {
    /// Reserved for future use.
    _reserved: (),
}

/// LZ4 encoder.
pub struct Lz4Encoder<W: Write> {
    inner: FrameEncoder<W>,
}

impl<W: Write + Send> Lz4Encoder<W> {
    /// Creates a new LZ4 encoder.
    pub fn new(output: W, _options: &Lz4EncoderOptions) -> Self {
        Self {
            inner: FrameEncoder::new(output),
        }
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish().map_err(io::Error::other)
    }
}

impl<W: Write + Send> Write for Lz4Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for Lz4Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ4
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let original = b"Hello, World! This is a test of LZ4 compression.";

        // Compress
        let mut compressed = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut compressed, &Lz4EncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = Lz4Decoder::new(Cursor::new(compressed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn method_id() {
        let data = vec![0u8; 16];
        let decoder = Lz4Decoder::new(Cursor::new(data)).unwrap();
        assert_eq!(decoder.method_id(), method::LZ4);
    }

    #[test]
    fn empty_input() {
        let mut decoder = Lz4Decoder::new(Cursor::new(Vec::new())).unwrap();
        let mut output = Vec::new();
        let n = decoder.read_to_end(&mut output).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn skippable_frame_single() {
        // Create a valid skippable frame with LZ4 data inside
        let original = b"test data for skippable frame";

        // First compress with standard LZ4
        let mut lz4_data = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut lz4_data, &Lz4EncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Wrap in skippable frame
        let mut framed = Vec::new();
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&LZ4_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(lz4_data.len() as u32).to_le_bytes());
        framed.extend_from_slice(&lz4_data);

        // Decode
        let mut decoder = Lz4Decoder::new(Cursor::new(framed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn skippable_frame_multi() {
        let data1 = b"first frame data";
        let data2 = b"second frame data";

        // Compress both
        let mut lz4_data1 = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut lz4_data1, &Lz4EncoderOptions::default());
            encoder.write_all(data1).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut lz4_data2 = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut lz4_data2, &Lz4EncoderOptions::default());
            encoder.write_all(data2).unwrap();
            encoder.try_finish().unwrap();
        }

        // Create two concatenated skippable frames
        let mut framed = Vec::new();
        // Frame 1
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&LZ4_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(lz4_data1.len() as u32).to_le_bytes());
        framed.extend_from_slice(&lz4_data1);
        // Frame 2
        framed.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        framed.extend_from_slice(&LZ4_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&(lz4_data2.len() as u32).to_le_bytes());
        framed.extend_from_slice(&lz4_data2);

        // Decode
        let mut decoder = Lz4Decoder::new(Cursor::new(framed)).unwrap();
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
        framed.extend_from_slice(&LZ4_FRAME_SIZE.to_le_bytes());
        framed.extend_from_slice(&0u32.to_le_bytes()); // empty payload

        let mut decoder = Lz4Decoder::new(Cursor::new(framed)).unwrap();
        let mut output = Vec::new();
        // This will likely fail when trying to decode empty LZ4 data
        // which is expected - empty compressed_size is unusual
        let _ = decoder.read_to_end(&mut output);
    }
}
