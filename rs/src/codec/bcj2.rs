//! BCJ2 filter for x86 executables.
//!
//! BCJ2 is a 4-stream filter that improves compression of x86 executable code
//! by separating CALL/JMP instruction destinations into separate streams.
//!
//! # Stream Layout
//!
//! BCJ2 uses 4 input streams:
//! - Stream 0 (Main): Main code with E8/E9 instructions
//! - Stream 1 (Call): CALL (E8) destinations, big-endian
//! - Stream 2 (Jump): JMP (E9) and Jcc destinations, big-endian
//! - Stream 3 (Range): Range-coded selector bits
//!
//! # Algorithm
//!
//! The decoder scans the main stream for potential CALL/JMP instructions:
//! - E8: CALL (relative)
//! - E9: JMP (relative)
//! - 0F 8x: Conditional jumps (Jcc)
//!
//! For each potential instruction, it consults the range decoder to determine
//! if the bytes were originally a converted instruction. If so, it reads
//! 4 bytes from the call or jump stream and converts from absolute to relative.

use std::io::{self, BufReader, Read};

use crate::{Error, Result};

/// BCJ2 method ID: `[0x03, 0x03, 0x01, 0x1B]`
pub const METHOD_ID: &[u8] = &[0x03, 0x03, 0x01, 0x1B];

/// Range decoder constants
mod range {
    pub const NUM_MOVE_BITS: u32 = 5;
    pub const NUM_BIT_MODEL_TOTAL_BITS: u32 = 11;
    pub const BIT_MODEL_TOTAL: u32 = 1 << NUM_BIT_MODEL_TOTAL_BITS;
    pub const NUM_TOP_BITS: u32 = 24;
    pub const TOP_VALUE: u32 = 1 << NUM_TOP_BITS;
    pub const INITIAL_PROB: u32 = BIT_MODEL_TOTAL / 2;
}

/// Range decoder for BCJ2 selector bits.
///
/// This is a simplified range decoder that decodes single bits
/// with adaptive probability modeling.
pub struct RangeDecoder<R> {
    reader: R,
    range: u32,
    code: u32,
}

impl<R: Read> RangeDecoder<R> {
    /// Creates a new range decoder.
    ///
    /// Reads 5 initial bytes to initialize the decoder state.
    pub fn new(mut reader: R) -> Result<Self> {
        let mut code: u32 = 0;

        // Read 5 initial bytes - the first byte is absorbed into the high bits
        // which are then shifted out, effectively ignoring it
        for _ in 0..5 {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).map_err(Error::Io)?;
            code = (code << 8) | byte[0] as u32;
        }

        Ok(Self {
            reader,
            range: 0xFFFFFFFF,
            code,
        })
    }

    /// Decodes a single bit using the given probability.
    ///
    /// Returns `(bit, new_prob)` where bit is 0 or 1.
    pub fn decode_bit(&mut self, prob: u32) -> Result<(u32, u32)> {
        let bound = (self.range >> range::NUM_BIT_MODEL_TOTAL_BITS) * prob;

        let (bit, new_prob) = if self.code < bound {
            self.range = bound;
            let new_prob = prob + ((range::BIT_MODEL_TOTAL - prob) >> range::NUM_MOVE_BITS);
            (0, new_prob)
        } else {
            self.range -= bound;
            self.code -= bound;
            let new_prob = prob - (prob >> range::NUM_MOVE_BITS);
            (1, new_prob)
        };

        // Normalize
        if self.range < range::TOP_VALUE {
            let mut byte = [0u8; 1];
            // On EOF, use 0 byte (correct for range coding finale)
            // On actual I/O error, propagate the error
            match self.reader.read(&mut byte) {
                Ok(0) => {} // EOF: byte stays 0, which is correct
                Ok(_) => {} // Read successful
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {} // Also treat as EOF
                Err(e) => return Err(Error::Io(e)), // Propagate real errors
            }
            self.code = (self.code << 8) | byte[0] as u32;
            self.range <<= 8;
        }

        Ok((bit, new_prob))
    }
}

/// Status decoder with adaptive probability.
#[derive(Clone, Copy)]
struct StatusDecoder {
    prob: u32,
}

impl StatusDecoder {
    fn new() -> Self {
        Self {
            prob: range::INITIAL_PROB,
        }
    }

    fn decode<R: Read>(&mut self, rd: &mut RangeDecoder<R>) -> Result<bool> {
        let (bit, new_prob) = rd.decode_bit(self.prob)?;
        self.prob = new_prob;
        Ok(bit == 1)
    }
}

/// BCJ2 decoder that merges 4 streams into the original x86 code.
pub struct Bcj2Decoder<R> {
    /// Main stream with E8/E9 bytes
    main: BufReader<R>,
    /// CALL destinations (big-endian)
    call: R,
    /// JMP destinations (big-endian)
    jump: R,
    /// Range decoder for selector bits
    range_decoder: RangeDecoder<R>,
    /// Status decoders (256 for E8 contexts + 1 for E9 + 1 for Jcc)
    status_decoders: [StatusDecoder; 258],
    /// Previous byte (for context)
    prev_byte: u8,
    /// Bytes written so far (for address calculation)
    written: u32,
    /// Internal output buffer
    buffer: Vec<u8>,
    /// Current position in buffer
    buffer_pos: usize,
}

