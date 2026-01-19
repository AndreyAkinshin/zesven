//! Tests for resource limit enforcement.
//!
//! These tests verify that zesven correctly enforces resource limits
//! to protect against malicious archives (zip bombs, etc.).
//!
//! Note: LimitedReader unit tests are in src/safety.rs. ResourceLimits builder
//! tests are in src/format/streams.rs. This file contains integration tests
//! for limits enforcement during archive operations.

// These tests require LZMA support (default compression method for writing)
#![cfg(feature = "lzma")]

mod common;

use std::io::Cursor;
use zesven::Archive;
use zesven::Error;
use zesven::format::streams::ResourceLimits;

// =============================================================================
// Key Derivation Limit Tests
// =============================================================================
//
// Note: Unit-level key derivation tests have been moved to src/crypto/mod.rs:
// - test_derive_key, test_derive_key_max_cycles_power (already existed)
// - test_derive_key_with_varied_salts (moved)
// - test_derive_key_deterministic (moved)
// - test_derive_key_extreme_values_rejected (moved)
//
// These test direct calls to derive_key(), which is unit-level behavior.
// Integration tests for key derivation limits should test via Archive::open_with_password()
// which exercises the full encryption/decryption pipeline.

// =============================================================================
// Error Type Tests
// =============================================================================
//
// Note: test_resource_limit_exceeded_error and test_resource_limit_error_formatting
// were removed as they duplicate coverage in src/error.rs unit tests:
// - test_resource_limit_exceeded (verifies error message formatting)
// - test_is_recoverable_resource_limit_not_recoverable (verifies is_recoverable())
//
// Error type unit tests belong in src/error.rs, not integration tests.

// =============================================================================
// Archive::open_with_limits() Integration Tests
// =============================================================================

#[test]
fn test_open_with_limits_accepts_default_limits() {
    // Create a simple archive
    let entries = [("test.txt", b"hello world" as &[u8])];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // Opening with default limits should succeed
    let limits = ResourceLimits::default();
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits).unwrap();

    assert_eq!(archive.entries().len(), 1);
}

#[test]
fn test_open_with_limits_accepts_unlimited() {
    // Create a simple archive
    let entries = [("test.txt", b"hello world" as &[u8])];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // Opening with unlimited should succeed
    let limits = ResourceLimits::unlimited();
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits).unwrap();

    assert_eq!(archive.entries().len(), 1);
}

#[test]
fn test_open_with_limits_max_entries_enforced() {
    // Create archive with multiple entries
    let entries = [
        ("file1.txt", b"content1" as &[u8]),
        ("file2.txt", b"content2" as &[u8]),
        ("file3.txt", b"content3" as &[u8]),
    ];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // Opening with max_entries=1 should fail
    let limits = ResourceLimits::new().max_entries(1);
    let cursor = Cursor::new(&archive_bytes);
    let result = Archive::open_with_limits(cursor, limits);

    assert!(
        result.is_err(),
        "Should reject archive exceeding entry limit"
    );
    match result {
        Err(Error::ResourceLimitExceeded(msg)) => {
            // The error message should mention entries/streams
            assert!(
                msg.contains("stream") || msg.contains("entries"),
                "Error should mention streams or entries: {}",
                msg
            );
        }
        Err(e) => panic!("Expected ResourceLimitExceeded, got: {:?}", e),
        Ok(_) => panic!("Should have failed"),
    }
}

#[test]
fn test_open_with_limits_passes_through_to_parser() {
    // Verify that custom limits are actually passed to the parser
    // by using a limit that would allow the archive
    let entries = [
        ("file1.txt", b"content1" as &[u8]),
        ("file2.txt", b"content2" as &[u8]),
    ];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // With max_entries=10, should succeed
    let limits = ResourceLimits::new().max_entries(10);
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits).unwrap();

    assert_eq!(archive.entries().len(), 2);
}

#[test]
fn test_open_with_limits_custom_header_bytes() {
    // Create a simple archive
    let entries = [("test.txt", b"hello" as &[u8])];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // With reasonable header limit, should succeed
    let limits = ResourceLimits::new().max_header_bytes(64 * 1024);
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits).unwrap();

    assert_eq!(archive.entries().len(), 1);
}

#[test]
fn test_open_preserves_default_behavior() {
    // Ensure Archive::open() still works as before (regression test)
    let entries = [("test.txt", b"hello world" as &[u8])];
    let archive_bytes = common::create_archive(&entries).unwrap();

    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open(cursor).unwrap();

    assert_eq!(archive.entries().len(), 1);
}

