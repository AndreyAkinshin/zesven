//! Low-level binary reading utilities for 7z format parsing.

use std::io::{self, Read};

/// Reads a variable-length encoded u64 from a reader.
///
/// 7z uses a variable-length integer encoding where the first byte's high bits
/// indicate the number of additional bytes to read:
///
/// - `0xxxxxxx` (1 byte): value 0-127
/// - `10xxxxxx` + 1 byte: value 0-16383
/// - `110xxxxx` + 2 bytes: value 0-2097151
/// - And so on...
/// - `11111111` + 8 bytes: full u64
///
/// # Errors
///
/// Returns an error if the reader encounters EOF or an I/O error.
pub fn read_variable_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)?;
    let first = first[0] as u64;

    let mut mask = 0x80u64;
    let mut value = 0u64;

    for i in 0..8 {
        if (first & mask) == 0 {
            // The remaining bits of first form part of the value
            return Ok(value | ((first & (mask - 1)) << (8 * i)));
        }
        // Read another byte
        let mut byte = [0u8; 1];
        r.read_exact(&mut byte)?;
        value |= (byte[0] as u64) << (8 * i);
        mask >>= 1;
    }

    // All 8 high bits were set, value is in the following 8 bytes
    Ok(value)
}

/// Writes a variable-length encoded u64 to a writer.
///
/// This is the inverse of `read_variable_u64`.
pub fn write_variable_u64<W: io::Write>(w: &mut W, value: u64) -> io::Result<()> {
    // Determine the minimum number of bytes needed
    if value < 0x80 {
        // 1 byte: 0xxxxxxx
        w.write_all(&[value as u8])
    } else if value < 0x4000 {
        // 2 bytes: 10xxxxxx xxxxxxxx
        let b0 = 0x80 | ((value >> 8) as u8 & 0x3F);
        let b1 = value as u8;
        w.write_all(&[b0, b1])
    } else if value < 0x20_0000 {
        // 3 bytes: 110xxxxx xxxxxxxx xxxxxxxx
        let b0 = 0xC0 | ((value >> 16) as u8 & 0x1F);
        let b1 = value as u8;
        let b2 = (value >> 8) as u8;
        w.write_all(&[b0, b1, b2])
    } else if value < 0x1000_0000 {
        // 4 bytes: 1110xxxx xxxxxxxx xxxxxxxx xxxxxxxx
        let b0 = 0xE0 | ((value >> 24) as u8 & 0x0F);
        let b1 = value as u8;
        let b2 = (value >> 8) as u8;
        let b3 = (value >> 16) as u8;
        w.write_all(&[b0, b1, b2, b3])
    } else if value < 0x08_0000_0000 {
        // 5 bytes
        let b0 = 0xF0 | ((value >> 32) as u8 & 0x07);
        w.write_all(&[
            b0,
            value as u8,
            (value >> 8) as u8,
            (value >> 16) as u8,
            (value >> 24) as u8,
        ])
    } else if value < 0x0400_0000_0000 {
        // 6 bytes
        let b0 = 0xF8 | ((value >> 40) as u8 & 0x03);
        w.write_all(&[
            b0,
            value as u8,
            (value >> 8) as u8,
            (value >> 16) as u8,
            (value >> 24) as u8,
            (value >> 32) as u8,
        ])
    } else if value < 0x0002_0000_0000_0000 {
        // 7 bytes
        let b0 = 0xFC | ((value >> 48) as u8 & 0x01);
        w.write_all(&[
            b0,
            value as u8,
            (value >> 8) as u8,
            (value >> 16) as u8,
            (value >> 24) as u8,
            (value >> 32) as u8,
            (value >> 40) as u8,
        ])
    } else if value < 0x0100_0000_0000_0000 {
        // 8 bytes
        w.write_all(&[
            0xFE,
            value as u8,
            (value >> 8) as u8,
            (value >> 16) as u8,
            (value >> 24) as u8,
            (value >> 32) as u8,
            (value >> 40) as u8,
            (value >> 48) as u8,
        ])
    } else {
        // 9 bytes (full u64)
        w.write_all(&[
            0xFF,
            value as u8,
            (value >> 8) as u8,
            (value >> 16) as u8,
            (value >> 24) as u8,
            (value >> 32) as u8,
            (value >> 40) as u8,
            (value >> 48) as u8,
            (value >> 56) as u8,
        ])
    }
}

/// Reads an unsigned 32-bit little-endian integer.
pub fn read_u32_le<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

/// Reads an unsigned 64-bit little-endian integer.
pub fn read_u64_le<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

/// Reads a single byte.
pub fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    Ok(buf[0])
}

