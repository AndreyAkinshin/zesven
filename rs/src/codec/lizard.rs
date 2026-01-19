//! Lizard compression codec (pure Rust implementation).
//!
//! Lizard is an LZ77-type compressor with a 5-stream block format.
//! It offers compression ratios comparable to zlib with very fast decompression.
//!
//! This implementation supports decompression of Lizard-compressed data
//! as used in 7-Zip archives created with 7-Zip-zstd.
//!
//! # Method ID
//!
//! Lizard uses method ID `0x04, 0xF7, 0x11, 0x06` (7-Zip-zstd convention).
//!
//! # Format
//!
//! Lizard uses a frame format with:
//! - Magic number: 0x184D2206
//! - Frame descriptor (flags, block size, optional content size)
//! - Data blocks (each with compression level + block data)
//! - End mark (4 bytes of 0)
//! - Optional content checksum
//!
//! See: <https://github.com/inikep/lizard>

use std::io::{self, Read, Write};

use super::{Decoder, Encoder, method};

/// Lizard frame magic number
const LIZARD_MAGIC: u32 = 0x184D2206;

/// Block flags
const LIZARD_FLAG_LITERALS: u8 = 1;
const LIZARD_FLAG_FLAGS: u8 = 2;
const LIZARD_FLAG_OFFSET16: u8 = 4;
const LIZARD_FLAG_OFFSET24: u8 = 8;
const LIZARD_FLAG_UNCOMPRESSED: u8 = 128;

/// Token parsing constants
const MAX_SHORT_LITLEN: usize = 7;
const MAX_SHORT_MATCHLEN: usize = 15;
const LIZARD_LAST_LONG_OFF: u8 = 31;
const MM_LONGOFF: usize = 16;

/// Maximum block sizes indexed by block size field value
const BLOCK_SIZES: [usize; 8] = [
    0,                 // 0: N/A
    128 * 1024,        // 1: 128 KB
    256 * 1024,        // 2: 256 KB
    1024 * 1024,       // 3: 1 MB
    4 * 1024 * 1024,   // 4: 4 MB
    16 * 1024 * 1024,  // 5: 16 MB
    64 * 1024 * 1024,  // 6: 64 MB
    256 * 1024 * 1024, // 7: 256 MB
];

/// Lizard decoder that decompresses Lizard frame format data.
pub struct LizardDecoder<R: Read> {
    inner: R,
    /// Output buffer (sliding window for match copying)
    buffer: Vec<u8>,
    /// Read position in buffer (data returned to caller up to this point)
    read_pos: usize,
    /// Whether we've finished reading all data
    finished: bool,
    /// Frame header parsed
    header_parsed: bool,
    /// Block independence flag from frame descriptor
    block_independent: bool,
    /// Maximum block size
    max_block_size: usize,
    /// Whether blocks have checksums
    block_checksum: bool,
    /// Whether content has checksum
    content_checksum: bool,
    /// Content size (if provided in frame)
    #[allow(dead_code)] // Stored for potential size validation
    content_size: Option<u64>,
    /// Last offset for repeat offset token
    last_offset: isize,
}

impl<R: Read> std::fmt::Debug for LizardDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LizardDecoder")
            .field("buffer_len", &self.buffer.len())
            .field("read_pos", &self.read_pos)
            .field("finished", &self.finished)
            .field("max_block_size", &self.max_block_size)
            .finish()
    }
}

