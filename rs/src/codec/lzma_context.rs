//! LZMA Encoder Context and Token Encoding.
//!
//! This module implements the LZMA probability context and encoding logic for:
#![allow(dead_code)]
//! - Literal bytes (with context-dependent encoding)
//! - Match lengths (using length encoder trees)
//! - Match distances (using slot and alignment encoding)
//! - State machine transitions (12 states)
//!
//! # Reference
//!
//! Based on the Fast LZMA2 implementation:
//! <https://github.com/mcmilk/7-Zip-zstd/blob/master/C/fast-lzma2/lzma2_enc.c>

use super::lzma_rc::{INITIAL_PROB, LzmaRangeEncoder, init_probs};

// LZMA Constants
const NUM_REPS: usize = 4;
const NUM_STATES: usize = 12;
const NUM_LIT_TABLES: usize = 3;

// Position state constants
const NUM_POS_BITS_MAX: usize = 4;
const NUM_POS_STATES_MAX: usize = 1 << NUM_POS_BITS_MAX;

// Length encoding constants
const LEN_NUM_LOW_BITS: u32 = 3;
const LEN_NUM_LOW_SYMBOLS: usize = 1 << LEN_NUM_LOW_BITS;
const LEN_NUM_MID_BITS: u32 = 3;
const LEN_NUM_MID_SYMBOLS: usize = 1 << LEN_NUM_MID_BITS;
const LEN_NUM_HIGH_BITS: u32 = 8;
const LEN_NUM_HIGH_SYMBOLS: usize = 1 << LEN_NUM_HIGH_BITS;

const MATCH_LEN_MIN: u32 = 2;

// Distance encoding constants
const NUM_LEN_TO_POS_STATES: usize = 4;
const NUM_POS_SLOT_BITS: u32 = 6;
const NUM_ALIGN_BITS: u32 = 4;
const ALIGN_TABLE_SIZE: usize = 1 << NUM_ALIGN_BITS;

const START_POS_MODEL_INDEX: usize = 4;
const END_POS_MODEL_INDEX: usize = 14;
const NUM_FULL_DISTANCES_BITS: usize = END_POS_MODEL_INDEX / 2;
const NUM_FULL_DISTANCES: usize = 1 << NUM_FULL_DISTANCES_BITS;

// State transitions
const LIT_NEXT_STATES: [usize; NUM_STATES] = [0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 4, 5];
const MATCH_NEXT_STATES: [usize; NUM_STATES] = [7, 7, 7, 7, 7, 7, 7, 10, 10, 10, 10, 10];
const REP_NEXT_STATES: [usize; NUM_STATES] = [8, 8, 8, 8, 8, 8, 8, 11, 11, 11, 11, 11];
const SHORT_REP_NEXT_STATES: [usize; NUM_STATES] = [9, 9, 9, 9, 9, 9, 9, 11, 11, 11, 11, 11];

/// Length encoder for match and rep lengths.
///
/// Uses a 3-tree structure:
/// - Low tree: lengths 0-7 (encoded as 2-9)
/// - Mid tree: lengths 8-15 (encoded as 10-17)
/// - High tree: lengths 16-271 (encoded as 18-273)
#[derive(Clone)]
pub struct LengthEncoder {
    choice: u16,
    choice2: u16,
    low: [[u16; LEN_NUM_LOW_SYMBOLS]; NUM_POS_STATES_MAX],
    mid: [[u16; LEN_NUM_MID_SYMBOLS]; NUM_POS_STATES_MAX],
    high: [u16; LEN_NUM_HIGH_SYMBOLS],
}

impl LengthEncoder {
    /// Creates a new length encoder with probabilities initialized.
    pub fn new() -> Self {
        let mut enc = Self {
            choice: INITIAL_PROB,
            choice2: INITIAL_PROB,
            low: [[INITIAL_PROB; LEN_NUM_LOW_SYMBOLS]; NUM_POS_STATES_MAX],
            mid: [[INITIAL_PROB; LEN_NUM_MID_SYMBOLS]; NUM_POS_STATES_MAX],
            high: [INITIAL_PROB; LEN_NUM_HIGH_SYMBOLS],
        };
        enc.reset();
        enc
    }

