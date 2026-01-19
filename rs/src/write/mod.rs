//! Archive writing API for 7z archives.
//!
//! This module provides the public API for creating 7z archives, including
//! adding files, directories, and streams with various compression options.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::write::{Writer, WriteOptions};
//!
//! // Create an archive
//! let mut writer = Writer::create_path("archive.7z")?;
//!
//! // Add files
//! writer.add_path("file.txt", "file.txt".try_into()?)?;
//!
//! // Finish writing
//! let result = writer.finish()?;
//! println!("Wrote {} entries", result.entries_written);
//! ```

// Existing modules
mod append;
pub(crate) mod options;

// Refactored modules
mod codecs;
mod compression;
mod encoding_utils;
mod entry_compression;
mod entry_input;
mod header_encode;
mod header_encryption;
mod metadata_encode;
mod writer_init;

// Re-exports
pub use append::{AppendResult, ArchiveAppender};
pub use options::{EntryMeta, Lzma2Variant, SolidOptions, WriteFilter, WriteOptions, WriteResult};

use crate::ArchivePath;

/// Maps zesven compression level (0-9) to Zstd level (1-22).
///
/// | Input | Zstd | Characteristic |
/// |-------|------|----------------|
/// | 0-1   | 1    | Fastest        |
/// | 2     | 2    | Fast           |
/// | 3     | 3    | Fast           |
/// | 4     | 5    | Balanced       |
/// | 5     | 7    | Balanced       |
/// | 6     | 9    | Balanced       |
/// | 7     | 12   | High           |
/// | 8     | 15   | High           |
/// | 9     | 19   | Maximum        |
#[cfg(feature = "zstd")]
const ZSTD_LEVEL_MAP: [i32; 10] = [1, 1, 2, 3, 5, 7, 9, 12, 15, 19];

/// Maps zesven compression level (0-9) to Brotli quality (0-11).
///
/// | Input | Brotli | Characteristic |
/// |-------|--------|----------------|
/// | 0-6   | 0-6    | Direct mapping |
/// | 7     | 8      | High           |
/// | 8     | 10     | High           |
/// | 9     | 11     | Maximum        |
#[cfg(feature = "brotli")]
const BROTLI_QUALITY_MAP: [u32; 10] = [0, 1, 2, 3, 4, 5, 6, 8, 10, 11];

/// State of the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriterState {
    /// Accepting new entries.
    AcceptingEntries,
    /// Building the archive (flushing, writing headers).
    Building,
    /// Archive is finished.
    Finished,
}

/// Entry data stored for header writing.
#[derive(Debug)]
pub(crate) struct PendingEntry {
    /// Archive path.
    path: ArchivePath,
    /// Entry metadata.
    meta: options::EntryMeta,
    /// Uncompressed size.
    uncompressed_size: u64,
}

/// Entry buffered for solid compression.
#[derive(Debug)]
struct SolidBufferEntry {
    /// Archive path.
    path: ArchivePath,
    /// Entry data (uncompressed).
    data: Vec<u8>,
    /// Entry metadata.
    meta: options::EntryMeta,
    /// CRC32 of uncompressed data.
    crc: u32,
}

/// Encryption metadata for a folder (used when content encryption is enabled).
#[cfg(feature = "aes")]
#[derive(Debug, Clone)]
pub(crate) struct EncryptedFolderInfo {
    /// AES properties (salt, iv, cycles) for this folder.
    aes_properties: Vec<u8>,
    /// Size after compression (before encryption) - needed for unpack_sizes in 2-coder chain.
    compressed_size: u64,
}

/// Filter metadata for a folder (used when pre-compression filter is enabled).
#[derive(Debug, Clone)]
pub(crate) struct FilteredFolderInfo {
    /// Filter method ID bytes.
    filter_method: Vec<u8>,
    /// Filter properties (e.g., delta distance).
    filter_properties: Option<Vec<u8>>,
    /// Size after filtering (before compression) - same as uncompressed size for filters.
    filtered_size: u64,
}

/// Metadata for BCJ2 4-stream folder.
///
/// BCJ2 is a special filter that splits x86 code into 4 streams:
/// - Stream 0 (Main): Main code with E8/E9 instructions
/// - Stream 1 (Call): CALL destinations, big-endian
/// - Stream 2 (Jump): JMP destinations, big-endian
/// - Stream 3 (Range): Range encoder output
#[derive(Debug, Clone)]
struct Bcj2FolderInfo {
    /// Sizes of the 4 pack streams [main, call, jump, range]
    pack_sizes: [u64; 4],
}

