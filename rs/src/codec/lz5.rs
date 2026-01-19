//! LZ5 compression codec (pure Rust implementation).
//!
//! LZ5 is an LZ77-type compressor with a fixed, byte-oriented encoding.
//! It offers compression ratios comparable to zip/zlib with very fast
//! decompression speeds.
//!
//! This implementation supports decompression of LZ5-compressed data
//! as used in 7-Zip archives created with 7-Zip-zstd.
//!
//! # Method ID
//!
//! LZ5 uses method ID `0x04, 0xF7, 0x11, 0x05` (7-Zip-zstd convention).
//!
//! # Format
//!
//! LZ5 uses a frame format with:
//! - Magic number: 0x184D2205
//! - Frame descriptor (flags, block size, optional content size)
//! - Data blocks (each with size header + compressed/raw data)
//! - End mark (4 bytes of 0)
//! - Optional content checksum

use std::io::{self, Read, Write};

use super::{Decoder, Encoder, method};

/// LZ5 frame magic number
const LZ5_MAGIC: u32 = 0x184D2205;

/// Minimum match length in LZ5
const MIN_MATCH: usize = 3;

/// Maximum block sizes indexed by block size field value
const BLOCK_SIZES: [usize; 8] = [
    0,                 // 0: N/A
    64 * 1024,         // 1: 64 KB
    256 * 1024,        // 2: 256 KB
    1024 * 1024,       // 3: 1 MB
    4 * 1024 * 1024,   // 4: 4 MB
    16 * 1024 * 1024,  // 5: 16 MB
    64 * 1024 * 1024,  // 6: 64 MB
    256 * 1024 * 1024, // 7: 256 MB
];

/// LZ5 decoder that decompresses LZ5 frame format data.
pub struct Lz5Decoder<R: Read> {
    inner: R,
    /// Output buffer (sliding window for match copying)
    buffer: Vec<u8>,
    /// Current position in buffer (data available up to this point)
    buffer_pos: usize,
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
    content_size: Option<u64>,
    /// Last offset for repeat offset codeword
    last_offset: usize,
}

impl<R: Read> std::fmt::Debug for Lz5Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lz5Decoder")
            .field("buffer_pos", &self.buffer_pos)
            .field("read_pos", &self.read_pos)
            .field("finished", &self.finished)
            .field("max_block_size", &self.max_block_size)
            .finish()
    }
}

impl<R: Read + Send> Lz5Decoder<R> {
    /// Creates a new LZ5 decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: input,
            buffer: Vec::new(),
            buffer_pos: 0,
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

    /// Parses the LZ5 frame header.
    fn parse_header(&mut self) -> io::Result<()> {
        // Read magic number (4 bytes, little endian)
        let mut magic_bytes = [0u8; 4];
        self.inner.read_exact(&mut magic_bytes)?;
        let magic = u32::from_le_bytes(magic_bytes);

        if magic != LZ5_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Invalid LZ5 magic number: 0x{:08X}, expected 0x{:08X}",
                    magic, LZ5_MAGIC
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
                format!("Unsupported LZ5 version: {}", version),
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

        // Highest bit indicates uncompressed data
        let is_uncompressed = (block_header & 0x80000000) != 0;
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
            self.buffer_pos = 0;
            self.read_pos = 0;
        }

        if is_uncompressed {
            // Copy uncompressed data directly
            self.buffer.extend_from_slice(&block_data);
            self.buffer_pos = self.buffer.len();
        } else {
            // Decompress block
            self.decompress_block(&block_data)?;
        }

