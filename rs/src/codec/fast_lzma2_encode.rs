//! Fast LZMA2 encoding implementation.
//!
//! This module provides the core encoding logic for Fast LZMA2, including:
//! - Token types (literals and matches)
//! - Greedy match selection
//! - LZMA2 block output (uncompressed and compressed)

use std::io::{self, Write};

use super::radix_mf::RadixMatchFinder;

/// Maximum size for LZMA2 uncompressed chunks (64KB - 1).
const MAX_UNCOMPRESSED_CHUNK_SIZE: usize = 65535;

/// Block size for compression (1MB default).
pub const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;

/// Compression token representing either a literal byte or a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// A literal byte (no match found).
    Literal(u8),
    /// A match with distance and length.
    Match {
        /// Distance back in the history (1-indexed).
        distance: u32,
        /// Length of the match (2-273 bytes).
        length: u32,
    },
}

impl Token {
    /// Returns true if this is a literal token.
    pub fn is_literal(&self) -> bool {
        matches!(self, Token::Literal(_))
    }

    /// Returns true if this is a match token.
    pub fn is_match(&self) -> bool {
        matches!(self, Token::Match { .. })
    }
}

/// Performs greedy encoding using the match finder.
///
/// This simple greedy algorithm:
/// 1. At each position, looks for the best match
/// 2. If a match is found (length >= 2), emits it and advances by match length
/// 3. Otherwise, emits a literal and advances by 1
///
/// # Arguments
///
/// * `data` - The input data to encode
/// * `match_finder` - The match finder with pre-built tables
///
/// # Returns
///
/// A vector of tokens representing the encoded data.
pub fn encode_greedy(data: &[u8], match_finder: &RadixMatchFinder) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(data.len());
    let mut pos = 0;

    while pos < data.len() {
        if let Some(m) = match_finder.get_match(data, pos) {
            if m.length >= 2 {
                tokens.push(Token::Match {
                    distance: m.offset,
                    length: m.length,
                });
                pos += m.length as usize;
                continue;
            }
        }
        tokens.push(Token::Literal(data[pos]));
        pos += 1;
    }

    tokens
}

/// Statistics about an encoding result.
#[derive(Debug, Clone, Default)]
pub struct EncodingStats {
    /// Number of literal tokens.
    pub literals: usize,
    /// Number of match tokens.
    pub matches: usize,
    /// Total bytes encoded by matches.
    pub matched_bytes: usize,
}

impl EncodingStats {
    /// Computes statistics from a token stream.
    pub fn from_tokens(tokens: &[Token]) -> Self {
        let mut stats = Self::default();
        for token in tokens {
            match token {
                Token::Literal(_) => stats.literals += 1,
                Token::Match { length, .. } => {
                    stats.matches += 1;
                    stats.matched_bytes += *length as usize;
                }
            }
        }
        stats
    }

    /// Returns the match ratio (matched bytes / total bytes).
    pub fn match_ratio(&self) -> f64 {
        let total = self.literals + self.matched_bytes;
        if total == 0 {
            0.0
        } else {
            self.matched_bytes as f64 / total as f64
        }
    }
}

/// Writes an LZMA2 uncompressed chunk.
///
/// Uncompressed chunks have the format:
/// ```text
/// [ctrl] [size_high] [size_low] [data...]
/// ```
///
/// Where ctrl is:
/// - 0x01: Uncompressed, reset dictionary
/// - 0x02: Uncompressed, no dictionary reset
///
/// # Arguments
///
/// * `output` - The output writer
/// * `data` - The uncompressed data (max 65535 bytes)
/// * `reset_dict` - Whether to reset the dictionary
pub fn write_uncompressed_chunk(
    output: &mut impl Write,
    data: &[u8],
    reset_dict: bool,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Chunk size is limited to 64KB - 1
    let chunk_size = data.len().min(MAX_UNCOMPRESSED_CHUNK_SIZE);
    let ctrl = if reset_dict { 0x01u8 } else { 0x02u8 };

    // Size is stored as (size - 1) in big-endian
    let size_minus_one = (chunk_size - 1) as u16;

    output.write_all(&[ctrl])?;
    output.write_all(&size_minus_one.to_be_bytes())?;
    output.write_all(&data[..chunk_size])?;

    Ok(())
}

