//! Encoding utility functions for archive writing.
//!
//! This module provides helper functions for encoding method IDs and boolean vectors
//! used in the 7z archive format.

/// Encodes a method ID to bytes.
///
/// Method IDs are variable-length integers stored in big-endian order.
/// A zero method ID results in a single zero byte.
pub(crate) fn encode_method_id(id: u64) -> Vec<u8> {
    if id == 0 {
        return vec![0];
    }

    let mut bytes = Vec::new();
    let mut val = id;
    while val > 0 {
        bytes.push((val & 0xFF) as u8);
        val >>= 8;
    }
    bytes.reverse();
    bytes
}

/// Encodes a boolean vector to bytes.
///
/// Each boolean is packed into bits, with the first boolean in the MSB
/// of the first byte. The vector is zero-padded to fill the final byte.
pub(crate) fn encode_bool_vector(bits: &[bool]) -> Vec<u8> {
    let num_bytes = bits.len().div_ceil(8);
    let mut bytes = vec![0u8; num_bytes];

    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            bytes[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_method_id() {
        assert_eq!(encode_method_id(0), vec![0]);
        assert_eq!(encode_method_id(0x21), vec![0x21]); // LZMA2
        assert_eq!(encode_method_id(0x030101), vec![0x03, 0x01, 0x01]); // LZMA
    }

    #[test]
    fn test_encode_bool_vector() {
        assert_eq!(encode_bool_vector(&[true, false, true]), vec![0b10100000]);
        assert_eq!(encode_bool_vector(&[true; 8]), vec![0b11111111]);
        assert_eq!(
            encode_bool_vector(&[true, false, true, false, true, false, true, false, true]),
            vec![0b10101010, 0b10000000]
        );
    }
}