    /// Resets all probabilities to initial values.
    pub fn reset(&mut self) {
        self.choice = INITIAL_PROB;
        self.choice2 = INITIAL_PROB;
        for ps in &mut self.low {
            init_probs(ps);
        }
        for ps in &mut self.mid {
            init_probs(ps);
        }
        init_probs(&mut self.high);
    }

    /// Encodes a length value.
    ///
    /// # Arguments
    /// * `rc` - Range encoder
    /// * `length` - Length to encode (2-273)
    /// * `pos_state` - Position state (0 to NUM_POS_STATES_MAX-1)
    pub fn encode(&mut self, rc: &mut LzmaRangeEncoder, length: u32, pos_state: usize) {
        let len = length - MATCH_LEN_MIN;

        if len < LEN_NUM_LOW_SYMBOLS as u32 {
            // Low tree: 0-7
            rc.encode_bit(&mut self.choice, false);
            rc.encode_bit_tree(&mut self.low[pos_state], LEN_NUM_LOW_BITS, len);
        } else if len < (LEN_NUM_LOW_SYMBOLS + LEN_NUM_MID_SYMBOLS) as u32 {
            // Mid tree: 8-15
            rc.encode_bit(&mut self.choice, true);
            rc.encode_bit(&mut self.choice2, false);
            let symbol = len - LEN_NUM_LOW_SYMBOLS as u32;
            rc.encode_bit_tree(&mut self.mid[pos_state], LEN_NUM_MID_BITS, symbol);
        } else {
            // High tree: 16-271
            rc.encode_bit(&mut self.choice, true);
            rc.encode_bit(&mut self.choice2, true);
            let symbol = len - (LEN_NUM_LOW_SYMBOLS + LEN_NUM_MID_SYMBOLS) as u32;
            rc.encode_bit_tree(&mut self.high, LEN_NUM_HIGH_BITS, symbol);
        }
    }
}

impl Default for LengthEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// LZMA encoder state with all probability arrays.
///
/// This structure maintains the adaptive probabilities used for
/// encoding literals, matches, and distances.
#[allow(dead_code)] // Some fields reserved for future use
pub struct LzmaEncoderState {
    // LZMA parameters
    lc: u32, // Literal context bits (0-8)
    lp: u32, // Literal position bits (0-4)
    pb: u32, // Position bits (0-4)

    // Derived values
    lc_lp_mask: u32,
    pos_state_mask: u32,

    // State machine (0-11)
    state: usize,

    // Last 4 repetition distances
    reps: [u32; NUM_REPS],

    // Decision probabilities
    is_match: [[u16; NUM_POS_STATES_MAX]; NUM_STATES],
    is_rep: [u16; NUM_STATES],
    is_rep_g0: [u16; NUM_STATES],
    is_rep_g1: [u16; NUM_STATES],
    is_rep_g2: [u16; NUM_STATES],
    is_rep0_long: [[u16; NUM_POS_STATES_MAX]; NUM_STATES],

    // Literal encoding (context-dependent)
    literal_probs: Vec<u16>,

    // Distance encoding
    dist_slot_encoders: [[u16; 1 << NUM_POS_SLOT_BITS]; NUM_LEN_TO_POS_STATES],
    dist_align_encoders: [u16; ALIGN_TABLE_SIZE],
    dist_encoders: [u16; NUM_FULL_DISTANCES - END_POS_MODEL_INDEX],

    // Length encoding (match + rep)
    len_encoder: LengthEncoder,
    rep_len_encoder: LengthEncoder,
}