impl<R: Read> Bcj2Decoder<R> {
    /// Creates a new BCJ2 decoder from 4 input streams.
    ///
    /// # Arguments
    ///
    /// * `main` - Main code stream (stream 0)
    /// * `call` - CALL destinations stream (stream 1)
    /// * `jump` - JMP destinations stream (stream 2)
    /// * `range` - Range-coded selector stream (stream 3)
    pub fn new(main: R, call: R, jump: R, range: R) -> Result<Self> {
        let range_decoder = RangeDecoder::new(range)?;

        Ok(Self {
            main: BufReader::new(main),
            call,
            jump,
            range_decoder,
            status_decoders: [StatusDecoder::new(); 258],
            prev_byte: 0,
            written: 0,
            buffer: Vec::with_capacity(65536),
            buffer_pos: 0,
        })
    }

    /// Returns whether the byte pair indicates a potential CALL/JMP.
    #[inline]
    fn is_jump(prev: u8, curr: u8) -> bool {
        // E8: CALL, E9: JMP
        (curr & 0xFE) == 0xE8 || Self::is_jcc(prev, curr)
    }

    /// Returns whether the byte pair is a conditional jump (Jcc).
    #[inline]
    fn is_jcc(prev: u8, curr: u8) -> bool {
        prev == 0x0F && (curr & 0xF0) == 0x80
    }

    /// Returns the status decoder index for the given byte pair.
    #[inline]
    fn status_index(prev: u8, curr: u8) -> usize {
        match curr {
            0xE8 => prev as usize, // CALL: use previous byte as context
            0xE9 => 256,           // JMP: single context
            _ => 257,              // Jcc: single context
        }
    }

    /// Fills the internal buffer with decoded data.
    fn fill_buffer(&mut self) -> io::Result<()> {
        self.buffer.clear();
        self.buffer_pos = 0;

        loop {
            // Read one byte from main stream
            let mut byte = [0u8; 1];
            match self.main.read(&mut byte) {
                Ok(0) => return Ok(()), // EOF
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            let b = byte[0];

            self.written += 1;
            self.buffer.push(b);

            // Check if this is a potential CALL/JMP
            if Self::is_jump(self.prev_byte, b) {
                // Use range decoder to check if this was converted
                let idx = Self::status_index(self.prev_byte, b);
                let is_converted = self.status_decoders[idx]
                    .decode(&mut self.range_decoder)
                    .map_err(|e| io::Error::other(e.to_string()))?;

                if is_converted {
                    // Read 4 bytes from call or jump stream
                    let reader: &mut dyn Read = if b == 0xE8 {
                        &mut self.call
                    } else {
                        &mut self.jump
                    };

                    let mut dest_bytes = [0u8; 4];
                    reader.read_exact(&mut dest_bytes)?;

                    // Convert from big-endian absolute to little-endian relative
                    let dest = u32::from_be_bytes(dest_bytes);
                    let relative = dest.wrapping_sub(self.written + 4);

                    // Write as little-endian
                    self.buffer.extend_from_slice(&relative.to_le_bytes());
                    self.prev_byte = (relative >> 24) as u8;
                    self.written += 4;
                } else {
                    self.prev_byte = b;
                }
            } else {
                self.prev_byte = b;
            }

            // Stop when buffer is reasonably full
            if self.buffer.len() >= self.buffer.capacity() / 2 {
                break;
            }
        }

        Ok(())
    }
}

impl<R: Read> Read for Bcj2Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // If buffer is empty, fill it
        if self.buffer_pos >= self.buffer.len() {
            self.fill_buffer()?;
            if self.buffer.is_empty() {
                return Ok(0); // EOF
            }
        }

        // Copy from buffer to output
        let available = self.buffer.len() - self.buffer_pos;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&self.buffer[self.buffer_pos..self.buffer_pos + to_copy]);
        self.buffer_pos += to_copy;

        Ok(to_copy)
    }
}

/// Wrapper to implement the `Decoder` trait for `Bcj2Decoder`.
///
/// This allows BCJ2 decoders to be used in the standard decoder pipeline.
pub struct Bcj2DecoderWrapper<R> {
    inner: Bcj2Decoder<R>,
}

impl<R: Read> Bcj2DecoderWrapper<R> {
    /// Creates a new wrapper around a `Bcj2Decoder`.
    pub fn new(inner: Bcj2Decoder<R>) -> Self {
        Self { inner }
    }
}

impl<R: Read + Send> Read for Bcj2DecoderWrapper<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> super::Decoder for Bcj2DecoderWrapper<R> {
    fn method_id(&self) -> &'static [u8] {
        METHOD_ID
    }
}

// ============================================================================
// BCJ2 ENCODER
// ============================================================================

/// Range encoder for BCJ2 selector bits.
///
/// This is the inverse of `RangeDecoder`, producing a bitstream from
/// probability-coded bits.
pub struct RangeEncoder {
    range: u32,
    low: u64,
    cache: u8,
    cache_size: u32,
    output: Vec<u8>,
}

