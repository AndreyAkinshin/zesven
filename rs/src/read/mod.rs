//! Archive reading API for 7z archives.
//!
//! This module provides the public API for reading 7z archives, including
//! listing entries, extracting files, and verifying integrity.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::read::{Archive, ExtractOptions};
//!
//! // Open an archive
//! let mut archive = Archive::open_path("archive.7z")?;
//!
//! // List entries
//! for entry in archive.entries() {
//!     println!("{}: {} bytes", entry.path.as_str(), entry.size);
//! }
//!
//! // Extract all files
//! archive.extract("output_dir", (), &ExtractOptions::default())?;
//! ```

// Core modules
mod destination;
pub(crate) mod entries;
mod entry;
mod info;
mod options;

// Refactored modules
mod archive_open;
mod archive_query;
mod archive_test;
mod decompression;
mod extraction;
mod metadata;
mod multivolume;
mod path_safety;
mod solid_blocks;

// Re-exports from core modules
pub use destination::{
    ExtractDestination, FilesystemDestination, MemoryDestination, NullDestination,
};
#[cfg(feature = "regex")]
pub use entry::SelectByRegex;
pub use entry::{
    Entry, EntrySelector, SelectAll, SelectByName, SelectByPredicate, SelectFilesOnly,
};
pub use info::{ArchiveInfo, EncryptionInfo, ExtractResult, TestResult};
pub use options::{
    ExtractOptions, FilterPolicy, LinkPolicy, OverwritePolicy, PathSafety, PreserveMetadata,
    TestOptions, Threads,
};

// Re-exports from refactored modules
pub(crate) use archive_open::{ExtractionLimits, map_io_error};

use std::path::PathBuf;

#[cfg(feature = "aes")]
use crate::Password;
use crate::format::parser::ArchiveHeader;

/// Volume information for multi-volume archives.
#[derive(Debug, Clone)]
pub(crate) struct VolumeInfo {
    /// Number of volumes.
    pub count: u32,
    /// Paths to each volume file.
    pub paths: Vec<PathBuf>,
}

/// A 7z archive reader.
pub struct Archive<R> {
    pub(crate) reader: R,
    pub(crate) header: ArchiveHeader,
    pub(crate) entries: Vec<Entry>,
    pub(crate) info: ArchiveInfo,
    /// Password for encrypted extraction (used by extraction methods).
    #[cfg(feature = "aes")]
    pub(crate) password: Option<Password>,
    /// Volume information for multi-volume archives.
    pub(crate) volume_info: Option<VolumeInfo>,
    /// Offset to the 7z signature (non-zero for SFX archives).
    pub(crate) sfx_offset: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Minimal valid 7z archive (empty) with proper header structure
    fn make_empty_archive() -> Vec<u8> {
        use crate::format::property_id;

        let mut data = Vec::new();

        // Signature
        data.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
        // Version (0.4)
        data.extend_from_slice(&[0x00, 0x04]);

        // Start header CRC (placeholder)
        let start_header_crc_pos = data.len();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        // Next header offset (0 - header immediately follows)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        // Header data: HEADER marker followed by END
        let header_data = vec![property_id::HEADER, property_id::END];

        // Next header size (2 bytes: HEADER + END)
        let header_size = header_data.len() as u64;
        data.extend_from_slice(&header_size.to_le_bytes());
        // Next header CRC
        let header_crc = crc32fast::hash(&header_data);
        data.extend_from_slice(&header_crc.to_le_bytes());

        // Compute start header CRC (covers bytes 12-31: offset, size, crc)
        let start_header_crc = crc32fast::hash(&data[12..32]);
        data[start_header_crc_pos..start_header_crc_pos + 4]
            .copy_from_slice(&start_header_crc.to_le_bytes());

        // Append header data
        data.extend_from_slice(&header_data);

        data
    }

    #[test]
    fn test_archive_info_default() {
        let info = ArchiveInfo::default();
        assert_eq!(info.entry_count, 0);
        assert!(!info.is_solid);
    }

    #[test]
    fn test_extract_options_builder() {
        let opts = ExtractOptions::new()
            .overwrite(OverwritePolicy::Overwrite)
            .path_safety(PathSafety::Strict);

        assert_eq!(opts.overwrite, OverwritePolicy::Overwrite);
        assert_eq!(opts.path_safety, PathSafety::Strict);
    }

    #[test]
    fn test_open_empty_archive() {
        let data = make_empty_archive();
        let cursor = Cursor::new(data);
        let archive = Archive::open(cursor).unwrap();

        assert!(archive.is_empty());
        assert_eq!(archive.len(), 0);
    }
}
