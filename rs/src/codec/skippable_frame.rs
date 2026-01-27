//! Common utilities for zstdmt skippable frame decoding.
//!
//! This module provides shared infrastructure for decoding data wrapped in
//! zstdmt skippable frames, as used by 7-Zip forks (7-Zip-zstd, NanaZip).
//!
//! # Skippable Frame Format
//!
//! The zstdmt library uses ZSTD's skippable frame format to wrap compressed data.
//! The basic structure is:
//!
//! ```text
//! ┌────────────────┬────────────────┬────────────────────────┐
//! │ Magic (4)      │ Frame Size (4) │ Metadata (frame_size)  │
//! │ 0x184D2A50     │                │                        │
//! └────────────────┴────────────────┴────────────────────────┘
//! ```
//!
//! For LZ4, frame_size = 4 (just compressed_size).
//! For Brotli, frame_size = 8 (compressed_size + brotli_magic + hint).

use std::io::{self, Cursor, Read};

/// Magic bytes for zstdmt skippable frame (same as ZSTD skippable frame).
pub const SKIPPABLE_FRAME_MAGIC: u32 = 0x184D2A50;

/// Maximum header size we need to read (Brotli uses 16 bytes).
pub const MAX_HEADER_SIZE: usize = 16;

/// Reads exactly `buf.len()` bytes, or fewer only if EOF is reached.
/// Returns the number of bytes actually read.
///
/// Unlike `read_exact`, this function does not error on EOF - it returns
/// however many bytes were available.
pub fn read_full_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Internal reader that handles both standard and skippable frame formats.
///
/// This enum wraps an underlying reader and provides frame-aware reading:
/// - `Standard`: Replays a buffered header, then reads from underlying reader
/// - `Skippable`: Reads a limited number of bytes (the compressed payload)
pub enum FrameReader<R: Read, const HEADER_SIZE: usize> {
    /// No more data available.
    Empty,

    /// Standard stream (header already read, needs to be replayed).
    /// Used when the input doesn't start with a skippable frame magic.
    Standard {
        reader: R,
        /// Buffered header bytes to replay before reading from `reader`.
        header_buf: Cursor<[u8; MAX_HEADER_SIZE]>,
        /// Number of valid bytes in header_buf.
        header_len: usize,
        /// Whether we've finished replaying the header.
        header_done: bool,
    },

    /// Skippable frame (reads limited number of bytes).
    /// Used when skippable frame magic is detected.
    Skippable {
        reader: R,
        /// Remaining bytes to read from the current frame.
        remaining: u32,
        /// Whether the current frame has been fully read.
        frame_done: bool,
    },
}

impl<R: Read, const HEADER_SIZE: usize> FrameReader<R, HEADER_SIZE> {
    /// Creates a new reader for standard (non-skippable) stream.
    ///
    /// The provided header bytes will be replayed before reading from the
    /// underlying reader.
    pub fn new_standard(reader: R, header: [u8; MAX_HEADER_SIZE], header_len: usize) -> Self {
        Self::Standard {
            reader,
            header_buf: Cursor::new(header),
            header_len,
            header_done: false,
        }
    }

    /// Creates a new reader for skippable frame stream.
    ///
    /// Will read exactly `compressed_size` bytes from the underlying reader.
    pub fn new_skippable(reader: R, compressed_size: u32) -> Self {
        Self::Skippable {
            reader,
            remaining: compressed_size,
            frame_done: false,
        }
    }

    /// Attempts to read the next skippable frame header.
    ///
    /// Returns `Ok(Some(compressed_size))` if a valid frame was found,
    /// `Ok(None)` if EOF was reached cleanly, or an error if the header
    /// is malformed or truncated.
    ///
    /// The `validate_header` function should check if the header is valid
    /// and return the compressed size if so.
    #[must_use = "ignoring the result may cause data loss"]
    pub fn try_read_next_frame<F>(&mut self, validate_header: F) -> io::Result<Option<u32>>
    where
        F: Fn(&[u8; HEADER_SIZE]) -> Option<u32>,
    {
        match self {
            Self::Empty | Self::Standard { .. } => Ok(None),
            Self::Skippable {
                reader,
                remaining,
                frame_done,
            } => {
                if !*frame_done {
                    return Ok(None);
                }

                let mut header = [0u8; MAX_HEADER_SIZE];
                let n = read_full_or_eof(reader, &mut header[..HEADER_SIZE])?;

                if n == 0 {
                    // Clean EOF
                    return Ok(None);
                }

                if n < HEADER_SIZE {
                    // Partial header is an error - stream is truncated
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "truncated skippable frame header: expected {} bytes, got {}",
                            HEADER_SIZE, n
                        ),
                    ));
                }

                // Try to validate as skippable frame
                let header_arr: [u8; HEADER_SIZE] = header[..HEADER_SIZE]
                    .try_into()
                    .expect("size checked above");

                match validate_header(&header_arr) {
                    Some(compressed_size) => {
                        *remaining = compressed_size;
                        *frame_done = false;
                        Ok(Some(compressed_size))
                    }
                    None => {
                        // Header doesn't match skippable frame format
                        // This could be trailing garbage or a different format
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid skippable frame header after valid frame",
                        ))
                    }
                }
            }
        }
    }
}

