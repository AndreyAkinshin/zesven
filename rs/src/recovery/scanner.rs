//! Signature scanning for archive recovery.
//!
//! This module provides utilities for scanning binary data to find
//! 7z archive signatures, which is essential for recovering data from
//! corrupted archives or extracting embedded archives.

use crate::format::SIGNATURE;
use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

/// Scanner for finding 7z signatures in binary data.
///
/// The scanner buffers data and searches for the 7z signature pattern,
/// validating version bytes to reduce false positives.
pub struct SignatureScanner<'a, R: Read + Seek> {
    reader: &'a mut R,
    search_limit: usize,
    buffer: Vec<u8>,
    current_offset: u64,
    bytes_read: usize,
}

impl<'a, R: Read + Seek> SignatureScanner<'a, R> {
    /// Creates a new signature scanner.
    ///
    /// # Arguments
    ///
    /// * `reader` - The reader to scan
    /// * `search_limit` - Maximum bytes to read from the reader
    pub fn new(reader: &'a mut R, search_limit: usize) -> Self {
        Self {
            reader,
            search_limit,
            buffer: Vec::new(),
            current_offset: 0,
            bytes_read: 0,
        }
    }

    /// Finds the next 7z signature starting from the current position.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(offset))` if a signature is found, where offset
    /// is the absolute position in the file. Returns `Ok(None)` if no
    /// signature is found within the search limit.
    pub fn find_next_signature(&mut self) -> Result<Option<u64>> {
        self.ensure_buffer_loaded()?;

        // Search for signature in buffer
        let mut search_start = 0;
        while search_start + 8 <= self.buffer.len() {
            if let Some(rel_pos) = self.buffer[search_start..]
                .windows(6)
                .position(|w| w == SIGNATURE)
            {
                let pos = search_start + rel_pos;
                // Validate version bytes
                if pos + 8 <= self.buffer.len() {
                    let version_major = self.buffer[pos + 6];
                    let version_minor = self.buffer[pos + 7];
                    // Valid 7z versions: major = 0, minor <= 10
                    if version_major == 0 && version_minor <= 10 {
                        return Ok(Some(self.current_offset + pos as u64));
                    }
                }
                // False positive, continue searching
                search_start = pos + 1;
            } else {
                break;
            }
        }

        Ok(None)
    }

    /// Finds all 7z signatures in the search range.
    ///
    /// # Returns
    ///
    /// Returns a vector of all signature offsets found.
    pub fn find_all_signatures(&mut self) -> Result<Vec<u64>> {
        self.ensure_buffer_loaded()?;

        let mut signatures = Vec::new();
        let mut search_start = 0;

        while search_start + 8 <= self.buffer.len() {
            if let Some(rel_pos) = self.buffer[search_start..]
                .windows(6)
                .position(|w| w == SIGNATURE)
            {
                let pos = search_start + rel_pos;
                // Validate version bytes
                if pos + 8 <= self.buffer.len() {
                    let version_major = self.buffer[pos + 6];
                    let version_minor = self.buffer[pos + 7];
                    // Valid 7z versions: major = 0, minor <= 10
                    if version_major == 0 && version_minor <= 10 {
                        signatures.push(self.current_offset + pos as u64);
                    }
                }
                search_start = pos + 1;
            } else {
                break;
            }
        }

        Ok(signatures)
    }

    /// Scans backwards from the end of the file.
    ///
    /// This is useful for finding backup headers that might be stored
    /// at the end of an archive.
    ///
    /// # Returns
    ///
    /// Returns a vector of signature offsets found, ordered from end to start.
    pub fn scan_backwards(&mut self) -> Result<Vec<u64>> {
        // Get file size
        let end_pos = self.reader.seek(SeekFrom::End(0)).map_err(Error::Io)?;

        // Calculate how much to read from the end
        let read_size = (end_pos as usize).min(self.search_limit);
        let start_pos = end_pos - read_size as u64;

        // Seek to start position and read
        self.reader
            .seek(SeekFrom::Start(start_pos))
            .map_err(Error::Io)?;
        self.current_offset = start_pos;
        self.buffer.clear();
        self.buffer.resize(read_size, 0);
        self.reader
            .read_exact(&mut self.buffer)
            .map_err(Error::Io)?;
        self.bytes_read = read_size;

        // Find all signatures and reverse the order
        let mut signatures = self.find_all_signatures()?;
        signatures.reverse();

        Ok(signatures)
    }

    /// Ensures the buffer is loaded with data.
    fn ensure_buffer_loaded(&mut self) -> Result<()> {
        if !self.buffer.is_empty() {
            return Ok(());
        }

        // Get current position
        self.current_offset = self.reader.stream_position().map_err(Error::Io)?;

        // Read up to search_limit bytes
        self.buffer.resize(self.search_limit, 0);
        let bytes_read = self.reader.read(&mut self.buffer).map_err(Error::Io)?;
        self.buffer.truncate(bytes_read);
        self.bytes_read = bytes_read;

        Ok(())
    }

    /// Returns the number of bytes that were read.
    pub fn bytes_scanned(&self) -> usize {
        self.bytes_read
    }

    /// Resets the scanner to scan again.
    pub fn reset(&mut self) -> Result<()> {
        self.buffer.clear();
        self.bytes_read = 0;
        self.reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
        self.current_offset = 0;
        Ok(())
    }
}

