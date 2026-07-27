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
pub(crate) mod codecs;
pub(crate) mod compression;
mod encoding_utils;
mod entry_compression;
mod entry_input;
mod header_compression;
pub(crate) mod header_encode;
mod header_encryption;
mod metadata_encode;
mod streaming_entry;
mod writer_init;

// Re-exports
pub use append::{AppendResult, ArchiveAppender};
pub use options::{EntryMeta, SolidOptions, WriteFilter, WriteOptions, WriteResult};

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
    /// A write failed partway through and the archive cannot be completed.
    ///
    /// Some paths write an entry to the sink as they compress it, so a failure
    /// can leave bytes behind that belong to no folder. Carrying on from there
    /// produces an archive that opens and then fails on some later entry, which
    /// is worse than refusing: the caller has no way to tell it happened.
    Failed,
}

/// Entry data stored for header writing.
#[derive(Debug)]
pub(crate) struct PendingEntry {
    /// Archive path.
    pub(crate) path: ArchivePath,
    /// Entry metadata.
    pub(crate) meta: options::EntryMeta,
    /// Uncompressed size.
    pub(crate) uncompressed_size: u64,
}

/// Entry read into memory and waiting to be compressed.
///
/// Used for both solid blocks, where the entries are compressed together, and
/// for the batch of independent entries a non-solid archive compresses at once.
#[derive(Debug)]
struct BufferedEntry {
    /// Archive path.
    path: ArchivePath,
    /// Entry data (uncompressed).
    data: Vec<u8>,
    /// Entry metadata.
    meta: options::EntryMeta,
    /// CRC32 of uncompressed data.
    crc: u32,
    /// The options this entry was accepted under.
    ///
    /// Carried by the entry rather than by the writer: an entry is compressed
    /// after the fact, and settings kept beside the buffer outlived the entries
    /// they belonged to every time a path emptied the buffer without clearing
    /// them. Here there is nothing to clear - the settings go when the entry
    /// does.
    options: std::sync::Arc<options::WriteOptions>,
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
pub(crate) struct Bcj2FolderInfo {
    /// Sizes of the 4 pack streams [main, call, jump, range]
    pack_sizes: [u64; 4],
}

/// Stream info for pack/unpack info.
#[derive(Debug, Default)]
pub(crate) struct StreamInfo {
    /// Packed sizes for each folder. Most folders have 1, BCJ2 has 4.
    /// For BCJ2 folders, this is empty; use bcj2_folder_info instead.
    pub(crate) pack_sizes: Vec<u64>,
    /// Total unpacked size for each folder.
    pub(crate) unpack_sizes: Vec<u64>,
    /// CRC of each folder's output, where it is known.
    ///
    /// A folder holding several entries has no single meaningful CRC - the
    /// per-entry ones live in SubStreamsInfo - so it records `None` rather than
    /// a zero the header would then declare as a real checksum.
    pub(crate) crcs: Vec<Option<u32>>,
    /// Number of unpack streams in each folder (for solid archives).
    pub(crate) num_unpack_streams_per_folder: Vec<u64>,
    /// Sizes of each substream within solid blocks.
    pub(crate) substream_sizes: Vec<u64>,
    /// CRCs of each substream within solid blocks.
    pub(crate) substream_crcs: Vec<u32>,
    /// Per-folder encryption info (Some if encrypted, None if not).
    #[cfg(feature = "aes")]
    pub(crate) encryption_info: Vec<Option<EncryptedFolderInfo>>,
    /// Per-folder filter info (Some if filtered, None if not).
    pub(crate) filter_info: Vec<Option<FilteredFolderInfo>>,
    /// Per-folder BCJ2 info (Some for BCJ2 folders, None for regular).
    pub(crate) bcj2_folder_info: Vec<Option<Bcj2FolderInfo>>,
    /// The compression method each folder was written with.
    ///
    /// Recorded per folder rather than read from the options when the header is
    /// built: the options can change between entries, and describing every
    /// folder with whichever method was set last produced an archive whose
    /// earlier entries decode as garbage.
    pub(crate) coder_methods: Vec<crate::codec::CodecMethod>,
    /// The compression coder's properties for each folder, as encoded.
    ///
    /// These come back from the encoder that produced the folder rather than
    /// being derived from the options a second time. The dictionary depends on
    /// how much data the folder holds, so a header that recomputes it from the
    /// level alone describes a stream that was never written.
    pub(crate) coder_properties: Vec<Vec<u8>>,
}

/// A 7z archive writer.
pub struct Writer<W> {
    sink: W,
    /// Where this archive begins in the sink.
    ///
    /// Not always nought: a caller may write an archive after a prefix of their
    /// own, and the signature header - written last, by seeking back - has to
    /// land on the archive rather than on whatever precedes it.
    start_pos: u64,
    options: options::WriteOptions,
    state: WriterState,
    entries: Vec<PendingEntry>,
    stream_info: StreamInfo,
    /// Total compressed bytes written.
    compressed_bytes: u64,
    /// Buffer for solid compression.
    solid_buffer: Vec<BufferedEntry>,
    /// Current size of solid buffer (uncompressed bytes).
    solid_buffer_size: u64,
    /// Entries waiting to be compressed together, in non-solid mode.
    ///
    /// Each becomes its own folder, so they are independent and can be
    /// compressed at the same time; they are written back in input order.
    pending_batch: Vec<BufferedEntry>,
    /// Current size of the pending batch (uncompressed bytes).
    pending_batch_size: u64,
    /// The options entries are being accepted under.
    ///
    /// Shared with each buffered entry so that comparing what a waiting entry
    /// belongs to against what is set now is a pointer comparison, and so that
    /// replacing the options cannot reach back over work already accepted.
    active_options: std::sync::Arc<options::WriteOptions>,
    /// The path of the entry added last, when order is being enforced.
    ///
    /// Only used for `deterministic`, which checks the caller's order rather
    /// than rearranging entries the writer has already committed to.
    last_path: Option<String>,
    /// Salt for this archive's encryption key, generated once.
    ///
    /// Every stream is encrypted under the same password, and the salt is what
    /// selects the key; a per-stream salt would mean re-deriving the key for
    /// every stream, which costs 2^19 SHA-256 rounds each and buys nothing.
    #[cfg(feature = "aes")]
    archive_salt: Option<Vec<u8>>,
    /// The password every encrypted stream in this archive is keyed on.
    ///
    /// One archive has one key: the salt is generated once and the header is
    /// encrypted with whatever is set at `finish`. Letting the password change
    /// between entries produced an archive no single password opens - the
    /// first entry under one, the header under another - which reads as
    /// corruption rather than as the mistake it is.
    #[cfg(feature = "aes")]
    archive_password: Option<crate::crypto::Password>,
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

