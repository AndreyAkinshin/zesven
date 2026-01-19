//! Radix match-finder for Fast LZMA2 compression.
//!
//! This module implements a hash-chain based match-finder optimized for
//! fast table building. It provides efficient string matching for LZMA2
//! compression.
#![allow(dead_code)]
//!
//! # Algorithm Overview
//!
//! 1. **Hash Table**: Maps 4-byte prefixes to positions
//! 2. **Chain Links**: Each position links to the previous position with same hash
//! 3. **Match Extension**: Matches are extended beyond the hash prefix
//!
//! # Bitpack Format
//!
//! Each table entry is a 32-bit value:
//! - Bits 0-25: Link to previous match position (26 bits, max 64MB)
//! - Bits 26-31: Stored match length hint (6 bits, max 63 bytes)
//!
//! # Reference
//!
//! Based on the Fast LZMA2 library by Conor McCarthy.

/// Null link value (26-bit maximum).
const RADIX_NULL_LINK: u32 = 0x03FF_FFFF;

/// Mask for extracting link from packed entry.
const RADIX_LINK_MASK: u32 = 0x03FF_FFFF;

/// Bit shift for length field.
const RADIX_LENGTH_SHIFT: u32 = 26;

/// Maximum storable match length (6 bits).
const RADIX_MAX_LENGTH: u32 = 63;

/// Minimum match length for LZMA.
const MIN_MATCH_LEN: usize = 2;

/// Maximum match length for LZMA.
const MAX_MATCH_LEN: usize = 273;

/// Hash table size (2^16 = 64K entries for 2-byte hash).
const HASH_SIZE: usize = 65536;
const HASH_MASK: u32 = (HASH_SIZE as u32) - 1;

/// A match found by the radix match-finder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Offset from current position (distance back in history).
    pub offset: u32,
    /// Length of the match in bytes.
    pub length: u32,
}

impl Match {
    /// Creates a new match.
    #[inline]
    pub fn new(offset: u32, length: u32) -> Self {
        Self { offset, length }
    }
}

/// Radix match-finder for fast string matching.
///
/// Uses hash-chain based matching for efficient string lookup.
///
/// # Memory Usage
///
/// - Match table: 4 bytes per input byte
/// - Hash table: 4MB (1M entries × 4 bytes)
pub struct RadixMatchFinder {
    /// Match table storing packed link entries.
    /// One entry per input byte position.
    table: Vec<u32>,

    /// Hash table mapping hash values to most recent position.
    hash_table: Vec<u32>,

    /// Dictionary size limit.
    dict_size: usize,

    /// Maximum search depth (chain traversal limit).
    max_depth: u32,
}

impl std::fmt::Debug for RadixMatchFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadixMatchFinder")
            .field("dict_size", &self.dict_size)
            .field("max_depth", &self.max_depth)
            .field("table_len", &self.table.len())
            .finish()
    }
}

impl RadixMatchFinder {
    /// Creates a new radix match-finder.
    ///
    /// # Arguments
    ///
    /// * `dict_size` - Dictionary size in bytes (max 64MB for bitpack format)
    /// * `max_depth` - Maximum search depth (typically 32-128)
    pub fn new(dict_size: usize, max_depth: u32) -> Self {
        // Clamp to 64MB maximum for bitpack format
        let dict_size = dict_size.min(64 * 1024 * 1024);

        Self {
            table: Vec::new(),
            hash_table: vec![RADIX_NULL_LINK; HASH_SIZE],
            dict_size,
            max_depth: max_depth.max(4),
        }
    }

    /// Computes a 2-byte hash for match finding.
    /// Using 2 bytes ensures consistent hashing for all positions.
    #[inline]
    fn hash2(data: &[u8], pos: usize) -> u32 {
        if pos + 2 > data.len() {
            return HASH_SIZE as u32; // Invalid hash
        }
        let h = ((data[pos] as u32) << 8) | (data[pos + 1] as u32);
        h & HASH_MASK
    }