        Ok(true)
    }

    /// Decompresses a single LZ5 block.
    fn decompress_block(&mut self, data: &[u8]) -> io::Result<()> {
        let mut pos = 0;
        let len = data.len();

        while pos < len {
            let token = data[pos];
            pos += 1;

            // Parse codeword type from token
            let (literal_len, match_len, offset) = if (token & 0x80) != 0 {
                // Type 1: [1_OO_LL_MMM] [OOOOOOOO] - 10-bit offset
                if pos >= len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete LZ5 block",
                    ));
                }
                let offset_high = ((token >> 5) & 0x03) as usize;
                let literal_len = ((token >> 3) & 0x03) as usize;
                let match_len = (token & 0x07) as usize;
                let offset_low = data[pos] as usize;
                pos += 1;
                let offset = (offset_high << 8) | offset_low;
                (literal_len, match_len, Some(offset))
            } else if (token & 0xC0) == 0x00 {
                // Type 2: [00_LLL_MMM] [OOOOOOOO] [OOOOOOOO] - 16-bit offset
                if pos + 1 >= len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete LZ5 block",
                    ));
                }
                let literal_len = ((token >> 3) & 0x07) as usize;
                let match_len = (token & 0x07) as usize;
                let offset = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
                pos += 2;
                (literal_len, match_len, Some(offset))
            } else if (token & 0xE0) == 0x40 {
                // Type 3: [010_LL_MMM] [OOOOOOOO] [OOOOOOOO] [OOOOOOOO] - 24-bit offset
                if pos + 2 >= len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete LZ5 block",
                    ));
                }
                let literal_len = ((token >> 3) & 0x03) as usize;
                let match_len = (token & 0x07) as usize;
                let offset = data[pos] as usize
                    | ((data[pos + 1] as usize) << 8)
                    | ((data[pos + 2] as usize) << 16);
                pos += 3;
                (literal_len, match_len, Some(offset))
            } else {
                // Type 4: [011_LL_MMM] - last offset
                let literal_len = ((token >> 3) & 0x03) as usize;
                let match_len = (token & 0x07) as usize;
                (literal_len, match_len, None)
            };

            // Read extended literal length
            let max_literal = if (token & 0xC0) == 0x00 { 7 } else { 3 };
            let literal_len = if literal_len == max_literal {
                self.read_extended_length(data, &mut pos, literal_len)?
            } else {
                literal_len
            };

            // Copy literals
            if literal_len > 0 {
                if pos + literal_len > len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete literals",
                    ));
                }
                self.buffer.extend_from_slice(&data[pos..pos + literal_len]);
                pos += literal_len;
            }

            // Check if this is the end of the block (last sequence has no match)
            if pos >= len {
                break;
            }

            // Get actual offset
            let actual_offset = match offset {
                Some(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid zero offset",
                    ));
                }
                Some(o) => {
                    self.last_offset = o;
                    o
                }
                None => self.last_offset,
            };

            if actual_offset == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "No last offset available",
                ));
            }

            // Read extended match length
            let match_len = if match_len == 7 {
                self.read_extended_length(data, &mut pos, match_len)?
            } else {
                match_len
            };

            // Add minimum match length
            let match_len = match_len + MIN_MATCH;

            // Perform match copy
            if actual_offset > self.buffer.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Match offset {} exceeds buffer size {}",
                        actual_offset,
                        self.buffer.len()
                    ),
                ));
            }

            let start = self.buffer.len() - actual_offset;
            for i in 0..match_len {
                let byte = self.buffer[start + (i % actual_offset)];
                self.buffer.push(byte);
            }
        }

        self.buffer_pos = self.buffer.len();
        Ok(())
    }

    /// Reads an extended length value using the 255-continuation scheme.
    fn read_extended_length(
        &self,
        data: &[u8],
        pos: &mut usize,
        initial: usize,
    ) -> io::Result<usize> {
        let mut length = initial;
        loop {
            if *pos >= data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Incomplete extended length",
                ));
            }
            let byte = data[*pos];
            *pos += 1;
            length += byte as usize;
            if byte != 255 {
                break;
            }
        }
        Ok(length)
    }
}