impl<R: Read + Send> LizardDecoder<R> {
    /// Creates a new Lizard decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: input,
            buffer: Vec::new(),
            read_pos: 0,
            finished: false,
            header_parsed: false,
            block_independent: true,
            max_block_size: 4 * 1024 * 1024, // Default 4MB
            block_checksum: false,
            content_checksum: false,
            content_size: None,
            last_offset: 0,
        }
    }

    /// Parses the Lizard frame header.
    fn parse_header(&mut self) -> io::Result<()> {
        // Read magic number (4 bytes, little endian)
        let mut magic_bytes = [0u8; 4];
        self.inner.read_exact(&mut magic_bytes)?;
        let magic = u32::from_le_bytes(magic_bytes);

        if magic != LIZARD_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Invalid Lizard magic number: 0x{:08X}, expected 0x{:08X}",
                    magic, LIZARD_MAGIC
                ),
            ));
        }

        // Read FLG byte
        let mut flg = [0u8; 1];
        self.inner.read_exact(&mut flg)?;
        let flg = flg[0];

        // Check version (bits 7-6 must be 01)
        let version = (flg >> 6) & 0x03;
        if version != 0x01 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported Lizard version: {}", version),
            ));
        }

        self.block_independent = (flg & 0x20) != 0;
        self.block_checksum = (flg & 0x10) != 0;
        let has_content_size = (flg & 0x08) != 0;
        self.content_checksum = (flg & 0x04) != 0;

        // Read BD byte
        let mut bd = [0u8; 1];
        self.inner.read_exact(&mut bd)?;
        let bd = bd[0];

        let block_size_id = ((bd >> 4) & 0x07) as usize;
        if block_size_id == 0 || block_size_id >= BLOCK_SIZES.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid block size ID: {}", block_size_id),
            ));
        }
        self.max_block_size = BLOCK_SIZES[block_size_id];

        // Read content size if present (8 bytes, little endian)
        if has_content_size {
            let mut size_bytes = [0u8; 8];
            self.inner.read_exact(&mut size_bytes)?;
            self.content_size = Some(u64::from_le_bytes(size_bytes));
        }

        // Read header checksum (1 byte) - we skip validation
        let mut _hc = [0u8; 1];
        self.inner.read_exact(&mut _hc)?;

        // Pre-allocate buffer
        self.buffer.reserve(self.max_block_size);

        self.header_parsed = true;
        Ok(())
    }

    /// Reads and decompresses the next block.
    fn read_block(&mut self) -> io::Result<bool> {
        // Read block size (4 bytes, little endian)
        let mut size_bytes = [0u8; 4];
        self.inner.read_exact(&mut size_bytes)?;
        let block_header = u32::from_le_bytes(size_bytes);

        // End mark: size == 0
        if block_header == 0 {
            // Read content checksum if present (propagate I/O errors)
            if self.content_checksum {
                let mut checksum = [0u8; 4];
                self.inner.read_exact(&mut checksum)?;
            }
            return Ok(false);
        }

        // Highest bit indicates uncompressed data (frame-level)
        let is_frame_uncompressed = (block_header & 0x80000000) != 0;
        let block_size = (block_header & 0x7FFFFFFF) as usize;

        if block_size > self.max_block_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Block size {} exceeds maximum {}",
                    block_size, self.max_block_size
                ),
            ));
        }

        // Read block data
        let mut block_data = vec![0u8; block_size];
        self.inner.read_exact(&mut block_data)?;

        // Skip block checksum if present
        if self.block_checksum {
            let mut _checksum = [0u8; 4];
            self.inner.read_exact(&mut _checksum)?;
        }

        // For block-independent mode, clear the buffer
        if self.block_independent {
            self.buffer.clear();
            self.read_pos = 0;
        }

        if is_frame_uncompressed {
            // Copy uncompressed data directly (frame-level uncompressed)
            self.buffer.extend_from_slice(&block_data);
        } else {
            // Decompress block (Lizard format)
            self.decompress_lizard_block(&block_data)?;
        }

        Ok(true)
    }

    /// Decompresses Lizard block data.
    ///
    /// Lizard block format:
    /// - 1 byte: compression level
    /// - Then one or more sub-blocks
    fn decompress_lizard_block(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        let mut pos = 0;

        // Read compression level byte (first byte of block data)
        let _compression_level = data[pos];
        pos += 1;

        // Process sub-blocks
        while pos < data.len() {
            let header_byte = data[pos];
            pos += 1;

            if header_byte == LIZARD_FLAG_UNCOMPRESSED {
                // Uncompressed sub-block
                if pos + 3 > data.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete uncompressed block header",
                    ));
                }
                let length = read_le24(&data[pos..]);
                pos += 3;

                if pos + length > data.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete uncompressed block data",
                    ));
                }
                self.buffer.extend_from_slice(&data[pos..pos + length]);
                pos += length;
            } else {
                // Compressed sub-block with 5 streams
                pos = self.decompress_compressed_block(data, pos - 1)?;
            }
        }

        Ok(())
    }

    /// Decompresses a compressed sub-block with 5 streams.
    fn decompress_compressed_block(&mut self, data: &[u8], start: usize) -> io::Result<usize> {
        let mut pos = start;

        if pos >= data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Missing block header",
            ));
        }

        let header_byte = data[pos];
        pos += 1;

        // Check for Huffman flags - not supported yet
        if (header_byte
            & (LIZARD_FLAG_LITERALS
                | LIZARD_FLAG_FLAGS
                | LIZARD_FLAG_OFFSET16
                | LIZARD_FLAG_OFFSET24))
            != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Huffman-compressed Lizard streams are not yet supported",
            ));
        }

        // Read 5 streams (all raw, no Huffman)
        // Order: Lengths, Offset16, Offset24, Flags/Tokens, Literals

        // Stream 1: Lengths
        if pos + 3 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete lengths stream",
            ));
        }
        let len_stream_len = read_le24(&data[pos..]);
        pos += 3;
        let len_stream_start = pos;
        let len_stream_end = pos + len_stream_len;
        if len_stream_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Lengths stream overflow",
            ));
        }
        pos = len_stream_end;

        // Stream 2: 16-bit Offsets
        if pos + 3 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete offset16 stream",
            ));
        }
        let off16_stream_len = read_le24(&data[pos..]);
        pos += 3;
        let off16_stream_start = pos;
        let off16_stream_end = pos + off16_stream_len;
        if off16_stream_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Offset16 stream overflow",
            ));
        }
        pos = off16_stream_end;

        // Stream 3: 24-bit Offsets
        if pos + 3 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete offset24 stream",
            ));
        }
        let off24_stream_len = read_le24(&data[pos..]);
        pos += 3;
        let off24_stream_start = pos;
        let off24_stream_end = pos + off24_stream_len;
        if off24_stream_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Offset24 stream overflow",
            ));
        }
        pos = off24_stream_end;

        // Stream 4: Flags/Tokens
        if pos + 3 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete flags stream",
            ));
        }
        let flags_stream_len = read_le24(&data[pos..]);
        pos += 3;
        let flags_stream_start = pos;
        let flags_stream_end = pos + flags_stream_len;
        if flags_stream_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Flags stream overflow",
            ));
        }
        pos = flags_stream_end;

        // Stream 5: Literals
        if pos + 3 > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete literals stream",
            ));
        }
        let lit_stream_len = read_le24(&data[pos..]);
        pos += 3;
        let lit_stream_start = pos;
        let lit_stream_end = pos + lit_stream_len;
        if lit_stream_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Literals stream overflow",
            ));
        }
        pos = lit_stream_end;

        // Create stream cursors
        let mut len_ptr = len_stream_start;
        let mut off16_ptr = off16_stream_start;
        let mut off24_ptr = off24_stream_start;
        let mut flags_ptr = flags_stream_start;
        let mut lit_ptr = lit_stream_start;

        // Process tokens
        while flags_ptr < flags_stream_end {
            let token = data[flags_ptr];
            flags_ptr += 1;

            if token >= 32 {
                // Token types [0_MMMM_LLL] or [1_MMMM_LLL]
                // 3-bit literal length, 4-bit match length
                let mut literal_len = (token & MAX_SHORT_LITLEN as u8) as usize;
                let mut match_len = ((token >> 3) & MAX_SHORT_MATCHLEN as u8) as usize;
                let use_last_offset = (token & 0x80) != 0;

                // Read extended literal length if needed
                if literal_len == MAX_SHORT_LITLEN {
                    let ext_len = self.read_length(data, &mut len_ptr, len_stream_end)?;
                    literal_len += ext_len;
                }

                // Copy literals
                if literal_len > 0 {
                    if lit_ptr + literal_len > lit_stream_end {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Literal overflow",
                        ));
                    }
                    self.buffer
                        .extend_from_slice(&data[lit_ptr..lit_ptr + literal_len]);
                    lit_ptr += literal_len;
                }

                // Get offset
                if !use_last_offset {
                    // Read 16-bit offset
                    if off16_ptr + 2 > off16_stream_end {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Offset16 overflow",
                        ));
                    }
                    let offset =
                        u16::from_le_bytes([data[off16_ptr], data[off16_ptr + 1]]) as isize;
                    off16_ptr += 2;
                    self.last_offset = -offset;
                }

                // For 16-bit offset tokens, minimum match is 4
                if !use_last_offset && match_len < 4 {
                    match_len = 4;
                }

                // Read extended match length if needed
                if match_len == MAX_SHORT_MATCHLEN {
                    let ext_len = self.read_length(data, &mut len_ptr, len_stream_end)?;
                    match_len += ext_len;
                }

                // Perform match copy
                self.copy_match(match_len)?;
            } else if token < LIZARD_LAST_LONG_OFF {
                // Token 0-30: 24-bit offset, match length = token + 16
                let match_len = token as usize + MM_LONGOFF;

                // Read 24-bit offset
                if off24_ptr + 3 > off24_stream_end {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Offset24 overflow",
                    ));
                }
                let offset = read_le24(&data[off24_ptr..]) as isize;
                off24_ptr += 3;
                self.last_offset = -offset;

                // Perform match copy
                self.copy_match(match_len)?;
            } else {
                // Token 31: 24-bit offset, extended match length
                let mut match_len = self.read_length(data, &mut len_ptr, len_stream_end)?;
                match_len += LIZARD_LAST_LONG_OFF as usize + MM_LONGOFF;

                // Read 24-bit offset
                if off24_ptr + 3 > off24_stream_end {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Offset24 overflow",
                    ));
                }
                let offset = read_le24(&data[off24_ptr..]) as isize;
                off24_ptr += 3;
                self.last_offset = -offset;

                // Perform match copy
                self.copy_match(match_len)?;
            }
        }

        // Copy remaining literals (last 16+ bytes are always literals)
        if lit_ptr < lit_stream_end {
            self.buffer
                .extend_from_slice(&data[lit_ptr..lit_stream_end]);
        }

        Ok(pos)
    }

    /// Reads a length value from the lengths stream.
    fn read_length(&self, data: &[u8], ptr: &mut usize, end: usize) -> io::Result<usize> {
        if *ptr >= end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Length stream underflow",
            ));
        }

        let first_byte = data[*ptr];
        *ptr += 1;

        if first_byte < 254 {
            Ok(first_byte as usize)
        } else if first_byte == 254 {
            // 2-byte length follows
            if *ptr + 2 > end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Length stream underflow (2-byte)",
                ));
            }
            let len = u16::from_le_bytes([data[*ptr], data[*ptr + 1]]) as usize;
            *ptr += 2;
            Ok(len)
        } else {
            // 3-byte length follows
            if *ptr + 3 > end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Length stream underflow (3-byte)",
                ));
            }
            let len = read_le24(&data[*ptr..]);
            *ptr += 3;
            Ok(len)
        }
    }

    /// Copies a match from the sliding window.
    fn copy_match(&mut self, length: usize) -> io::Result<()> {
        let offset = (-self.last_offset) as usize;

        if offset == 0 || offset > self.buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Invalid match offset {} (buffer size {})",
                    offset,
                    self.buffer.len()
                ),
            ));
        }

        let start = self.buffer.len() - offset;
        for i in 0..length {
            let byte = self.buffer[start + (i % offset)];
            self.buffer.push(byte);
        }

        Ok(())
    }
}