impl LzmaEncoderState {
    /// Creates a new encoder state with the given LZMA parameters.
    ///
    /// # Arguments
    /// * `lc` - Literal context bits (typically 3)
    /// * `lp` - Literal position bits (typically 0)
    /// * `pb` - Position bits (typically 2)
    pub fn new(lc: u32, lp: u32, pb: u32) -> Self {
        let num_literal_probs = (NUM_LIT_TABLES * 256) << (lc + lp);

        let mut state = Self {
            lc,
            lp,
            pb,
            lc_lp_mask: (1 << (lc + lp)) - 1,
            pos_state_mask: (1 << pb) - 1,
            state: 0,
            reps: [0; NUM_REPS],
            is_match: [[INITIAL_PROB; NUM_POS_STATES_MAX]; NUM_STATES],
            is_rep: [INITIAL_PROB; NUM_STATES],
            is_rep_g0: [INITIAL_PROB; NUM_STATES],
            is_rep_g1: [INITIAL_PROB; NUM_STATES],
            is_rep_g2: [INITIAL_PROB; NUM_STATES],
            is_rep0_long: [[INITIAL_PROB; NUM_POS_STATES_MAX]; NUM_STATES],
            literal_probs: vec![INITIAL_PROB; num_literal_probs],
            dist_slot_encoders: [[INITIAL_PROB; 1 << NUM_POS_SLOT_BITS]; NUM_LEN_TO_POS_STATES],
            dist_align_encoders: [INITIAL_PROB; ALIGN_TABLE_SIZE],
            dist_encoders: [INITIAL_PROB; NUM_FULL_DISTANCES - END_POS_MODEL_INDEX],
            len_encoder: LengthEncoder::new(),
            rep_len_encoder: LengthEncoder::new(),
        };
        state.reset();
        state
    }

    /// Resets the encoder state for a new block.
    pub fn reset(&mut self) {
        self.state = 0;
        self.reps = [0; NUM_REPS];

        for row in &mut self.is_match {
            init_probs(row);
        }
        init_probs(&mut self.is_rep);
        init_probs(&mut self.is_rep_g0);
        init_probs(&mut self.is_rep_g1);
        init_probs(&mut self.is_rep_g2);
        for row in &mut self.is_rep0_long {
            init_probs(row);
        }
        init_probs(&mut self.literal_probs);
        for row in &mut self.dist_slot_encoders {
            init_probs(row);
        }
        init_probs(&mut self.dist_align_encoders);
        init_probs(&mut self.dist_encoders);
        self.len_encoder.reset();
        self.rep_len_encoder.reset();
    }

    /// Returns the position state for the given position.
    fn pos_state(&self, pos: usize) -> usize {
        pos & self.pos_state_mask as usize
    }

    /// Returns the literal context index.
    fn literal_context(&self, pos: usize, prev_byte: u8) -> usize {
        // Position is masked by lp bits only (not lc+lp)
        let lp_mask = (1usize << self.lp) - 1;
        let pos_bits = pos & lp_mask;
        let prev_bits = (prev_byte as usize) >> (8 - self.lc as usize);
        (pos_bits << self.lc as usize) + prev_bits
    }

    /// Encodes a literal byte.
    ///
    /// # Arguments
    /// * `rc` - Range encoder
    /// * `byte` - The literal byte to encode
    /// * `pos` - Current position in output
    /// * `prev_byte` - Previous byte (for context)
    /// * `match_byte` - Match byte if in "after match" state (for matched literal)
    pub fn encode_literal(
        &mut self,
        rc: &mut LzmaRangeEncoder,
        byte: u8,
        pos: usize,
        prev_byte: u8,
        match_byte: Option<u8>,
    ) {
        let pos_state = self.pos_state(pos);

        // Encode is_match = 0 (this is a literal)
        rc.encode_bit(&mut self.is_match[self.state][pos_state], false);

        let context = self.literal_context(pos, prev_byte);
        let probs_offset = context * NUM_LIT_TABLES * 256;

        if self.state >= 7 {
            // After match/rep: use matched literal encoding
            if let Some(mb) = match_byte {
                self.encode_matched_literal(rc, probs_offset, byte, mb);
            } else {
                self.encode_normal_literal(rc, probs_offset, byte);
            }
        } else {
            // Normal literal encoding
            self.encode_normal_literal(rc, probs_offset, byte);
        }

        // Update state
        self.state = LIT_NEXT_STATES[self.state];
    }