    /// The encrypted header must be stored the way every 7z implementation
    /// expects to find it: as a packed stream in the data area, addressed by an
    /// absolute `PackPos`, with the structure that describes it written after it.
    ///
    /// Storing the payload inline behind the structure and recording
    /// `PackPos = 0` round-trips fine through our own reader while making the
    /// archive unopenable everywhere else, so the layout is asserted directly.
    #[cfg(feature = "aes")]
    #[test]
    fn test_encrypted_header_layout_is_standard() {
        use crate::crypto::Password;
        use crate::format::property_id;
        use crate::format::reader::read_variable_u64;

        const SIGNATURE_HEADER_SIZE: u64 = 32;

        let buffer = Cursor::new(Vec::new());
        let (_result, cursor) = {
            let mut writer = Writer::create(buffer).unwrap().options(
                WriteOptions::new()
                    .password(Password::new("secret123"))
                    .encrypt_header(true),
            );
            writer
                .add_bytes(ArchivePath::new("secret.txt").unwrap(), b"Secret content!")
                .unwrap();
            writer.finish_into_inner().unwrap()
        };
        let archive = cursor.into_inner();

        let next_header_offset = u64::from_le_bytes(archive[12..20].try_into().unwrap());
        let next_header_size = u64::from_le_bytes(archive[20..28].try_into().unwrap());
        let structure_start = (SIGNATURE_HEADER_SIZE + next_header_offset) as usize;

        assert_eq!(archive[structure_start], property_id::ENCODED_HEADER);
        assert_eq!(archive[structure_start + 1], property_id::PACK_INFO);

        let mut rest = &archive[structure_start + 2..];
        let pack_pos = read_variable_u64(&mut rest).unwrap();
        let num_pack_streams = read_variable_u64(&mut rest).unwrap();
        assert_eq!(num_pack_streams, 1);
        assert_eq!(rest[0], property_id::SIZE);
        rest = &rest[1..];
        let pack_size = read_variable_u64(&mut rest).unwrap();

        assert!(
            pack_pos > 0,
            "the packed header stream cannot start at the beginning of the data area, \
             where the file data lives"
        );
        assert_eq!(
            SIGNATURE_HEADER_SIZE + pack_pos + pack_size,
            structure_start as u64,
            "the packed header stream must end exactly where the structure describing it begins"
        );
        assert_eq!(
            structure_start as u64 + next_header_size,
            archive.len() as u64,
            "the next header is the structure alone, not the structure plus its payload"
        );
    }

    /// Asking for encryption without a password must fail, not write plaintext.
    #[cfg(feature = "aes")]
    #[test]
    fn test_encryption_without_password_is_rejected() {
        for options in [
            WriteOptions::new().encrypt_header(true),
            WriteOptions::new().encrypt_data(true),
        ] {
            let mut writer = Writer::create(Cursor::new(Vec::new()))
                .unwrap()
                .options(options);

            let error = writer
                .add_bytes(ArchivePath::new("test.txt").unwrap(), b"Hello")
                .expect_err("encryption without a password must be rejected");

            assert!(
                error.to_string().contains("without a password"),
                "unexpected error: {error}"
            );
        }
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