/// Reads a 24-bit little-endian integer.
fn read_le24(data: &[u8]) -> usize {
    data[0] as usize | ((data[1] as usize) << 8) | ((data[2] as usize) << 16)
}

impl<R: Read + Send> Read for LizardDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished {
            return Ok(0);
        }

        // Parse header on first read
        if !self.header_parsed {
            self.parse_header()?;
        }

        // Check if we have data in buffer to return
        if self.read_pos < self.buffer.len() {
            let available = self.buffer.len() - self.read_pos;
            let to_copy = buf.len().min(available);
            buf[..to_copy].copy_from_slice(&self.buffer[self.read_pos..self.read_pos + to_copy]);
            self.read_pos += to_copy;
            return Ok(to_copy);
        }

        // Need to read more blocks
        loop {
            let has_more = self.read_block()?;
            if !has_more {
                self.finished = true;
                return Ok(0);
            }

            if self.read_pos < self.buffer.len() {
                let available = self.buffer.len() - self.read_pos;
                let to_copy = buf.len().min(available);
                buf[..to_copy]
                    .copy_from_slice(&self.buffer[self.read_pos..self.read_pos + to_copy]);
                self.read_pos += to_copy;
                return Ok(to_copy);
            }
        }
    }
}

impl<R: Read + Send> Decoder for LizardDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LIZARD
    }
}