    /// Encodes a normal literal using bit tree.
    fn encode_normal_literal(&mut self, rc: &mut LzmaRangeEncoder, probs_offset: usize, byte: u8) {
        let mut symbol = 1u32;
        for i in (0..8).rev() {
            let bit = ((byte >> i) & 1) != 0;
            let prob_idx = probs_offset + symbol as usize;
            rc.encode_bit(&mut self.literal_probs[prob_idx], bit);
            symbol = (symbol << 1) | (bit as u32);
        }
    }

    /// Encodes a matched literal (after match/rep state).
    fn encode_matched_literal(
        &mut self,
        rc: &mut LzmaRangeEncoder,
        probs_offset: usize,
        byte: u8,
        match_byte: u8,
    ) {
        let mut symbol = 1u32;
        let mut offset = 0x100usize;

        for i in (0..8).rev() {
            let bit = ((byte >> i) & 1) != 0;
            let match_bit = ((match_byte >> i) & 1) as usize;

            let prob_idx = probs_offset + offset + match_bit * 0x100 + symbol as usize;
            rc.encode_bit(&mut self.literal_probs[prob_idx], bit);

            symbol = (symbol << 1) | (bit as u32);

            if match_bit != (bit as usize) {
                // Mismatch: switch to normal encoding for remaining bits
                offset = 0;
            }
        }
    }

    /// Encodes a match (distance + length).
    ///
    /// # Arguments
    /// * `rc` - Range encoder
    /// * `distance` - Match distance (1-indexed, so distance=1 means offset of 1)
    /// * `length` - Match length (2-273)
    /// * `pos` - Current position in output
    pub fn encode_match(
        &mut self,
        rc: &mut LzmaRangeEncoder,
        distance: u32,
        length: u32,
        pos: usize,
    ) {
        let pos_state = self.pos_state(pos);

        // Encode is_match = 1
        rc.encode_bit(&mut self.is_match[self.state][pos_state], true);

        // Encode is_rep = 0 (this is a new match, not a repetition)
        rc.encode_bit(&mut self.is_rep[self.state], false);

        // Encode length
        self.len_encoder.encode(rc, length, pos_state);

        // Encode distance
        let dist = distance - 1; // Convert to 0-indexed
        self.encode_distance(rc, dist, length);

        // Update reps (shift in the new distance)
        self.reps[3] = self.reps[2];
        self.reps[2] = self.reps[1];
        self.reps[1] = self.reps[0];
        self.reps[0] = dist;

        // Update state
        self.state = MATCH_NEXT_STATES[self.state];
    }

    /// Encodes a repetition match (using a previous distance).
    ///
    /// # Arguments
    /// * `rc` - Range encoder
    /// * `rep_index` - Which rep to use (0-3)
    /// * `length` - Match length (2-273, or 1 for short rep)
    /// * `pos` - Current position in output
    pub fn encode_rep(
        &mut self,
        rc: &mut LzmaRangeEncoder,
        rep_index: usize,
        length: u32,
        pos: usize,
    ) {
        let pos_state = self.pos_state(pos);

        // Encode is_match = 1
        rc.encode_bit(&mut self.is_match[self.state][pos_state], true);

        // Encode is_rep = 1
        rc.encode_bit(&mut self.is_rep[self.state], true);

        if rep_index == 0 {
            rc.encode_bit(&mut self.is_rep_g0[self.state], false);
            if length == 1 {
                // Short rep (single byte)
                rc.encode_bit(&mut self.is_rep0_long[self.state][pos_state], false);
                self.state = SHORT_REP_NEXT_STATES[self.state];
                return;
            } else {
                rc.encode_bit(&mut self.is_rep0_long[self.state][pos_state], true);
            }
        } else {
            rc.encode_bit(&mut self.is_rep_g0[self.state], true);
            if rep_index == 1 {
                rc.encode_bit(&mut self.is_rep_g1[self.state], false);
            } else {
                rc.encode_bit(&mut self.is_rep_g1[self.state], true);
                if rep_index == 2 {
                    rc.encode_bit(&mut self.is_rep_g2[self.state], false);
                } else {
                    rc.encode_bit(&mut self.is_rep_g2[self.state], true);
                }
            }
            // Rotate reps to put used one at position 0
            let rep_dist = self.reps[rep_index];
            for i in (1..=rep_index).rev() {
                self.reps[i] = self.reps[i - 1];
            }
            self.reps[0] = rep_dist;
        }

        // Encode length
        self.rep_len_encoder.encode(rc, length, pos_state);

        // Update state
        self.state = REP_NEXT_STATES[self.state];
    }

