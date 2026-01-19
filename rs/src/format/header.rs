//! 7z archive header structures and parsing.

use crate::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

use super::reader::{read_u8, read_u32_le};
use super::{SIGNATURE, VERSION_MAJOR, VERSION_MINOR};

/// Maximum search range for 7z signature in self-extracting archives.
pub const SFX_SEARCH_LIMIT: usize = 1024 * 1024; // 1 MiB

/// The start header of a 7z archive.
///
/// This is the first structure in a 7z file, located immediately after
/// the 6-byte signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartHeader {
    /// Archive format version - major number.
    pub version_major: u8,
    /// Archive format version - minor number.
    pub version_minor: u8,
    /// CRC of the following 20 bytes (offset, size, crc).
    pub start_header_crc: u32,
    /// Offset from the end of the start header to the next header.
    pub next_header_offset: u64,
    /// Size of the next header (compressed if encoded).
    pub next_header_size: u64,
    /// CRC of the next header data.
    pub next_header_crc: u32,
    /// Offset to the 7z signature (non-zero for SFX archives).
    pub sfx_offset: u64,
}

impl StartHeader {
    /// Parses the signature and start header from a reader.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The signature is invalid
    /// - The version is unsupported
    /// - The CRC doesn't match
    /// - An I/O error occurs
    pub fn parse<R: Read>(r: &mut R) -> Result<Self> {
        // Read and validate signature
        let mut sig = [0u8; 6];
        r.read_exact(&mut sig).map_err(Error::from)?;
        if sig != *SIGNATURE {
            return Err(Error::InvalidFormat("invalid 7z signature".into()));
        }

        // Read version
        let version_major = read_u8(r)?;
        let version_minor = read_u8(r)?;

        // Check version compatibility (we support 0.x where x <= 4)
        // Archives with higher versions may have features we don't understand
        if version_major > VERSION_MAJOR
            || (version_major == VERSION_MAJOR && version_minor > VERSION_MINOR)
        {
            return Err(Error::UnsupportedFeature {
                feature: "unsupported archive version",
            });
        }

        // Read start header CRC
        let start_header_crc = read_u32_le(r)?;

        // Read the next header info (20 bytes that should be CRC'd)
        let mut header_data = [0u8; 20];
        r.read_exact(&mut header_data).map_err(Error::from)?;

        // Verify CRC
        let calculated_crc = crc32fast::hash(&header_data);
        if calculated_crc != start_header_crc {
            return Err(Error::CorruptHeader {
                offset: 12,
                reason: format!(
                    "start header CRC mismatch: expected {:#x}, got {:#x}",
                    start_header_crc, calculated_crc
                ),
            });
        }

        // Parse the header data
        let next_header_offset = u64::from_le_bytes(header_data[0..8].try_into().unwrap());
        let next_header_size = u64::from_le_bytes(header_data[8..16].try_into().unwrap());
        let next_header_crc = u32::from_le_bytes(header_data[16..20].try_into().unwrap());

        Ok(Self {
            version_major,
            version_minor,
            start_header_crc,
            next_header_offset,
            next_header_size,
            next_header_crc,
            sfx_offset: 0, // Set by caller for SFX archives
        })
    }

    /// Returns the byte position where the next header starts.
    ///
    /// This is the offset from the beginning of the file, including any
    /// SFX stub offset for self-extracting archives.
    pub fn next_header_position(&self) -> u64 {
        self.sfx_offset + super::SIGNATURE_HEADER_SIZE + self.next_header_offset
    }
}

/// Basic offset information detected when scanning for a 7z signature in an SFX archive.
///
/// This is a minimal struct used internally by [`detect_sfx`]. For the full SFX info
/// including format detection, see [`crate::sfx::SfxInfo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSfxOffset {
    /// Offset from the beginning of the file to the 7z signature.
    pub archive_offset: u64,
    /// Size of the SFX stub (same as archive_offset).
    pub stub_size: u64,
}