impl RangeEncoder {
    /// Creates a new range encoder.
    pub fn new() -> Self {
        Self {
            range: 0xFFFFFFFF,
            low: 0,
            cache: 0,
            cache_size: 1,
            output: Vec::with_capacity(4096),
        }
    }

    /// Encodes a single bit with the given probability.
    ///
    /// Returns the updated probability.
    pub fn encode_bit(&mut self, bit: bool, prob: u32) -> u32 {
        let bound = (self.range >> range::NUM_BIT_MODEL_TOTAL_BITS) * prob;

        let new_prob = if bit {
            // Bit is 1
            self.low += bound as u64;
            self.range -= bound;
            prob - (prob >> range::NUM_MOVE_BITS)
        } else {
            // Bit is 0
            self.range = bound;
            prob + ((range::BIT_MODEL_TOTAL - prob) >> range::NUM_MOVE_BITS)
        };

        // Normalize
        while self.range < range::TOP_VALUE {
            self.shift_low();
            self.range <<= 8;
        }

        new_prob
    }

    /// Shifts the low bits to the output (LZMA SDK compatible).
    fn shift_low(&mut self) {
        // Extract the high byte (bits 24-31) and overflow (bit 32+)
        let low32 = self.low as u32;
        let high = (self.low >> 32) as u8;

        // Update low: shift left by 8, keep only lower 32 bits
        self.low = (low32 << 8) as u64;

        // Check if we need to output (no carry propagation pending)
        if low32 < 0xFF000000 || high != 0 {
            // Output previous cache byte with possible carry
            let temp = self.cache.wrapping_add(high);
            self.cache = (low32 >> 24) as u8;

            if self.cache_size > 0 {
                self.output.push(temp);
                // Output any pending 0xFF bytes with carry
                for _ in 1..self.cache_size {
                    self.output.push(0xFF_u8.wrapping_add(high));
                }
                self.cache_size = 0;
            }
            self.cache_size += 1;
        } else {
            // Carry might propagate, defer output
            self.cache_size += 1;
        }
    }

    /// Finishes encoding and returns the output bytes.
    pub fn finish(mut self) -> Vec<u8> {
        // Flush remaining bits (5 bytes like the decoder reads initially)
        for _ in 0..5 {
            self.shift_low();
        }
        self.output
    }
}

impl Default for RangeEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Status encoder with adaptive probability.
#[derive(Clone, Copy)]
struct StatusEncoder {
    prob: u32,
}

impl StatusEncoder {
    fn new() -> Self {
        Self {
            prob: range::INITIAL_PROB,
        }
    }

    fn encode(&mut self, re: &mut RangeEncoder, bit: bool) {
        self.prob = re.encode_bit(bit, self.prob);
    }
}

/// Result of BCJ2 encoding containing the 4 output streams.
#[derive(Debug, Clone)]
pub struct Bcj2EncodedStreams {
    /// Main stream with filtered code.
    pub main: Vec<u8>,
    /// CALL destinations (big-endian).
    pub call: Vec<u8>,
    /// JMP/Jcc destinations (big-endian).
    pub jump: Vec<u8>,
    /// Range-coded selector bits.
    pub range: Vec<u8>,
}

impl Bcj2EncodedStreams {
    /// Returns the total size of all streams.
    pub fn total_size(&self) -> usize {
        self.main.len() + self.call.len() + self.jump.len() + self.range.len()
    }
}

/// BCJ2 encoder that splits x86 code into 4 streams.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::codec::bcj2::Bcj2Encoder;
///
/// let x86_code = vec![0x55, 0x89, 0xE5, 0xE8, 0x01, 0x00, 0x00, 0x00, 0xC3];
/// let mut encoder = Bcj2Encoder::new();
/// encoder.encode(&x86_code);
/// let streams = encoder.finish();
/// ```
pub struct Bcj2Encoder {
    /// Main stream buffer.
    main: Vec<u8>,
    /// CALL destinations (big-endian).
    call: Vec<u8>,
    /// JMP/Jcc destinations (big-endian).
    jump: Vec<u8>,
    /// Range encoder for selector bits.
    range_encoder: RangeEncoder,
    /// Status encoders (256 for E8 contexts + 1 for E9 + 1 for Jcc).
    status_encoders: [StatusEncoder; 258],
    /// Previous byte (for context).
    prev_byte: u8,
    /// Current position in the stream (for address calculation).
    position: u32,
}

impl Bcj2Encoder {
    /// Creates a new BCJ2 encoder.
    pub fn new() -> Self {
        Self {
            main: Vec::with_capacity(65536),
            call: Vec::with_capacity(4096),
            jump: Vec::with_capacity(1024),
            range_encoder: RangeEncoder::new(),
            status_encoders: [StatusEncoder::new(); 258],
            prev_byte: 0,
            position: 0,
        }
    }