    /// Encodes a distance value.
    ///
    /// Distance encoding uses:
    /// - Slots 0-3: Direct distance (no extra bits)
    /// - Slots 4-13: Slot + reverse bit tree encoding
    /// - Slots 14+: Slot + direct bits + alignment bits
    fn encode_distance(&mut self, rc: &mut LzmaRangeEncoder, dist: u32, length: u32) {
        // Get length-to-pos state
        let len_state = ((length - MATCH_LEN_MIN) as usize).min(NUM_LEN_TO_POS_STATES - 1);

        // Get distance slot
        let slot = get_dist_slot(dist);

        // Encode slot
        rc.encode_bit_tree(
            &mut self.dist_slot_encoders[len_state],
            NUM_POS_SLOT_BITS,
            slot,
        );

        if slot >= START_POS_MODEL_INDEX as u32 {
            let num_direct_bits = (slot >> 1) - 1;
            let base = (2 | (slot & 1)) << num_direct_bits;
            let dist_reduced = dist - base;

            if slot < END_POS_MODEL_INDEX as u32 {
                // Mid-range distances: use reverse bit tree with special probability array
                // Calculate the base index for this slot in dist_encoders
                let footer_bits = num_direct_bits;
                let base_idx = self.get_dist_encoder_base(slot);
                self.encode_dist_special(rc, base_idx, dist_reduced, footer_bits);
            } else {
                // Large distances: direct bits + alignment
                let direct_bits = num_direct_bits - NUM_ALIGN_BITS;
                rc.encode_direct_bits(dist_reduced >> NUM_ALIGN_BITS, direct_bits);

                // Alignment bits (reverse bit tree)
                let align_symbol = dist_reduced & (ALIGN_TABLE_SIZE as u32 - 1);
                rc.encode_bit_tree_reverse(
                    &mut self.dist_align_encoders,
                    NUM_ALIGN_BITS,
                    align_symbol,
                );
            }
        }
    }

    /// Gets the base index in dist_encoders for a given slot.
    fn get_dist_encoder_base(&self, slot: u32) -> usize {
        // For slots 4-13, calculate cumulative position
        // Slot 4: starts at 0 (2 probs)
        // Slot 5: starts at 2 (2 probs)
        // Slot 6: starts at 4 (4 probs)
        // Slot 7: starts at 8 (4 probs)
        // etc.
        let mut base = 0usize;
        for s in START_POS_MODEL_INDEX as u32..slot {
            let bits = (s >> 1) - 1;
            base += 1 << bits;
        }
        base
    }

