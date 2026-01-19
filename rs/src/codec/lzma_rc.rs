//! LZMA Range Encoder.
//!
//! This module provides a range encoder specifically designed for LZMA compression.
#![allow(dead_code)]
//! It supports:
//! - Adaptive probability-based bit encoding
//! - Direct bit encoding (for distance extra bits)
//! - Bit tree encoding (for length and slot encoding)
//!
//! # Range Coding Overview
//!
//! Range coding is an entropy coding method that represents a sequence of symbols
//! as a single number within a range [0, 1). Each symbol narrows the range based
//! on its probability, and the final range is output as bytes.
//!
//! # Reference
//!
//! Based on the Fast LZMA2 implementation:
//! <https://github.com/mcmilk/7-Zip-zstd/blob/master/C/fast-lzma2/range_enc.c>

/// Number of bits for probability model total.
pub const NUM_BIT_MODEL_TOTAL_BITS: u32 = 11;

/// Total probability value (2048).
pub const BIT_MODEL_TOTAL: u32 = 1 << NUM_BIT_MODEL_TOTAL_BITS;

/// Number of bits to shift for probability updates.
pub const NUM_MOVE_BITS: u32 = 5;

/// Number of top bits for normalization threshold.
pub const NUM_TOP_BITS: u32 = 24;

/// Normalization threshold.
pub const TOP_VALUE: u32 = 1 << NUM_TOP_BITS;

/// Initial probability value (50% = 1024).
pub const INITIAL_PROB: u16 = (BIT_MODEL_TOTAL / 2) as u16;

/// LZMA Range Encoder.
///
/// Encodes a sequence of bits using range coding with adaptive probabilities.
/// The output is a byte stream that can be decoded by any LZMA-compatible decoder.
#[derive(Debug)]
pub struct LzmaRangeEncoder {
    range: u32,
    low: u64,
    cache: u8,
    cache_size: u32,
    output: Vec<u8>,
}

impl LzmaRangeEncoder {
    /// Creates a new range encoder.
    pub fn new() -> Self {
        Self {
            range: 0xFFFFFFFF,
            low: 0,
            cache: 0,
            cache_size: 0, // Count of pending 0xFF bytes (not including cache)
            output: Vec::with_capacity(4096),
        }
    }

    /// Returns the current encoded output length.
    ///
    /// This is an estimate that includes:
    /// - Bytes already written to output
    /// - The pending cache byte (1)
    /// - Pending 0xFF bytes awaiting carry resolution
    pub fn len(&self) -> usize {
        self.output.len() + 1 + self.cache_size as usize
    }

    /// Returns true if no data has been encoded.
    pub fn is_empty(&self) -> bool {
        self.output.is_empty() && self.cache_size == 0
    }

    /// Encodes a single bit with adaptive probability.
    ///
    /// Updates the probability based on the bit value:
    /// - If bit=0: prob increases toward 2048
    /// - If bit=1: prob decreases toward 0
    ///
    /// # Arguments
    /// * `prob` - Mutable reference to the probability value (0-2047)
    /// * `bit` - The bit to encode (true=1, false=0)
    pub fn encode_bit(&mut self, prob: &mut u16, bit: bool) {
        let p = *prob as u32;
        let bound = (self.range >> NUM_BIT_MODEL_TOTAL_BITS) * p;

        if bit {
            // Bit is 1: upper subrange
            self.low += bound as u64;
            self.range -= bound;
            *prob -= *prob >> NUM_MOVE_BITS;
        } else {
            // Bit is 0: lower subrange
            self.range = bound;
            *prob += ((BIT_MODEL_TOTAL - p) >> NUM_MOVE_BITS) as u16;
        }

        // Normalize
        self.normalize();
    }

    /// Encodes a bit with fixed 50% probability (no adaptation).
    ///
    /// Used for encoding direct bits in distance extra bits.
    pub fn encode_direct_bit(&mut self, bit: bool) {
        self.range >>= 1;
        if bit {
            self.low += self.range as u64;
        }
        self.normalize();
    }

    /// Encodes multiple direct bits (most significant first).
    ///
    /// Used for encoding distance extra bits.
    ///
    /// # Arguments
    /// * `value` - The value containing the bits to encode
    /// * `num_bits` - Number of bits to encode (1-32)
    pub fn encode_direct_bits(&mut self, value: u32, num_bits: u32) {
        for i in (0..num_bits).rev() {
            let bit = (value >> i) & 1;
            self.encode_direct_bit(bit != 0);
        }
    }

    /// Encodes a symbol using a bit tree (most significant bit first).
    ///
    /// A bit tree is a binary tree where each node has a probability.
    /// The symbol is encoded by traversing from root to leaf, encoding
    /// each decision bit along the way.
    ///
    /// # Arguments
    /// * `probs` - Array of probabilities (size = 2^num_bits)
    /// * `num_bits` - Number of bits in the symbol
    /// * `symbol` - The symbol to encode (0 to 2^num_bits - 1)
    pub fn encode_bit_tree(&mut self, probs: &mut [u16], num_bits: u32, symbol: u32) {
        let mut m = 1u32;
        for i in (0..num_bits).rev() {
            let bit = (symbol >> i) & 1;
            self.encode_bit(&mut probs[m as usize], bit != 0);
            m = (m << 1) | bit;
        }
    }