impl<R: Read + Send> Read for Lz5Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished {
            return Ok(0);
        }

        // Parse header on first read
        if !self.header_parsed {
            self.parse_header()?;
        }

        // Check if we have data in buffer to return
        if self.read_pos < self.buffer_pos {
            let available = self.buffer_pos - self.read_pos;
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

            if self.read_pos < self.buffer_pos {
                let available = self.buffer_pos - self.read_pos;
                let to_copy = buf.len().min(available);
                buf[..to_copy]
                    .copy_from_slice(&self.buffer[self.read_pos..self.read_pos + to_copy]);
                self.read_pos += to_copy;
                return Ok(to_copy);
            }
        }
    }
}

impl<R: Read + Send> Decoder for Lz5Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ5
    }
}

/// LZ5 encoder options.
#[derive(Debug, Clone)]
pub struct Lz5EncoderOptions {
    /// Compression level (1-15).
    pub level: i32,
    /// Block size ID (1-7). Default: 4 (4MB).
    pub block_size_id: u8,
    /// Whether blocks are independent (can be decompressed in parallel).
    pub block_independent: bool,
    /// Whether to include content checksum.
    pub content_checksum: bool,
    /// Whether to include block checksums.
    pub block_checksum: bool,
}

impl Default for Lz5EncoderOptions {
    fn default() -> Self {
        Self {
            level: 6,
            block_size_id: 4, // 4MB blocks
            block_independent: true,
            content_checksum: false,
            block_checksum: false,
        }
    }
}