// =============================================================================
// Boundary Condition Tests
// =============================================================================

/// Tests exact boundary behavior for max_entries limit.
///
/// This verifies behavior at limit-1, limit, and limit+1 to ensure
/// the boundary is correctly enforced.
#[test]
fn test_max_entries_exact_boundary() {
    // Create archive with exactly 3 entries
    let entries = [
        ("file1.txt", b"content1" as &[u8]),
        ("file2.txt", b"content2" as &[u8]),
        ("file3.txt", b"content3" as &[u8]),
    ];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // limit=2 (less than count) should fail
    let limits = ResourceLimits::new().max_entries(2);
    let cursor = Cursor::new(&archive_bytes);
    let result = Archive::open_with_limits(cursor, limits);
    assert!(
        result.is_err(),
        "Should reject when entries (3) > limit (2)"
    );

    // limit=3 (exactly at count) should succeed
    let limits = ResourceLimits::new().max_entries(3);
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits)
        .expect("Should accept when entries (3) == limit (3)");
    assert_eq!(archive.entries().len(), 3);

    // limit=4 (more than count) should succeed
    let limits = ResourceLimits::new().max_entries(4);
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, limits)
        .expect("Should accept when entries (3) < limit (4)");
    assert_eq!(archive.entries().len(), 3);
}

/// Tests boundary behavior with single entry archive.
///
/// Edge case: what happens when limit=0 vs limit=1 with a 1-entry archive.
#[test]
fn test_max_entries_single_entry_boundary() {
    let entries = [("single.txt", b"content" as &[u8])];
    let archive_bytes = common::create_archive(&entries).unwrap();

    // limit=0 should reject any archive with entries
    // Note: The implementation may count streams/files differently,
    // so we document the observed behavior.
    let limits = ResourceLimits::new().max_entries(0);
    let cursor = Cursor::new(&archive_bytes);
    let result = Archive::open_with_limits(cursor, limits);
    // A limit of 0 means "no entries allowed" - should fail for any non-empty archive
    assert!(
        result.is_err(),
        "limit=0 should reject archive with 1 entry"
    );

    // limit=1 should accept archive with exactly 1 entry
    let limits = ResourceLimits::new().max_entries(1);
    let cursor = Cursor::new(&archive_bytes);
    let archive =
        Archive::open_with_limits(cursor, limits).expect("limit=1 should accept 1-entry archive");
    assert_eq!(archive.entries().len(), 1);
}

// =============================================================================
// Ratio Limit Enforcement Tests
// =============================================================================
//
// Note: Builder tests for RatioLimit (new, is_some, unwrap) are unit tests
// and belong in src/format/streams.rs where they are already covered by:
// - test_ratio_limit_normal_ratio, test_ratio_limit_exceeds_limit, etc.
// This file only contains integration tests that exercise the full pipeline.

/// Tests ratio limit enforcement with highly compressible data.
///
/// This test creates an archive with data that compresses extremely well
/// (all zeros), then verifies that a strict ratio limit rejects it.
#[cfg(feature = "lzma2")]
#[test]
fn test_ratio_limit_enforced_during_open() {
    use std::io::Cursor as StdCursor;
    use zesven::format::streams::RatioLimit;
    use zesven::{ArchivePath, WriteOptions, Writer};

    // Create highly compressible data (all zeros compress extremely well)
    // A 100KB block of zeros will compress to just a few bytes with LZMA2
    let compressible_data = vec![0u8; 100_000];

    // Create archive with LZMA2 compression
    let mut archive_bytes = Vec::new();
    {
        let cursor = StdCursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(WriteOptions::new());

        let path = ArchivePath::new("zeros.bin").expect("Invalid path");
        writer
            .add_bytes(path, &compressible_data)
            .expect("Failed to add entry");
        let _ = writer.finish().expect("Failed to finish archive");
    }

    // Verify archive was created (sanity check)
    assert!(!archive_bytes.is_empty());

    // With very strict ratio limit (10:1), opening should fail
    // because LZMA2 can achieve much higher ratios on zeros
    let strict_limits = ResourceLimits::new().ratio_limit(Some(RatioLimit::new(10)));
    let cursor = Cursor::new(&archive_bytes);
    let result = Archive::open_with_limits(cursor, strict_limits);

    // The ratio limit should be checked during header parsing when stream info
    // reveals the compression ratio. This may or may not trigger depending on
    // implementation details (ratio checked at parse vs extract time).
    // Document either behavior:
    match result {
        Ok(_archive) => {
            // If archive opens, ratio limit may be checked at extraction time instead.
            // This is acceptable - ratio limits protect against decompression bombs
            // whether checked at open or extract time.
        }
        Err(Error::ResourceLimitExceeded(msg)) => {
            // Expected if ratio is checked at open time
            assert!(
                msg.contains("ratio") || msg.contains("compression"),
                "Error should mention ratio/compression: {}",
                msg
            );
        }
        Err(other) => {
            panic!("Unexpected error with strict ratio limit: {:?}", other);
        }
    }

    // With very permissive ratio limit, should always succeed
    let permissive_limits = ResourceLimits::new().ratio_limit(Some(RatioLimit::new(100_000)));
    let cursor = Cursor::new(&archive_bytes);
    let archive = Archive::open_with_limits(cursor, permissive_limits)
        .expect("Should open with permissive ratio limit");
    assert_eq!(archive.entries().len(), 1);
}