/// Writes multiple LZMA2 uncompressed chunks for large data.
///
/// Splits data into chunks of up to 64KB each.
///
/// # Arguments
///
/// * `output` - The output writer
/// * `data` - The data to write
/// * `reset_dict_first` - Whether to reset dictionary on first chunk
pub fn write_uncompressed_data(
    output: &mut impl Write,
    data: &[u8],
    reset_dict_first: bool,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut pos = 0;
    let mut first = true;

    while pos < data.len() {
        let remaining = data.len() - pos;
        let chunk_size = remaining.min(MAX_UNCOMPRESSED_CHUNK_SIZE);

        let reset = first && reset_dict_first;
        write_uncompressed_chunk(output, &data[pos..pos + chunk_size], reset)?;

        pos += chunk_size;
        first = false;
    }

    Ok(())
}

/// Writes the LZMA2 end-of-stream marker (0x00).
pub fn write_end_marker(output: &mut impl Write) -> io::Result<()> {
    output.write_all(&[0x00])
}

/// LZMA2 control byte flags for compressed chunks.
#[allow(dead_code)] // Constants module reserved for full LZMA2 encoder
mod ctrl {
    /// End of stream marker.
    pub const END_MARKER: u8 = 0x00;
    /// Uncompressed chunk, reset dictionary.
    pub const UNCOMPRESSED_RESET: u8 = 0x01;
    /// Uncompressed chunk, no dictionary reset.
    pub const UNCOMPRESSED_NO_RESET: u8 = 0x02;
    /// LZMA compressed chunk base (bits 0-4 are uncompressed size high bits).
    pub const LZMA_BASE: u8 = 0x80;
    /// LZMA compressed, reset state.
    pub const LZMA_RESET_STATE: u8 = 0x80;
    /// LZMA compressed, reset state and properties.
    pub const LZMA_RESET_STATE_PROPS: u8 = 0xC0;
    /// LZMA compressed, reset state, properties, and dictionary.
    pub const LZMA_RESET_ALL: u8 = 0xE0;
}

/// Reset mode for LZMA2 compressed chunks.
///
/// The mode determines what encoder state is reset at the start of a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChunkResetMode {
    /// No reset - continue with previous encoder state.
    /// Use for subsequent chunks after the first.
    None = 0xA0,
    /// Reset state only - keep dictionary and properties.
    StateReset = 0x80,
    /// Reset state and properties - keep dictionary.
    StatePropsReset = 0xC0,
    /// Reset everything - state, properties, and dictionary.
    /// Must be used for the first chunk.
    AllReset = 0xE0,
}

impl ChunkResetMode {
    /// Returns true if this mode includes properties in the chunk.
    pub fn includes_props(self) -> bool {
        matches!(
            self,
            ChunkResetMode::StatePropsReset | ChunkResetMode::AllReset
        )
    }
}

/// Maximum size for LZMA2 compressed chunks (64KB uncompressed).
const MAX_COMPRESSED_CHUNK_UNPACK_SIZE: usize = 1 << 16;

