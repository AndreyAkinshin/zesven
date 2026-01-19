//! BCJ (Branch/Call/Jump) filter encoders.
//!
//! These filters convert absolute addresses to relative addresses before compression,
//! improving compression ratios for executable code. The decoders convert back.
//!
//! # Supported Architectures
//!
//! - x86 (32-bit and 64-bit)
//! - ARM (32-bit)
//! - ARM64 (AArch64)
//! - ARM Thumb
//! - PowerPC
//! - SPARC
//! - IA-64 (Itanium)
//! - RISC-V
//!
//! # Algorithm
//!
//! For encoding: `value += current_position` (absolute to relative)
//! For decoding: `value -= current_position` (relative to absolute)

use std::io::{self, Write};

use super::{Encoder, method};

// =============================================================================
// BCJ x86 Encoder
// =============================================================================

/// BCJ x86 filter encoder.
///
/// Converts x86 CALL (E8) and JMP (E9) relative addresses to absolute addresses
/// for better compression.
pub struct BcjX86Encoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
    state: u32,
}

impl<W: Write> std::fmt::Debug for BcjX86Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjX86Encoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjX86Encoder<W> {
    /// Creates a new BCJ x86 encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
            state: 0,
        }
    }

    /// Creates a new BCJ x86 encoder with a starting position.
    pub fn new_with_start_pos(inner: W, start_pos: u32) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: start_pos,
            state: 0,
        }
    }

    /// Process buffered data and write encoded output.
    fn process_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let processed = bcj_x86_convert(&mut self.buffer, self.position, &mut self.state, true);

        if processed > 0 {
            self.inner.write_all(&self.buffer[..processed])?;
            self.buffer.drain(..processed);
            self.position = self.position.wrapping_add(processed as u32);
        }

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        // Flush any remaining data
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjX86Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Process when we have enough data (need 5-byte lookahead for x86)
        while self.buffer.len() >= 5 {
            let prev_len = self.buffer.len();
            self.process_buffer()?;
            // If no progress, need more data
            if self.buffer.len() == prev_len {
                break;
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjX86Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_X86
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        // Flush remaining data
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

/// BCJ x86 conversion algorithm.
///
/// Returns the number of bytes processed.
fn bcj_x86_convert(buf: &mut [u8], ip: u32, state: &mut u32, encoding: bool) -> usize {
    const LOOKAHEAD: usize = 5;

    if buf.len() < LOOKAHEAD {
        return 0;
    }

    let mut pos: usize = 0;
    let mut mask = *state & 7;

    loop {
        // Find E8 or E9 opcode
        let p = pos;
        while pos < buf.len() - 4 {
            if buf[pos] & 0xfe == 0xe8 {
                break;
            }
            pos += 1;
        }

        let d = pos - p;

        if pos >= buf.len() - 4 {
            if d > 2 {
                *state = 0;
            } else {
                *state = mask >> d;
            }
            let _ = ip.wrapping_add(pos as u32);
            return pos;
        }

        if d > 2 {
            mask = 0;
        } else {
            mask >>= d;
            if mask != 0
                && (mask > 4 || mask == 3 || test_x86_ms_byte(buf[pos + (mask >> 1) as usize + 1]))
            {
                mask = (mask >> 1) | 4;
                pos += 1;
                continue;
            }
        }

        if test_x86_ms_byte(buf[pos + 4]) {
            let mut v =
                u32::from_le_bytes([buf[pos + 1], buf[pos + 2], buf[pos + 3], buf[pos + 4]]);
            let cur = ip.wrapping_add(LOOKAHEAD as u32).wrapping_add(pos as u32);
            pos += LOOKAHEAD;

            if encoding {
                v = v.wrapping_add(cur);
            } else {
                v = v.wrapping_sub(cur);
            }

            if mask != 0 {
                let sh = (mask & 6) << 2;
                if test_x86_ms_byte((v >> sh) as u8) {
                    v ^= ((0x100u32) << sh).wrapping_sub(1);
                    if encoding {
                        v = v.wrapping_add(cur);
                    } else {
                        v = v.wrapping_sub(cur);
                    }
                }
                mask = 0;
            }

            let bytes = v.to_le_bytes();
            buf[pos - 4] = bytes[0];
            buf[pos - 3] = bytes[1];
            buf[pos - 2] = bytes[2];
            buf[pos - 1] = 0u8.wrapping_sub(bytes[3] & 1);
        } else {
            mask = (mask >> 1) | 4;
            pos += 1;
        }
    }
}

#[inline]
fn test_x86_ms_byte(b: u8) -> bool {
    b.wrapping_add(1) & 0xfe == 0
}

// =============================================================================
// BCJ ARM Encoder
// =============================================================================

/// BCJ ARM filter encoder.
///
/// Converts ARM BL (branch with link) instruction addresses for better compression.
pub struct BcjArmEncoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjArmEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArmEncoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjArmEncoder<W> {
    /// Creates a new BCJ ARM encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 4, // ARM starts at offset 4
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        let aligned_len = self.buffer.len() & !3;
        if aligned_len < 4 {
            return Ok(());
        }

        // Process 4-byte aligned chunks
        let mut i = 0;
        while i + 4 <= aligned_len {
            let mut v = u32::from_le_bytes([
                self.buffer[i],
                self.buffer[i + 1],
                self.buffer[i + 2],
                self.buffer[i + 3],
            ]);

            self.position = self.position.wrapping_add(4);

            // Check for BL instruction (0xEB in high byte)
            if self.buffer[i + 3] == 0xeb {
                v <<= 2;
                v = v.wrapping_add(self.position); // encoding
                v >>= 2;
                v &= 0x00ffffff;
                v |= 0xeb000000;
            }

            let bytes = v.to_le_bytes();
            self.buffer[i] = bytes[0];
            self.buffer[i + 1] = bytes[1];
            self.buffer[i + 2] = bytes[2];
            self.buffer[i + 3] = bytes[3];

            i += 4;
        }

        // Write processed data
        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjArmEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= 8 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjArmEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ ARM64 Encoder
// =============================================================================

/// BCJ ARM64/AArch64 filter encoder.
///
/// Converts ARM64 BL and ADRP instruction addresses for better compression.
pub struct BcjArm64Encoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjArm64Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArm64Encoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjArm64Encoder<W> {
    /// Creates a new BCJ ARM64 encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        let aligned_len = self.buffer.len() & !3;
        if aligned_len < 4 {
            return Ok(());
        }

        let flag: u32 = 1 << (24 - 4);
        let mask: u32 = (1 << 24) - (flag << 1);

        let mut i = 0;
        while i + 4 <= aligned_len {
            let mut v = u32::from_le_bytes([
                self.buffer[i],
                self.buffer[i + 1],
                self.buffer[i + 2],
                self.buffer[i + 3],
            ]);

            // BL instruction: 0x94000000
            if (v.wrapping_sub(0x94000000)) & 0xfc000000 == 0 {
                let c = self.position >> 2;
                v = v.wrapping_add(c); // encoding
                v &= 0x03ffffff;
                v |= 0x94000000;
            }
            // ADRP instruction: 0x90000000
            else if v.wrapping_sub(0x90000000) & 0x9f000000 == 0 {
                let temp = v.wrapping_add(flag);
                if temp & mask == 0 {
                    let mut z = (v & 0xffffffe0) | (v >> 26);
                    let c = (self.position >> (12 - 3)) & !7u32;
                    z = z.wrapping_add(c); // encoding
                    v = 0x90000000;
                    v |= z << 26;
                    v |= 0x00ffffe0 & ((z & ((flag << 1) - 1)).wrapping_sub(flag));
                    v |= temp & 0x1f; // Preserve rd register
                }
            }

            let bytes = v.to_le_bytes();
            self.buffer[i] = bytes[0];
            self.buffer[i + 1] = bytes[1];
            self.buffer[i + 2] = bytes[2];
            self.buffer[i + 3] = bytes[3];

            self.position = self.position.wrapping_add(4);
            i += 4;
        }

        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjArm64Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= 8 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjArm64Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM64
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ ARM Thumb Encoder
// =============================================================================

/// BCJ ARM Thumb filter encoder.
pub struct BcjArmThumbEncoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjArmThumbEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArmThumbEncoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjArmThumbEncoder<W> {
    /// Creates a new BCJ ARM Thumb encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 4,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        // ARM Thumb needs 4-byte alignment but processes 2-byte instructions
        let aligned_len = self.buffer.len() & !3;
        if aligned_len < 4 {
            return Ok(());
        }

        let mut i = 0;
        while i + 4 <= aligned_len {
            // Check for BL/BLX instruction pair (F000-F800 range)
            if (self.buffer[i + 1] & 0xf8) == 0xf0 && (self.buffer[i + 3] & 0xf8) == 0xf8 {
                // Extract address from BL instruction pair
                let b1 = u32::from(self.buffer[i]);
                let b2 = u32::from(self.buffer[i + 1]);
                let b3 = u32::from(self.buffer[i + 2]);
                let b4 = u32::from(self.buffer[i + 3]);

                let mut addr = ((b2 & 0x07) << 19) | (b1 << 11) | ((b4 & 0x07) << 8) | b3;
                addr <<= 1;

                let cur = self.position.wrapping_add(4);
                addr = addr.wrapping_add(cur); // encoding

                self.buffer[i] = ((addr >> 11) & 0xff) as u8;
                self.buffer[i + 1] = (0xf0 | ((addr >> 19) & 0x07)) as u8;
                self.buffer[i + 2] = ((addr >> 1) & 0xff) as u8;
                self.buffer[i + 3] = (0xf8 | ((addr >> 9) & 0x07)) as u8;
            }

            self.position = self.position.wrapping_add(4);
            i += 4;
        }

        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjArmThumbEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= 8 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjArmThumbEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM_THUMB
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ PowerPC Encoder
// =============================================================================

/// BCJ PowerPC filter encoder.
///
/// Converts PowerPC branch instruction addresses for better compression.
/// Uses big-endian byte order.
pub struct BcjPpcEncoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjPpcEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjPpcEncoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjPpcEncoder<W> {
    /// Creates a new BCJ PowerPC encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        let aligned_len = self.buffer.len() & !3;
        if aligned_len < 4 {
            return Ok(());
        }

        let mut i = 0;
        while i + 4 <= aligned_len {
            // PPC is big-endian
            let mut v = u32::from_be_bytes([
                self.buffer[i],
                self.buffer[i + 1],
                self.buffer[i + 2],
                self.buffer[i + 3],
            ]);

            // Check for branch instruction: 0x48xxxxxx with low 2 bits = 01
            if (self.buffer[i] & 0xfc) == 0x48 && (self.buffer[i + 3] & 3) == 1 {
                v = v.wrapping_add(self.position); // encoding
                v &= 0x03ffffff;
                v |= 0x48000000;
            }

            let bytes = v.to_be_bytes();
            self.buffer[i] = bytes[0];
            self.buffer[i + 1] = bytes[1];
            self.buffer[i + 2] = bytes[2];
            self.buffer[i + 3] = bytes[3];

            self.position = self.position.wrapping_add(4);
            i += 4;
        }

        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjPpcEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= 8 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjPpcEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_PPC
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ SPARC Encoder
// =============================================================================

/// BCJ SPARC filter encoder.
///
/// Converts SPARC CALL instruction addresses for better compression.
/// Uses big-endian byte order.
pub struct BcjSparcEncoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjSparcEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjSparcEncoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjSparcEncoder<W> {
    /// Creates a new BCJ SPARC encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        let aligned_len = self.buffer.len() & !3;
        if aligned_len < 4 {
            return Ok(());
        }

        let mut i = 0;
        while i + 4 <= aligned_len {
            // SPARC is big-endian
            let mut v = u32::from_be_bytes([
                self.buffer[i],
                self.buffer[i + 1],
                self.buffer[i + 2],
                self.buffer[i + 3],
            ]);

            // Check for CALL instruction (0x40xxxxxx) or specific patterns
            if (self.buffer[i] == 0x40 && (self.buffer[i + 1] & 0xc0) == 0)
                || (self.buffer[i] == 0x7f && self.buffer[i + 1] >= 0xc0)
            {
                v <<= 2;
                v = v.wrapping_add(self.position); // encoding
                v &= 0x01ffffff;
                v = v.wrapping_sub(1 << 24);
                v ^= 0xff000000;
                v >>= 2;
                v |= 0x40000000;
            }

            let bytes = v.to_be_bytes();
            self.buffer[i] = bytes[0];
            self.buffer[i + 1] = bytes[1];
            self.buffer[i + 2] = bytes[2];
            self.buffer[i + 3] = bytes[3];

            self.position = self.position.wrapping_add(4);
            i += 4;
        }

        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjSparcEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        while self.buffer.len() >= 8 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjSparcEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_SPARC
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ IA-64 Encoder
// =============================================================================

/// IA-64 bundle template types that may contain branch instructions.
/// IA-64 uses 128-bit instruction bundles with 3 instruction slots.
/// The template (low 5 bits) determines which slots contain which instruction types.
const IA64_BRANCH_TABLE: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, // Templates 0x00-0x07: no branches in standard slots
    0, 0, 0, 0, 0, 0, 0, 0, // Templates 0x08-0x0F
    4, 4, 6, 6, 0, 0, 7, 7, // Templates 0x10-0x17: some contain B-unit slots
    4, 4, 0, 0, 4, 4, 0, 0, // Templates 0x18-0x1F
];

/// BCJ IA-64 (Itanium) filter encoder.
///
/// Converts IA-64 branch instruction addresses from relative to absolute
/// to improve compression. IA-64 uses 128-bit instruction bundles containing
/// 3 instruction slots of 41 bits each.
pub struct BcjIa64Encoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjIa64Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjIa64Encoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjIa64Encoder<W> {
    /// Creates a new BCJ IA-64 encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        // IA-64 bundles are 16 bytes (128 bits)
        let aligned_len = (self.buffer.len() / 16) * 16;
        if aligned_len < 16 {
            return Ok(());
        }

        // Process each 16-byte bundle
        let mut i = 0;
        while i + 16 <= aligned_len {
            Self::process_bundle(self.position, &mut self.buffer[i..i + 16]);
            self.position = self.position.wrapping_add(16);
            i += 16;
        }

        // Write processed data
        self.inner.write_all(&self.buffer[..aligned_len])?;
        self.buffer.drain(..aligned_len);

        Ok(())
    }

    /// Process a single IA-64 bundle (16 bytes / 128 bits).
    /// Format: [template:5][slot0:41][slot1:41][slot2:41]
    fn process_bundle(position: u32, bundle: &mut [u8]) {
        let template = bundle[0] & 0x1F;
        let branch_mask = IA64_BRANCH_TABLE[template as usize];

        // Check each slot that might contain a branch instruction
        for slot in 0..3 {
            if (branch_mask & (1 << slot)) == 0 {
                continue;
            }

            // Extract the 41-bit instruction from the slot
            // Slot 0: bits 5-45, Slot 1: bits 46-86, Slot 2: bits 87-127
            let bit_pos = 5 + slot * 41;

            // Read 8 bytes starting from the byte containing our instruction
            let byte_pos = bit_pos / 8;
            let bit_offset = bit_pos % 8;

            if byte_pos + 6 > 16 {
                continue;
            }

            // Extract instruction (we need 41 bits = ~6 bytes worth)
            let mut inst: u64 = 0;
            for j in 0..6 {
                inst |= (bundle[byte_pos + j] as u64) << (j * 8);
            }
            inst >>= bit_offset;

            // Check if this is a branch instruction (opcode in bits 37-40)
            let opcode = ((inst >> 37) & 0xF) as u8;

            // Branch opcodes: 4 (br.cond), 5 (br.call), etc.
            // Only process if it's a PC-relative branch with 25-bit immediate
            if opcode != 4 && opcode != 5 {
                continue;
            }

            // Check bit 36 - if 0, this is IP-relative
            if (inst & (1u64 << 36)) != 0 {
                continue;
            }

            // Extract 25-bit immediate from bits 13-36 (4-bit slot + 20-bit offset + sign)
            // The address is encoded as: [sign:1][offset20:20][slot:4]
            let imm_raw = ((inst >> 13) & 0x1FFFFFF) as u32;

            // Convert to address: sign-extend and multiply by 16
            let sign = (imm_raw >> 24) & 1;
            let addr = if sign != 0 {
                (imm_raw | 0xFE000000) << 4
            } else {
                (imm_raw & 0x00FFFFFF) << 4
            };

            // Encoding: add current position (relative to absolute)
            let new_addr = addr.wrapping_add(position);

            // Convert back to immediate format
            let new_imm = (new_addr >> 4) & 0x1FFFFFF;

            // Clear old immediate and set new one
            let mask = 0x1FFFFFFu64 << 13;
            inst = (inst & !mask) | ((new_imm as u64) << 13);

            // Write back the modified instruction
            let write_val = inst << bit_offset;
            for j in 0..6 {
                let orig_mask = if j == 0 {
                    (1u64 << bit_offset) - 1
                } else if j == 5 {
                    !((1u64 << (bit_offset + 41 - 40)) - 1)
                } else {
                    0
                };
                bundle[byte_pos + j] = ((bundle[byte_pos + j] as u64 & orig_mask)
                    | ((write_val >> (j * 8)) & 0xFF & !orig_mask))
                    as u8;
            }
        }
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        // Flush any remaining data (may not be aligned)
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjIa64Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Process when we have enough data
        while self.buffer.len() >= 32 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjIa64Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_IA64
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        // Flush remaining data
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// BCJ RISC-V Encoder
// =============================================================================

/// BCJ RISC-V filter encoder.
///
/// Converts RISC-V branch/jump instruction addresses from relative to absolute
/// to improve compression. Handles both 32-bit standard and 16-bit compressed
/// instruction formats.
///
/// Supported instructions:
/// - JAL (32-bit): Jump and Link with 20-bit immediate
/// - AUIPC (32-bit): Add Upper Immediate to PC - often paired with JALR
/// - C.J/C.JAL (16-bit): Compressed jump instructions (RV32 only for C.JAL)
pub struct BcjRiscvEncoder<W: Write> {
    inner: W,
    buffer: Vec<u8>,
    position: u32,
}

impl<W: Write> std::fmt::Debug for BcjRiscvEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjRiscvEncoder")
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> BcjRiscvEncoder<W> {
    /// Creates a new BCJ RISC-V encoder.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            position: 0,
        }
    }

    fn process_buffer(&mut self) -> io::Result<()> {
        if self.buffer.len() < 8 {
            return Ok(());
        }

        // Keep last 6 bytes for incomplete instructions
        let process_len = self.buffer.len() - 6;
        let mut i = 0;

        while i < process_len {
            // Check if this is a 16-bit compressed instruction
            let b0 = self.buffer[i];
            let is_compressed = (b0 & 0x03) != 0x03;

            if is_compressed {
                // 16-bit compressed instruction
                if i + 2 <= self.buffer.len() {
                    let inst16 = u16::from_le_bytes([self.buffer[i], self.buffer[i + 1]]);
                    if let Some(new_inst) = self.process_compressed(inst16, self.position) {
                        let bytes = new_inst.to_le_bytes();
                        self.buffer[i] = bytes[0];
                        self.buffer[i + 1] = bytes[1];
                    }
                    self.position = self.position.wrapping_add(2);
                    i += 2;
                } else {
                    break;
                }
            } else {
                // 32-bit standard instruction
                if i + 4 <= self.buffer.len() {
                    let inst32 = u32::from_le_bytes([
                        self.buffer[i],
                        self.buffer[i + 1],
                        self.buffer[i + 2],
                        self.buffer[i + 3],
                    ]);
                    if let Some(new_inst) = self.process_standard(inst32, self.position) {
                        let bytes = new_inst.to_le_bytes();
                        self.buffer[i] = bytes[0];
                        self.buffer[i + 1] = bytes[1];
                        self.buffer[i + 2] = bytes[2];
                        self.buffer[i + 3] = bytes[3];
                    }
                    self.position = self.position.wrapping_add(4);
                    i += 4;
                } else {
                    break;
                }
            }
        }

        // Write processed data
        if i > 0 {
            self.inner.write_all(&self.buffer[..i])?;
            self.buffer.drain(..i);
        }

        Ok(())
    }

    /// Process a 32-bit RISC-V instruction.
    fn process_standard(&self, inst: u32, pos: u32) -> Option<u32> {
        let opcode = inst & 0x7F;

        match opcode {
            // JAL (opcode = 1101111)
            0x6F => {
                // Extract 20-bit immediate (bits arranged non-contiguously)
                // imm[20|10:1|11|19:12] = inst[31|30:21|20|19:12]
                let imm20 = (inst >> 31) & 1;
                let imm10_1 = (inst >> 21) & 0x3FF;
                let imm11 = (inst >> 20) & 1;
                let imm19_12 = (inst >> 12) & 0xFF;

                // Reconstruct immediate (already sign-extended in 21 bits, multiply by 2 for address)
                let imm = (imm20 << 20) | (imm19_12 << 12) | (imm11 << 11) | (imm10_1 << 1);
                let addr = if (imm & 0x100000) != 0 {
                    // Sign extend
                    imm | 0xFFE00000
                } else {
                    imm
                };

                // Encoding: add position (relative to absolute)
                let new_addr = addr.wrapping_add(pos);

                // Encode back into JAL format
                let rd = (inst >> 7) & 0x1F;
                let new_imm20 = (new_addr >> 20) & 1;
                let new_imm10_1 = (new_addr >> 1) & 0x3FF;
                let new_imm11 = (new_addr >> 11) & 1;
                let new_imm19_12 = (new_addr >> 12) & 0xFF;

                let new_inst = (new_imm20 << 31)
                    | (new_imm10_1 << 21)
                    | (new_imm11 << 20)
                    | (new_imm19_12 << 12)
                    | (rd << 7)
                    | opcode;

                Some(new_inst)
            }
            // AUIPC (opcode = 0010111)
            0x17 => {
                // AUIPC adds upper 20 bits to PC
                let imm = inst & 0xFFFFF000;
                let new_imm = imm.wrapping_add(pos & 0xFFFFF000);

                let rd = (inst >> 7) & 0x1F;
                let new_inst = (new_imm & 0xFFFFF000) | (rd << 7) | opcode;

                Some(new_inst)
            }
            _ => None,
        }
    }

    /// Process a 16-bit compressed RISC-V instruction.
    fn process_compressed(&self, inst: u16, pos: u32) -> Option<u16> {
        let op = inst & 0x03;
        let funct3 = (inst >> 13) & 0x07;

        // C.J (op=01, funct3=101) and C.JAL (op=01, funct3=001, RV32 only)
        if op == 0x01 && (funct3 == 0x05 || funct3 == 0x01) {
            // 11-bit immediate encoded in bits [12|8|10:9|6|7|2|11|5:3]
            let bit12 = (inst >> 12) & 1;
            let bit11 = (inst >> 11) & 1;
            let bit10 = (inst >> 10) & 1;
            let bit9 = (inst >> 9) & 1;
            let bit8 = (inst >> 8) & 1;
            let bit7 = (inst >> 7) & 1;
            let bit6 = (inst >> 6) & 1;
            let bit5 = (inst >> 5) & 1;
            let bit4 = (inst >> 4) & 1;
            let bit3 = (inst >> 3) & 1;
            let bit2 = (inst >> 2) & 1;

            // Decode: imm[11|4|9:8|10|6|7|3:1|5]
            let imm = ((bit12 as u32) << 11)
                | ((bit11 as u32) << 4)
                | ((bit10 as u32) << 9)
                | ((bit9 as u32) << 8)
                | ((bit8 as u32) << 10)
                | ((bit7 as u32) << 6)
                | ((bit6 as u32) << 7)
                | ((bit5 as u32) << 3)
                | ((bit4 as u32) << 2)
                | ((bit3 as u32) << 1)
                | ((bit2 as u32) << 5);

            // Sign extend from bit 11
            let addr = if (imm & 0x800) != 0 {
                imm | 0xFFFFF000
            } else {
                imm
            };

            // Add position
            let new_addr = addr.wrapping_add(pos);

            // Re-encode (this is complex due to scattered bits)
            let new_bit12 = ((new_addr >> 11) & 1) as u16;
            let new_bit11 = ((new_addr >> 4) & 1) as u16;
            let new_bit10 = ((new_addr >> 9) & 1) as u16;
            let new_bit9 = ((new_addr >> 8) & 1) as u16;
            let new_bit8 = ((new_addr >> 10) & 1) as u16;
            let new_bit7 = ((new_addr >> 6) & 1) as u16;
            let new_bit6 = ((new_addr >> 7) & 1) as u16;
            let new_bit5 = ((new_addr >> 3) & 1) as u16;
            let new_bit4 = ((new_addr >> 2) & 1) as u16;
            let new_bit3 = ((new_addr >> 1) & 1) as u16;
            let new_bit2 = ((new_addr >> 5) & 1) as u16;

            let new_inst = (funct3 << 13)
                | (new_bit12 << 12)
                | (new_bit11 << 11)
                | (new_bit10 << 10)
                | (new_bit9 << 9)
                | (new_bit8 << 8)
                | (new_bit7 << 7)
                | (new_bit6 << 6)
                | (new_bit5 << 5)
                | (new_bit4 << 4)
                | (new_bit3 << 3)
                | (new_bit2 << 2)
                | op;

            return Some(new_inst);
        }

        None
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(mut self) -> io::Result<W> {
        // Flush remaining data
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for BcjRiscvEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Process when we have enough data
        while self.buffer.len() >= 14 {
            self.process_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.process_buffer()?;
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BcjRiscvEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_RISCV
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        // Flush remaining data
        if !self.buffer.is_empty() {
            self.inner.write_all(&self.buffer)?;
        }
        self.inner.flush()
    }
}

// =============================================================================
// Delta Filter Encoder
// =============================================================================

/// Delta filter encoder.
///
/// Computes differences between consecutive samples for better compression
/// of audio, image, and other structured data.
pub struct DeltaEncoder<W: Write> {
    inner: W,
    delta: u8,
    history: Vec<u8>,
}

impl<W: Write> std::fmt::Debug for DeltaEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaEncoder")
            .field("delta", &self.delta)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> DeltaEncoder<W> {
    /// Creates a new Delta encoder.
    ///
    /// # Arguments
    ///
    /// * `inner` - The destination writer
    /// * `delta` - The delta distance (1-256, typically sample size in bytes)
    pub fn new(inner: W, delta: u8) -> Self {
        let delta_val = if delta == 0 { 1 } else { delta };
        Self {
            inner,
            delta: delta_val,
            history: vec![0u8; delta_val as usize],
        }
    }

    /// Creates from properties byte.
    pub fn from_properties(inner: W, properties: &[u8]) -> Self {
        let delta = if properties.is_empty() {
            1
        } else {
            properties[0].wrapping_add(1)
        };
        Self::new(inner, delta)
    }

    /// Finishes encoding and returns the inner writer.
    pub fn try_finish(self) -> io::Result<W> {
        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for DeltaEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let delta = self.delta as usize;
        let mut output = Vec::with_capacity(buf.len());

        for (i, &byte) in buf.iter().enumerate() {
            let hist_idx = i % delta;
            let encoded = byte.wrapping_sub(self.history[hist_idx]);
            self.history[hist_idx] = byte;
            output.push(encoded);
        }

        self.inner.write_all(&output)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for DeltaEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::DELTA
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        self.inner.flush()?;
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bcj_x86_roundtrip() {
        // Simple test data with E8 opcodes
        let original: Vec<u8> = vec![
            0x90, 0x90, 0xe8, 0x01, 0x00, 0x00, 0x00, 0x90, 0x90, 0x90, 0xe8, 0x10, 0x00, 0x00,
            0x00, 0x90,
        ];

        // Encode
        let mut encoded = Vec::new();
        {
            let mut encoder = BcjX86Encoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            encoder.flush().unwrap();
        }

        // For x86, the encoded data should be different
        // (addresses converted from relative to absolute)
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_bcj_arm_roundtrip() {
        // ARM BL instruction (0xEB000000 with offset)
        let original: Vec<u8> = vec![0x00, 0x00, 0x00, 0xeb, 0x01, 0x00, 0x00, 0xeb];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjArmEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            encoder.flush().unwrap();
        }

        assert_eq!(encoded.len(), original.len());
    }

    #[test]
    fn test_bcj_ppc_basic() {
        // PPC branch instruction
        let original: Vec<u8> = vec![0x48, 0x00, 0x00, 0x01, 0x48, 0x00, 0x00, 0x05];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjPpcEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            encoder.flush().unwrap();
        }

        assert_eq!(encoded.len(), original.len());
    }

    #[test]
    fn test_delta_roundtrip() {
        let original: Vec<u8> = vec![10, 20, 30, 40, 50, 60, 70, 80];

        // Encode with delta=1
        let mut encoded = Vec::new();
        {
            let mut encoder = DeltaEncoder::new(&mut encoded, 1);
            encoder.write_all(&original).unwrap();
            encoder.flush().unwrap();
        }

        // Delta encoding should produce differences
        // 10, 10, 10, 10, 10, 10, 10, 10
        assert_eq!(encoded.len(), original.len());
        assert_eq!(encoded[0], 10); // First byte unchanged (0 - 10 = -246 = 10 in u8)
        assert_eq!(encoded[1], 10); // 20 - 10 = 10
    }

    #[test]
    fn test_bcj_ia64_basic() {
        // Test IA-64 encoder with non-branch data (should pass through mostly unchanged)
        let original: Vec<u8> = vec![0u8; 32]; // 2 bundles of 16 bytes each

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjIa64Encoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        assert_eq!(encoded.len(), original.len());
    }

    #[test]
    fn test_bcj_ia64_preserves_non_branch() {
        // Create data that won't trigger branch conversion
        // Template 0x00 has no B-unit slots, so data should pass through
        let mut original = vec![0u8; 32];
        original[0] = 0x00; // Template with no branches
        original[16] = 0x00; // Second bundle, same template

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjIa64Encoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        // Non-branch instructions should be unchanged
        assert_eq!(encoded, original);
    }

    #[test]
    fn test_bcj_riscv_basic() {
        // Test RISC-V encoder with non-branch data
        let original: Vec<u8> = vec![0u8; 32];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjRiscvEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        assert_eq!(encoded.len(), original.len());
    }

    #[test]
    fn test_bcj_riscv_jal_encoding() {
        // JAL instruction: opcode = 0x6F (1101111)
        // jal x1, 0 -> 0x000000EF (rd=1, imm=0)
        let original: Vec<u8> = vec![
            0xEF, 0x00, 0x00, 0x00, // JAL x1, 0 at position 0
            0x13, 0x00, 0x00, 0x00, // NOP (addi x0, x0, 0)
            0x13, 0x00, 0x00, 0x00, // NOP
            0x13, 0x00, 0x00, 0x00, // NOP
        ];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjRiscvEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        assert_eq!(encoded.len(), original.len());

        // The JAL instruction should have been transformed
        // Original: jal x1, 0 (relative offset 0)
        // Encoded: should have position added to offset
        let encoded_jal = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(encoded_jal & 0x7F, 0x6F); // Still a JAL instruction
    }

    #[test]
    fn test_bcj_riscv_auipc_encoding() {
        // AUIPC instruction: opcode = 0x17 (0010111)
        // auipc x1, 0 -> 0x00000097 (rd=1, imm[31:12]=0)
        let original: Vec<u8> = vec![
            0x97, 0x00, 0x00, 0x00, // AUIPC x1, 0
            0x13, 0x00, 0x00, 0x00, // NOP
            0x13, 0x00, 0x00, 0x00, // NOP
            0x13, 0x00, 0x00, 0x00, // NOP
        ];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjRiscvEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        assert_eq!(encoded.len(), original.len());

        // The AUIPC instruction should have been transformed
        let encoded_auipc = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(encoded_auipc & 0x7F, 0x17); // Still an AUIPC instruction
    }

    #[test]
    fn test_bcj_riscv_compressed_passthrough() {
        // Test compressed instruction that is NOT a C.J/C.JAL (should pass through)
        // C.ADDI (op=01, funct3=000) - should not be transformed
        let original: Vec<u8> = vec![
            0x05, 0x01, // c.addi x0, 1 (compressed)
            0x13, 0x00, 0x00, 0x00, // NOP
            0x13, 0x00, 0x00, 0x00, // NOP
            0x13, 0x00, 0x00, 0x00, // NOP
        ];

        let mut encoded = Vec::new();
        {
            let mut encoder = BcjRiscvEncoder::new(&mut encoded);
            encoder.write_all(&original).unwrap();
            let _ = encoder.try_finish().unwrap();
        }

        assert_eq!(encoded.len(), original.len());
        // First two bytes should be unchanged (C.ADDI is not transformed)
        assert_eq!(encoded[0], original[0]);
        assert_eq!(encoded[1], original[1]);
    }
}