/// Reads a boolean vector (bit array) of the specified length.
///
/// Each bit in the input bytes represents one boolean value.
/// Bits are read from MSB to LSB within each byte.
///
/// # Arguments
///
/// * `r` - The reader
/// * `count` - Number of boolean values to read
pub fn read_bool_vector<R: Read>(r: &mut R, count: usize) -> io::Result<Vec<bool>> {
    let byte_count = count.div_ceil(8);
    let mut bytes = vec![0u8; byte_count];
    r.read_exact(&mut bytes)?;

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        result.push((bytes[byte_idx] >> bit_idx) & 1 != 0);
    }

    Ok(result)
}

/// Reads either an all-true vector or a bit vector based on a marker byte.
///
/// If the first byte is non-zero, returns a vector of all `true` values.
/// Otherwise, reads a bit vector from the remaining bytes.
///
/// This is used for optional property presence markers in 7z headers.
pub fn read_all_or_bits<R: Read>(r: &mut R, count: usize) -> io::Result<Vec<bool>> {
    let all_defined = read_u8(r)?;
    if all_defined != 0 {
        Ok(vec![true; count])
    } else {
        read_bool_vector(r, count)
    }
}

/// Reads exact number of bytes into a new vector.
pub fn read_bytes<R: Read>(r: &mut R, count: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; count];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_variable_u64_zero() {
        let data = [0x00u8];
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_variable_u64(&mut cursor).unwrap(), 0);
    }

    #[test]
    fn test_variable_u64_one_byte_max() {
        let data = [0x7Fu8]; // 127
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_variable_u64(&mut cursor).unwrap(), 127);
    }

    #[test]
    fn test_variable_u64_two_bytes_min() {
        let data = [0x80u8, 0x80]; // 10000000 10000000 -> 128
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_variable_u64(&mut cursor).unwrap(), 128);
    }

    #[test]
    fn test_variable_u64_two_bytes() {
        // 10xxxxxx + 1 byte
        // 0xBF 0xFF = 10_111111 11111111 -> (0x3F << 8) | 0xFF = 16383
        let data = [0xBFu8, 0xFF];
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_variable_u64(&mut cursor).unwrap(), 16383);
    }

    #[test]
    fn test_variable_u64_roundtrip() {
        let test_values = [
            0u64,
            1,
            127,
            128,
            255,
            256,
            16383,
            16384,
            2097151,
            2097152,
            u32::MAX as u64,
            u64::MAX,
        ];

        for &value in &test_values {
            let mut buf = Vec::new();
            write_variable_u64(&mut buf, value).unwrap();

            let mut cursor = Cursor::new(&buf);
            let result = read_variable_u64(&mut cursor).unwrap();
            assert_eq!(
                result, value,
                "Round-trip failed for {}: encoded as {:?}, decoded as {}",
                value, buf, result
            );
        }
    }

    #[test]
    fn test_variable_u64_eof() {
        let data = [0x80u8]; // Indicates 2 bytes but only 1 provided
        let mut cursor = Cursor::new(&data);
        assert!(read_variable_u64(&mut cursor).is_err());
    }

    #[test]
    fn test_read_u32_le() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_u32_le(&mut cursor).unwrap(), 0x04030201);
    }

    #[test]
    fn test_read_u64_le() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let mut cursor = Cursor::new(&data);
        assert_eq!(read_u64_le(&mut cursor).unwrap(), 0x0807060504030201);
    }

    #[test]
    fn test_bool_vector() {
        let data = [0b10110001u8, 0b11000000];
        let mut cursor = Cursor::new(&data);
        let result = read_bool_vector(&mut cursor, 10).unwrap();
        assert_eq!(
            result,
            vec![
                true, false, true, true, false, false, false, true, true, true
            ]
        );
    }

    #[test]
    fn test_bool_vector_single_bit() {
        let data = [0b10000000u8];
        let mut cursor = Cursor::new(&data);
        let result = read_bool_vector(&mut cursor, 1).unwrap();
        assert_eq!(result, vec![true]);
    }

    #[test]
    fn test_all_or_bits_all_true() {
        let data = [0x01u8]; // Non-zero means all true
        let mut cursor = Cursor::new(&data);
        let result = read_all_or_bits(&mut cursor, 5).unwrap();
        assert_eq!(result, vec![true, true, true, true, true]);
    }

    #[test]
    fn test_all_or_bits_bit_vector() {
        let data = [0x00u8, 0b10100000]; // Zero means read bits
        let mut cursor = Cursor::new(&data);
        let result = read_all_or_bits(&mut cursor, 3).unwrap();
        assert_eq!(result, vec![true, false, true]);
    }

    #[test]
    fn test_read_bytes() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let mut cursor = Cursor::new(&data);
        let result = read_bytes(&mut cursor, 3).unwrap();
        assert_eq!(result, vec![0x01, 0x02, 0x03]);
    }
}