impl<R: Read, const HEADER_SIZE: usize> Read for FrameReader<R, HEADER_SIZE> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Empty => Ok(0),

            Self::Standard {
                reader,
                header_buf,
                header_len,
                header_done,
            } => {
                // First, drain the buffered header
                if !*header_done {
                    let pos = header_buf.position() as usize;
                    let remaining_header = *header_len - pos;
                    if remaining_header > 0 {
                        let to_copy = buf.len().min(remaining_header);
                        let inner = header_buf.get_ref();
                        buf[..to_copy].copy_from_slice(&inner[pos..pos + to_copy]);
                        header_buf.set_position((pos + to_copy) as u64);
                        return Ok(to_copy);
                    }
                    *header_done = true;
                }
                // Then read from underlying reader
                reader.read(buf)
            }

            Self::Skippable {
                reader,
                remaining,
                frame_done,
            } => {
                if *frame_done || *remaining == 0 {
                    *frame_done = true;
                    return Ok(0);
                }

                let to_read = buf.len().min(*remaining as usize);
                let n = reader.read(&mut buf[..to_read])?;

                if n == 0 {
                    // Unexpected EOF - payload is truncated
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "truncated skippable frame payload: {} bytes remaining",
                            *remaining
                        ),
                    ));
                }

                *remaining -= n as u32;
                if *remaining == 0 {
                    *frame_done = true;
                }

                Ok(n)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TEST_HEADER_SIZE: usize = 12;

    fn validate_test_header(header: &[u8; TEST_HEADER_SIZE]) -> Option<u32> {
        let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let frame_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if magic == SKIPPABLE_FRAME_MAGIC && frame_size == 4 {
            Some(u32::from_le_bytes([
                header[8], header[9], header[10], header[11],
            ]))
        } else {
            None
        }
    }

    #[test]
    fn standard_reader_replays_header() {
        let data = b"world!";
        let mut header = [0u8; MAX_HEADER_SIZE];
        header[..6].copy_from_slice(b"hello ");

        let reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_standard(Cursor::new(data.as_slice()), header, 6);

        let mut output = Vec::new();
        let mut reader = reader;
        reader.read_to_end(&mut output).unwrap();

        assert_eq!(output, b"hello world!");
    }

    #[test]
    fn skippable_reader_limits_bytes() {
        let data = b"hello world! extra garbage";
        let reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data.as_slice()), 12);

        let mut output = Vec::new();
        let mut reader = reader;
        reader.read_to_end(&mut output).unwrap();

        assert_eq!(output, b"hello world!");
    }

    #[test]
    fn skippable_reader_truncated_payload() {
        let data = b"short"; // Only 5 bytes, but we expect 12
        let reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data.as_slice()), 12);

        let mut output = Vec::new();
        let mut reader = reader;
        let result = reader.read_to_end(&mut output);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn try_read_next_frame_valid() {
        // Create a skippable frame header followed by another
        let mut data = Vec::new();
        // First frame payload (empty for this test)
        // Second frame header
        data.extend_from_slice(&SKIPPABLE_FRAME_MAGIC.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes()); // frame_size
        data.extend_from_slice(&100u32.to_le_bytes()); // compressed_size

        let mut reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data), 0);

        // Mark first frame as done (it was empty)
        if let FrameReader::Skippable { frame_done, .. } = &mut reader {
            *frame_done = true;
        }

        let result = reader.try_read_next_frame(validate_test_header);
        assert_eq!(result.unwrap(), Some(100));
    }

    #[test]
    fn try_read_next_frame_truncated_header() {
        // Only 5 bytes - not enough for header
        let data = vec![0x50, 0x2A, 0x4D, 0x18, 0x04];

        let mut reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data), 0);

        if let FrameReader::Skippable { frame_done, .. } = &mut reader {
            *frame_done = true;
        }

        let result = reader.try_read_next_frame(validate_test_header);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn try_read_next_frame_invalid_magic() {
        // Valid length but wrong magic
        let mut data = Vec::new();
        data.extend_from_slice(&0xDEADBEEFu32.to_le_bytes()); // wrong magic
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&100u32.to_le_bytes());

        let mut reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data), 0);

        if let FrameReader::Skippable { frame_done, .. } = &mut reader {
            *frame_done = true;
        }

        let result = reader.try_read_next_frame(validate_test_header);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn try_read_next_frame_clean_eof() {
        let data: Vec<u8> = Vec::new(); // Empty - clean EOF

        let mut reader: FrameReader<_, TEST_HEADER_SIZE> =
            FrameReader::new_skippable(Cursor::new(data), 0);

        if let FrameReader::Skippable { frame_done, .. } = &mut reader {
            *frame_done = true;
        }

        let result = reader.try_read_next_frame(validate_test_header);
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn empty_reader() {
        let mut reader: FrameReader<Cursor<Vec<u8>>, TEST_HEADER_SIZE> = FrameReader::Empty;
        let mut buf = [0u8; 10];
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }
}