/// Lizard encoder options.
#[derive(Debug, Clone)]
pub struct LizardEncoderOptions {
    /// Compression level (10-49).
    ///
    /// - 10-19: fastLZ4 (LZ4-compatible)
    /// - 20-29: LIZv1 (better ratio)
    /// - 30-39: fastLZ4 + Huffman
    /// - 40-49: LIZv1 + Huffman
    pub level: i32,
    /// Block size ID (1-7). Default: 4 (4MB).
    pub block_size_id: u8,
    /// Whether blocks are independent.
    pub block_independent: bool,
    /// Whether to include content checksum.
    pub content_checksum: bool,
    /// Whether to include block checksums.
    pub block_checksum: bool,
}

impl Default for LizardEncoderOptions {
    fn default() -> Self {
        Self {
            level: 17,
            block_size_id: 4, // 4MB blocks
            block_independent: true,
            content_checksum: false,
            block_checksum: false,
        }
    }
}

impl LizardEncoderOptions {
    /// Creates new encoder options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the compression level (10-49).
    pub fn level(mut self, level: i32) -> Self {
        self.level = level.clamp(10, 49);
        self
    }

    /// Sets the block size ID (1-7).
    pub fn block_size_id(mut self, id: u8) -> Self {
        self.block_size_id = id.clamp(1, 7);
        self
    }