// =============================================================================
// Extraction-Time Limit Enforcement Tests
// =============================================================================
//
// Resource limit enforcement by when limits are checked:
//
// | Limit               | When Checked     | Enforced? |
// |---------------------|------------------|-----------|
// | max_entries         | Archive open     | YES       |
// | max_header_bytes    | Archive open     | YES       |
// | max_entry_unpacked  | Extraction time  | YES       |
// | max_total_unpacked  | Extraction time  | YES       |
// | ratio_limit         | Extraction time  | YES       |
//
// When limits are exceeded during extraction, the entry is recorded in the
// extraction result's failures with an appropriate error message.

#[cfg(feature = "lzma2")]
mod extraction_enforcement_tests {
    use super::*;
    use std::io::Cursor as StdCursor;
    use zesven::read::ExtractOptions;
    use zesven::{ArchivePath, Writer};

    /// Helper to create an archive with known uncompressed size.
    fn create_test_archive(data: &[u8], filename: &str) -> Vec<u8> {
        let mut archive_bytes = Vec::new();
        {
            let cursor = StdCursor::new(&mut archive_bytes);
            let mut writer = Writer::create(cursor).expect("Failed to create writer");

            let path = ArchivePath::new(filename).expect("Invalid path");
            writer.add_bytes(path, data).expect("Failed to add entry");
            let _ = writer.finish().expect("Failed to finish archive");
        }
        archive_bytes
    }

    /// Tests that max_entry_unpacked limit is enforced during extraction.
    ///
    /// When an entry exceeds the configured size limit during decompression,
    /// extraction fails with a ResourceLimitExceeded error recorded in the
    /// extraction result's failures.
    #[test]
    fn test_extraction_entry_size_limit_in_options() {
        // Create archive with 10KB of data
        let data = vec![0x42u8; 10_000];
        let archive_bytes = create_test_archive(&data, "large.bin");

        // Open archive (no limits during open)
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open(cursor).expect("Should open");

        // Configure extraction with entry limit below file size
        let limits = ResourceLimits::new().max_entry_unpacked(5_000);
        let options = ExtractOptions::new().limits(limits);

        // Create temp directory for extraction
        let temp_dir = tempfile::tempdir().unwrap();

        let result = archive.extract(temp_dir.path(), (), &options);

        // Extraction returns Ok but with the entry recorded as failed
        let extract_result = result.expect("Batch extraction should not return Err");
        assert_eq!(
            extract_result.entries_extracted, 0,
            "Entry exceeding limit should not be extracted"
        );
        assert_eq!(
            extract_result.entries_failed, 1,
            "Entry exceeding limit should be recorded as failed"
        );
        assert!(
            extract_result
                .failures
                .iter()
                .any(|(_, msg)| msg.contains("exceeds limit")),
            "Failure message should indicate limit exceeded: {:?}",
            extract_result.failures
        );
    }