/// Stream info for pack/unpack info.
#[derive(Debug, Default)]
struct StreamInfo {
    /// Packed sizes for each folder. Most folders have 1, BCJ2 has 4.
    /// For BCJ2 folders, this is empty; use bcj2_folder_info instead.
    pack_sizes: Vec<u64>,
    /// Total unpacked size for each folder.
    unpack_sizes: Vec<u64>,
    /// CRCs for each folder (used for non-solid).
    crcs: Vec<u32>,
    /// Number of unpack streams in each folder (for solid archives).
    num_unpack_streams_per_folder: Vec<u64>,
    /// Sizes of each substream within solid blocks.
    substream_sizes: Vec<u64>,
    /// CRCs of each substream within solid blocks.
    substream_crcs: Vec<u32>,
    /// Per-folder encryption info (Some if encrypted, None if not).
    #[cfg(feature = "aes")]
    encryption_info: Vec<Option<EncryptedFolderInfo>>,
    /// Per-folder filter info (Some if filtered, None if not).
    filter_info: Vec<Option<FilteredFolderInfo>>,
    /// Per-folder BCJ2 info (Some for BCJ2 folders, None for regular).
    bcj2_folder_info: Vec<Option<Bcj2FolderInfo>>,
}

/// A 7z archive writer.
pub struct Writer<W> {
    sink: W,
    options: options::WriteOptions,
    state: WriterState,
    entries: Vec<PendingEntry>,
    stream_info: StreamInfo,
    /// Total compressed bytes written.
    compressed_bytes: u64,
    /// Buffer for solid compression.
    solid_buffer: Vec<SolidBufferEntry>,
    /// Current size of solid buffer (uncompressed bytes).
    solid_buffer_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_writer_create() {
        let buffer = Cursor::new(Vec::new());
        let writer = Writer::create(buffer).unwrap();
        assert_eq!(writer.state, WriterState::AcceptingEntries);
    }

    #[test]
    fn test_writer_options() {
        let buffer = Cursor::new(Vec::new());
        let writer = Writer::create(buffer)
            .unwrap()
            .options(WriteOptions::new().level(9).unwrap());
        assert_eq!(writer.options.level, 9);
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_writer_add_bytes_and_finish() {
        let buffer = Cursor::new(Vec::new());
        let mut writer = Writer::create(buffer).unwrap();

        let path = ArchivePath::new("test.txt").unwrap();
        writer.add_bytes(path, b"Hello, World!").unwrap();

        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 1);
        assert_eq!(result.total_size, 13);
    }