/// Finds the 7z signature in a reader.
///
/// This is useful for opening self-extracting (SFX) archives where the
/// 7z archive data is embedded after an executable stub.
///
/// # Arguments
///
/// * `reader` - The reader to search in
/// * `search_limit` - Maximum bytes to search (default: 1 MiB)
///
/// # Returns
///
/// Returns `Ok(Some(offset))` if the signature is found, where offset is
/// the byte position of the signature. Returns `Ok(None)` if the signature
/// is not found within the search limit.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::format::header::find_signature;
///
/// let mut file = File::open("archive.exe")?;
/// if let Some(offset) = find_signature(&mut file, None)? {
///     println!("Found 7z archive at offset {}", offset);
///     file.seek(SeekFrom::Start(offset))?;
///     // Now parse the archive normally
/// }
/// ```
pub fn find_signature<R: Read + Seek>(
    reader: &mut R,
    search_limit: Option<usize>,
) -> Result<Option<u64>> {
    let limit = search_limit.unwrap_or(SFX_SEARCH_LIMIT);

    // Remember the starting position
    let start_pos = reader.stream_position().map_err(Error::Io)?;

    // Read up to limit bytes
    let mut buffer = vec![0u8; limit];
    let bytes_read = reader.read(&mut buffer).map_err(Error::Io)?;
    buffer.truncate(bytes_read);

    // Search for signature with valid version bytes
    // The signature is 6 bytes, followed by 2 version bytes (major, minor)
    // We need at least 8 bytes to validate
    let mut search_start = 0;
    while search_start + 8 <= buffer.len() {
        if let Some(rel_pos) = buffer[search_start..]
            .windows(6)
            .position(|w| w == SIGNATURE)
        {
            let pos = search_start + rel_pos;
            // Check if we have room for version bytes
            if pos + 8 <= buffer.len() {
                let version_major = buffer[pos + 6];
                let version_minor = buffer[pos + 7];
                // Valid 7z versions are 0.x where x is typically 2, 3, or 4
                // Major version should be 0, minor should be reasonable (< 10)
                if version_major == VERSION_MAJOR && version_minor <= 10 {
                    let absolute_offset = start_pos + pos as u64;
                    return Ok(Some(absolute_offset));
                }
            }
            // Move past this false positive and continue searching
            search_start = pos + 1;
        } else {
            break;
        }
    }

    Ok(None)
}

/// Detects if a file is a self-extracting archive and returns information about it.
///
/// # Arguments
///
/// * `reader` - The reader to check
///
/// # Returns
///
/// Returns `Ok(Some(DetectedSfxOffset))` if this is an SFX archive (signature not at offset 0),
/// returns `Ok(None)` if this is a regular 7z archive (signature at offset 0).
///
/// # Errors
///
/// Returns an error if no 7z signature is found within the search limit.
pub fn detect_sfx<R: Read + Seek>(reader: &mut R) -> Result<Option<DetectedSfxOffset>> {
    // Save position
    let start_pos = reader.stream_position().map_err(Error::Io)?;

    // Seek to beginning
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;

    // Find signature
    let offset = find_signature(reader, None)?;

    // Restore position
    reader.seek(SeekFrom::Start(start_pos)).map_err(Error::Io)?;

    match offset {
        Some(0) => Ok(None), // Regular archive
        Some(offset) => Ok(Some(DetectedSfxOffset {
            archive_offset: offset,
            stub_size: offset,
        })),
        None => Err(Error::InvalidFormat(
            "no 7z signature found (not a 7z or SFX archive)".into(),
        )),
    }
}