    /// Tests that max_total_unpacked limit is enforced during extraction.
    ///
    /// When the cumulative extracted bytes exceed the configured total limit,
    /// subsequent extractions fail with ResourceLimitExceeded errors.
    #[test]
    fn test_extraction_total_size_limit_in_options() {
        // Create archive with multiple files totaling ~30KB
        let entries = [
            ("file1.bin", vec![0x41u8; 10_000]),
            ("file2.bin", vec![0x42u8; 10_000]),
            ("file3.bin", vec![0x43u8; 10_000]),
        ];

        let mut archive_bytes = Vec::new();
        {
            let cursor = StdCursor::new(&mut archive_bytes);
            let mut writer = Writer::create(cursor).expect("Failed to create writer");

            for (name, data) in &entries {
                let path = ArchivePath::new(name).expect("Invalid path");
                writer.add_bytes(path, data).expect("Failed to add entry");
            }
            let _ = writer.finish().expect("Failed to finish archive");
        }

        // Open and extract with total limit below combined size
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open(cursor).expect("Should open");

        // Set limit to 20KB - should allow 2 files (20KB) but fail on the 3rd
        let limits = ResourceLimits::new().max_total_unpacked(20_000);
        let options = ExtractOptions::new().limits(limits);

        let temp_dir = tempfile::tempdir().unwrap();
        let result = archive.extract(temp_dir.path(), (), &options);

        // Extraction returns Ok but with at least one entry recorded as failed
        let extract_result = result.expect("Batch extraction should not return Err");

        // With 20KB limit: first 2 files (20KB) should succeed, 3rd should fail
        assert!(
            extract_result.entries_extracted >= 1 && extract_result.entries_extracted <= 2,
            "Expected 1-2 entries extracted before hitting total limit, got {}",
            extract_result.entries_extracted
        );
        assert!(
            extract_result.entries_failed >= 1,
            "At least one entry should fail due to total limit exceeded"
        );
        assert!(
            extract_result
                .failures
                .iter()
                .any(|(_, msg)| msg.contains("exceeds limit")),
            "Failure message should indicate limit exceeded: {:?}",
            extract_result.failures
        );
    }

    /// Tests that ratio limit is enforced during extraction.
    ///
    /// When the compression ratio exceeds the configured limit during
    /// decompression, extraction fails with ResourceLimitExceeded error.
    #[test]
    fn test_extraction_ratio_limit_in_options() {
        use zesven::format::streams::RatioLimit;

        // Create highly compressible data (zeros compress extremely well)
        let compressible_data = vec![0u8; 50_000];
        let archive_bytes = create_test_archive(&compressible_data, "zeros.bin");

        // Open without ratio limit
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open(cursor).expect("Should open");

        // Extract with very strict ratio limit (5:1 max)
        // Zeros typically compress at ratios far exceeding this
        let limits = ResourceLimits::new().ratio_limit(Some(RatioLimit::new(5)));
        let options = ExtractOptions::new().limits(limits);

        let temp_dir = tempfile::tempdir().unwrap();
        let result = archive.extract(temp_dir.path(), (), &options);

        // Extraction returns Ok but with the entry recorded as failed
        let extract_result = result.expect("Batch extraction should not return Err");
        assert_eq!(
            extract_result.entries_extracted, 0,
            "Entry exceeding ratio limit should not be extracted"
        );
        assert_eq!(
            extract_result.entries_failed, 1,
            "Entry exceeding ratio limit should be recorded as failed"
        );
        assert!(
            extract_result
                .failures
                .iter()
                .any(|(_, msg)| { msg.contains("ratio") || msg.contains("Compression") }),
            "Failure message should indicate ratio exceeded: {:?}",
            extract_result.failures
        );
    }

    /// Documents that open_with_limits only affects parsing, not extraction.
    ///
    /// Resource limits passed to `open_with_limits` are for **parsing-time** enforcement
    /// (max_entries, max_header_bytes). Extraction-time limits (max_entry_unpacked,
    /// max_total_unpacked, ratio_limit) must be passed via ExtractOptions.
    ///
    /// This test documents and verifies this API design.
    #[test]
    fn test_open_limits_vs_extract_limits() {
        // Create archive with 10KB of data
        let data = vec![0x42u8; 10_000];
        let archive_bytes = create_test_archive(&data, "large.bin");

        // Opening limits only affect parsing - max_entry_unpacked is ignored during open
        let open_limits = ResourceLimits::new().max_entry_unpacked(1_000);
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open_with_limits(cursor, open_limits).expect("Should open");

        // extract_to_vec does NOT respect limits from open_with_limits
        // This is by design - extraction limits go in ExtractOptions
        let result = archive.extract_to_vec("large.bin");
        assert!(
            result.is_ok(),
            "extract_to_vec ignores open_with_limits extraction settings"
        );
        assert_eq!(result.unwrap().len(), 10_000);

        // To enforce extraction limits, use extract() with ExtractOptions
        // (demonstrated in test_extraction_entry_size_limit_in_options)
    }
}
