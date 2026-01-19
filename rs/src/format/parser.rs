//! Main header parser for 7z archives.
//!
//! This module provides the unified header parsing infrastructure that handles
//! both plain and encoded (compressed/encrypted) headers.

use crate::{Error, Result, codec};
use std::io::{Cursor, Read, Seek, SeekFrom};

use super::SIGNATURE_HEADER_SIZE;
use super::files::{ArchiveEntry, FilesInfo};
use super::header::StartHeader;
use super::property_id;
use super::reader::read_u8;
use super::streams::{Folder, PackInfo, ResourceLimits, SubStreamsInfo, UnpackInfo};

/// Parsed archive header data.
#[derive(Debug, Clone, Default)]
pub struct ArchiveHeader {
    /// Pack info (compressed stream info).
    pub pack_info: Option<PackInfo>,
    /// Unpack info (folder/coder definitions).
    pub unpack_info: Option<UnpackInfo>,
    /// Substreams info (per-file info within folders).
    pub substreams_info: Option<SubStreamsInfo>,
    /// Files info (file metadata).
    pub files_info: Option<FilesInfo>,
    /// Whether the header itself was encrypted (required password to read metadata).
    pub header_encrypted: bool,
}

impl ArchiveHeader {
    /// Returns all file entries.
    pub fn entries(&self) -> &[ArchiveEntry] {
        self.files_info.as_ref().map_or(&[], |f| &f.entries)
    }

    /// Returns all folders.
    pub fn folders(&self) -> &[Folder] {
        self.unpack_info.as_ref().map_or(&[], |u| &u.folders)
    }
}

/// Header parser with resource limit enforcement.
#[derive(Debug)]
pub struct HeaderParser {
    /// Resource limits for parsing.
    limits: ResourceLimits,
    /// Bytes read so far.
    bytes_read: u64,
    /// Current recursion depth for encoded headers.
    recursion_depth: u32,
    /// Maximum recursion depth.
    max_recursion_depth: u32,
    /// Optional password for encrypted headers.
    #[cfg(feature = "aes")]
    password: Option<crate::crypto::Password>,
}

impl HeaderParser {
    /// Creates a new header parser with default limits.
    pub fn new() -> Self {
        Self::with_limits(ResourceLimits::default())
    }