    /// Sets whether blocks are independent.
    pub fn block_independent(mut self, independent: bool) -> Self {
        self.block_independent = independent;
        self
    }

    /// Returns the maximum block size in bytes.
    pub fn max_block_size(&self) -> usize {
        BLOCK_SIZES[self.block_size_id as usize]
    }
}

/// Lizard encoder that compresses data using the Lizard frame format.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::codec::lizard::{LizardEncoder, LizardEncoderOptions};
/// use std::io::Write;
///
/// let mut output = Vec::new();
/// let mut encoder = LizardEncoder::new(&mut output, LizardEncoderOptions::default());
/// encoder.write_all(b"Hello, World!")?;
/// encoder.try_finish()?;
/// ```
pub struct LizardEncoder<W: Write> {
    inner: W,
    options: LizardEncoderOptions,
    /// Input buffer
    buffer: Vec<u8>,
    /// Maximum block size
    max_block_size: usize,
    /// Whether header has been written
    header_written: bool,
}

impl<W: Write> std::fmt::Debug for LizardEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LizardEncoder")
            .field("options", &self.options)
            .field("buffer_len", &self.buffer.len())
            .field("max_block_size", &self.max_block_size)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> LizardEncoder<W> {
    /// Creates a new Lizard encoder.
    pub fn new(output: W, options: LizardEncoderOptions) -> Self {
        let max_block_size = options.max_block_size();
        Self {
            inner: output,
            options,
            buffer: Vec::with_capacity(max_block_size),
            max_block_size,
            header_written: false,
        }
    }

    /// Writes the Lizard frame header.
    fn write_header(&mut self) -> io::Result<()> {
        // Magic number (4 bytes, little endian)
        self.inner.write_all(&LIZARD_MAGIC.to_le_bytes())?;

        // FLG byte: version 01, flags
        let mut flg: u8 = 0x40; // Version 01 in bits 7-6
        if self.options.block_independent {
            flg |= 0x20;
        }
        if self.options.block_checksum {
            flg |= 0x10;
        }
        if self.options.content_checksum {
            flg |= 0x04;
        }
        self.inner.write_all(&[flg])?;

        // BD byte: block max size
        let bd: u8 = (self.options.block_size_id & 0x07) << 4;
        self.inner.write_all(&[bd])?;

        // Header checksum (simplified)
        let hc = ((flg as u16 + bd as u16) % 256) as u8;
        self.inner.write_all(&[hc])?;

        self.header_written = true;
        Ok(())
    }

    /// Compresses a block of data.
    ///
    /// Currently uses uncompressed sub-blocks for reliability.
    /// Full 5-stream compression can be added in the future.
    fn compress_block(&mut self, data: &[u8]) -> io::Result<Vec<u8>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        let mut output = Vec::with_capacity(data.len() + 5);

        // Write compression level byte (first byte of Lizard block)
        output.push(self.options.level as u8);

        // Use uncompressed sub-blocks
        // Format: LIZARD_FLAG_UNCOMPRESSED (1 byte) + length (3 bytes) + data
        let mut pos = 0;
        let max_subblock_size = 0xFFFFFF; // 24-bit max

        while pos < data.len() {
            let remaining = data.len() - pos;
            let subblock_size = remaining.min(max_subblock_size);

            // Write uncompressed sub-block header
            output.push(LIZARD_FLAG_UNCOMPRESSED);

            // Write 24-bit length
            write_le24(&mut output, subblock_size);

            // Write data
            output.extend_from_slice(&data[pos..pos + subblock_size]);

            pos += subblock_size;
        }

        Ok(output)
    }

    /// Flushes the current buffer as a compressed block.
    fn flush_block(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Compress the block
        let data = std::mem::take(&mut self.buffer);
        let compressed = self.compress_block(&data)?;

        // For Lizard, we always write the compressed block (which uses uncompressed sub-blocks)
        // The format already handles the encoding efficiently
        let block_size = compressed.len() as u32;
        self.inner.write_all(&block_size.to_le_bytes())?;
        self.inner.write_all(&compressed)?;

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        // Write header if not yet written
        if !self.header_written {
            self.write_header()?;
        }

        // Flush remaining data
        self.flush_block()?;

        // Write end mark (4 bytes of 0)
        self.inner.write_all(&0u32.to_le_bytes())?;

        Ok(self.inner)
    }
}