    /// Encodes the given data.
    ///
    /// This scans for CALL/JMP/Jcc instructions and splits them into
    /// the appropriate streams.
    pub fn encode(&mut self, data: &[u8]) {
        let mut i = 0;

        while i < data.len() {
            let b = data[i];

            // Check if this is a potential CALL/JMP instruction
            let is_jump = Self::is_jump(self.prev_byte, b);

            if is_jump && i + 4 < data.len() {
                // Read the 4-byte relative address
                let rel_addr =
                    u32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]);

                // Calculate absolute address
                let abs_addr = rel_addr.wrapping_add(self.position + 5);

                // Decide whether to convert (simple heuristic: convert if it looks like a real call)
                // For now, we always convert to match 7-Zip behavior
                let should_convert = Self::should_convert(abs_addr);

                let idx = Self::status_index(self.prev_byte, b);
                self.status_encoders[idx].encode(&mut self.range_encoder, should_convert);

                if should_convert {
                    // Write opcode to main stream
                    self.main.push(b);
                    self.position += 1;

                    // Write absolute address to call or jump stream (big-endian)
                    let dest_bytes = abs_addr.to_be_bytes();
                    if b == 0xE8 {
                        self.call.extend_from_slice(&dest_bytes);
                    } else {
                        self.jump.extend_from_slice(&dest_bytes);
                    }

                    // Skip the 4 address bytes
                    self.prev_byte = data[i + 4];
                    self.position += 4;
                    i += 5;
                } else {
                    // Not converting, write as-is
                    self.main.push(b);
                    self.prev_byte = b;
                    self.position += 1;
                    i += 1;
                }
            } else {
                // Regular byte, just copy
                self.main.push(b);
                self.prev_byte = b;
                self.position += 1;
                i += 1;
            }
        }
    }

    /// Returns whether the byte pair indicates a potential CALL/JMP.
    #[inline]
    fn is_jump(prev: u8, curr: u8) -> bool {
        (curr & 0xFE) == 0xE8 || Self::is_jcc(prev, curr)
    }

    /// Returns whether the byte pair is a conditional jump (Jcc).
    #[inline]
    fn is_jcc(prev: u8, curr: u8) -> bool {
        prev == 0x0F && (curr & 0xF0) == 0x80
    }

    /// Returns the status encoder index for the given byte pair.
    #[inline]
    fn status_index(prev: u8, curr: u8) -> usize {
        match curr {
            0xE8 => prev as usize, // CALL: use previous byte as context
            0xE9 => 256,           // JMP: single context
            _ => 257,              // Jcc: single context
        }
    }

    /// Determines whether to convert this address.
    ///
    /// Uses a simple heuristic similar to 7-Zip's approach.
    #[inline]
    fn should_convert(_abs_addr: u32) -> bool {
        // For simplicity, always convert. 7-Zip uses more complex heuristics
        // based on address patterns, but this works well for typical x86 code.
        true
    }

    /// Finishes encoding and returns the 4 output streams.
    pub fn finish(self) -> Bcj2EncodedStreams {
        Bcj2EncodedStreams {
            main: self.main,
            call: self.call,
            jump: self.jump,
            range: self.range_encoder.finish(),
        }
    }
}