    #[test]
    fn test_writer_empty_archive() {
        let buffer = Cursor::new(Vec::new());
        let writer = Writer::create(buffer).unwrap();
        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 0);
    }

    #[test]
    fn test_writer_with_directory() {
        let buffer = Cursor::new(Vec::new());
        let mut writer = Writer::create(buffer).unwrap();

        let dir_path = ArchivePath::new("mydir").unwrap();
        writer
            .add_directory(dir_path, options::EntryMeta::directory())
            .unwrap();

        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 0);
        assert_eq!(result.directories_written, 1);
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_writer_with_anti_item() {
        let buffer = Cursor::new(Vec::new());
        let mut writer = Writer::create(buffer).unwrap();

        // Add a regular file
        let file_path = ArchivePath::new("keep.txt").unwrap();
        writer.add_bytes(file_path, b"Keep this file").unwrap();

        // Add an anti-item (marks a file for deletion)
        let anti_path = ArchivePath::new("deleted.txt").unwrap();
        writer.add_anti_item(anti_path).unwrap();

        // Add an anti-directory
        let anti_dir_path = ArchivePath::new("deleted_dir").unwrap();
        writer.add_anti_directory(anti_dir_path).unwrap();

        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 1); // Only the regular file counts as written
        assert_eq!(result.directories_written, 1); // Anti-directory is counted as directory
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_anti_item_roundtrip() {
        use crate::read::Archive;

        let buffer = Cursor::new(Vec::new());
        let mut writer = Writer::create(buffer).unwrap();

        // Add mixed entries
        let file_path = ArchivePath::new("normal.txt").unwrap();
        writer.add_bytes(file_path, b"Normal content").unwrap();

        let anti_path = ArchivePath::new("delete_me.txt").unwrap();
        writer.add_anti_item(anti_path).unwrap();

        let (_result, cursor) = writer.finish_into_inner().unwrap();
        let data = cursor.into_inner();

        // Read it back
        let archive = Archive::open(Cursor::new(data)).unwrap();

        // Check entries
        let entries = archive.entries();
        assert_eq!(entries.len(), 2);

        // Normal file
        let normal = &entries[0];
        assert_eq!(normal.path.as_str(), "normal.txt");
        assert!(!normal.is_anti);
        assert!(!normal.is_directory);

        // Anti-item
        let anti = &entries[1];
        assert_eq!(anti.path.as_str(), "delete_me.txt");
        assert!(anti.is_anti);
        assert!(!anti.is_directory);
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_comment_roundtrip() {
        use crate::read::Archive;

        let buffer = Cursor::new(Vec::new());
        let options = WriteOptions::new().comment("Test archive comment with Unicode: 你好世界");
        let mut writer = Writer::create(buffer).unwrap().options(options);

        let file_path = ArchivePath::new("test.txt").unwrap();
        writer.add_bytes(file_path, b"Hello").unwrap();

        let (_result, cursor) = writer.finish_into_inner().unwrap();
        let data = cursor.into_inner();

        // Read it back
        let archive = Archive::open(Cursor::new(data)).unwrap();

        // Verify comment
        let comment = archive.comment();
        assert!(comment.is_some());
        assert_eq!(
            comment.unwrap(),
            "Test archive comment with Unicode: 你好世界"
        );
    }

    #[cfg(feature = "lzma")]
    #[test]
    fn test_no_comment() {
        use crate::read::Archive;

        let buffer = Cursor::new(Vec::new());
        let mut writer = Writer::create(buffer).unwrap();

        let file_path = ArchivePath::new("test.txt").unwrap();
        writer.add_bytes(file_path, b"Hello").unwrap();

        let (_result, cursor) = writer.finish_into_inner().unwrap();
        let data = cursor.into_inner();

        // Read it back
        let archive = Archive::open(Cursor::new(data)).unwrap();

        // Verify no comment
        assert!(archive.comment().is_none());
    }

    #[cfg(feature = "aes")]
    #[test]
    fn test_header_encryption_write() {
        use crate::crypto::Password;
        use crate::format::property_id;

        let buffer = Cursor::new(Vec::new());
        let password = Password::new("secret123");

        // Create archive with encrypted header
        let (result, cursor) = {
            let mut writer = Writer::create(buffer).unwrap().options(
                WriteOptions::new()
                    .password(password.clone())
                    .encrypt_header(true),
            );

            let path = ArchivePath::new("secret.txt").unwrap();
            writer.add_bytes(path, b"Secret content!").unwrap();

            writer.finish_into_inner().unwrap()
        };

        assert_eq!(result.entries_written, 1);
        let archive_data = cursor.into_inner();
        assert!(!archive_data.is_empty());

        // Verify the archive structure:
        // - First 32 bytes are signature header
        // - After compressed data, we should have an ENCODED_HEADER marker
        // Find the header marker
        let header_pos = {
            // Read signature header to get next header offset
            let offset = u64::from_le_bytes(archive_data[12..20].try_into().unwrap());
            32 + offset as usize
        };

        // The header should start with ENCODED_HEADER (0x17)
        assert_eq!(
            archive_data[header_pos],
            property_id::ENCODED_HEADER,
            "Archive should have encrypted header"
        );
    }

    #[cfg(feature = "aes")]
    #[test]
    fn test_header_encryption_without_password() {
        // Verify that encrypt_header(true) without a password does nothing
        let buffer = Cursor::new(Vec::new());

        let (result, cursor) = {
            let mut writer = Writer::create(buffer)
                .unwrap()
                .options(WriteOptions::new().encrypt_header(true)); // No password set

            let path = ArchivePath::new("test.txt").unwrap();
            writer.add_bytes(path, b"Hello").unwrap();

            writer.finish_into_inner().unwrap()
        };

        assert_eq!(result.entries_written, 1);
        let archive_data = cursor.into_inner();

        // Without password, header should NOT be encrypted
        let header_pos = {
            let offset = u64::from_le_bytes(archive_data[12..20].try_into().unwrap());
            32 + offset as usize
        };

        // Should start with regular HEADER marker (0x01), not ENCODED_HEADER (0x17)
        assert_eq!(
            archive_data[header_pos],
            crate::format::property_id::HEADER,
            "Without password, header should not be encrypted"
        );
    }

    #[cfg(all(feature = "aes", feature = "lzma2"))]
    #[test]
    fn test_content_encryption_write_and_read() {
        use crate::crypto::Password;
        use crate::read::Archive;

        let buffer = Cursor::new(Vec::new());
        let password = Password::new("secret_password_123");

        // Create archive with encrypted content
        let (result, cursor) = {
            let mut writer = Writer::create(buffer).unwrap().options(
                WriteOptions::new()
                    .password(password.clone())
                    .encrypt_data(true),
            );

            let path = ArchivePath::new("secret.txt").unwrap();
            writer
                .add_bytes(path, b"This is encrypted content!")
                .unwrap();

            writer.finish_into_inner().unwrap()
        };

        assert_eq!(result.entries_written, 1);
        let archive_data = cursor.into_inner();
        assert!(!archive_data.is_empty());

        // Read the archive back with correct password
        let mut archive =
            Archive::open_with_password(Cursor::new(archive_data.clone()), password.clone())
                .expect("Should open archive with correct password");

        // Extract content to verify
        let extracted = archive
            .extract_to_vec("secret.txt")
            .expect("Should extract encrypted content");

        assert_eq!(extracted, b"This is encrypted content!");
    }
}
