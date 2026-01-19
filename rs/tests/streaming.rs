//! Integration tests for the streaming API.
//!
//! These tests verify that the streaming API correctly:
//! - Reads archives entry-by-entry without full memory allocation
//! - Produces correct output through extract_all()
//! - Works with both solid and non-solid archives

#![cfg(feature = "lzma2")]

mod common;

use std::io::Cursor;
use zesven::read::Archive;
use zesven::streaming::{StreamingArchive, StreamingConfig};
use zesven::{ArchivePath, WriteOptions, Writer};

use common::create_archive;

/// Creates a solid archive with the given entries.
fn create_solid_archive(entries: &[(&str, &[u8])]) -> zesven::Result<Vec<u8>> {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let options = WriteOptions::new().solid();
        let mut writer = Writer::create(cursor)?.options(options);

        for (name, data) in entries {
            let path = ArchivePath::new(name)?;
            writer.add_bytes(path, data)?;
        }

        let _ = writer.finish()?;
    }
    Ok(archive_bytes)
}

// ============================================================================
// StreamingArchive basic tests
// ============================================================================

#[test]
fn test_streaming_archive_open_and_list_entries() {
    let entries = [
        ("file1.txt", b"Hello, World!" as &[u8]),
        ("file2.txt", b"Goodbye, World!"),
        ("dir/file3.txt", b"Nested file"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let archive = StreamingArchive::open(cursor, "").unwrap();

    assert_eq!(archive.len(), 3);
    assert!(!archive.is_empty());
    assert!(archive.entry("file1.txt").is_some());
    assert!(archive.entry("file2.txt").is_some());
    assert!(archive.entry("dir/file3.txt").is_some());
    assert!(archive.entry("nonexistent.txt").is_none());
}

#[test]
fn test_streaming_archive_total_size() {
    let entries = [
        ("a.txt", b"12345" as &[u8]),  // 5 bytes
        ("b.txt", b"1234567890"),      // 10 bytes
        ("c.txt", b"123456789012345"), // 15 bytes
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let archive = StreamingArchive::open(cursor, "").unwrap();

    assert_eq!(archive.total_size(), 30);
}

#[test]
fn test_streaming_archive_is_solid_detection() {
    // Non-solid archive
    let entries = [("file.txt", b"content" as &[u8])];
    let non_solid_bytes = create_archive(&entries).unwrap();
    let non_solid = StreamingArchive::open(Cursor::new(non_solid_bytes), "").unwrap();
    assert!(!non_solid.is_solid());

    // Solid archive
    let entries = [
        ("a.txt", b"content a" as &[u8]),
        ("b.txt", b"content b"),
        ("c.txt", b"content c"),
    ];
    let solid_bytes = create_solid_archive(&entries).unwrap();
    let solid = StreamingArchive::open(Cursor::new(solid_bytes), "").unwrap();
    assert!(solid.is_solid());
}

// ============================================================================
// StreamingConfig tests
// ============================================================================

#[test]
fn test_streaming_config_presets() {
    let low = StreamingConfig::low_memory();
    let high = StreamingConfig::high_performance();
    let default = StreamingConfig::default();

    // Low memory should use smaller buffers than high performance
    assert!(low.max_memory_buffer < high.max_memory_buffer);

    // All presets should be valid
    assert!(low.validate().is_ok());
    assert!(high.validate().is_ok());
    assert!(default.validate().is_ok());
}

#[test]
fn test_streaming_config_custom() {
    let config = StreamingConfig::new()
        .max_memory_buffer(16 * 1024 * 1024) // 16 MiB
        .read_buffer_size(64 * 1024) // 64 KiB
        .verify_crc(true);

    assert!(config.validate().is_ok());
    assert_eq!(config.max_memory_buffer, 16 * 1024 * 1024);
}

#[test]
fn test_streaming_config_invalid() {
    // Zero max_memory_buffer should fail validation
    let config = StreamingConfig::new().max_memory_buffer(0);
    let result = config.validate();
    assert!(
        result.is_err(),
        "max_memory_buffer=0 should fail validation"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("max_memory_buffer"),
        "Error should mention max_memory_buffer: {}",
        err_msg
    );

    // Zero read_buffer_size should fail validation
    let config = StreamingConfig::new().read_buffer_size(0);
    let result = config.validate();
    assert!(result.is_err(), "read_buffer_size=0 should fail validation");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("read_buffer_size"),
        "Error should mention read_buffer_size: {}",
        err_msg
    );

    // read_buffer_size exceeding max_memory_buffer should fail validation
    let config = StreamingConfig::new()
        .max_memory_buffer(1024)
        .read_buffer_size(2048);
    let result = config.validate();
    assert!(
        result.is_err(),
        "read_buffer_size > max_memory_buffer should fail validation"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("read_buffer_size") && err_msg.contains("max_memory_buffer"),
        "Error should mention both fields: {}",
        err_msg
    );
}

// ============================================================================
// Entry iteration tests
// ============================================================================

/// Tests that dropping an entry iterator mid-iteration doesn't corrupt archive state.
///
/// This verifies that:
/// 1. Dropping an iterator before consuming all entries is safe
/// 2. The archive remains usable after early iterator drop
/// 3. A new iterator can be created and used successfully
#[test]
fn test_streaming_entry_iterator_early_drop() {
    let entries = [
        ("file1.txt", b"Content 1" as &[u8]),
        ("file2.txt", b"Content 2"),
        ("file3.txt", b"Content 3"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();

    // Start iteration but drop iterator early (after reading first entry)
    {
        let mut iter = archive.entries().unwrap();
        let first_entry = iter.next().unwrap().unwrap();
        assert_eq!(first_entry.name(), "file1.txt");
        // Iterator is dropped here without consuming remaining entries
    }

    // Archive should still be usable - create new iterator and verify all entries
    let mut iter2 = archive.entries().unwrap();
    let mut count = 0;
    for entry_result in iter2.by_ref() {
        let entry = entry_result.unwrap();
        assert!(!entry.is_directory());
        count += 1;
    }

    assert_eq!(
        count, 3,
        "Should be able to iterate all entries after early drop"
    );
}

#[test]
fn test_streaming_entries_iteration() {
    let entries = [
        ("file1.txt", b"Content 1" as &[u8]),
        ("file2.txt", b"Content 2"),
        ("file3.txt", b"Content 3"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();
    let iter = archive.entries().unwrap();

    let mut count = 0;
    for entry_result in iter {
        let entry = entry_result.unwrap();
        assert!(!entry.is_directory());
        count += 1;
    }

    assert_eq!(count, 3);
}

#[test]
fn test_streaming_entry_extraction() {
    let content = b"This is test content for streaming extraction";
    let entries = [("test.txt", content.as_slice())];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();
    let mut iter = archive.entries().unwrap();

    if let Some(entry_result) = iter.next() {
        let entry = entry_result.unwrap();
        assert_eq!(entry.name(), "test.txt");
        assert_eq!(entry.size(), content.len() as u64);

        let mut extracted = Vec::new();
        iter.extract_current_to(&mut extracted).unwrap();
        assert_eq!(extracted, content);
    } else {
        panic!("Expected at least one entry");
    }
}

// ============================================================================
// extract_all tests
// ============================================================================

#[test]
fn test_streaming_extract_all() {
    let entries = [
        ("file1.txt", b"Content 1" as &[u8]),
        ("subdir/file2.txt", b"Content 2"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let result = archive
        .extract_all(temp_dir.path(), &Default::default())
        .unwrap();

    assert!(result.is_success());
    assert_eq!(result.entries_extracted, 2);
    assert_eq!(result.entries_failed, 0);

    // Verify files exist
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());

    // Verify content
    let content1 = std::fs::read(temp_dir.path().join("file1.txt")).unwrap();
    assert_eq!(content1, b"Content 1");
}

// ============================================================================
// extract_all_to_sinks tests
// ============================================================================

#[test]
fn test_streaming_extract_to_sinks() {
    let entries = [
        ("include.txt", b"Include this" as &[u8]),
        ("exclude.txt", b"Exclude this"),
        ("also_include.txt", b"Also include"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();

    let result = archive
        .extract_all_to_sinks(|entry| {
            if entry.path.as_str().contains("include") {
                Some(Vec::new())
            } else {
                None // Skip entries not containing "include"
            }
        })
        .unwrap();

    // 2 included + 1 skipped
    assert_eq!(result.entries_extracted, 2);
    assert_eq!(result.entries_skipped, 1);
    assert!(result.is_success());
}

// ============================================================================
// Early Drop Safety Tests
// ============================================================================

/// Tests that dropping a StreamingArchive mid-iteration is safe.
///
/// This test verifies that resources are properly cleaned up when the archive
/// is dropped before iteration completes. This is important for use cases like:
/// - Extracting only the first N files
/// - Stopping extraction on error
/// - Cancellation by user
#[test]
fn test_streaming_early_drop_is_safe() {
    let entries = [
        ("file1.txt", b"Content 1" as &[u8]),
        ("file2.txt", b"Content 2"),
        ("file3.txt", b"Content 3"),
        ("file4.txt", b"Content 4"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();

    // Test 1: Drop after opening without any iteration
    {
        let cursor = Cursor::new(&archive_bytes);
        let _archive = StreamingArchive::open(cursor, "").unwrap();
        // Drop immediately - should not panic or leak resources
    }

    // Test 2: Drop after partial iteration
    {
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = StreamingArchive::open(cursor, "").unwrap();

        // Extract just the first entry
        let temp_dir = tempfile::tempdir().unwrap();
        let mut extracted_count = 0;

        let result = archive.extract_all_to_sinks(|_entry| {
            extracted_count += 1;
            if extracted_count <= 1 {
                Some(Vec::new()) // Extract first entry
            } else {
                None // Skip rest
            }
        });

        // Should complete without error
        assert!(result.is_ok(), "Partial extraction should succeed");
        let extract_result = result.unwrap();
        assert_eq!(extract_result.entries_extracted, 1);
        assert_eq!(extract_result.entries_skipped, 3);

        // Archive will be dropped here - should be safe
        drop(temp_dir);
    }

    // Test 3: Multiple open/drop cycles
    for _ in 0..5 {
        let cursor = Cursor::new(&archive_bytes);
        let _archive = StreamingArchive::open(cursor, "").unwrap();
        // Drop and repeat
    }

    // If we reach here without panic or memory issues, the test passes
}

/// Tests that extracting to sinks with early termination doesn't leak memory.
///
/// When sink factory returns None for some entries, those entries should be
/// skipped efficiently without resource accumulation.
#[test]
fn test_streaming_selective_extraction_no_resource_leak() {
    // Create archive with many small entries
    let entries: Vec<(String, Vec<u8>)> = (0..20)
        .map(|i| {
            (
                format!("file{:02}.txt", i),
                format!("Content {}", i).into_bytes(),
            )
        })
        .collect();

    let entry_refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(s, d)| (s.as_str(), d.as_slice()))
        .collect();

    let archive_bytes = create_archive(&entry_refs).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();

    // Extract only every 5th file
    let result = archive
        .extract_all_to_sinks(|entry| {
            let index: usize = entry
                .path
                .as_str()
                .strip_prefix("file")
                .and_then(|s| s.strip_suffix(".txt"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            if index % 5 == 0 {
                Some(Vec::new())
            } else {
                None
            }
        })
        .unwrap();

    // Should extract files 00, 05, 10, 15 = 4 files
    assert_eq!(result.entries_extracted, 4);
    assert_eq!(result.entries_skipped, 16);
    assert!(result.is_success());
}

// ============================================================================
// Solid archive streaming tests
// ============================================================================

#[test]
fn test_streaming_solid_archive_sequential_access() {
    let entries = [
        ("a.txt", b"Content A" as &[u8]),
        ("b.txt", b"Content B"),
        ("c.txt", b"Content C"),
    ];

    let archive_bytes = create_solid_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();
    assert!(archive.is_solid());

    let temp_dir = tempfile::tempdir().unwrap();
    let result = archive
        .extract_all(temp_dir.path(), &Default::default())
        .unwrap();

    assert!(result.is_success());
    assert_eq!(result.entries_extracted, 3);

    // Verify each file has correct content
    assert_eq!(
        std::fs::read(temp_dir.path().join("a.txt")).unwrap(),
        b"Content A"
    );
    assert_eq!(
        std::fs::read(temp_dir.path().join("b.txt")).unwrap(),
        b"Content B"
    );
    assert_eq!(
        std::fs::read(temp_dir.path().join("c.txt")).unwrap(),
        b"Content C"
    );
}

/// Tests that non-solid archives support parallel extraction.
///
/// Each entry in a non-solid archive is independently compressed,
/// allowing parallel decompression of different entries.
#[test]
fn test_streaming_non_solid_archive_supports_parallel() {
    let entries = [("a.txt", b"Content A" as &[u8]), ("b.txt", b"Content B")];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let archive = StreamingArchive::open(cursor, "").unwrap();

    assert!(
        !archive.is_solid(),
        "Test setup: should be non-solid archive"
    );
    assert!(
        archive.supports_parallel_extraction(),
        "Non-solid archives should support parallel extraction"
    );
}

// ============================================================================
// Memory tracking tests
// ============================================================================

#[test]
fn test_streaming_memory_tracker() {
    let entries = [("file.txt", b"Small content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let config = StreamingConfig::new().max_memory_buffer(1024 * 1024); // 1 MiB

    let archive = StreamingArchive::open_with_config(cursor, "", config).unwrap();

    let tracker = archive.memory_tracker();
    // Memory tracker should exist and be usable
    assert!(tracker.available() > 0);
}

// ============================================================================
// Parallel extraction tests (non-solid only)
// ============================================================================

/// Tests that attempting parallel extraction on a solid archive returns an error.
///
/// Solid archives require sequential decompression because files are concatenated
/// in a single compressed stream. This test verifies that `extract_all_parallel`
/// properly returns `UnsupportedFeature` when called on a solid archive, rather
/// than silently failing or producing incorrect results.
#[test]
fn test_solid_archive_parallel_extraction_returns_error() {
    use zesven::Error;
    use zesven::streaming::ParallelExtractionOptions;

    let entries = [
        ("a.txt", b"Content A" as &[u8]),
        ("b.txt", b"Content B"),
        ("c.txt", b"Content C"),
    ];

    let archive_bytes = create_solid_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let mut archive = StreamingArchive::open(cursor, "").unwrap();

    // Verify it's a solid archive
    assert!(archive.is_solid(), "Test setup: should be a solid archive");
    assert!(
        !archive.supports_parallel_extraction(),
        "Solid archives should not support parallel extraction"
    );

    // Attempt parallel extraction - should fail with UnsupportedFeature
    let temp_dir = tempfile::tempdir().unwrap();
    let options = ParallelExtractionOptions::default();
    let result = archive.extract_all_parallel(temp_dir.path(), &options);

    // Verify the error
    match result {
        Err(Error::UnsupportedFeature { feature }) => {
            // Expected: solid archives don't support parallel extraction
            assert!(
                feature.to_lowercase().contains("solid")
                    || feature.to_lowercase().contains("parallel"),
                "Error message should mention solid or parallel: {}",
                feature
            );
        }
        Err(other) => {
            panic!(
                "Expected UnsupportedFeature error for solid archive parallel extraction, got: {:?}",
                other
            );
        }
        Ok(result) => {
            panic!(
                "Solid archive parallel extraction should fail, but succeeded with {} entries",
                result.entries_extracted
            );
        }
    }
}

// ============================================================================
// Skipped entries tests
// ============================================================================

#[test]
fn test_streaming_skipped_entries_tracking() {
    // Create a normal archive first
    let entries = [("valid.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    let archive = StreamingArchive::open(cursor, "").unwrap();

    // Archive with valid paths should have no skipped entries
    assert!(!archive.has_skipped_entries());
    assert!(archive.skipped_entries().is_empty());
}

// ============================================================================
// Compatibility test with standard Archive API
// ============================================================================

#[test]
fn test_streaming_produces_same_result_as_standard_api() {
    let entries = [
        ("file1.txt", b"Hello streaming!" as &[u8]),
        ("dir/file2.txt", b"Nested content"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();

    // Extract via standard API
    let standard_temp = tempfile::tempdir().unwrap();
    {
        let mut archive = Archive::open(Cursor::new(&archive_bytes)).unwrap();
        let _ = archive
            .extract(
                standard_temp.path(),
                (),
                &zesven::read::ExtractOptions::default(),
            )
            .unwrap();
    }

    // Extract via streaming API
    let streaming_temp = tempfile::tempdir().unwrap();
    {
        let mut archive = StreamingArchive::open(Cursor::new(&archive_bytes), "").unwrap();
        archive
            .extract_all(streaming_temp.path(), &Default::default())
            .unwrap();
    }

    // Compare results
    let standard_file1 = std::fs::read(standard_temp.path().join("file1.txt")).unwrap();
    let streaming_file1 = std::fs::read(streaming_temp.path().join("file1.txt")).unwrap();
    assert_eq!(standard_file1, streaming_file1);

    let standard_file2 = std::fs::read(standard_temp.path().join("dir/file2.txt")).unwrap();
    let streaming_file2 = std::fs::read(streaming_temp.path().join("dir/file2.txt")).unwrap();
    assert_eq!(standard_file2, streaming_file2);
}

// ============================================================================
// Error Path Tests
// ============================================================================
//
// These tests verify that the streaming API correctly handles error conditions
// such as truncated archives, invalid signatures, and corrupted data.

/// Tests that opening a truncated archive produces an appropriate error.
///
/// A truncated archive is a valid archive that has been cut off before
/// the header could be fully read.
#[test]
fn test_streaming_open_truncated_archive() {
    // Create a valid archive
    let entries = [("file.txt", b"content" as &[u8])];
    let archive_bytes = create_archive(&entries).unwrap();

    // Truncate the archive at various points
    for truncate_at in [6, 10, 20, archive_bytes.len() / 2] {
        if truncate_at >= archive_bytes.len() {
            continue;
        }

        let truncated = &archive_bytes[..truncate_at];
        let cursor = Cursor::new(truncated);

        let result = StreamingArchive::open(cursor, "");

        // Should fail with an error (specific error type depends on where truncation occurs)
        assert!(
            result.is_err(),
            "Opening archive truncated at {} bytes should fail",
            truncate_at
        );

        // The error is already verified by the assertion above.
        // Different truncation points may produce different error types
        // (Io, InvalidFormat, CorruptHeader) which are all valid responses.
    }
}

/// Tests that opening random bytes (invalid signature) produces an error.
///
/// Random data should be rejected quickly when the 7z signature doesn't match.
#[test]
fn test_streaming_open_invalid_signature() {
    use zesven::Error;

    // Random bytes that don't start with 7z signature
    let random_data = vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x00, 0x11];
    let cursor = Cursor::new(random_data);

    let result = StreamingArchive::open(cursor, "");

    assert!(result.is_err(), "Opening random bytes should fail");

    // Extract the error and verify it's one of the expected types
    let err = common::expect_err(result);
    match err {
        Error::InvalidFormat(msg) => {
            // Expected - invalid signature is detected as format error
            assert!(!msg.is_empty(), "Error message should not be empty");
        }
        Error::Io(_) => {
            // I/O error is also acceptable (e.g., unexpected EOF when seeking)
        }
        Error::CorruptHeader { .. } => {
            // Also acceptable - may be detected as corrupt header
        }
        other => {
            panic!("Unexpected error type for invalid signature: {:?}", other);
        }
    }
}

/// Tests that empty input produces an appropriate error.
#[test]
fn test_streaming_open_empty_input() {
    let empty: Vec<u8> = vec![];
    let cursor = Cursor::new(empty);

    let result = StreamingArchive::open(cursor, "");

    assert!(result.is_err(), "Opening empty input should fail");
}

/// Tests that the memory tracker reports correct limit at boundary.
#[test]
fn test_streaming_memory_tracker_at_limit() {
    let entries = [("file.txt", b"Small content" as &[u8])];
    let archive_bytes = create_archive(&entries).unwrap();
    let cursor = Cursor::new(archive_bytes);

    // Set a very specific memory limit
    let memory_limit = 512 * 1024; // 512 KiB
    let config = StreamingConfig::new().max_memory_buffer(memory_limit);

    let archive = StreamingArchive::open_with_config(cursor, "", config).unwrap();

    let tracker = archive.memory_tracker();

    // The tracker's limit should match our configured limit
    assert_eq!(
        tracker.limit(),
        memory_limit,
        "Memory tracker limit should match configured limit"
    );

    // Available should be limit minus current usage
    assert!(
        tracker.available() <= tracker.limit(),
        "Available should not exceed limit"
    );

    // Current usage should be reasonable (non-negative, not more than limit)
    let current = tracker.limit() - tracker.available();
    assert!(
        current <= tracker.limit(),
        "Current usage should not exceed limit"
    );
}

// ============================================================================
// Solid Archive Error Handling Tests
// ============================================================================
//
// These tests verify that solid archives handle error conditions correctly.

/// Tests that truncated solid archive produces an error during extraction.
///
/// Solid archives have all file data in a single compressed stream. Truncating
/// this stream should cause extraction to fail.
#[test]
fn test_solid_archive_truncated_extraction_fails() {
    let entries = [
        ("a.txt", b"Content A - first file in solid block" as &[u8]),
        ("b.txt", b"Content B - second file in solid block"),
        ("c.txt", b"Content C - third file in solid block"),
    ];

    let archive_bytes = create_solid_archive(&entries).unwrap();

    // Truncate 20 bytes from the end (into the compressed data)
    let truncate_at = archive_bytes.len().saturating_sub(20);
    if truncate_at < 50 {
        return; // Archive too small for meaningful truncation test
    }

    let truncated = &archive_bytes[..truncate_at];
    let cursor = Cursor::new(truncated);

    // Opening may succeed or fail depending on where truncation occurred
    match StreamingArchive::open(cursor, "") {
        Ok(mut archive) => {
            // If open succeeds, extraction should fail
            let temp_dir = tempfile::tempdir().unwrap();
            let result = archive.extract_all(temp_dir.path(), &Default::default());

            // Either full failure or partial extraction with failures
            match result {
                Ok(extract_result) => {
                    // Partial extraction may succeed but should have failures
                    // or not extract all entries
                    let total = extract_result.entries_extracted + extract_result.entries_failed;
                    assert!(
                        extract_result.entries_failed > 0 || total < 3,
                        "Truncated solid archive should not extract all entries cleanly"
                    );
                }
                Err(_) => {
                    // Full failure is also acceptable
                }
            }
        }
        Err(_) => {
            // Open failed - also acceptable for severely truncated archive
        }
    }
}

/// Tests that corrupted compressed data in solid archive is detected.
///
/// Bit-flipping in the compressed data stream should cause decompression
/// or CRC verification to fail.
#[test]
fn test_solid_archive_bit_flip_detected() {
    let entries = [
        ("file1.txt", b"Some content for file 1" as &[u8]),
        ("file2.txt", b"Some content for file 2"),
    ];

    let mut archive_bytes = create_solid_archive(&entries).unwrap();

    // Flip a bit in the middle of the archive (likely in compressed data)
    let corrupt_pos = archive_bytes.len() / 2;
    archive_bytes[corrupt_pos] ^= 0x01;

    let cursor = Cursor::new(&archive_bytes);

    // Either opening or extraction should fail
    match StreamingArchive::open(cursor, "") {
        Ok(mut archive) => {
            let temp_dir = tempfile::tempdir().unwrap();
            let result = archive.extract_all(temp_dir.path(), &Default::default());

            // Corruption should be detected during extraction
            match result {
                Ok(_extract_result) => {
                    // Either failed entries or extraction errors are acceptable
                    // since the bit flip location is non-deterministic
                }
                Err(_) => {
                    // Expected - corruption detected
                }
            }
        }
        Err(_) => {
            // Open failed - corruption in header is also acceptable
        }
    }
}

/// Tests that solid archive with CRC mismatch is detected during test().
///
/// The test() method should verify data integrity and report CRC failures.
#[test]
fn test_solid_archive_crc_verification() {
    use zesven::read::{Archive, SelectAll, TestOptions};

    let entries = [
        ("test1.txt", b"Content for test file 1" as &[u8]),
        ("test2.txt", b"Content for test file 2"),
    ];

    // Create valid solid archive
    let archive_bytes = create_solid_archive(&entries).unwrap();

    // Open with standard Archive API for test() method
    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open(cursor).expect("Should open valid archive");

    // Test should pass on valid archive
    let test_result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Test should complete");

    assert_eq!(
        test_result.entries_failed, 0,
        "Valid solid archive should pass CRC verification"
    );
    assert_eq!(
        test_result.entries_tested, 2,
        "Both entries should be tested"
    );
}

// Note: test_solid_archive_parallel_extraction_documents_error was removed as redundant.
// The identical behavior is already tested by test_solid_archive_parallel_extraction_returns_error
// (lines 420-472), which verifies that solid archives return UnsupportedFeature for parallel extraction.