    /// Creates a new header parser with custom limits.
    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self {
            limits,
            bytes_read: 0,
            recursion_depth: 0,
            max_recursion_depth: 4,
            #[cfg(feature = "aes")]
            password: None,
        }
    }

    /// Sets the password for decrypting encrypted headers.
    #[cfg(feature = "aes")]
    pub fn with_password(mut self, password: Option<crate::crypto::Password>) -> Self {
        self.password = password;
        self
    }

    /// Parses the archive header from a reader (Read-only variant).
    ///
    /// This variant only supports plain headers. For encoded headers,
    /// use `parse_header_with_seek` instead.
    pub fn parse_header<R: Read>(&mut self, r: &mut R) -> Result<ArchiveHeader> {
        let first_byte = read_u8(r)?;
        self.bytes_read += 1;

        match first_byte {
            property_id::HEADER => self.parse_main_header(r),
            property_id::ENCODED_HEADER => Err(Error::UnsupportedFeature {
                feature: "encoded headers require seekable reader - use parse_header_with_seek",
            }),
            _ => Err(Error::InvalidFormat(format!(
                "expected header marker, got {:#x}",
                first_byte
            ))),
        }
    }

    /// Parses the archive header from a seekable reader.
    ///
    /// This variant supports both plain and encoded (compressed) headers.
    /// The reader should be positioned at the start of the header data.
    pub fn parse_header_with_seek<R: Read + Seek>(
        &mut self,
        r: &mut R,
        archive_data_start: u64,
    ) -> Result<ArchiveHeader> {
        let first_byte = read_u8(r)?;
        self.bytes_read += 1;

        match first_byte {
            property_id::HEADER => self.parse_main_header(r),
            property_id::ENCODED_HEADER => self.parse_encoded_header(r, archive_data_start),
            _ => Err(Error::InvalidFormat(format!(
                "expected header marker, got {:#x}",
                first_byte
            ))),
        }
    }

    /// Parses the main header content.
    fn parse_main_header<R: Read>(&mut self, r: &mut R) -> Result<ArchiveHeader> {
        let mut header = ArchiveHeader::default();

        loop {
            let prop_id = read_u8(r)?;
            self.bytes_read += 1;
            self.check_byte_limit()?;

            match prop_id {
                property_id::END => break,

                property_id::MAIN_STREAMS_INFO => {
                    self.parse_streams_info(r, &mut header)?;
                }

                property_id::FILES_INFO => {
                    // Get sizes and CRCs from substreams or folders
                    let (sizes, crcs) = self.get_file_sizes_and_crcs(&header);
                    header.files_info = Some(FilesInfo::parse(r, &sizes, &crcs, &self.limits)?);
                }

                _ => {
                    return Err(Error::CorruptHeader {
                        offset: self.bytes_read,
                        reason: format!("unexpected property ID in header: {:#x}", prop_id),
                    });
                }
            }
        }

        Ok(header)
    }

    /// Parses streams info section.
    fn parse_streams_info<R: Read>(&mut self, r: &mut R, header: &mut ArchiveHeader) -> Result<()> {
        loop {
            let prop_id = read_u8(r)?;
            self.bytes_read += 1;
            self.check_byte_limit()?;

            match prop_id {
                property_id::END => break,

                property_id::PACK_INFO => {
                    header.pack_info = Some(PackInfo::parse(r, &self.limits)?);
                }

                property_id::UNPACK_INFO => {
                    header.unpack_info = Some(UnpackInfo::parse(r, &self.limits)?);
                }

                property_id::SUBSTREAMS_INFO => {
                    let folders = header
                        .unpack_info
                        .as_ref()
                        .map_or(&[] as &[Folder], |u| &u.folders);
                    header.substreams_info = Some(SubStreamsInfo::parse(r, folders, &self.limits)?);
                }

                _ => {
                    return Err(Error::CorruptHeader {
                        offset: self.bytes_read,
                        reason: format!("unexpected property ID in streams info: {:#x}", prop_id),
                    });
                }
            }
        }

        Ok(())
    }

    /// Parses an encoded (compressed) header.
    ///
    /// Encoded headers contain StreamsInfo describing how to decompress the actual header.
    /// This method decompresses the header data and recursively parses the result.
    fn parse_encoded_header<R: Read + Seek>(
        &mut self,
        r: &mut R,
        archive_data_start: u64,
    ) -> Result<ArchiveHeader> {
        self.recursion_depth += 1;
        if self.recursion_depth > self.max_recursion_depth {
            return Err(Error::ResourceLimitExceeded(
                "maximum encoded header recursion depth exceeded".into(),
            ));
        }

        // Parse the streams info that describes how to decompress the header
        let mut streams_header = ArchiveHeader::default();
        self.parse_streams_info(r, &mut streams_header)?;

        // Get current position - for embedded encoded headers (like encrypted headers),
        // the packed data follows immediately after the streams info
        let current_pos = r.stream_position()?;

        // Check if header is encrypted (uses AES codec)
        let header_encrypted = Self::folder_uses_encryption(&streams_header);

        // For encrypted headers, the data is embedded right after streams_info.
        // Use current position as the base for pack_pos instead of archive_data_start.
        let data_base = if header_encrypted {
            current_pos
        } else {
            archive_data_start
        };

        // Decompress the header
        let decompressed = self.decompress_header(r, &streams_header, data_base)?;

        if decompressed.is_empty() {
            return Err(Error::InvalidFormat("empty decompressed header".into()));
        }

        // Parse the decompressed data
        let first_byte = decompressed[0];
        match first_byte {
            property_id::HEADER => {
                let mut cursor = Cursor::new(&decompressed[1..]);
                let mut header = self.parse_main_header(&mut cursor)?;
                header.header_encrypted = header_encrypted;
                Ok(header)
            }
            property_id::ENCODED_HEADER => {
                // Recursively handle nested encoded headers
                let mut cursor = Cursor::new(&decompressed[1..]);
                let mut nested_streams = ArchiveHeader::default();
                self.parse_streams_info(&mut cursor, &mut nested_streams)?;

                // Check nested header for encryption too
                let nested_encrypted = Self::folder_uses_encryption(&nested_streams);

                let nested_decompressed =
                    self.decompress_header(r, &nested_streams, archive_data_start)?;

                if nested_decompressed.is_empty() {
                    return Err(Error::InvalidFormat(
                        "empty nested decompressed header".into(),
                    ));
                }

                if nested_decompressed[0] == property_id::HEADER {
                    let mut inner_cursor = Cursor::new(&nested_decompressed[1..]);
                    let mut header = self.parse_main_header(&mut inner_cursor)?;
                    // Header is encrypted if either level used encryption
                    header.header_encrypted = header_encrypted || nested_encrypted;
                    Ok(header)
                } else {
                    Err(Error::InvalidFormat(format!(
                        "unexpected header marker in nested decompressed data: {:#x}",
                        nested_decompressed[0]
                    )))
                }
            }
            _ => Err(Error::InvalidFormat(format!(
                "unexpected header marker in decompressed data: {:#x}",
                first_byte
            ))),
        }
    }

    /// Checks if any folder in the header uses AES encryption.
    fn folder_uses_encryption(header: &ArchiveHeader) -> bool {
        if let Some(ref unpack_info) = header.unpack_info {
            for folder in &unpack_info.folders {
                for coder in &folder.coders {
                    if coder.method_id.as_slice() == codec::method::AES {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Decompresses encoded header data.
    ///
    /// Uses the pack_info and unpack_info from the streams header to locate
    /// and decompress the actual header data.
    fn decompress_header<R: Read + Seek>(
        &self,
        r: &mut R,
        header: &ArchiveHeader,
        archive_data_start: u64,
    ) -> Result<Vec<u8>> {
        let pack_info = header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("encoded header missing pack info".into()))?;

        let unpack_info = header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("encoded header missing unpack info".into()))?;

        if unpack_info.folders.is_empty() {
            return Err(Error::InvalidFormat("encoded header has no folders".into()));
        }

        // Validate packed_streams and bind_pairs indices
        let num_pack_streams = pack_info.num_streams();
        for (folder_idx, folder) in unpack_info.folders.iter().enumerate() {
            folder
                .validate_packed_streams(num_pack_streams)
                .map_err(|e| Error::InvalidFormat(format!("folder[{}]: {}", folder_idx, e)))?;
            folder
                .validate_bind_pairs()
                .map_err(|e| Error::InvalidFormat(format!("folder[{}]: {}", folder_idx, e)))?;
        }

        let folder = &unpack_info.folders[0];

        // Calculate position of compressed header data
        let pack_pos = archive_data_start + pack_info.pack_pos;
        r.seek(SeekFrom::Start(pack_pos))?;

        // Read compressed data
        let pack_size = pack_info
            .pack_sizes
            .first()
            .copied()
            .ok_or_else(|| Error::InvalidFormat("encoded header missing pack size".into()))?;

        let mut packed_data = vec![0u8; pack_size as usize];
        r.read_exact(&mut packed_data)?;

        // Get uncompressed size
        let unpack_size = folder
            .final_unpack_size()
            .ok_or_else(|| Error::InvalidFormat("encoded header missing unpack size".into()))?;

        // Build decoder and decompress
        let cursor = Cursor::new(packed_data);
        let mut decoder = self.build_header_decoder(cursor, folder, unpack_size)?;

        let mut decompressed = Vec::with_capacity(unpack_size as usize);
        decoder.read_to_end(&mut decompressed)?;

        // Verify CRC if available
        if let Some(expected_crc) = folder.unpack_crc {
            let actual_crc = crc32fast::hash(&decompressed);
            if actual_crc != expected_crc {
                return Err(Error::CorruptHeader {
                    offset: pack_pos,
                    reason: format!(
                        "encoded header CRC mismatch: expected {:#x}, got {:#x}",
                        expected_crc, actual_crc
                    ),
                });
            }
        }

        Ok(decompressed)
    }

    /// Builds a decoder for header decompression.
    ///
    /// Most headers use a single LZMA coder, but some may use a filter chain.
    /// Encrypted headers use AES + LZMA2 and require a password.
    fn build_header_decoder(
        &self,
        input: Cursor<Vec<u8>>,
        folder: &Folder,
        unpack_size: u64,
    ) -> Result<Box<dyn Read>> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        // Single coder case (most common for headers - usually LZMA)
        if folder.coders.len() == 1 {
            let coder = &folder.coders[0];
            return Ok(Box::new(codec::build_decoder(input, coder, unpack_size)?));
        }

        // Two-coder chain
        if folder.coders.len() == 2 {
            let outer_coder = &folder.coders[0]; // Applied second (e.g., LZMA2 or filter)
            let inner_coder = &folder.coders[1]; // Applied first (e.g., AES or LZMA)

            // Check if this is an encrypted header (AES + compression)
            #[cfg(feature = "aes")]
            if inner_coder.method_id.as_slice() == codec::method::AES {
                // Encrypted header: AES (inner) -> LZMA2 (outer)
                let password = self.password.as_ref().ok_or(Error::PasswordRequired)?;

                // Get intermediate size (AES output = compressed size)
                let aes_unpack_size = folder.unpack_sizes.get(1).copied().unwrap_or(unpack_size);

                // First decrypt with AES
                let decrypted =
                    codec::build_decoder_encrypted(input, inner_coder, aes_unpack_size, password)?;

                // Then decompress with LZMA2
                return Ok(Box::new(codec::build_decoder(
                    decrypted,
                    outer_coder,
                    unpack_size,
                )?));
            }

            // Non-encrypted two-coder chain (e.g., filter + LZMA)
            // In 7z, for a 2-coder chain like [BCJ, LZMA], the data flows:
            // compressed -> LZMA -> BCJ -> uncompressed
            // So we apply the second coder first (inner), then the first (outer)
            let codec_unpack_size = folder.unpack_sizes.get(1).copied().unwrap_or(unpack_size);

            // First decompress with inner codec
            let inner = codec::build_decoder(input, inner_coder, codec_unpack_size)?;

            // Then apply outer filter/codec
            return Ok(Box::new(codec::build_decoder(
                inner,
                outer_coder,
                unpack_size,
            )?));
        }

        Err(Error::UnsupportedFeature {
            feature: "encoded headers with more than 2 coders",
        })
    }

    /// Gets file sizes and CRCs from parsed structures.
    fn get_file_sizes_and_crcs(&self, header: &ArchiveHeader) -> (Vec<u64>, Vec<Option<u32>>) {
        if let Some(ref substreams) = header.substreams_info {
            (substreams.unpack_sizes.clone(), substreams.digests.clone())
        } else if let Some(ref unpack_info) = header.unpack_info {
            // Single file per folder
            let sizes: Vec<u64> = unpack_info
                .folders
                .iter()
                .filter_map(|f| f.final_unpack_size())
                .collect();
            let crcs: Vec<Option<u32>> = unpack_info.folders.iter().map(|f| f.unpack_crc).collect();
            (sizes, crcs)
        } else {
            (Vec::new(), Vec::new())
        }
    }

    /// Checks if we've exceeded the byte limit.
    fn check_byte_limit(&self) -> Result<()> {
        if self.bytes_read > self.limits.max_header_bytes {
            Err(Error::ResourceLimitExceeded(
                "header byte limit exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Default for HeaderParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads a complete 7z archive's headers.
///
/// This is the main entry point for parsing a 7z file.
/// Supports both plain and encoded (compressed) headers.
pub fn read_archive_header<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
) -> Result<(StartHeader, ArchiveHeader)> {
    read_archive_header_internal(r, limits, 0, None)
}

/// Reads a complete 7z archive's headers with a password for encrypted headers.
///
/// This variant supports encrypted headers that require a password to decrypt.
#[cfg(feature = "aes")]
pub fn read_archive_header_with_password<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
    password: Option<crate::crypto::Password>,
) -> Result<(StartHeader, ArchiveHeader)> {
    read_archive_header_internal(r, limits, 0, password)
}

/// Reads a complete 7z archive's headers with an optional SFX offset.
///
/// This is the main entry point for parsing a 7z file.
/// Supports both plain and encoded (compressed) headers.
///
/// # Arguments
///
/// * `r` - Reader positioned at the start of the 7z signature
/// * `limits` - Resource limits for parsing
/// * `sfx_offset` - Offset to the 7z signature (non-zero for SFX archives)
pub fn read_archive_header_with_offset<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
    sfx_offset: u64,
) -> Result<(StartHeader, ArchiveHeader)> {
    read_archive_header_internal(r, limits, sfx_offset, None)
}

/// Reads a complete 7z archive's headers with SFX offset and optional password.
///
/// This variant supports encrypted headers that require a password to decrypt.
#[cfg(feature = "aes")]
pub fn read_archive_header_with_offset_and_password<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
    sfx_offset: u64,
    password: Option<crate::crypto::Password>,
) -> Result<(StartHeader, ArchiveHeader)> {
    read_archive_header_internal(r, limits, sfx_offset, password)
}

/// Internal implementation for reading archive headers.
#[cfg(feature = "aes")]
fn read_archive_header_internal<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
    sfx_offset: u64,
    password: Option<crate::crypto::Password>,
) -> Result<(StartHeader, ArchiveHeader)> {
    // Parse start header (validates signature and gets next header location)
    let mut start_header = StartHeader::parse(r)?;
    start_header.sfx_offset = sfx_offset;

    // Handle empty archives
    if start_header.next_header_size == 0 {
        return Ok((start_header, ArchiveHeader::default()));
    }

    // Seek to the next header
    let header_pos = start_header.next_header_position();
    r.seek(SeekFrom::Start(header_pos))?;

    // Read header data into buffer for CRC verification
    let mut header_data = vec![0u8; start_header.next_header_size as usize];
    r.read_exact(&mut header_data)?;

    // Verify CRC of header data
    let actual_crc = crc32fast::hash(&header_data);
    if actual_crc != start_header.next_header_crc {
        return Err(Error::CorruptHeader {
            offset: header_pos,
            reason: format!(
                "next header CRC mismatch: expected {:#x}, got {:#x}",
                start_header.next_header_crc, actual_crc
            ),
        });
    }

    // Parse the header from the buffer, but pass the original reader
    // for seeking if we encounter an encoded header
    let mut parser = limits
        .map(HeaderParser::with_limits)
        .unwrap_or_default()
        .with_password(password);

    // Check first byte to determine header type
    if header_data.is_empty() {
        return Err(Error::InvalidFormat("empty header data".into()));
    }

    let first_byte = header_data[0];
    let archive_header = match first_byte {
        property_id::HEADER => {
            // Plain header - parse from buffer
            let mut cursor = Cursor::new(&header_data[1..]);
            parser.parse_main_header(&mut cursor)?
        }
        property_id::ENCODED_HEADER => {
            // Encoded header - need to seek back and use the reader
            // for accessing compressed header data
            r.seek(SeekFrom::Start(header_pos))?;
            // For SFX archives, the data start includes the SFX offset
            let data_start = sfx_offset + SIGNATURE_HEADER_SIZE;
            parser.parse_header_with_seek(r, data_start)?
        }
        _ => {
            return Err(Error::InvalidFormat(format!(
                "expected header marker, got {:#x}",
                first_byte
            )));
        }
    };

    Ok((start_header, archive_header))
}

/// Internal implementation for reading archive headers (non-AES version).
#[cfg(not(feature = "aes"))]
fn read_archive_header_internal<R: Read + Seek>(
    r: &mut R,
    limits: Option<ResourceLimits>,
    sfx_offset: u64,
    _password: Option<()>,
) -> Result<(StartHeader, ArchiveHeader)> {
    // Parse start header (validates signature and gets next header location)
    let mut start_header = StartHeader::parse(r)?;
    start_header.sfx_offset = sfx_offset;

    // Handle empty archives
    if start_header.next_header_size == 0 {
        return Ok((start_header, ArchiveHeader::default()));
    }

    // Seek to the next header
    let header_pos = start_header.next_header_position();
    r.seek(SeekFrom::Start(header_pos))?;

    // Read header data into buffer for CRC verification
    let mut header_data = vec![0u8; start_header.next_header_size as usize];
    r.read_exact(&mut header_data)?;

    // Verify CRC of header data
    let actual_crc = crc32fast::hash(&header_data);
    if actual_crc != start_header.next_header_crc {
        return Err(Error::CorruptHeader {
            offset: header_pos,
            reason: format!(
                "next header CRC mismatch: expected {:#x}, got {:#x}",
                start_header.next_header_crc, actual_crc
            ),
        });
    }

    // Parse the header from the buffer, but pass the original reader
    // for seeking if we encounter an encoded header
    let mut parser = limits.map(HeaderParser::with_limits).unwrap_or_default();

    // Check first byte to determine header type
    if header_data.is_empty() {
        return Err(Error::InvalidFormat("empty header data".into()));
    }

    let first_byte = header_data[0];
    let archive_header = match first_byte {
        property_id::HEADER => {
            // Plain header - parse from buffer
            let mut cursor = Cursor::new(&header_data[1..]);
            parser.parse_main_header(&mut cursor)?
        }
        property_id::ENCODED_HEADER => {
            // Encoded header - need to seek back and use the reader
            // for accessing compressed header data
            r.seek(SeekFrom::Start(header_pos))?;
            // For SFX archives, the data start includes the SFX offset
            let data_start = sfx_offset + SIGNATURE_HEADER_SIZE;
            parser.parse_header_with_seek(r, data_start)?
        }
        _ => {
            return Err(Error::InvalidFormat(format!(
                "expected header marker, got {:#x}",
                first_byte
            )));
        }
    };

    Ok((start_header, archive_header))
}

#[cfg(test)]
#[allow(clippy::vec_init_then_push)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_variable_u64(buf: &mut Vec<u8>, value: u64) {
        use super::super::reader::write_variable_u64;
        write_variable_u64(buf, value).unwrap();
    }

    #[test]
    fn test_header_parser_empty() {
        // Empty header (just END)
        let data = vec![property_id::HEADER, property_id::END];

        let mut cursor = Cursor::new(&data);
        let mut parser = HeaderParser::new();
        let header = parser.parse_header(&mut cursor).unwrap();

        assert!(header.pack_info.is_none());
        assert!(header.unpack_info.is_none());
        assert!(header.files_info.is_none());
    }

    #[test]
    fn test_header_parser_invalid_first_byte() {
        let data = vec![0x99]; // Invalid marker

        let mut cursor = Cursor::new(&data);
        let mut parser = HeaderParser::new();
        let err = parser.parse_header(&mut cursor).unwrap_err();

        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_header_parser_with_streams() {
        let mut data = Vec::new();

        // K_HEADER
        data.push(property_id::HEADER);

        // K_MAIN_STREAMS_INFO
        data.push(property_id::MAIN_STREAMS_INFO);

        // K_PACK_INFO
        data.push(property_id::PACK_INFO);
        write_variable_u64(&mut data, 0); // pack_pos
        write_variable_u64(&mut data, 1); // 1 stream
        data.push(property_id::SIZE);
        write_variable_u64(&mut data, 1000); // size
        data.push(property_id::END);

        // End streams info
        data.push(property_id::END);

        // End header
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let mut parser = HeaderParser::new();
        let header = parser.parse_header(&mut cursor).unwrap();

        assert!(header.pack_info.is_some());
        let pack_info = header.pack_info.unwrap();
        assert_eq!(pack_info.pack_sizes, vec![1000]);
    }

    #[test]
    fn test_encoded_header_requires_codecs() {
        // Simulate an encoded header
        let data = vec![
            property_id::ENCODED_HEADER,
            property_id::END, // Minimal streams info
        ];

        let mut cursor = Cursor::new(&data);
        let mut parser = HeaderParser::new();
        let err = parser.parse_header(&mut cursor).unwrap_err();

        assert!(matches!(err, Error::UnsupportedFeature { .. }));
    }

    #[test]
    fn test_resource_limits() {
        let limits = ResourceLimits {
            max_entries: 10,
            max_header_bytes: 100,
            ..Default::default()
        };

        let parser = HeaderParser::with_limits(limits.clone());
        assert_eq!(parser.limits.max_entries, 10);
        assert_eq!(parser.limits.max_header_bytes, 100);
    }
}