/// Writes a 24-bit little-endian integer to a vector.
fn write_le24(output: &mut Vec<u8>, value: usize) {
    output.push(value as u8);
    output.push((value >> 8) as u8);
    output.push((value >> 16) as u8);
}

impl<W: Write + Send> Write for LizardEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Write header on first write
        if !self.header_written {
            self.write_header()?;
        }

        // Add to buffer
        self.buffer.extend_from_slice(buf);

        // Flush complete blocks
        while self.buffer.len() >= self.max_block_size {
            let block_data: Vec<u8> = self.buffer.drain(..self.max_block_size).collect();
            let compressed = self.compress_block(&block_data)?;

            let block_size = compressed.len() as u32;
            self.inner.write_all(&block_size.to_le_bytes())?;
            self.inner.write_all(&compressed)?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for LizardEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LIZARD
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        // Write header if not yet written
        if !self.header_written {
            self.write_header()?;
        }

        // Flush remaining data
        self.flush_block()?;

        // Write end mark
        self.inner.write_all(&0u32.to_le_bytes())?;
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_lizard_decoder_method_id() {
        let data = vec![0u8; 16];
        let decoder = LizardDecoder::new(Cursor::new(data));
        assert_eq!(decoder.method_id(), method::LIZARD);
    }

    #[test]
    fn test_lizard_encoder_method_id() {
        let output = Vec::new();
        let encoder = LizardEncoder::new(output, LizardEncoderOptions::default());
        assert_eq!(encoder.method_id(), method::LIZARD);
    }

    #[test]
    fn test_lizard_encoder_options() {
        let opts = LizardEncoderOptions::new().level(30);
        assert_eq!(opts.level, 30);

        // Test clamping
        let opts_low = LizardEncoderOptions::new().level(5);
        assert_eq!(opts_low.level, 10);

        let opts_high = LizardEncoderOptions::new().level(100);
        assert_eq!(opts_high.level, 49);

        // Test block size
        let opts_bs = LizardEncoderOptions::new().block_size_id(3);
        assert_eq!(opts_bs.max_block_size(), 1024 * 1024); // 1MB
    }

    #[test]
    fn test_lizard_invalid_magic() {
        // Create data with wrong magic number
        let data = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut decoder = LizardDecoder::new(Cursor::new(data));
        let mut buf = [0u8; 10];
        let result = decoder.read(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_le24() {
        assert_eq!(read_le24(&[0x01, 0x02, 0x03]), 0x030201);
        assert_eq!(read_le24(&[0xFF, 0xFF, 0xFF]), 0xFFFFFF);
        assert_eq!(read_le24(&[0x00, 0x00, 0x00]), 0x000000);
    }

    #[test]
    fn test_write_le24() {
        let mut output = Vec::new();
        write_le24(&mut output, 0x030201);
        assert_eq!(output, vec![0x01, 0x02, 0x03]);

        output.clear();
        write_le24(&mut output, 0xFFFFFF);
        assert_eq!(output, vec![0xFF, 0xFF, 0xFF]);
    }

    // Test helper to create a minimal valid Lizard frame with uncompressed data
    fn create_minimal_lizard_frame(uncompressed_data: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();

        // Magic number
        frame.extend_from_slice(&LIZARD_MAGIC.to_le_bytes());

        // FLG: version 01, block independent, no checksums
        frame.push(0x60); // 01_1_0_0_0_00

        // BD: block max size = 4 (4MB)
        frame.push(0x40); // 0_100_0000

        // Header checksum (simplified - just use 0)
        frame.push(0x00);

        // Block header: uncompressed data (highest bit set)
        let block_size = uncompressed_data.len() as u32 | 0x80000000;
        frame.extend_from_slice(&block_size.to_le_bytes());

        // Block data
        frame.extend_from_slice(uncompressed_data);

        // End mark
        frame.extend_from_slice(&0u32.to_le_bytes());

        frame
    }

    #[test]
    fn test_lizard_uncompressed_frame() {
        let original = b"Hello, World! This is a test of uncompressed Lizard data.";
        let frame = create_minimal_lizard_frame(original);

        let mut decoder = LizardDecoder::new(Cursor::new(frame));
        let mut output = Vec::new();
        decoder
            .read_to_end(&mut output)
            .expect("Failed to decompress");

        assert_eq!(output, original);
    }

    #[test]
    fn test_lizard_encoder_roundtrip_small() {
        // Test roundtrip with small data
        let original = b"Hello, World!";

        let mut compressed = Vec::new();
        {
            let mut encoder = LizardEncoder::new(&mut compressed, LizardEncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = LizardDecoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lizard_encoder_roundtrip_medium() {
        // Test with medium-sized data
        let original: Vec<u8> = b"ABCDEFGHIJKLMNOP".repeat(100);

        let mut compressed = Vec::new();
        {
            let mut encoder = LizardEncoder::new(&mut compressed, LizardEncoderOptions::default());
            encoder.write_all(&original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = LizardDecoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lizard_encoder_empty_input() {
        let mut compressed = Vec::new();
        {
            let encoder = LizardEncoder::new(&mut compressed, LizardEncoderOptions::default());
            encoder.try_finish().unwrap();
        }

        // Should produce valid frame with just header and end mark
        assert!(!compressed.is_empty());

        // Decompress should give empty output
        let mut decoder = LizardDecoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert!(decompressed.is_empty());
    }
}