    /// Encodes distance extra bits using special distance encoders (reverse bit tree).
    fn encode_dist_special(
        &mut self,
        rc: &mut LzmaRangeEncoder,
        base_idx: usize,
        symbol: u32,
        num_bits: u32,
    ) {
        let mut m = 1u32;
        for i in 0..num_bits {
            let bit = (symbol >> i) & 1;
            let idx = base_idx + m as usize - 1;
            if idx < self.dist_encoders.len() {
                rc.encode_bit(&mut self.dist_encoders[idx], bit != 0);
            }
            m = (m << 1) | bit;
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> usize {
        self.state
    }

    /// Returns the current reps array.
    pub fn reps(&self) -> &[u32; NUM_REPS] {
        &self.reps
    }
}

/// Gets the distance slot for a distance value.
///
/// Distance slots are logarithmically distributed:
/// - Slots 0-3: distances 0-3
/// - Slots 4-5: distances 4-7
/// - Slots 6-7: distances 8-15
/// - etc.
///
/// For dist >= 4, the slot is: 2 * (highest_bit_pos - 1) + second_highest_bit
fn get_dist_slot(dist: u32) -> u32 {
    if dist < 4 {
        return dist;
    }

    // Find position of highest bit (1-indexed)
    let highest_bit_pos = 32 - dist.leading_zeros(); // e.g., dist=4 → pos=3

    // Get the second highest bit
    let second_bit = (dist >> (highest_bit_pos - 2)) & 1;

    // Slot = 2 * (highest_bit_pos - 1) + second_bit
    (highest_bit_pos - 1) * 2 + second_bit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_length_encoder_low() {
        let mut enc = LengthEncoder::new();
        let mut rc = LzmaRangeEncoder::new();

        // Encode length 2 (minimum, uses low tree)
        enc.encode(&mut rc, 2, 0);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_length_encoder_mid() {
        let mut enc = LengthEncoder::new();
        let mut rc = LzmaRangeEncoder::new();

        // Encode length 12 (uses mid tree)
        enc.encode(&mut rc, 12, 0);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_length_encoder_high() {
        let mut enc = LengthEncoder::new();
        let mut rc = LzmaRangeEncoder::new();

        // Encode length 100 (uses high tree)
        enc.encode(&mut rc, 100, 0);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_dist_slot() {
        // Slot 0-3: direct distances
        assert_eq!(get_dist_slot(0), 0);
        assert_eq!(get_dist_slot(1), 1);
        assert_eq!(get_dist_slot(2), 2);
        assert_eq!(get_dist_slot(3), 3);

        // Slot 4-5: distances 4-7
        assert_eq!(get_dist_slot(4), 4);
        assert_eq!(get_dist_slot(5), 4);
        assert_eq!(get_dist_slot(6), 5);
        assert_eq!(get_dist_slot(7), 5);

        // Slot 6-7: distances 8-15
        assert_eq!(get_dist_slot(8), 6);
        assert_eq!(get_dist_slot(15), 7);
    }

    #[test]
    fn test_state_transitions() {
        // After literal in state 0, should stay at 0
        assert_eq!(LIT_NEXT_STATES[0], 0);

        // After match in state 0, should go to 7
        assert_eq!(MATCH_NEXT_STATES[0], 7);

        // After rep in state 7, should go to 11
        assert_eq!(REP_NEXT_STATES[7], 11);

        // After short rep in state 0, should go to 9
        assert_eq!(SHORT_REP_NEXT_STATES[0], 9);
    }

    #[test]
    fn test_encoder_state_new() {
        let state = LzmaEncoderState::new(3, 0, 2);
        assert_eq!(state.state(), 0);
        assert_eq!(state.reps(), &[0, 0, 0, 0]);
    }

    #[test]
    fn test_encode_literal() {
        let mut state = LzmaEncoderState::new(3, 0, 2);
        let mut rc = LzmaRangeEncoder::new();

        // Encode a literal 'A'
        state.encode_literal(&mut rc, b'A', 0, 0, None);

        assert_eq!(state.state(), 0); // Still in state 0 after literal
        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_encode_match() {
        let mut state = LzmaEncoderState::new(3, 0, 2);
        let mut rc = LzmaRangeEncoder::new();

        // Encode a match at distance 10, length 5
        state.encode_match(&mut rc, 10, 5, 0);

        assert_eq!(state.state(), 7); // State after match
        assert_eq!(state.reps()[0], 9); // Distance is stored 0-indexed

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_encode_rep() {
        let mut state = LzmaEncoderState::new(3, 0, 2);
        let mut rc = LzmaRangeEncoder::new();

        // First encode a match to set up reps
        state.encode_match(&mut rc, 10, 5, 0);

        // Then encode a rep0 of length 3
        state.encode_rep(&mut rc, 0, 3, 5);

        let output = rc.finish();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_literal_context() {
        let state = LzmaEncoderState::new(3, 0, 2);

        // With lc=3, lp=0: context depends on position and high 3 bits of prev_byte
        let ctx1 = state.literal_context(0, 0);
        let ctx2 = state.literal_context(0, 0x80); // High bit set

        assert_ne!(ctx1, ctx2);
    }
}