/// Parses the start header from an SFX archive, seeking to the correct position first.
///
/// # Arguments
///
/// * `reader` - The reader positioned anywhere
/// * `sfx_offset` - The offset where the 7z archive begins (0 for regular archives)
///
/// # Returns
///
/// Returns the parsed `StartHeader`.
pub fn parse_sfx_header<R: Read + Seek>(reader: &mut R, sfx_offset: u64) -> Result<StartHeader> {
    reader
        .seek(SeekFrom::Start(sfx_offset))
        .map_err(Error::Io)?;
    StartHeader::parse(reader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Creates a valid start header with the given next header info.
    fn create_valid_header(offset: u64, size: u64, next_crc: u32) -> Vec<u8> {
        let mut data = Vec::new();

        // Signature
        data.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);

        // Version
        data.push(0x00); // major
        data.push(0x04); // minor

        // Create the 20-byte header data
        let mut header_data = Vec::new();
        header_data.extend_from_slice(&offset.to_le_bytes());
        header_data.extend_from_slice(&size.to_le_bytes());
        header_data.extend_from_slice(&next_crc.to_le_bytes());

        // Calculate and add CRC
        let crc = crc32fast::hash(&header_data);
        data.extend_from_slice(&crc.to_le_bytes());

        // Add header data
        data.extend_from_slice(&header_data);

        data
    }

    #[test]
    fn test_valid_start_header() {
        let data = create_valid_header(100, 50, 0xDEADBEEF);
        let mut cursor = Cursor::new(&data);

        let header = StartHeader::parse(&mut cursor).unwrap();
        assert_eq!(header.version_major, 0);
        assert_eq!(header.version_minor, 4);
        assert_eq!(header.next_header_offset, 100);
        assert_eq!(header.next_header_size, 50);
        assert_eq!(header.next_header_crc, 0xDEADBEEF);
    }

    #[test]
    fn test_invalid_signature() {
        let mut data = create_valid_header(100, 50, 0);
        data[0] = 0x00; // Corrupt signature

        let mut cursor = Cursor::new(&data);
        let err = StartHeader::parse(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_crc_mismatch() {
        let mut data = create_valid_header(100, 50, 0);
        // Corrupt the offset (byte 12 onwards, after CRC)
        data[12] = 0xFF;

        let mut cursor = Cursor::new(&data);
        let err = StartHeader::parse(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::CorruptHeader { .. }));
    }

    #[test]
    fn test_truncated_header() {
        let data = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00]; // Just signature + partial version

        let mut cursor = Cursor::new(&data);
        let err = StartHeader::parse(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn test_next_header_position() {
        let data = create_valid_header(100, 50, 0);
        let mut cursor = Cursor::new(&data);

        let header = StartHeader::parse(&mut cursor).unwrap();
        assert_eq!(header.next_header_position(), 32 + 100);
    }

    #[test]
    fn test_empty_archive() {
        // A valid empty archive has next_header_size = 0
        let data = create_valid_header(0, 0, 0);
        let mut cursor = Cursor::new(&data);

        let header = StartHeader::parse(&mut cursor).unwrap();
        assert_eq!(header.next_header_size, 0);
    }

    #[test]
    fn test_find_signature_at_start() {
        let data = create_valid_header(0, 0, 0);
        let mut cursor = Cursor::new(&data);

        let offset = find_signature(&mut cursor, None).unwrap();
        assert_eq!(offset, Some(0));
    }

    #[test]
    fn test_find_signature_with_offset() {
        // Create data with garbage before the signature
        let mut data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00]; // 8 bytes of garbage
        data.extend_from_slice(&create_valid_header(0, 0, 0));

        let mut cursor = Cursor::new(&data);
        let offset = find_signature(&mut cursor, None).unwrap();
        assert_eq!(offset, Some(8)); // Signature at offset 8
    }

    #[test]
    fn test_find_signature_not_found() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF]; // No signature
        let mut cursor = Cursor::new(&data);

        let offset = find_signature(&mut cursor, None).unwrap();
        assert_eq!(offset, None);
    }

    #[test]
    fn test_detect_sfx_regular_archive() {
        let data = create_valid_header(0, 0, 0);
        let mut cursor = Cursor::new(&data);

        let sfx_info = detect_sfx(&mut cursor).unwrap();
        assert!(sfx_info.is_none()); // Regular archive, not SFX
    }

    #[test]
    fn test_detect_sfx_archive() {
        // Create SFX-like data with stub before the archive
        let mut data = vec![0u8; 100]; // 100 bytes of "executable stub"
        data.extend_from_slice(&create_valid_header(0, 0, 0));

        let mut cursor = Cursor::new(&data);
        let sfx_info = detect_sfx(&mut cursor).unwrap();

        assert!(sfx_info.is_some());
        let info = sfx_info.unwrap();
        assert_eq!(info.archive_offset, 100);
        assert_eq!(info.stub_size, 100);
    }

    #[test]
    fn test_detect_sfx_no_signature() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF]; // No signature
        let mut cursor = Cursor::new(&data);

        let err = detect_sfx(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_parse_sfx_header() {
        // Create SFX-like data with stub before the archive
        let mut data = vec![0u8; 100]; // 100 bytes of "executable stub"
        data.extend_from_slice(&create_valid_header(50, 30, 0xCAFE));

        let mut cursor = Cursor::new(&data);
        let header = parse_sfx_header(&mut cursor, 100).unwrap();

        assert_eq!(header.next_header_offset, 50);
        assert_eq!(header.next_header_size, 30);
    }
}