impl Lz5EncoderOptions {
    /// Creates new encoder options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the compression level (1-15).
    pub fn level(mut self, level: i32) -> Self {
        self.level = level.clamp(1, 15);
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

/// Hash table size for match finding (2^20 entries).
const HASH_TABLE_SIZE: usize = 1 << 20;
const HASH_MASK: usize = HASH_TABLE_SIZE - 1;

/// LZ5 encoder that compresses data using the LZ5 frame format.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::codec::lz5::{Lz5Encoder, Lz5EncoderOptions};
/// use std::io::Write;
///
/// let mut output = Vec::new();
/// let mut encoder = Lz5Encoder::new(&mut output, Lz5EncoderOptions::default());
/// encoder.write_all(b"Hello, World!")?;
/// encoder.try_finish()?;
/// ```
pub struct Lz5Encoder<W: Write> {
    inner: W,
    options: Lz5EncoderOptions,
    /// Input buffer
    buffer: Vec<u8>,
    /// Maximum block size
    max_block_size: usize,
    /// Whether header has been written
    header_written: bool,
    /// Hash table for match finding (position indexed by hash)
    hash_table: Vec<u32>,
    /// Last match offset (for repeat offset token)
    last_offset: usize,
}

impl<W: Write> std::fmt::Debug for Lz5Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lz5Encoder")
            .field("options", &self.options)
            .field("buffer_len", &self.buffer.len())
            .field("max_block_size", &self.max_block_size)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> Lz5Encoder<W> {
    /// Creates a new LZ5 encoder.
    pub fn new(output: W, options: Lz5EncoderOptions) -> Self {
        let max_block_size = options.max_block_size();
        Self {
            inner: output,
            options,
            buffer: Vec::with_capacity(max_block_size),
            max_block_size,
            header_written: false,
            hash_table: vec![0; HASH_TABLE_SIZE],
            last_offset: 0,
        }
    }

    /// Writes the LZ5 frame header.
    fn write_header(&mut self) -> io::Result<()> {
        // Magic number (4 bytes, little endian)
        self.inner.write_all(&LZ5_MAGIC.to_le_bytes())?;

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

        // Header checksum (simplified - xxhash of FLG and BD)
        // For simplicity, we use a basic checksum
        let hc = ((flg as u16 + bd as u16) % 256) as u8;
        self.inner.write_all(&[hc])?;

        self.header_written = true;
        Ok(())
    }

    /// Computes a 4-byte hash for match finding.
    #[inline]
    fn hash4(data: &[u8]) -> usize {
        if data.len() < 4 {
            return 0;
        }
        let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        // Simple multiplicative hash
        ((v.wrapping_mul(2654435761)) >> 12) as usize & HASH_MASK
    }

    /// Finds the best match at the current position.
    fn find_match(&self, data: &[u8], pos: usize) -> Option<(usize, usize)> {
        if pos + 4 > data.len() {
            return None;
        }

        let hash = Self::hash4(&data[pos..]);
        let match_pos = self.hash_table[hash] as usize;

        // Check if match position is valid and within window
        if match_pos == 0 || match_pos > pos {
            return None;
        }

        let offset = pos - match_pos;
        if offset == 0 || offset > 0xFFFFFF {
            return None;
        }

        // Calculate match length
        let mut length = 0;
        let max_len = (data.len() - pos).min(65535);
        while length < max_len && data[match_pos + length] == data[pos + length] {
            length += 1;
        }

        if length >= MIN_MATCH {
            Some((offset, length))
        } else {
            None
        }
    }

    /// Compresses a block of data using LZ5 algorithm.
    fn compress_block(&mut self, data: &[u8]) -> io::Result<Vec<u8>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        let mut output = Vec::with_capacity(data.len());
        let mut pos = 0;
        let mut lit_start = 0;

        // Reset hash table for block-independent mode
        if self.options.block_independent {
            self.hash_table.fill(0);
            self.last_offset = 0;
        }

        while pos < data.len() {
            // Try to find a match at current position
            let match_result = self.find_match(data, pos);

            if let Some((match_offset, match_len)) = match_result {
                // Found a match - write accumulated literals + this match
                let literals = &data[lit_start..pos];
                self.encode_sequence(&mut output, literals, match_offset, match_len)?;

                // Update hash table for matched region
                let match_end = pos + match_len;
                for i in pos..match_end.min(data.len().saturating_sub(3)) {
                    if i + 4 <= data.len() {
                        let hash = Self::hash4(&data[i..]);
                        self.hash_table[hash] = i as u32;
                    }
                }

                self.last_offset = match_offset;
                pos = match_end;
                lit_start = pos;
            } else {
                // No match found, accumulate as literal
                if pos + 4 <= data.len() {
                    let hash = Self::hash4(&data[pos..]);
                    self.hash_table[hash] = pos as u32;
                }
                pos += 1;
            }
        }

        // Write any remaining literals at the end (last sequence)
        if lit_start < data.len() {
            let remaining = &data[lit_start..];
            self.write_final_sequence(&mut output, remaining);
        }

        Ok(output)
    }

    /// Writes the final sequence (literals only, no match needed).
    fn write_final_sequence(&self, output: &mut Vec<u8>, literals: &[u8]) {
        if literals.is_empty() {
            return;
        }

        let lit_len = literals.len();
        // Use Type 2 token format: [00_LLL_MMM]
        // The decoder will read the offset but won't use it since we'll be at EOF after literals
        let ll = lit_len.min(7);
        let mm = 0u8; // No match for final sequence
        let token = (ll as u8) << 3 | mm;
        output.push(token);

        // 16-bit offset (required by format, but won't be used for match copy)
        output.extend_from_slice(&1u16.to_le_bytes());

        // Extended literal length
        if ll == 7 && lit_len > 7 {
            self.write_extended_length(output, lit_len - 7);
        }

        // Literals - this is the last thing in the block
        output.extend_from_slice(literals);
    }

    /// Encodes a single LZ5 sequence (literals + match).
    fn encode_sequence(
        &self,
        output: &mut Vec<u8>,
        literals: &[u8],
        offset: usize,
        match_len: usize,
    ) -> io::Result<()> {
        let lit_len = literals.len();
        let ml = match_len.saturating_sub(MIN_MATCH); // Match length minus minimum

        // Choose token type based on offset size
        if offset <= 0x3FF {
            // Type 1: 10-bit offset [1_OO_LL_MMM] [OOOOOOOO]
            let ll = lit_len.min(3);
            let mm = ml.min(7);
            let token = 0x80 | ((offset >> 8) as u8 & 0x03) << 5 | (ll as u8) << 3 | mm as u8;
            output.push(token);
            output.push(offset as u8);

            // Extended literal length
            if ll == 3 && lit_len > 3 {
                self.write_extended_length(output, lit_len - 3);
            }

            // Literals
            output.extend_from_slice(literals);

            // Extended match length
            if mm == 7 && ml > 7 {
                self.write_extended_length(output, ml - 7);
            }
        } else if offset <= 0xFFFF {
            // Type 2: 16-bit offset [00_LLL_MMM] [OOOOOOOO] [OOOOOOOO]
            let ll = lit_len.min(7);
            let mm = ml.min(7);
            let token = (ll as u8) << 3 | mm as u8;
            output.push(token);
            output.extend_from_slice(&(offset as u16).to_le_bytes());

            // Extended literal length
            if ll == 7 && lit_len > 7 {
                self.write_extended_length(output, lit_len - 7);
            }

            // Literals
            output.extend_from_slice(literals);

            // Extended match length
            if mm == 7 && ml > 7 {
                self.write_extended_length(output, ml - 7);
            }
        } else {
            // Type 3: 24-bit offset [010_LL_MMM] [OOOOOOOO] [OOOOOOOO] [OOOOOOOO]
            let ll = lit_len.min(3);
            let mm = ml.min(7);
            let token = 0x40 | (ll as u8) << 3 | mm as u8;
            output.push(token);
            output.push(offset as u8);
            output.push((offset >> 8) as u8);
            output.push((offset >> 16) as u8);

            // Extended literal length
            if ll == 3 && lit_len > 3 {
                self.write_extended_length(output, lit_len - 3);
            }

            // Literals
            output.extend_from_slice(literals);

            // Extended match length
            if mm == 7 && ml > 7 {
                self.write_extended_length(output, ml - 7);
            }
        }

        Ok(())
    }

    /// Writes an extended length value using the 255-continuation scheme.
    fn write_extended_length(&self, output: &mut Vec<u8>, mut length: usize) {
        while length >= 255 {
            output.push(255);
            length -= 255;
        }
        output.push(length as u8);
    }

    /// Flushes the current buffer as a compressed block.
    fn flush_block(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Compress the block
        let data = std::mem::take(&mut self.buffer);
        let compressed = self.compress_block(&data)?;

        // Decide whether to use compressed or uncompressed
        if compressed.len() >= data.len() {
            // Store uncompressed (set high bit in block size)
            let block_size = data.len() as u32 | 0x80000000;
            self.inner.write_all(&block_size.to_le_bytes())?;
            self.inner.write_all(&data)?;
        } else {
            // Store compressed
            let block_size = compressed.len() as u32;
            self.inner.write_all(&block_size.to_le_bytes())?;
            self.inner.write_all(&compressed)?;
        }

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

impl<W: Write + Send> Write for Lz5Encoder<W> {
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

            if compressed.len() >= block_data.len() {
                let block_size = block_data.len() as u32 | 0x80000000;
                self.inner.write_all(&block_size.to_le_bytes())?;
                self.inner.write_all(&block_data)?;
            } else {
                let block_size = compressed.len() as u32;
                self.inner.write_all(&block_size.to_le_bytes())?;
                self.inner.write_all(&compressed)?;
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for Lz5Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ5
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
    fn test_lz5_decoder_method_id() {
        let data = vec![0u8; 16];
        let decoder = Lz5Decoder::new(Cursor::new(data));
        assert_eq!(decoder.method_id(), method::LZ5);
    }

    #[test]
    fn test_lz5_encoder_method_id() {
        let output = Vec::new();
        let encoder = Lz5Encoder::new(output, Lz5EncoderOptions::default());
        assert_eq!(encoder.method_id(), method::LZ5);
    }

    #[test]
    fn test_lz5_encoder_options() {
        let opts = Lz5EncoderOptions::new().level(9);
        assert_eq!(opts.level, 9);

        // Test clamping
        let opts_low = Lz5EncoderOptions::new().level(0);
        assert_eq!(opts_low.level, 1);

        let opts_high = Lz5EncoderOptions::new().level(20);
        assert_eq!(opts_high.level, 15);

        // Test block size
        let opts_bs = Lz5EncoderOptions::new().block_size_id(3);
        assert_eq!(opts_bs.max_block_size(), 1024 * 1024); // 1MB
    }

    #[test]
    fn test_lz5_invalid_magic() {
        // Create data with wrong magic number
        let data = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut decoder = Lz5Decoder::new(Cursor::new(data));
        let mut buf = [0u8; 10];
        let result = decoder.read(&mut buf);
        assert!(result.is_err());
    }

    // Test helper to create a minimal valid LZ5 frame
    fn create_minimal_lz5_frame(uncompressed_data: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();

        // Magic number
        frame.extend_from_slice(&LZ5_MAGIC.to_le_bytes());

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
    fn test_lz5_uncompressed_block() {
        let original = b"Hello, World! This is a test of uncompressed LZ5 data.";
        let frame = create_minimal_lz5_frame(original);

        let mut decoder = Lz5Decoder::new(Cursor::new(frame));
        let mut output = Vec::new();
        decoder
            .read_to_end(&mut output)
            .expect("Failed to decompress");

        assert_eq!(output, original);
    }

    #[test]
    fn test_lz5_encoder_roundtrip_small() {
        // Test roundtrip with small data (will be stored uncompressed)
        let original = b"Hello, World!";

        let mut compressed = Vec::new();
        {
            let mut encoder = Lz5Encoder::new(&mut compressed, Lz5EncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = Lz5Decoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz5_encoder_roundtrip_repetitive() {
        // Test with repetitive data that should compress well
        let original: Vec<u8> = b"ABCDEFGHIJKLMNOP".repeat(1000);

        let mut compressed = Vec::new();
        {
            let mut encoder = Lz5Encoder::new(&mut compressed, Lz5EncoderOptions::default());
            encoder.write_all(&original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = Lz5Decoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz5_encoder_roundtrip_random() {
        // Test with pseudo-random data (won't compress well, stored as uncompressed)
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut original = vec![0u8; 1000];
        for (i, byte) in original.iter_mut().enumerate() {
            let mut hasher = DefaultHasher::new();
            i.hash(&mut hasher);
            *byte = hasher.finish() as u8;
        }

        let mut compressed = Vec::new();
        {
            let mut encoder = Lz5Encoder::new(&mut compressed, Lz5EncoderOptions::default());
            encoder.write_all(&original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = Lz5Decoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz5_encoder_empty_input() {
        let mut compressed = Vec::new();
        {
            let encoder = Lz5Encoder::new(&mut compressed, Lz5EncoderOptions::default());
            encoder.try_finish().unwrap();
        }

        // Should produce valid frame with just header and end mark
        assert!(!compressed.is_empty());

        // Decompress should give empty output
        let mut decoder = Lz5Decoder::new(Cursor::new(&compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_lz5_hash4() {
        // Test the hash function produces valid indices
        let data = b"test";
        let hash = Lz5Encoder::<Vec<u8>>::hash4(data);
        assert!(hash < HASH_TABLE_SIZE);

        // Different data should (usually) produce different hashes
        let data2 = b"abcd";
        let hash2 = Lz5Encoder::<Vec<u8>>::hash4(data2);
        assert!(hash2 < HASH_TABLE_SIZE);
        // Note: hash collision is possible, so we don't assert hash != hash2
    }
}