    /// Builds the match table for input data.
    ///
    /// After calling this method, `get_match()` can be used to query
    /// matches at any position.
    ///
    /// # Arguments
    ///
    /// * `data` - Input data to index
    pub fn build(&mut self, data: &[u8]) {
        let len = data.len();
        if len < MIN_MATCH_LEN {
            self.table.clear();
            return;
        }

        // Resize table for this data
        self.table.clear();
        self.table.resize(len, RADIX_NULL_LINK);

        // Reset hash table
        self.hash_table.fill(RADIX_NULL_LINK);

        // Process each position
        let end = len.saturating_sub(MIN_MATCH_LEN - 1);
        for pos in 0..end {
            let hash = Self::hash2(data, pos);
            if hash >= HASH_SIZE as u32 {
                continue; // Invalid position
            }
            let prev_pos = self.hash_table[hash as usize];

            if prev_pos != RADIX_NULL_LINK {
                let prev = prev_pos as usize;
                // Verify this is actually a match (hash collision check)
                if prev < pos && Self::prefix_matches(data, prev, pos) {
                    // Calculate match length for the stored hint
                    let match_len = Self::match_length(data, prev, pos);
                    let stored_len = (match_len as u32).min(RADIX_MAX_LENGTH);
                    self.table[pos] = prev_pos | (stored_len << RADIX_LENGTH_SHIFT);
                }
            }

            // Update hash table with current position
            self.hash_table[hash as usize] = pos as u32;
        }
    }

    /// Check if two positions have matching prefix (at least 2 bytes).
    #[inline]
    fn prefix_matches(data: &[u8], pos1: usize, pos2: usize) -> bool {
        if pos1 + 2 > data.len() || pos2 + 2 > data.len() {
            return false;
        }
        data[pos1] == data[pos2] && data[pos1 + 1] == data[pos2 + 1]
    }

    /// Calculate the actual match length between two positions.
    /// Note: Overlapping matches are allowed (e.g., "aaaa" at distance 1 can match 3+ bytes).
    #[inline]
    fn match_length(data: &[u8], pos1: usize, pos2: usize) -> usize {
        let max_len = (data.len() - pos2).min(MAX_MATCH_LEN);

        let mut len = 0;
        while len < max_len && data[pos1 + len] == data[pos2 + len] {
            len += 1;
        }
        len
    }

    /// Finds the best match at the given position.
    ///
    /// # Arguments
    ///
    /// * `data` - Input data (must be same data passed to `build()`)
    /// * `pos` - Current position to find match for
    ///
    /// # Returns
    ///
    /// The best match found, or `None` if no match of at least 2 bytes exists.
    pub fn get_match(&self, data: &[u8], pos: usize) -> Option<Match> {
        if pos >= self.table.len() || pos + MIN_MATCH_LEN > data.len() {
            return None;
        }

        let entry = self.table[pos];
        let link = entry & RADIX_LINK_MASK;

        if link == RADIX_NULL_LINK {
            return None;
        }

        let match_pos = link as usize;
        if match_pos >= pos {
            // Invalid: match position must be before current position
            return None;
        }

        let distance = pos - match_pos;
        if distance > self.dict_size {
            return None;
        }

        // Calculate actual match length
        let actual_len = Self::match_length(data, match_pos, pos);

        if actual_len >= MIN_MATCH_LEN {
            Some(Match::new(distance as u32, actual_len as u32))
        } else {
            None
        }
    }

    /// Find all matches at position, traversing the chain.
    ///
    /// Returns matches sorted by length (longest first).
    pub fn find_matches(&self, data: &[u8], pos: usize, max_matches: usize) -> Vec<Match> {
        let mut matches = Vec::with_capacity(max_matches.min(16));

        if pos >= self.table.len() || pos + MIN_MATCH_LEN > data.len() {
            return matches;
        }

        let mut current_pos = pos;
        let mut iterations = 0;
        let max_iterations = self.max_depth as usize;

        // Follow the chain
        loop {
            if current_pos >= self.table.len() {
                break;
            }

            let entry = self.table[current_pos];
            let link = entry & RADIX_LINK_MASK;

            if link == RADIX_NULL_LINK {
                break;
            }

            let match_pos = link as usize;
            if match_pos >= current_pos {
                break;
            }

            let distance = pos - match_pos;
            if distance > self.dict_size {
                break;
            }

            // Calculate actual match length from original position
            let actual_len = Self::match_length(data, match_pos, pos);

            if actual_len >= MIN_MATCH_LEN {
                matches.push(Match::new(distance as u32, actual_len as u32));
                if matches.len() >= max_matches {
                    break;
                }
            }

            current_pos = match_pos;
            iterations += 1;
            if iterations >= max_iterations {
                break;
            }
        }

        // Sort by length descending
        matches.sort_by(|a, b| b.length.cmp(&a.length));
        matches
    }

