//! Round-trip integration tests for zesven.
//!
//! These tests verify edge cases and special scenarios for archive round-trips.
//! Basic codec roundtrip tests are in `codec_combinations.rs`.
//!
//! This file focuses on:
//! - Empty archives
//! - Unicode filenames
//! - Deep directory structures
//! - Directory entries
//! - Memory destination extraction

// These tests require LZMA support (default compression method for writing)
#![cfg(feature = "lzma")]

mod common;

use std::io::Cursor;
use zesven::read::Archive;
use zesven::{ArchivePath, Writer};

// Note: verify_archive_contents and create_archive_with_result are provided by common module.
// We use create_archive_with_result directly when WriteResult is needed.

#[test]
fn test_empty_archive() {
    let (archive_bytes, result) =
        common::create_archive_with_result(None, &[]).expect("Failed to create test archive");

    assert_eq!(result.entries_written, 0);
    assert_eq!(result.directories_written, 0);

    // Check signature
    assert!(archive_bytes.len() >= 32);
    assert_eq!(&archive_bytes[0..6], &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
}

// Note: Several tests were removed as they duplicate coverage in codec_combinations.rs:
// - test_single_small_file -> test_*_text()
// - test_multiple_files -> test_*_multiple_files()
// - test_empty_file -> test_copy_empty_file()
// - test_binary_data -> test_*_binary()
// - test_unicode_filename -> test_unicode_paths() (covers Japanese, Chinese, Greek, Emoji)
// - test_deterministic_mode -> test_deterministic_archives_identical()

#[test]
fn test_deep_directory_structure() {
    let data = b"Deeply nested file";
    let entries = [("a/b/c/d/e/f/g/deep.txt", data.as_slice())];
    let (archive_bytes, result) =
        common::create_archive_with_result(None, &entries).expect("Failed to create test archive");

    assert_eq!(result.entries_written, 1);

    // Verify read-back (deep paths should survive round-trip)
    common::verify_archive_contents(&archive_bytes, &entries);
}

#[test]
fn test_with_directory_entry() {
    use zesven::write::EntryMeta;

    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();

        // Add a directory
        let dir_path = ArchivePath::new("mydir").unwrap();
        writer
            .add_directory(dir_path, EntryMeta::directory())
            .unwrap();

        // Add a file in that directory
        let file_path = ArchivePath::new("mydir/file.txt").unwrap();
        writer.add_bytes(file_path, b"content").unwrap();

        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 1); // Only file counts as entry
        assert_eq!(result.directories_written, 1);
    }
}

// Note: Codec-specific modules (lzma_tests, deflate_tests, bzip2_tests) were removed.
// More thorough codec roundtrip tests exist in codec_combinations.rs which:
// - Tests multiple data types (text, binary, random, repetitive)
// - Verifies actual content extraction, not just compression stats
// - Tests multiple compression levels

#[test]
fn test_memory_destination_extraction() {
    use zesven::read::MemoryDestination;

    // Create an archive with multiple files
    let entries = [
        ("file1.txt", b"Hello, World!" as &[u8]),
        ("file2.txt", b"Second file content"),
        ("dir/nested.txt", b"Nested file data"),
    ];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create test archive");

    // Extract to memory destination
    let cursor = Cursor::new(archive_bytes);
    let mut archive = Archive::open(cursor).expect("Failed to open archive");

    let mut dest = MemoryDestination::new();
    let result = archive
        .extract_to_destination(&mut dest)
        .expect("Failed to extract to memory destination");

    assert_eq!(result.entries_extracted, 3);
    assert_eq!(result.entries_failed, 0);

    // Verify extracted contents
    assert_eq!(dest.len(), 3);

    for (path, expected_data) in &entries {
        let extracted = dest
            .get(path)
            .unwrap_or_else(|| panic!("Missing file: {}", path));
        assert_eq!(extracted, *expected_data, "Content mismatch for '{}'", path);
    }

    // Test into_files()
    let files = dest.into_files();
    assert_eq!(files.len(), 3);
}

/// Tests that extraction produces correct results (verifies content correctness).
///
/// This test uses parallel extraction to verify content integrity after extraction.
/// A separate single-threaded test was removed as redundant - content correctness
/// is thread-agnostic (thread count doesn't affect data integrity). Thread-safety
/// testing requires data race detection tools, not content verification.
#[test]
fn test_parallel_extraction_produces_correct_results() {
    use zesven::read::{ExtractOptions, Threads};

    // Create an archive with multiple files to test extraction
    let entries = [
        ("file1.txt", b"Content of file 1" as &[u8]),
        ("file2.txt", b"Content of file 2"),
        ("file3.txt", b"Content of file 3"),
        ("dir/file4.txt", b"Nested content 4"),
        ("dir/file5.txt", b"Nested content 5"),
    ];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create test archive");

    // Extract with explicit thread count (4 threads)
    let temp_dir = tempfile::tempdir().unwrap();
    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open(cursor).expect("Failed to open archive");

    let options = ExtractOptions::new().threads(Threads::count_or_single(4));

    let result = archive
        .extract(temp_dir.path(), (), &options)
        .expect("Parallel extraction failed");

    assert_eq!(result.entries_extracted, 5);
    assert_eq!(result.entries_failed, 0);

    // Verify all files have correct content
    for (path, expected_content) in &entries {
        let extracted_path = temp_dir.path().join(path);
        assert!(
            extracted_path.exists(),
            "File {} should exist after extraction",
            path
        );

        let content = std::fs::read(&extracted_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
        assert_eq!(
            content.as_slice(),
            *expected_content,
            "Content mismatch for {} after extraction",
            path
        );
    }
}