/// Checks if data at the given position looks like a valid 7z start header.
///
/// This performs additional validation beyond just the signature.
#[allow(dead_code)] // Part of recovery API for archive scanning
pub fn validate_start_header(data: &[u8]) -> bool {
    // Need at least 32 bytes for a start header
    if data.len() < 32 {
        return false;
    }

    // Check signature
    if &data[0..6] != SIGNATURE {
        return false;
    }

    // Check version
    let version_major = data[6];
    let version_minor = data[7];
    if version_major != 0 || version_minor > 10 {
        return false;
    }

    // Check CRC of header data (bytes 12-31)
    let stored_crc = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let calculated_crc = crc32fast::hash(&data[12..32]);

    stored_crc == calculated_crc
}

/// Attempts to find a valid header by scanning with CRC validation.
///
/// This is more thorough than just finding signatures, as it validates
/// the start header CRC to confirm a valid archive location.
#[allow(dead_code)] // Part of recovery API for archive scanning
pub fn find_valid_header<R: Read + Seek>(
    reader: &mut R,
    search_limit: usize,
) -> Result<Option<u64>> {
    let start_pos = reader.stream_position().map_err(Error::Io)?;

    // Read data for scanning
    let mut buffer = vec![0u8; search_limit];
    let bytes_read = reader.read(&mut buffer).map_err(Error::Io)?;
    buffer.truncate(bytes_read);

    // Search for valid headers
    let mut search_start = 0;
    while search_start + 32 <= buffer.len() {
        if let Some(rel_pos) = buffer[search_start..]
            .windows(6)
            .position(|w| w == SIGNATURE)
        {
            let pos = search_start + rel_pos;
            if pos + 32 <= buffer.len() && validate_start_header(&buffer[pos..pos + 32]) {
                return Ok(Some(start_pos + pos as u64));
            }
            search_start = pos + 1;
        } else {
            break;
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn create_valid_header() -> Vec<u8> {
        let mut data = Vec::new();
        // Signature
        data.extend_from_slice(SIGNATURE);
        // Version (0.4)
        data.push(0x00);
        data.push(0x04);
        // Header data (bytes 12-31) - all zeros
        let header_data = [0u8; 20];
        // Calculate CRC of header data
        let crc = crc32fast::hash(&header_data);
        data.extend_from_slice(&crc.to_le_bytes());
        // Header data
        data.extend_from_slice(&header_data);
        data
    }

    #[test]
    fn test_scanner_find_at_start() {
        let data = create_valid_header();
        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1024);

        let offset = scanner.find_next_signature().unwrap();
        assert_eq!(offset, Some(0));
    }

    #[test]
    fn test_scanner_find_with_prefix() {
        let mut data = vec![0xFFu8; 100]; // Junk prefix
        data.extend_from_slice(&create_valid_header());

        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1024);

        let offset = scanner.find_next_signature().unwrap();
        assert_eq!(offset, Some(100));
    }

    #[test]
    fn test_scanner_no_signature() {
        let data = vec![0x00u8; 1024];
        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1024);

        let offset = scanner.find_next_signature().unwrap();
        assert_eq!(offset, None);
    }

    #[test]
    fn test_scanner_find_all() {
        let header = create_valid_header();
        let mut data = Vec::new();
        data.extend_from_slice(&header);
        data.extend_from_slice(&[0x00u8; 50]); // Gap
        data.extend_from_slice(&header);

        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1024);

        let offsets = scanner.find_all_signatures().unwrap();
        assert_eq!(offsets.len(), 2);
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[1], header.len() as u64 + 50);
    }

    #[test]
    fn test_scanner_bytes_scanned() {
        let data = vec![0x00u8; 500];
        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1000);

        scanner.find_next_signature().unwrap();
        assert_eq!(scanner.bytes_scanned(), 500);
    }

    #[test]
    fn test_validate_start_header_valid() {
        let header = create_valid_header();
        assert!(validate_start_header(&header));
    }

    #[test]
    fn test_validate_start_header_bad_signature() {
        let mut header = create_valid_header();
        header[0] = 0x00; // Corrupt signature
        assert!(!validate_start_header(&header));
    }

    #[test]
    fn test_validate_start_header_bad_version() {
        let mut header = create_valid_header();
        header[6] = 1; // Invalid major version
        assert!(!validate_start_header(&header));
    }

    #[test]
    fn test_validate_start_header_too_short() {
        let header = vec![0u8; 16];
        assert!(!validate_start_header(&header));
    }

    #[test]
    fn test_find_valid_header() {
        let header = create_valid_header();
        let mut data = vec![0xFFu8; 64]; // Junk prefix
        data.extend_from_slice(&header);

        let mut cursor = Cursor::new(data);
        let offset = find_valid_header(&mut cursor, 1024).unwrap();
        assert_eq!(offset, Some(64));
    }

    #[test]
    fn test_scanner_reset() {
        let data = create_valid_header();
        let mut cursor = Cursor::new(data);
        let mut scanner = SignatureScanner::new(&mut cursor, 1024);

        // First scan
        let _ = scanner.find_next_signature().unwrap();
        assert!(scanner.bytes_scanned() > 0);

        // Reset and scan again
        scanner.reset().unwrap();
        let offset = scanner.find_next_signature().unwrap();
        assert_eq!(offset, Some(0));
    }
}