    /// Returns the dictionary size.
    pub fn dict_size(&self) -> usize {
        self.dict_size
    }

    /// Resets the match-finder for a new block.
    pub fn reset(&mut self) {
        self.table.clear();
        self.hash_table.fill(RADIX_NULL_LINK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_struct() {
        let m = Match::new(100, 5);
        assert_eq!(m.offset, 100);
        assert_eq!(m.length, 5);
    }

    #[test]
    fn test_radix_mf_new() {
        let mf = RadixMatchFinder::new(1024 * 1024, 64);
        assert_eq!(mf.dict_size(), 1024 * 1024);
    }

    #[test]
    fn test_build_empty() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        mf.build(&[]);
        assert!(mf.table.is_empty());
    }

    #[test]
    fn test_build_short() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        mf.build(&[0]);
        assert!(mf.table.is_empty());
    }

    #[test]
    fn test_build_simple() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        let data = b"abcabcabc";
        mf.build(data);

        // Position 3 should have a match at position 0 (distance 3)
        let m = mf.get_match(data, 3);
        assert!(m.is_some(), "Expected match at position 3");
        let m = m.unwrap();
        assert_eq!(m.offset, 3); // Distance back to position 0
        assert!(m.length >= 3, "Expected at least 3 bytes match"); // At least "abc"
    }

    #[test]
    fn test_build_repeated() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        let data = b"aaaaaaaa";
        mf.build(data);

        // Position 2 should have a match (positions with same bytes)
        let m = mf.get_match(data, 2);
        assert!(m.is_some(), "Expected match at position 2");
        let m = m.unwrap();
        assert!(m.length >= 2);
    }

    #[test]
    fn test_no_match() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        let data = b"abcdefgh";
        mf.build(data);

        // No repeated patterns
        // Position 0 never has a match (nothing before it)
        let m = mf.get_match(data, 0);
        assert!(m.is_none());
    }

    #[test]
    fn test_find_matches() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        let data = b"abcabcabcabc";
        mf.build(data);

        // Position 9 should have matches
        let matches = mf.find_matches(data, 9, 10);
        assert!(!matches.is_empty(), "Expected at least one match");
        // Should be sorted by length descending
        for i in 1..matches.len() {
            assert!(matches[i - 1].length >= matches[i].length);
        }
    }

    #[test]
    fn test_extend_match() {
        let mut mf = RadixMatchFinder::new(1024, 4); // Low depth
        let data = b"abcdefabcdefgh"; // 6-char repeat at pos 6
        mf.build(data);

        let m = mf.get_match(data, 6);
        assert!(m.is_some(), "Expected match at position 6");
        let m = m.unwrap();
        // Should find full 6-byte match
        assert_eq!(m.length, 6);
    }

    #[test]
    fn test_reset() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        let data = b"abcabc";
        mf.build(data);
        assert!(!mf.table.is_empty());

        mf.reset();
        assert!(mf.table.is_empty());
    }

    #[test]
    fn test_dict_size_limit() {
        // Request larger than 64MB
        let mf = RadixMatchFinder::new(100 * 1024 * 1024, 32);
        // Should be clamped to 64MB
        assert_eq!(mf.dict_size(), 64 * 1024 * 1024);
    }

    #[test]
    fn test_longer_pattern() {
        let mut mf = RadixMatchFinder::new(1024, 32);
        // Pattern with longer repeated sequences
        let data = b"hello world hello world hello";
        mf.build(data);

        // Position 12 should match position 0 ("hello world")
        let m = mf.get_match(data, 12);
        assert!(m.is_some(), "Expected match at position 12");
        let m = m.unwrap();
        assert_eq!(m.offset, 12);
        assert!(m.length >= 11, "Expected at least 'hello world' match");
    }
}
