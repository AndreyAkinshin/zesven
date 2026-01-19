//! 7z archive format constants, definitions, and low-level parsing utilities.
//!
//! This module contains the magic numbers, property IDs, and other constants
//! defined by the 7z archive format specification.

pub mod detect;
pub mod files;
pub mod header;
pub mod parser;
pub mod reader;
pub mod streams;

/// The 7z file signature (magic bytes).
///
/// Every valid 7z archive starts with these 6 bytes: `'7' 'z' 0xBC 0xAF 0x27 0x1C`
pub const SIGNATURE: &[u8; 6] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// Size of the signature header in bytes.
///
/// The signature header contains:
/// - 6 bytes: signature
/// - 2 bytes: version (major, minor)
/// - 4 bytes: start header CRC
/// - 8 bytes: next header offset
/// - 8 bytes: next header size
/// - 4 bytes: next header CRC
pub const SIGNATURE_HEADER_SIZE: u64 = 32;

/// Archive version - major.
pub const VERSION_MAJOR: u8 = 0;

/// Archive version - minor.
pub const VERSION_MINOR: u8 = 4;

/// Property IDs used in 7z archive headers.
pub mod property_id {
    /// End of header marker.
    pub const END: u8 = 0x00;
    /// Header marker.
    pub const HEADER: u8 = 0x01;
    /// Archive properties.
    pub const ARCHIVE_PROPERTIES: u8 = 0x02;
    /// Additional streams info.
    pub const ADDITIONAL_STREAMS_INFO: u8 = 0x03;
    /// Main streams info.
    pub const MAIN_STREAMS_INFO: u8 = 0x04;
    /// Files info.
    pub const FILES_INFO: u8 = 0x05;
    /// Pack info.
    pub const PACK_INFO: u8 = 0x06;
    /// Unpack info.
    pub const UNPACK_INFO: u8 = 0x07;
    /// Substreams info.
    pub const SUBSTREAMS_INFO: u8 = 0x08;
    /// Size info.
    pub const SIZE: u8 = 0x09;
    /// CRC info.
    pub const CRC: u8 = 0x0A;
    /// Folder info.
    pub const FOLDER: u8 = 0x0B;
    /// Coders unpack size.
    pub const CODERS_UNPACK_SIZE: u8 = 0x0C;
    /// Number of unpack streams in folders.
    pub const NUM_UNPACK_STREAM: u8 = 0x0D;
    /// Empty stream indicator.
    pub const EMPTY_STREAM: u8 = 0x0E;
    /// Empty file indicator.
    pub const EMPTY_FILE: u8 = 0x0F;
    /// Anti-file indicator.
    pub const ANTI: u8 = 0x10;
    /// File names.
    pub const NAME: u8 = 0x11;
    /// Creation time.
    pub const CTIME: u8 = 0x12;
    /// Access time.
    pub const ATIME: u8 = 0x13;
    /// Modification time.
    pub const MTIME: u8 = 0x14;
    /// Windows file attributes.
    pub const WIN_ATTRIBUTES: u8 = 0x15;
    /// Comment.
    pub const COMMENT: u8 = 0x16;
    /// Encoded header.
    pub const ENCODED_HEADER: u8 = 0x17;
    /// Start position.
    pub const START_POS: u8 = 0x18;
    /// Dummy marker.
    pub const DUMMY: u8 = 0x19;
}

/// Windows file attribute constants.
pub mod attributes {
    /// Read-only file.
    pub const READONLY: u32 = 0x01;
    /// Hidden file.
    pub const HIDDEN: u32 = 0x02;
    /// System file.
    pub const SYSTEM: u32 = 0x04;
    /// Directory.
    pub const DIRECTORY: u32 = 0x10;
    /// Archive file.
    pub const ARCHIVE: u32 = 0x20;
    /// Symbolic link (reparse point).
    pub const REPARSE_POINT: u32 = 0x400;
    /// Compressed file (NTFS).
    pub const COMPRESSED: u32 = 0x800;
    /// Unix permissions shift (high 16 bits).
    pub const UNIX_EXTENSION: u32 = 0x8000;
}

/// Compression method IDs used in 7z archives.
pub mod method_id {
    /// Copy (no compression).
    pub const COPY: u64 = 0x00;
    /// Delta filter.
    pub const DELTA: u64 = 0x03;
    /// BCJ (x86) filter.
    pub const BCJ: u64 = 0x04_01_00;
    /// BCJ2 (x86) filter.
    pub const BCJ2: u64 = 0x04_01_02;
    /// PPC filter.
    pub const PPC: u64 = 0x04_02_05;
    /// IA64 filter.
    pub const IA64: u64 = 0x04_03_01;
    /// ARM filter.
    pub const ARM: u64 = 0x04_04_01;
    /// ARM Thumb filter.
    pub const ARMT: u64 = 0x04_05_01;
    /// SPARC filter.
    pub const SPARC: u64 = 0x04_06_05;
    /// ARM64 filter.
    pub const ARM64: u64 = 0x04_09_01;
    /// Deflate.
    pub const DEFLATE: u64 = 0x04_01_08;
    /// Deflate64.
    pub const DEFLATE64: u64 = 0x04_01_09;
    /// BZip2.
    pub const BZIP2: u64 = 0x04_02_02;
    /// LZMA.
    pub const LZMA: u64 = 0x03_01_01;
    /// LZMA2.
    pub const LZMA2: u64 = 0x21;
    /// PPMd.
    pub const PPMD: u64 = 0x03_04_01;
    /// AES-256-CBC + SHA-256.
    pub const AES_256_SHA_256: u64 = 0x06_F1_07_01;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature() {
        assert_eq!(SIGNATURE.len(), 6);
        assert_eq!(SIGNATURE[0], b'7');
        assert_eq!(SIGNATURE[1], b'z');
    }

    #[test]
    fn test_signature_header_size() {
        assert_eq!(SIGNATURE_HEADER_SIZE, 32);
    }

    #[test]
    fn test_property_ids() {
        assert_eq!(property_id::END, 0x00);
        assert_eq!(property_id::HEADER, 0x01);
        assert_eq!(property_id::MTIME, 0x14);
    }

    #[test]
    fn test_method_ids() {
        assert_eq!(method_id::COPY, 0x00);
        assert_eq!(method_id::LZMA, 0x03_01_01);
        assert_eq!(method_id::LZMA2, 0x21);
    }
}