    /// Encodes a symbol using a reverse bit tree (least significant bit first).
    ///
    /// Used for encoding distance alignment bits.
    ///
    /// # Arguments
    /// * `probs` - Array of probabilities (size = 2^num_bits)
    /// * `num_bits` - Number of bits in the symbol
    /// * `symbol` - The symbol to encode (0 to 2^num_bits - 1)
    pub fn encode_bit_tree_reverse(&mut self, probs: &mut [u16], num_bits: u32, symbol: u32) {
        let mut m = 1u32;
        for i in 0..num_bits {
            let bit = (symbol >> i) & 1;
            self.encode_bit(&mut probs[m as usize], bit != 0);
            m = (m << 1) | bit;
        }
    }

    /// Normalizes the range and outputs bytes when needed.
    fn normalize(&mut self) {
        while self.range < TOP_VALUE {
            self.shift_low();
            self.range <<= 8;
        }
    }

    /// Shifts low bits to output, handling carry propagation.
    ///
    /// This function implements the standard LZMA range encoder carry handling:
    /// - Bits 0-31 of `low`: The working accumulator
    /// - Bits 32+: Overflow/carry bits that need to propagate
    ///
    /// Based on the reference implementation in range_enc.c
    fn shift_low(&mut self) {
        // Get overflow (carry) from bits 32+
        let overflow = (self.low >> 32) as u8;

        // Get lower 32 bits
        let low32 = self.low as u32;

        // Check if we need to output bytes:
        // - If top byte of low32 < 0xFF: no potential carry, safe to output
        // - If overflow != 0: carry happened, must propagate
        if (low32 < 0xFF000000) || (overflow != 0) {
            // Output the cached byte plus any overflow
            self.output.push(self.cache.wrapping_add(overflow));

            // Output pending 0xFF bytes with carry propagation
            // When overflow=0: 0xFF stays 0xFF
            // When overflow=1: 0xFF wraps to 0x00 (carry propagates through)
            let carry_byte = 0xFF_u8.wrapping_add(overflow);
            for _ in 0..self.cache_size {
                self.output.push(carry_byte);
            }

            // Update cache to top byte of low32
            self.cache = (low32 >> 24) as u8;
            self.cache_size = 0;
        } else {
            // Top byte is 0xFF and no overflow yet - accumulate pending byte
            self.cache_size += 1;
        }

        // Shift low left by 8 and keep only lower 32 bits (clear overflow)
        self.low = low32.wrapping_shl(8) as u64;
    }

    /// Finishes encoding and returns the output bytes.
    ///
    /// Flushes the remaining 5 bytes of state.
    pub fn finish(mut self) -> Vec<u8> {
        // Flush remaining bits (5 bytes like the decoder reads initially)
        for _ in 0..5 {
            self.shift_low();
        }
        self.output
    }
}

impl Default for LzmaRangeEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Initializes a probability array to INITIAL_PROB.
pub fn init_probs(probs: &mut [u16]) {
    for p in probs.iter_mut() {
        *p = INITIAL_PROB;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_bit_probability_update() {
        let mut rc = LzmaRangeEncoder::new();
        let mut prob = INITIAL_PROB;

        // Encoding 0 should increase probability
        let initial_prob = prob;
        rc.encode_bit(&mut prob, false);
        assert!(prob > initial_prob, "prob should increase for bit=0");

        // Encoding 1 should decrease probability
        let mid_prob = prob;
        rc.encode_bit(&mut prob, true);
        assert!(prob < mid_prob, "prob should decrease for bit=1");
    }

    #[test]
    fn test_encode_bit_tree() {
        let mut rc = LzmaRangeEncoder::new();
        let mut probs = [INITIAL_PROB; 8];

        // Encode symbol 5 (binary: 101) with 3 bits
        rc.encode_bit_tree(&mut probs, 3, 5);

        // Finish and verify output is valid
        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_encode_bit_tree_reverse() {
        let mut rc = LzmaRangeEncoder::new();
        let mut probs = [INITIAL_PROB; 16];

        // Encode symbol 10 (binary: 1010) with 4 bits in reverse
        rc.encode_bit_tree_reverse(&mut probs, 4, 10);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_encode_direct_bits() {
        let mut rc = LzmaRangeEncoder::new();

        // Encode various values with different bit widths
        rc.encode_direct_bits(0b1010, 4);
        rc.encode_direct_bits(0xFF, 8);
        rc.encode_direct_bits(0x1234, 16);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_init_probs() {
        let mut probs = [0u16; 100];
        init_probs(&mut probs);

        for p in probs.iter() {
            assert_eq!(*p, INITIAL_PROB);
        }
    }

    #[test]
    fn test_encoder_empty() {
        let rc = LzmaRangeEncoder::new();
        assert!(rc.is_empty());
    }

    #[test]
    fn test_encoder_finish_produces_5_bytes_minimum() {
        let rc = LzmaRangeEncoder::new();
        let output = rc.finish();
        // Even with no data, finish() flushes 5 bytes
        assert!(output.len() >= 5);
    }
}