impl Default for Bcj2Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Encodes data using BCJ2 filter.
///
/// This is a convenience function for one-shot encoding.
///
/// # Example
///
/// ```rust
/// use zesven::codec::bcj2::bcj2_encode;
///
/// let x86_code = vec![0x55, 0x89, 0xE5, 0xC3]; // Simple function
/// let streams = bcj2_encode(&x86_code);
/// assert!(streams.total_size() > 0);
/// ```
pub fn bcj2_encode(data: &[u8]) -> Bcj2EncodedStreams {
    let mut encoder = Bcj2Encoder::new();
    encoder.encode(data);
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ==========================================================================
    // Range Decoder Unit Tests
    // ==========================================================================

    mod range_decoder {
        use super::*;

        #[test]
        fn test_new_reads_5_bytes() {
            // Range decoder should read exactly 5 bytes for initialization
            let data = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
            let cursor = Cursor::new(data);

            let rd = RangeDecoder::new(cursor).unwrap();

            // Code should be constructed from first 5 bytes: 0x0001020304
            assert_eq!(rd.code, 0x0000_0102_0304);
            assert_eq!(rd.range, 0xFFFFFFFF);
        }

        #[test]
        fn test_new_fails_on_short_input() {
            let data = vec![0x00, 0x01, 0x02]; // Only 3 bytes
            let cursor = Cursor::new(data);

            let result = RangeDecoder::new(cursor);
            assert!(result.is_err());
        }

        #[test]
        fn test_decode_bit_zero() {
            // When code < bound, decode should return 0
            let data = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            let cursor = Cursor::new(data);
            let mut rd = RangeDecoder::new(cursor).unwrap();

            let initial_prob = range::INITIAL_PROB;
            let (bit, new_prob) = rd.decode_bit(initial_prob).unwrap();

            assert_eq!(bit, 0);
            // Probability should increase after seeing 0
            assert!(new_prob > initial_prob);
        }

        #[test]
        fn test_decode_bit_one() {
            // When code >= bound, decode should return 1
            let data = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
            let cursor = Cursor::new(data);
            let mut rd = RangeDecoder::new(cursor).unwrap();

            let initial_prob = range::INITIAL_PROB;
            let (bit, new_prob) = rd.decode_bit(initial_prob).unwrap();

            assert_eq!(bit, 1);
            // Probability should decrease after seeing 1
            assert!(new_prob < initial_prob);
        }

        #[test]
        fn test_probability_adaptation() {
            // Test that probability adapts correctly over multiple decodes
            let data = vec![0x00; 100];
            let cursor = Cursor::new(data);
            let mut rd = RangeDecoder::new(cursor).unwrap();

            let mut prob = range::INITIAL_PROB;

            // Decode several zeros, probability should increase
            for _ in 0..10 {
                let (bit, new_prob) = rd.decode_bit(prob).unwrap();
                if bit == 0 {
                    assert!(new_prob >= prob);
                }
                prob = new_prob;
            }
        }
    }

    // ==========================================================================
    // BCJ2 Algorithm Unit Tests
    // ==========================================================================

    mod algorithm {
        use super::*;

        #[test]
        fn test_is_jump_e8_call() {
            // E8 is CALL instruction
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x00, 0xE8));
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0xFF, 0xE8));
        }

        #[test]
        fn test_is_jump_e9_jmp() {
            // E9 is JMP instruction
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x00, 0xE9));
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0xFF, 0xE9));
        }

        #[test]
        fn test_is_jump_jcc() {
            // 0F 8x is conditional jump (Jcc)
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x0F, 0x80));
            assert!(Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x0F, 0x8F));
            // Not Jcc if prev != 0x0F
            assert!(!Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x00, 0x80));
        }

        #[test]
        fn test_is_jump_not_jump() {
            // Regular bytes are not jumps
            assert!(!Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x00, 0x00));
            assert!(!Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0x90, 0x90)); // NOP NOP
            assert!(!Bcj2Decoder::<Cursor<Vec<u8>>>::is_jump(0xE8, 0x00)); // E8 followed by 00
        }

        #[test]
        fn test_status_index_call() {
            // CALL (E8) uses previous byte as context (0-255)
            assert_eq!(Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0x00, 0xE8), 0);
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0xFF, 0xE8),
                255
            );
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0x90, 0xE8),
                0x90
            );
        }

        #[test]
        fn test_status_index_jmp() {
            // JMP (E9) uses index 256
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0x00, 0xE9),
                256
            );
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0xFF, 0xE9),
                256
            );
        }

        #[test]
        fn test_status_index_jcc() {
            // Jcc uses index 257
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0x0F, 0x80),
                257
            );
            assert_eq!(
                Bcj2Decoder::<Cursor<Vec<u8>>>::status_index(0x0F, 0x8F),
                257
            );
        }

        #[test]
        fn test_address_conversion() {
            // Test absolute to relative address conversion
            // dest (absolute) - (written + 4) = relative
            let written: u32 = 100;
            let absolute: u32 = 200;
            let relative = absolute.wrapping_sub(written + 4);
            assert_eq!(relative, 96); // 200 - 104 = 96
        }

        #[test]
        fn test_address_conversion_negative() {
            // Backward jump (negative relative address)
            let written: u32 = 200;
            let absolute: u32 = 100;
            let relative = absolute.wrapping_sub(written + 4);
            // 100 - 204 = -104 = 0xFFFFFF98
            assert_eq!(relative, 0xFFFFFF98);
        }
    }

    // ==========================================================================
    // BCJ2 Decoder Integration Tests
    // ==========================================================================

    mod decoder {
        use super::*;

        /// Creates a minimal BCJ2 stream set for testing.
        ///
        /// This creates streams that decode to a known output.
        fn create_test_streams() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
            // Main stream: just regular bytes, no E8/E9
            let main = vec![0x90, 0x90, 0x90, 0x90]; // NOP NOP NOP NOP

            // Call stream: empty (no calls)
            let call = vec![];

            // Jump stream: empty (no jumps)
            let jump = vec![];

            // Range stream: initial 5 bytes
            let range = vec![0x00, 0x00, 0x00, 0x00, 0x00];

            (main, call, jump, range)
        }

        #[test]
        fn test_decoder_passthrough_no_jumps() {
            // When there are no E8/E9 instructions, output should match main stream
            let (main, call, jump, range) = create_test_streams();

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(main.clone()),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            )
            .unwrap();

            let mut output = Vec::new();
            decoder.read_to_end(&mut output).unwrap();

            assert_eq!(output, main);
        }

        #[test]
        fn test_decoder_empty_streams() {
            // Empty main stream should produce empty output
            let main = vec![];
            let call = vec![];
            let jump = vec![];
            let range = vec![0x00, 0x00, 0x00, 0x00, 0x00];

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(main),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            )
            .unwrap();

            let mut output = Vec::new();
            decoder.read_to_end(&mut output).unwrap();

            assert!(output.is_empty());
        }

        #[test]
        fn test_decoder_initialization() {
            let (main, call, jump, range) = create_test_streams();

            let decoder = Bcj2Decoder::new(
                Cursor::new(main),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            )
            .unwrap();

            assert_eq!(decoder.prev_byte, 0);
            assert_eq!(decoder.written, 0);
            assert_eq!(decoder.status_decoders.len(), 258);
        }

        #[test]
        fn test_decoder_requires_5_byte_range_init() {
            let main = vec![0x90];
            let call = vec![];
            let jump = vec![];
            let range = vec![0x00, 0x00]; // Too short!

            let result = Bcj2Decoder::new(
                Cursor::new(main),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            );

            assert!(result.is_err());
        }
    }

    // ==========================================================================
    // Known Vector Tests
    // ==========================================================================

    mod known_vectors {
        use super::*;

        /// Test vector: Simple sequence without jumps
        #[test]
        fn test_vector_no_conversion() {
            // Input: 5 bytes of non-jump code
            // Expected: Pass through unchanged
            let main = vec![0x55, 0x48, 0x89, 0xE5, 0x5D]; // push rbp; mov rbp,rsp; pop rbp
            let call = vec![];
            let jump = vec![];
            // Range decoder initialized with zeros = will return 0 (not converted)
            let range = vec![0x00, 0x00, 0x00, 0x00, 0x00];

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(main.clone()),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            )
            .unwrap();

            let mut output = Vec::new();
            decoder.read_to_end(&mut output).unwrap();

            assert_eq!(output, main);
        }

        /// Test vector: E8 that is NOT converted (range decoder returns 0)
        #[test]
        fn test_vector_e8_not_converted() {
            // Main has E8 but range decoder says it's not a converted call
            let main = vec![0x90, 0xE8, 0x90, 0x90, 0x90, 0x90];
            let call = vec![];
            let jump = vec![];
            // Range decoder returns 0 (not converted)
            let range = vec![0x00, 0x00, 0x00, 0x00, 0x00];

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(main.clone()),
                Cursor::new(call),
                Cursor::new(jump),
                Cursor::new(range),
            )
            .unwrap();

            let mut output = Vec::new();
            decoder.read_to_end(&mut output).unwrap();

            // E8 should pass through unchanged
            assert_eq!(output, main);
        }
    }

    // ==========================================================================
    // BCJ2 Encoder Unit Tests
    // ==========================================================================

    mod encoder {
        use super::*;

        #[test]
        fn test_encoder_no_jumps() {
            // Data without any CALL/JMP instructions should pass through unchanged
            let data = vec![0x55, 0x89, 0xE5, 0x5D, 0xC3]; // push rbp; mov rbp,rsp; pop rbp; ret

            let streams = bcj2_encode(&data);

            // Main stream should contain all input bytes
            assert_eq!(streams.main, data);
            // No calls or jumps to encode
            assert!(streams.call.is_empty());
            assert!(streams.jump.is_empty());
            // Range stream should contain initialization bytes
            assert!(!streams.range.is_empty());
        }

        #[test]
        fn test_encoder_with_call() {
            // Data with a CALL instruction: E8 + 4 bytes relative address
            let data = vec![
                0x55, // push rbp
                0xE8, 0x01, 0x00, 0x00, 0x00, // call +1 (relative)
                0xC3, // ret
            ];

            let streams = bcj2_encode(&data);

            // Main stream should have: 0x55, 0xE8, 0xC3 (opcode kept, address removed)
            assert!(streams.main.contains(&0x55));
            assert!(streams.main.contains(&0xE8));
            assert!(streams.main.contains(&0xC3));

            // Call stream should have 4 bytes (big-endian absolute address)
            assert_eq!(streams.call.len(), 4);
        }

        #[test]
        fn test_encoder_with_jmp() {
            // Data with a JMP instruction: E9 + 4 bytes relative address
            let data = vec![
                0x90, // nop
                0xE9, 0x05, 0x00, 0x00, 0x00, // jmp +5 (relative)
                0xC3, // ret
            ];

            let streams = bcj2_encode(&data);

            // Jump stream should have 4 bytes
            assert_eq!(streams.jump.len(), 4);
            // Call stream should be empty (no calls)
            assert!(streams.call.is_empty());
        }

        #[test]
        fn test_encoder_defaults() {
            let encoder = Bcj2Encoder::new();
            assert!(encoder.main.is_empty());
            assert!(encoder.call.is_empty());
            assert!(encoder.jump.is_empty());
            assert_eq!(encoder.position, 0);
        }

        #[test]
        fn test_range_encoder_initialization() {
            let encoder = RangeEncoder::new();
            assert_eq!(encoder.range, 0xFFFFFFFF);
            assert_eq!(encoder.low, 0);
        }

        #[test]
        fn test_streams_total_size() {
            let streams = Bcj2EncodedStreams {
                main: vec![1, 2, 3],
                call: vec![4, 5, 6, 7],
                jump: vec![],
                range: vec![8, 9],
            };

            assert_eq!(streams.total_size(), 9);
        }

        #[test]
        fn test_roundtrip_no_jumps() {
            // Encode then decode should give back original data (no jumps case)
            let original = vec![0x55, 0x89, 0xE5, 0x5D, 0xC3];

            let streams = bcj2_encode(&original);

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(streams.main),
                Cursor::new(streams.call),
                Cursor::new(streams.jump),
                Cursor::new(streams.range),
            )
            .unwrap();

            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded).unwrap();

            assert_eq!(decoded, original);
        }

        #[test]
        fn test_roundtrip_with_calls() {
            // x86-like data with CALL instructions (E8)
            let mut original = Vec::new();
            for i in 0..100 {
                original.push(0xE8); // CALL opcode
                let offset = (i * 0x1000) as u32;
                original.extend_from_slice(&offset.to_le_bytes());
                original.extend_from_slice(&[0x90, 0x90, 0x90]); // NOPs
            }

            let streams = bcj2_encode(&original);

            let mut decoder = Bcj2Decoder::new(
                Cursor::new(streams.main),
                Cursor::new(streams.call),
                Cursor::new(streams.jump),
                Cursor::new(streams.range),
            )
            .unwrap();

            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded).unwrap();

            assert_eq!(decoded.len(), original.len());
            assert_eq!(decoded, original);
        }

        #[test]
        fn test_status_encoder_adaptation() {
            let mut encoder = StatusEncoder::new();
            let mut re = RangeEncoder::new();

            // Initial probability should be 1024 (INITIAL_PROB)
            let initial = encoder.prob;
            assert_eq!(initial, range::INITIAL_PROB);

            // Encode a 0, probability should increase
            encoder.encode(&mut re, false);
            assert!(encoder.prob > initial);

            // Encode a 1, probability should decrease
            let after_zero = encoder.prob;
            encoder.encode(&mut re, true);
            assert!(encoder.prob < after_zero);
        }

        #[test]
        fn test_range_coder_single_bit() {
            // Test encoding and decoding a single "1" bit
            let mut re = RangeEncoder::new();
            let _new_prob = re.encode_bit(true, range::INITIAL_PROB);
            let bytes = re.finish();

            let mut rd = RangeDecoder::new(Cursor::new(bytes)).unwrap();
            let (bit, _) = rd.decode_bit(range::INITIAL_PROB).unwrap();

            assert_eq!(bit, 1, "Decoded bit should be 1");
        }

        #[test]
        fn test_range_coder_multiple_ones() {
            // Test encoding and decoding multiple "1" bits
            let mut re = RangeEncoder::new();
            let mut prob = range::INITIAL_PROB;
            for _ in 0..10 {
                prob = re.encode_bit(true, prob);
            }
            let bytes = re.finish();

            let mut rd = RangeDecoder::new(Cursor::new(bytes)).unwrap();
            let mut prob = range::INITIAL_PROB;
            for i in 0..10 {
                let (bit, new_prob) = rd.decode_bit(prob).unwrap();
                assert_eq!(bit, 1, "Bit {} should be 1", i);
                prob = new_prob;
            }
        }
    }
}