/// Writes an LZMA2 compressed chunk.
///
/// Compressed chunks have the format:
/// ```text
/// [ctrl] [unpacked_size_low] [packed_size] [props?] [data...]
/// ```
///
/// Where:
/// - ctrl: Control byte with reset mode and high 5 bits of (unpacked_size - 1)
/// - unpacked_size_low: Low 16 bits of (unpacked_size - 1) (big-endian u16)
/// - packed_size: Compressed data size minus 1 (big-endian u16)
/// - props: LZMA properties byte (only if reset mode includes props)
/// - data: Range-encoded LZMA data
///
/// # Arguments
///
/// * `output` - The output writer
/// * `compressed` - The range-encoded LZMA data
/// * `uncompressed_size` - Original uncompressed size (max 64KB)
/// * `reset_mode` - The reset mode for this chunk
/// * `props` - LZMA properties byte (required if reset_mode includes props)
pub fn write_compressed_chunk(
    output: &mut impl Write,
    compressed: &[u8],
    uncompressed_size: usize,
    reset_mode: ChunkResetMode,
    props: Option<u8>,
) -> io::Result<()> {
    if compressed.is_empty() || uncompressed_size == 0 {
        return Ok(());
    }

    // Validate chunk size
    debug_assert!(uncompressed_size <= MAX_COMPRESSED_CHUNK_UNPACK_SIZE);

    // Uncompressed size is stored as (size - 1)
    let unpack_size_minus1 = (uncompressed_size - 1) as u32;

    // Control byte: reset_mode base + high 5 bits of (uncompressed_size - 1)
    let unpack_high_bits = ((unpack_size_minus1 >> 16) & 0x1F) as u8;
    let ctrl = (reset_mode as u8) | unpack_high_bits;

    // Packed size minus 1
    let pack_size_minus1 = (compressed.len() - 1) as u16;

    // Low 16 bits of uncompressed size minus 1
    let unpack_size_low = (unpack_size_minus1 & 0xFFFF) as u16;

    output.write_all(&[ctrl])?;
    // LZMA2 format: unpack_size_low comes first, then pack_size
    // (per lzma_rust2 decoder: reads uncompressed_size, then compressed_size)
    output.write_all(&unpack_size_low.to_be_bytes())?;
    output.write_all(&pack_size_minus1.to_be_bytes())?;

    if reset_mode.includes_props() {
        // Default properties: lc=3, lp=0, pb=2 -> 0x5D
        output.write_all(&[props.unwrap_or(0x5D)])?;
    }

    output.write_all(compressed)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_is_literal() {
        assert!(Token::Literal(b'a').is_literal());
        assert!(
            !Token::Match {
                distance: 1,
                length: 2
            }
            .is_literal()
        );
    }

    #[test]
    fn test_token_is_match() {
        assert!(!Token::Literal(b'a').is_match());
        assert!(
            Token::Match {
                distance: 1,
                length: 2
            }
            .is_match()
        );
    }

    #[test]
    fn test_encoding_stats() {
        let tokens = vec![
            Token::Literal(b'a'),
            Token::Literal(b'b'),
            Token::Match {
                distance: 2,
                length: 5,
            },
            Token::Literal(b'c'),
        ];

        let stats = EncodingStats::from_tokens(&tokens);
        assert_eq!(stats.literals, 3);
        assert_eq!(stats.matches, 1);
        assert_eq!(stats.matched_bytes, 5);
    }

    #[test]
    fn test_write_uncompressed_chunk() {
        let mut output = Vec::new();
        let data = b"Hello, World!";

        write_uncompressed_chunk(&mut output, data, true).unwrap();

        // Check control byte
        assert_eq!(output[0], 0x01); // reset dict

        // Check size (big-endian, size - 1)
        let size = u16::from_be_bytes([output[1], output[2]]);
        assert_eq!(size, 12); // 13 - 1 = 12

        // Check data
        assert_eq!(&output[3..], data);
    }

    #[test]
    fn test_write_uncompressed_chunk_no_reset() {
        let mut output = Vec::new();
        let data = b"Test";

        write_uncompressed_chunk(&mut output, data, false).unwrap();

        // Check control byte
        assert_eq!(output[0], 0x02); // no reset
    }

    #[test]
    fn test_write_end_marker() {
        let mut output = Vec::new();
        write_end_marker(&mut output).unwrap();
        assert_eq!(output, vec![0x00]);
    }

    #[test]
    fn test_encode_greedy_all_literals() {
        // Data with no repeats
        let data = b"abcdefgh";
        let mut mf = RadixMatchFinder::new(1024, 32);
        mf.build(data);

        let tokens = encode_greedy(data, &mf);

        // Should be all literals
        assert_eq!(tokens.len(), 8);
        for (i, token) in tokens.iter().enumerate() {
            assert_eq!(*token, Token::Literal(data[i]));
        }
    }

    #[test]
    fn test_encode_greedy_with_matches() {
        // Data with repeating pattern
        let data = b"abcabcabc";
        let mut mf = RadixMatchFinder::new(1024, 32);
        mf.build(data);

        let tokens = encode_greedy(data, &mf);

        // First "abc" should be literals, then matches for the rest
        let stats = EncodingStats::from_tokens(&tokens);
        assert!(stats.matches > 0, "Should find at least one match");
        assert!(stats.literals > 0, "Should have some literals");
    }

    #[test]
    fn test_write_uncompressed_data_large() {
        // Create data larger than one chunk (64KB)
        let data: Vec<u8> = (0..70000).map(|i| (i % 256) as u8).collect();
        let mut output = Vec::new();

        write_uncompressed_data(&mut output, &data, true).unwrap();

        // Should have two chunks
        // First chunk: ctrl + 2 bytes size + 65535 bytes data
        // Second chunk: ctrl + 2 bytes size + 4465 bytes data
        assert!(output.len() > data.len()); // Overhead from headers
    }

    #[test]
    fn test_chunk_reset_mode_includes_props() {
        assert!(!ChunkResetMode::None.includes_props());
        assert!(!ChunkResetMode::StateReset.includes_props());
        assert!(ChunkResetMode::StatePropsReset.includes_props());
        assert!(ChunkResetMode::AllReset.includes_props());
    }

    #[test]
    fn test_write_compressed_chunk_format() {
        let mut output = Vec::new();
        let compressed = vec![0x00, 0x01, 0x02, 0x03, 0x04]; // 5 bytes fake compressed data
        let uncompressed_size = 100;

        write_compressed_chunk(
            &mut output,
            &compressed,
            uncompressed_size,
            ChunkResetMode::AllReset,
            Some(0x5D),
        )
        .unwrap();

        // Check control byte: 0xE0 (AllReset) | 0 (high bits of 99)
        assert_eq!(output[0], 0xE0);

        // Check unpacked size - 1 (big-endian): 100 - 1 = 99 (comes first!)
        let unpack_size = u16::from_be_bytes([output[1], output[2]]);
        assert_eq!(unpack_size, 99);

        // Check packed size - 1 (big-endian): 5 - 1 = 4 (comes second!)
        let pack_size = u16::from_be_bytes([output[3], output[4]]);
        assert_eq!(pack_size, 4);

        // Check properties byte
        assert_eq!(output[5], 0x5D);

        // Check compressed data
        assert_eq!(&output[6..], &compressed[..]);
    }

    #[test]
    fn test_write_compressed_chunk_no_props() {
        let mut output = Vec::new();
        let compressed = vec![0xAA, 0xBB, 0xCC];
        let uncompressed_size = 50;

        write_compressed_chunk(
            &mut output,
            &compressed,
            uncompressed_size,
            ChunkResetMode::StateReset, // No props
            None,
        )
        .unwrap();

        // Check control byte: 0x80 (StateReset) | 0
        assert_eq!(output[0], 0x80);

        // No properties byte, so data starts at offset 5
        // Header: 1 (ctrl) + 2 (pack_size) + 2 (unpack_size) = 5 bytes
        assert_eq!(output.len(), 5 + 3);
        assert_eq!(&output[5..], &compressed[..]);
    }

    #[test]
    fn test_write_compressed_chunk_empty() {
        let mut output = Vec::new();

        // Empty compressed data should do nothing
        write_compressed_chunk(&mut output, &[], 100, ChunkResetMode::AllReset, None).unwrap();
        assert!(output.is_empty());

        // Zero uncompressed size should do nothing
        write_compressed_chunk(
            &mut output,
            &[0x01, 0x02],
            0,
            ChunkResetMode::AllReset,
            None,
        )
        .unwrap();
        assert!(output.is_empty());
    }
}