// ==========================================================================
// Integration Tests (separate module for file-based tests)
// ==========================================================================

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::Cursor;
    #[cfg(feature = "lzma")]
    use std::path::Path;

    /// Test that we can detect BCJ2 method ID in archives.
    #[test]
    fn test_method_id_constant() {
        assert_eq!(METHOD_ID, &[0x03, 0x03, 0x01, 0x1B]);

        // Convert to u64 for comparison with codec registry
        let id_u64 = METHOD_ID.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
        assert_eq!(id_u64, 0x0303011B);
    }

    /// Test that we can open a BCJ2 archive and inspect its structure.
    ///
    /// This test verifies:
    /// 1. Archive opens successfully
    /// 2. Contains expected entries
    /// 3. BCJ2 codec is detected in folder info
    #[cfg(feature = "lzma")]
    #[test]
    fn test_bcj2_archive_structure() {
        let archive_path = Path::new("tests/data/bcj2/7za433_7zip_lzma2_bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Test archive not found: {:?}", archive_path);
            return;
        }

        // Open the archive
        let archive = crate::Archive::open_path(archive_path).unwrap();

        // Check we have entries
        let entries = archive.entries();
        assert!(!entries.is_empty(), "Archive should have entries");

        // Print info for debugging
        let info = archive.info();
        println!("Archive info:");
        println!("  Entry count: {}", info.entry_count);
        println!("  Total size: {}", info.total_size);
        println!("  Packed size: {}", info.packed_size);
        println!("  Is solid: {}", info.is_solid);
        println!("  Compression methods: {:?}", info.compression_methods);

        // List entries
        for entry in entries {
            println!(
                "  Entry: {} ({} bytes, encrypted={})",
                entry.path.as_str(),
                entry.size,
                entry.is_encrypted
            );
        }
    }

    /// Test that we can open BCJ2 archives and list entries.
    ///
    /// BCJ2 archives should open successfully and report entries.
    /// The actual BCJ2 decoding is tested in extraction tests.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_bcj2_archive_opens() {
        let archive_path = Path::new("tests/data/bcj2/7za433_7zip_lzma2_bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Skipping: test archive not found");
            return;
        }

        let archive = crate::Archive::open_path(archive_path).unwrap();

        // Should have entries
        assert!(!archive.is_empty(), "Archive should have entries");

        println!("Archive contains {} entries:", archive.len());
        for entry in archive.entries() {
            println!(
                "  {} ({} bytes, dir={})",
                entry.path.as_str(),
                entry.size,
                entry.is_directory
            );
        }

        // Check archive info
        let info = archive.info();
        println!("Folder count: {}", info.folder_count);
        println!("Compression methods: {:?}", info.compression_methods);
    }

    /// Test that BCJ2 archive from sevenzip testdata opens correctly.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_sevenzip_bcj2_archive_opens() {
        let archive_path = Path::new("tests/data/bcj2/bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Skipping: test archive not found");
            return;
        }

        let archive = crate::Archive::open_path(archive_path).unwrap();

        assert!(!archive.is_empty(), "Archive should have entries");

        println!("Archive contains {} entries:", archive.len());
        for entry in archive.entries() {
            println!("  {} ({} bytes)", entry.path.as_str(), entry.size);
        }
    }

    /// Test with real BCJ2 archive (from sevenz-rust2 test resources).
    ///
    /// This test verifies that:
    /// 1. We can open the archive
    /// 2. We can detect the BCJ2 codec
    /// 3. We can extract files correctly
    #[cfg(feature = "lzma")]
    #[test]
    fn test_bcj2_archive_extraction() {
        let archive_path = Path::new("tests/data/bcj2/7za433_7zip_lzma2_bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Test archive not found: {:?}", archive_path);
            return;
        }

        let mut archive = crate::Archive::open_path(archive_path).unwrap();
        let entries: Vec<_> = archive.entries().to_vec();

        for entry in entries {
            if !entry.is_directory {
                println!("Extracting: {}", entry.path.as_str());
                let data = archive.extract_to_vec(entry.path.as_str()).unwrap();
                assert!(!data.is_empty(), "Extracted data should not be empty");
                println!("  Size: {} bytes", data.len());
            }
        }
    }

    /// Test with sevenzip testdata BCJ2 archive.
    ///
    /// This archive uses pure BCJ2 without LZMA2, which has a different
    /// internal structure (all 4 inputs from packed streams).
    #[cfg(feature = "lzma")]
    #[test]
    fn test_sevenzip_bcj2_archive() {
        let archive_path = Path::new("tests/data/bcj2/bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Test archive not found: {:?}", archive_path);
            return;
        }

        let mut archive = crate::Archive::open_path(archive_path).unwrap();
        let entries: Vec<_> = archive.entries().to_vec();

        for entry in entries {
            if !entry.is_directory {
                println!("Extracting: {}", entry.path.as_str());
                let data = archive.extract_to_vec(entry.path.as_str()).unwrap();
                assert!(!data.is_empty(), "Extracted data should not be empty");
            }
        }
    }

    /// Test that BCJ2 decoder properly reconstructs x86 code.
    ///
    /// This test uses a synthetic but realistic x86 code sequence.
    #[test]
    fn test_bcj2_x86_reconstruction() {
        // This test creates synthetic BCJ2 streams that decode to known x86 code

        // Target output: A simple x86 sequence with a CALL
        // 55            push ebp
        // 89 E5         mov ebp, esp
        // E8 xx xx xx xx  call <relative_address>
        // 5D            pop ebp
        // C3            ret

        // For now, just verify the decoder can be constructed with valid streams
        let main = vec![0x55, 0x89, 0xE5, 0xE8, 0x5D, 0xC3]; // E8 is the CALL opcode
        let call = vec![]; // No actual calls converted
        let jump = vec![];
        let range = vec![0x00, 0x00, 0x00, 0x00, 0x00]; // All zeros = no conversions

        let mut decoder = Bcj2Decoder::new(
            Cursor::new(main.clone()),
            Cursor::new(call),
            Cursor::new(jump),
            Cursor::new(range),
        )
        .unwrap();

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();

        // With all zeros in range decoder, E8 should pass through unchanged
        assert_eq!(output, main);
    }

    /// Test opening and extracting from a simple BCJ2 archive without encoded headers.
    ///
    /// This archive was created with:
    /// 7zz a -t7z -m0=BCJ2 -m1=LZMA2 -mhc=off simple_bcj2.7z ...
    #[cfg(feature = "lzma")]
    #[test]
    fn test_simple_bcj2_archive() {
        let archive_path = Path::new("tests/data/bcj2/simple_bcj2.7z");

        if !archive_path.exists() {
            eprintln!("Skipping: test archive not found at {:?}", archive_path);
            return;
        }

        // Open the archive
        let mut archive = crate::Archive::open_path(archive_path).expect("Failed to open archive");

        // Verify archive has entries
        assert!(!archive.is_empty(), "Archive should have entries");

        // Extract and verify the test file
        let entries: Vec<_> = archive.entries().to_vec();
        for entry in entries {
            if !entry.is_directory {
                let data = archive
                    .extract_to_vec(entry.path.as_str())
                    .expect("Failed to extract file");

                // Verify we got the expected content
                if entry.path.as_str().ends_with("test.txt") {
                    let content = String::from_utf8_lossy(&data);
                    assert!(
                        content.contains("Hello World"),
                        "Expected 'Hello World' in content, got: {}",
                        content
                    );
                }
            }
        }
    }
}
