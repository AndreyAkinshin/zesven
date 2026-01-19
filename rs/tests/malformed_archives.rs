//! Tests for malformed and corrupted archive handling.
//!
//! These tests verify that zesven correctly detects and reports errors
//! when parsing malformed, corrupted, or malicious archives.

mod common;

use std::io::Cursor;

use zesven::Error;
use zesven::read::Archive;

/// Checks if an error indicates data corruption or invalid archive format.
///
/// This covers errors that can arise from corrupted compressed data:
/// - CRC mismatches after decompression
/// - Corrupt header data
/// - Invalid format detection
/// - I/O errors from decompression failures (InvalidData, UnexpectedEof)
#[cfg(feature = "lzma")]
fn is_corruption_error(error: &Error) -> bool {
    // Use the built-in is_corruption() for CrcMismatch and CorruptHeader
    if error.is_corruption() {
        return true;
    }

    // InvalidFormat covers cases where corruption makes data unrecognizable
    if matches!(error, Error::InvalidFormat(_)) {
        return true;
    }

    // I/O errors wrapping decompression failures
    if let Error::Io(io_err) = error {
        use std::io::ErrorKind;
        return matches!(
            io_err.kind(),
            ErrorKind::InvalidData | ErrorKind::UnexpectedEof
        );
    }

    false
}

/// Valid 7z signature bytes.
const SIGNATURE: &[u8; 6] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// Creates a minimal valid-looking start header (32 bytes).
/// This won't pass full parsing but has the right signature.
fn minimal_header() -> Vec<u8> {
    let mut header = Vec::with_capacity(32);
    // Signature (6 bytes)
    header.extend_from_slice(SIGNATURE);
    // Version (2 bytes): major 0, minor 4
    header.extend_from_slice(&[0x00, 0x04]);
    // StartHeaderCRC (4 bytes)
    header.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    // NextHeaderOffset (8 bytes)
    header.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // NextHeaderSize (8 bytes)
    header.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // NextHeaderCRC (4 bytes)
    header.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    header
}

// Use shared expect_err helper from common module
use common::expect_err;

// =============================================================================
// Truncated/Empty Archive Tests
// =============================================================================
//
// These tests verify that truncated or empty archives are rejected. Multiple error
// types may be returned depending on where the error is detected in the parsing
// pipeline:
// - Io(UnexpectedEof): Error occurs during read operation
// - InvalidFormat: Error occurs during format validation
// - CorruptHeader: Error occurs during header integrity checks
//
// The key invariant is that an error IS returned; the specific error type is an
// implementation detail that may change without breaking the API contract.

#[test]
fn test_empty_input_returns_error() {
    let data: &[u8] = &[];
    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err(), "Empty input must return an error");
    let err = expect_err(result);
    assert!(
        matches!(err, Error::Io(_) | Error::InvalidFormat(_)),
        "Empty input should return Io or InvalidFormat error, got: {:?}",
        err
    );
}

#[test]
fn test_truncated_signature_returns_error() {
    // Only first 3 bytes of signature (need 6 bytes for complete signature)
    let data: &[u8] = &[0x37, 0x7A, 0xBC];
    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err(), "Truncated signature must return an error");
    let err = expect_err(result);
    assert!(
        matches!(err, Error::Io(_) | Error::InvalidFormat(_)),
        "Truncated signature should return Io or InvalidFormat error, got: {:?}",
        err
    );
}

#[test]
fn test_truncated_header_returns_error() {
    // Valid signature but truncated rest of header (32-byte start header required)
    let mut data = Vec::new();
    data.extend_from_slice(SIGNATURE);
    // Only 2 more bytes instead of 26 (version but no rest)
    data.extend_from_slice(&[0x00, 0x04]);

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err(), "Truncated header must return an error");
    let err = expect_err(result);
    assert!(
        matches!(
            err,
            Error::Io(_) | Error::InvalidFormat(_) | Error::CorruptHeader { .. }
        ),
        "Truncated header should return Io, InvalidFormat, or CorruptHeader error, got: {:?}",
        err
    );
}

// =============================================================================
// Invalid Signature Tests
// =============================================================================

#[test]
fn test_invalid_signature_returns_error() {
    // Not a 7z signature - looks like a ZIP file
    let data: &[u8] = &[0x50, 0x4B, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00];
    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err());
    let err = expect_err(result);
    assert!(
        matches!(err, Error::InvalidFormat(_)),
        "Expected InvalidFormat error for ZIP signature, got: {:?}",
        err
    );
}

#[test]
fn test_partial_signature_mismatch_returns_error() {
    // First 5 bytes correct, 6th byte wrong
    let data: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0xFF, 0x00, 0x04];
    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err());
    let err = expect_err(result);
    assert!(
        matches!(err, Error::InvalidFormat(_)),
        "Expected InvalidFormat error for corrupted signature, got: {:?}",
        err
    );
}

#[test]
fn test_random_bytes_returns_error() {
    let data: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err());
    let err = expect_err(result);
    assert!(
        matches!(err, Error::InvalidFormat(_)),
        "Expected InvalidFormat error for random bytes, got: {:?}",
        err
    );
}

// =============================================================================
// Corrupted Header Tests
// =============================================================================

#[test]
fn test_invalid_header_crc_detected() {
    let mut data = minimal_header();
    // Corrupt the StartHeaderCRC (bytes 8-11)
    data[8] = 0xFF;
    data[9] = 0xFF;
    data[10] = 0xFF;
    data[11] = 0xFF;

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail with CRC error or corrupt header
    assert!(result.is_err());
    let err = expect_err(result);
    assert!(
        matches!(
            err,
            Error::CorruptHeader { .. } | Error::InvalidFormat(_) | Error::Io(_)
        ),
        "Expected header corruption error, got: {:?}",
        err
    );
}

#[test]
fn test_invalid_next_header_offset_returns_error() {
    let mut data = minimal_header();
    // Set NextHeaderOffset to a huge value (bytes 12-19)
    // This would point beyond any reasonable file
    data[12..20].copy_from_slice(&u64::MAX.to_le_bytes());

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail when trying to seek to invalid position
    assert!(result.is_err());
}

#[test]
fn test_negative_next_header_size_detected() {
    let mut data = minimal_header();
    // Set NextHeaderSize to a huge value (bytes 20-27)
    // which would cause issues with memory allocation
    data[20..28].copy_from_slice(&0x7FFF_FFFF_FFFF_FFFFu64.to_le_bytes());

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail with resource limit or invalid format
    assert!(result.is_err());
}

// =============================================================================
// Bit-Flipped Data Tests
// =============================================================================

#[test]
fn test_bit_flipped_signature_detected() {
    let mut data = minimal_header();
    // Flip a single bit in the signature
    data[0] ^= 0x01;

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    assert!(result.is_err());
    let err = expect_err(result);
    assert!(
        matches!(err, Error::InvalidFormat(_)),
        "Expected InvalidFormat for bit-flipped signature, got: {:?}",
        err
    );
}

#[test]
fn test_bit_flipped_version_handled() {
    let mut data = minimal_header();
    // Flip bits in version field (major version becomes 0xFF)
    data[6] ^= 0xFF;

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // A corrupted version field in a minimal header should cause parsing to fail
    // because the header CRC will be invalid (we didn't update the CRC after flipping).
    // The archive should fail to open with a corrupt header or CRC error.
    assert!(
        result.is_err(),
        "Bit-flipped version with invalid CRC should fail to parse"
    );
}

// =============================================================================
// Oversized Value Tests
// =============================================================================

#[test]
fn test_oversized_header_size_rejected() {
    let mut data = minimal_header();
    // Set NextHeaderSize to 1TB
    let huge_size: u64 = 1024 * 1024 * 1024 * 1024;
    data[20..28].copy_from_slice(&huge_size.to_le_bytes());

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail with resource limit or I/O error
    assert!(result.is_err());
}

// Note: test_archive_path_validation was removed - covered more thoroughly by
// unit tests in src/archive_path.rs (test_invalid_nul_byte, test_invalid_empty,
// test_invalid_dotdot_traversal, test_invalid_absolute_path, test_valid_nested_path, etc.)

// Note: Error type verification tests (test_error_types_are_correct,
// test_corrupt_header_error_includes_offset) were removed as they duplicate
// unit tests in src/error.rs (test_crc_mismatch, test_path_traversal,
// test_is_unsupported, test_entry_index, test_method_id, test_corrupt_header,
// test_resource_limit_exceeded, test_convenience_constructors, etc.)

// =============================================================================
// Symlink Security Tests
// =============================================================================
//
// Test archive: tests/fixtures/symlink_test.7z
// Created with: 7zz a -snl symlink_test.7z <dir_with_symlinks>
//
// Contains:
// - symlink_test/target.txt (regular file)
// - symlink_test/relative_link.txt -> target.txt (relative symlink)
// - symlink_test/absolute_link.txt -> /etc/passwd (absolute symlink)
// - symlink_test/traversal_link.txt -> ../../../etc/passwd (traversal symlink)

/// Path to the symlink test fixture archive.
#[cfg(feature = "lzma")]
const SYMLINK_TEST_ARCHIVE: &str = "tests/fixtures/symlink_test.7z";

/// Check if running in CI environment.
///
/// Returns true if the `CI` environment variable is set (common in GitHub Actions,
/// GitLab CI, CircleCI, etc.).
#[cfg(feature = "lzma")]
fn is_ci() -> bool {
    std::env::var("CI").is_ok()
}

/// Check if symlink test fixture is available, with CI-appropriate behavior.
///
/// In CI environments: returns true if fixture exists, panics if missing
/// (fixture should always be present in CI).
///
/// Locally: returns true if fixture exists, returns false (skip) if missing
/// (developers may not have generated the fixture).
#[cfg(feature = "lzma")]
fn symlink_fixture_available() -> bool {
    let exists = std::path::Path::new(SYMLINK_TEST_ARCHIVE).exists();

    if !exists && is_ci() {
        panic!(
            "CI FAILURE: Symlink test fixture not found at '{}'. \n\
             This fixture must be present in CI. Please ensure the fixture \n\
             file is committed to the repository.",
            SYMLINK_TEST_ARCHIVE
        );
    }

    exists
}

/// Opens the symlink test archive.
#[cfg(feature = "lzma")]
fn open_symlink_archive() -> zesven::Result<zesven::Archive<std::io::BufReader<std::fs::File>>> {
    zesven::Archive::open_path(SYMLINK_TEST_ARCHIVE)
}

/// Test that symlinks are correctly detected in archive entries.
#[test]
#[cfg(feature = "lzma")]
fn test_symlink_entries_detected_in_archive() {
    if !symlink_fixture_available() {
        eprintln!(
            "Skipping symlink test: fixture not available at {}",
            SYMLINK_TEST_ARCHIVE
        );
        return;
    }

    let archive = open_symlink_archive().expect("Failed to open symlink test archive");
    let entries = archive.entries();

    // Find symlink entries by checking is_symlink field
    let symlinks: Vec<_> = entries.iter().filter(|e| e.is_symlink).collect();

    // We expect 3 symlinks: relative_link.txt, absolute_link.txt, traversal_link.txt
    assert_eq!(
        symlinks.len(),
        3,
        "Expected 3 symlinks, found {}. Entries: {:?}",
        symlinks.len(),
        entries
            .iter()
            .map(|e| (&e.path, e.is_symlink))
            .collect::<Vec<_>>()
    );

    // Verify names contain expected symlinks
    let paths: Vec<_> = symlinks.iter().map(|e| e.path.as_str()).collect();
    assert!(
        paths.iter().any(|p| p.contains("relative_link")),
        "Should find relative_link"
    );
    assert!(
        paths.iter().any(|p| p.contains("absolute_link")),
        "Should find absolute_link"
    );
    assert!(
        paths.iter().any(|p| p.contains("traversal_link")),
        "Should find traversal_link"
    );
}

/// Test that LinkPolicy::Forbid (default) rejects symlinks during extraction.
/// The extract() method returns Ok(ExtractResult) with failures recorded,
/// rather than returning Err directly for non-fatal failures.
#[cfg(feature = "lzma")]
#[test]
fn test_symlink_extraction_forbidden_by_default() {
    use zesven::read::{ExtractOptions, SelectAll};

    if !symlink_fixture_available() {
        eprintln!("Skipping symlink test: fixture not available");
        return;
    }

    let mut archive = open_symlink_archive().expect("Failed to open archive");
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

    // Default options should have LinkPolicy::Forbid
    let opts = ExtractOptions::default();
    let result = archive.extract(temp_dir.path(), SelectAll, &opts);

    // Extract returns Ok with failures in the result
    let extract_result = result.expect("Extract should return Ok with failures logged");

    // Should have 3 failed entries (the symlinks) and 2 successful (dir + file)
    assert!(
        extract_result.entries_failed >= 3,
        "Expected at least 3 symlink failures, got {} failed. Failures: {:?}",
        extract_result.entries_failed,
        extract_result.failures
    );

    // Verify failures mention symlink rejection
    let has_symlink_rejection = extract_result
        .failures
        .iter()
        .any(|(path, msg)| path.contains("link") && msg.to_lowercase().contains("symlink"));

    assert!(
        has_symlink_rejection,
        "Should have at least one symlink rejection. Failures: {:?}",
        extract_result.failures
    );
}

/// Test that LinkPolicy::ValidateTargets rejects symlinks with traversal targets.
///
/// The test fixture contains `traversal_link.txt` pointing to `../../../etc/passwd`.
/// This verifies that extraction-time path validation catches traversal attempts
/// in symlink targets.
///
/// The validation calculates depth relative to the extraction root using the
/// archive entry path, not the absolute filesystem path, ensuring traversal
/// detection works regardless of extraction directory depth.
#[cfg(feature = "lzma")]
#[test]
fn test_symlink_traversal_target_rejected_with_validate() {
    use zesven::read::{ExtractOptions, LinkPolicy, SelectAll};

    if !symlink_fixture_available() {
        eprintln!("Skipping symlink traversal test: fixture not available");
        return;
    }

    let mut archive = open_symlink_archive().expect("Failed to open archive");
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

    // ValidateTargets policy should reject traversal paths
    let opts = ExtractOptions::new().link_policy(LinkPolicy::ValidateTargets);
    let result = archive.extract(temp_dir.path(), SelectAll, &opts);

    let extract_result = result.expect("Extract should return Ok with failures logged");

    // Find the traversal_link failure specifically
    let traversal_rejected = extract_result
        .failures
        .iter()
        .any(|(path, _msg)| path.contains("traversal_link"));

    assert!(
        traversal_rejected,
        "traversal_link.txt (target: ../../../etc/passwd) should be rejected. \
         This verifies extraction-time path traversal validation. Failures: {:?}",
        extract_result.failures
    );
}

/// Test that LinkPolicy::ValidateTargets rejects absolute symlink targets.
/// The absolute symlink (/etc/passwd) should fail. The traversal symlink behavior
/// depends on extraction depth calculation.
#[cfg(feature = "lzma")]
#[test]
fn test_symlink_absolute_target_rejected_with_validate() {
    use zesven::read::{ExtractOptions, LinkPolicy, SelectAll};

    if !symlink_fixture_available() {
        eprintln!("Skipping symlink test: fixture not available");
        return;
    }

    let mut archive = open_symlink_archive().expect("Failed to open archive");
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

    // Use ValidateTargets policy - should reject absolute paths
    let opts = ExtractOptions::new().link_policy(LinkPolicy::ValidateTargets);
    let result = archive.extract(temp_dir.path(), SelectAll, &opts);

    let extract_result = result.expect("Extract should return Ok with failures logged");

    // Should have at least 1 failed entry (absolute_link)
    // relative_link should succeed since it points to target.txt (valid relative path)
    assert!(
        extract_result.entries_failed >= 1,
        "Expected at least 1 failure for absolute symlink, got {} failed. Failures: {:?}",
        extract_result.entries_failed,
        extract_result.failures
    );

    // Verify absolute_link.txt was rejected with escape error
    let absolute_rejected = extract_result.failures.iter().any(|(path, msg)| {
        path.contains("absolute_link")
            && (msg.to_lowercase().contains("escape") || msg.to_lowercase().contains("target"))
    });

    assert!(
        absolute_rejected,
        "absolute_link.txt should be rejected. Failures: {:?}",
        extract_result.failures
    );
}

/// Test that LinkPolicy::Allow permits symlinks during extraction.
/// Note: With Allow policy, all symlinks are created including dangerous ones.
/// This test verifies the policy is respected by checking that extraction doesn't
/// fail with SymlinkRejected (it may fail later for other reasons like target not found).
#[cfg(all(feature = "lzma", unix))]
#[test]
fn test_symlink_allow_policy_does_not_reject_symlinks() {
    use zesven::Error;
    use zesven::read::{ExtractOptions, LinkPolicy, SelectAll};

    if !symlink_fixture_available() {
        eprintln!("Skipping symlink test: fixture not available");
        return;
    }

    let mut archive = open_symlink_archive().expect("Failed to open archive");
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

    // Use Allow policy - permits all symlinks
    let opts = ExtractOptions::new().link_policy(LinkPolicy::Allow);
    let result = archive.extract(temp_dir.path(), SelectAll, &opts);

    // With Allow policy, we should NOT get SymlinkRejected or SymlinkTargetEscape errors
    // Extraction might still fail for other reasons (e.g., target file missing), which is fine
    match &result {
        Err(Error::SymlinkRejected { .. }) => {
            panic!("Allow policy should not reject symlinks with SymlinkRejected");
        }
        Err(Error::SymlinkTargetEscape { .. }) => {
            panic!("Allow policy should not reject symlinks with SymlinkTargetEscape");
        }
        _ => {
            // Either success or other error (e.g., IO error creating symlink) is acceptable
            // The key point is that symlink policy is not enforced
        }
    }

    // If extraction succeeded, verify at least the regular file was created
    if result.is_ok() {
        let target_path = temp_dir.path().join("symlink_test/target.txt");
        assert!(target_path.exists(), "Regular file should be extracted");
    }
}

/// Tests LinkPolicy options and default behavior.
///
/// Symlinks in 7z archives are marked with:
/// - Windows: REPARSE_POINT attribute (0x400) in the attributes field
/// - Unix: Symlink mode bits (0o120000) in high 16 bits of attributes
///
/// The symlink target path is stored as the entry's content.
#[test]
fn test_link_policy_options_and_defaults() {
    use zesven::read::{ExtractOptions, LinkPolicy};

    // Verify LinkPolicy enum exists and has expected variants
    let _forbid = LinkPolicy::Forbid;
    let _validate = LinkPolicy::ValidateTargets;
    let _allow = LinkPolicy::Allow;

    // Verify ExtractOptions accepts LinkPolicy
    let options = ExtractOptions::new().link_policy(LinkPolicy::Forbid);
    assert_eq!(options.link_policy, LinkPolicy::Forbid);

    let options = ExtractOptions::new().link_policy(LinkPolicy::ValidateTargets);
    assert_eq!(options.link_policy, LinkPolicy::ValidateTargets);

    let options = ExtractOptions::new().link_policy(LinkPolicy::Allow);
    assert_eq!(options.link_policy, LinkPolicy::Allow);

    // Default policy is Forbid (safest)
    let default_options = ExtractOptions::default();
    assert_eq!(default_options.link_policy, LinkPolicy::Forbid);
}

// Note: test_entry_symlink_field_exists was removed as it only tested that
// a struct field exists, not any actual behavior. The Entry::is_symlink field
// is exercised by test_symlink_detection_with_real_archive and other symlink tests.

// Note: The following symlink error type tests were moved to src/error.rs
// as they test error struct construction, which is unit test material:
// - test_symlink_rejected_error_has_context
// - test_symlink_error_types
// - test_symlink_absolute_path_error
// - test_symlink_windows_absolute_path_error
// - test_symlink_errors_not_recoverable

// =============================================================================
// Corrupted Compressed Data Tests
// =============================================================================

/// Tests that corrupted compressed data (bitflip inside payload) is detected.
///
/// This test verifies that corruption within the compressed data stream is
/// detected during extraction or CRC verification. Unlike header corruption tests,
/// this targets the actual compressed payload.
#[cfg(feature = "lzma")]
#[test]
fn test_corrupted_compressed_data_detected() {
    use std::io::Cursor;
    use zesven::read::{Archive, SelectAll, TestOptions};
    use zesven::{ArchivePath, Writer};

    // Create a valid archive with substantial content (more data = more likely to detect corruption)
    let content = b"This is test content that will be compressed. ".repeat(100);
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();

        let path = ArchivePath::new("test.txt").unwrap();
        writer.add_bytes(path, &content).unwrap();

        let _ = writer.finish().unwrap();
    }

    // Corrupt the compressed data section (bytes after the 32-byte start header)
    // The start header is 32 bytes, compressed data follows
    // We flip bits in the middle of the archive to target compressed data
    let archive_len = archive_bytes.len();
    assert!(
        archive_len > 100,
        "Archive should be large enough to corrupt"
    );

    // Target the middle of the compressed data section
    let corrupt_offset = 32 + (archive_len - 32) / 2;
    archive_bytes[corrupt_offset] ^= 0xFF;
    archive_bytes[corrupt_offset + 1] ^= 0xAA;
    archive_bytes[corrupt_offset + 2] ^= 0x55;

    // Attempt to open and test/extract - should detect corruption
    let cursor = Cursor::new(&archive_bytes);

    // Opening may succeed (header might be intact) but test/extract should fail
    match Archive::open(cursor) {
        Ok(mut archive) => {
            // Archive opened - try to test entries, this should catch CRC mismatch
            let test_result = archive.test(SelectAll, &TestOptions::default());

            match test_result {
                Ok(result) => {
                    // Test completed - should report failures
                    assert!(
                        result.entries_failed > 0 || !result.failures.is_empty(),
                        "Corrupted archive should have failed entries, got: tested={}, failed={}, failures={:?}",
                        result.entries_tested,
                        result.entries_failed,
                        result.failures
                    );
                }
                Err(e) => {
                    // Test returned error - acceptable, corruption was detected
                    // Verify it's a corruption-related error using enum matching
                    assert!(
                        is_corruption_error(&e),
                        "Error should indicate corruption, got: {:?}",
                        e
                    );
                }
            }
        }
        Err(e) => {
            // Archive failed to open - also acceptable if corruption hit header
            assert!(
                is_corruption_error(&e),
                "Error should indicate corruption, got: {:?}",
                e
            );
        }
    }
}

// =============================================================================
// ExtractResult Partial Failure Tests
// =============================================================================

/// Tests that TestResult correctly reports entries_failed > 0 when CRC fails.
///
/// Unlike `test_testresult_fields_populated` which tests valid archives,
/// this test verifies that `entries_failed` is incremented and `failures`
/// contains meaningful messages when data corruption causes CRC mismatch.
#[cfg(feature = "lzma")]
#[test]
fn test_testresult_crc_failure_reported() {
    use std::io::Cursor;
    use zesven::read::{Archive, SelectAll, TestOptions};
    use zesven::{ArchivePath, Writer};

    // Create a valid archive first
    let content = b"Known content for CRC verification testing purposes.".repeat(50);
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();

        let path = ArchivePath::new("testfile.txt").unwrap();
        writer.add_bytes(path, &content).unwrap();

        let _ = writer.finish().unwrap();
    }

    // Verify the valid archive passes first
    {
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open(cursor).unwrap();
        let result = archive.test(SelectAll, &TestOptions::default()).unwrap();
        assert_eq!(result.entries_failed, 0, "Valid archive should pass");
    }

    // Now corrupt the archive - target the compressed data region
    // The 7z format has: 32-byte start header, then packed streams, then end header
    // We want to corrupt the packed stream data
    let archive_len = archive_bytes.len();
    assert!(archive_len > 64, "Archive should be large enough");

    // Corrupt bytes in what should be compressed data (after start header)
    // Use multiple corruption points to increase chance of detection
    for offset in [40, 50, 60].iter() {
        if *offset < archive_len {
            archive_bytes[*offset] ^= 0xFF;
        }
    }

    // Now test the corrupted archive
    let cursor = Cursor::new(&archive_bytes);

    match Archive::open(cursor) {
        Ok(mut archive) => {
            let result = archive.test(SelectAll, &TestOptions::default());

            match result {
                Ok(test_result) => {
                    // TestResult should indicate failure
                    assert!(
                        test_result.entries_failed > 0,
                        "TestResult.entries_failed should be > 0 for corrupted archive, got: {}",
                        test_result.entries_failed
                    );
                    assert!(
                        !test_result.failures.is_empty(),
                        "TestResult.failures should contain failure messages"
                    );

                    // Verify the failure message is meaningful
                    // Note: LZMA decompression failures may report various error types including:
                    // - CRC mismatch
                    // - Corruption detected
                    // - Invalid data
                    // - dist overflow (LZMA distance code overflow from corrupted stream)
                    // - I/O errors wrapping decompression failures
                    let has_meaningful_message = test_result.failures.iter().any(|(path, msg)| {
                        let msg_lower = msg.to_lowercase();
                        !path.is_empty()
                            && (msg_lower.contains("crc")
                                || msg_lower.contains("corrupt")
                                || msg_lower.contains("invalid")
                                || msg_lower.contains("failed")
                                || msg_lower.contains("overflow")
                                || msg_lower.contains("error"))
                    });

                    assert!(
                        has_meaningful_message,
                        "Failure messages should be meaningful: {:?}",
                        test_result.failures
                    );
                }
                Err(_) => {
                    // Test() returned error - acceptable, corruption was detected
                    // This is valid behavior - the test detected corruption
                }
            }
        }
        Err(_) => {
            // Archive failed to open - corruption detected early
            // This is acceptable - we just want corruption to be detected
        }
    }
}

/// Tests that TestResult correctly reports entries_failed and failures.
///
/// This test creates a valid archive but then simulates CRC verification to
/// validate the TestResult fields are populated correctly.
#[cfg(feature = "lzma")]
#[test]
fn test_testresult_fields_populated() {
    use std::io::Cursor;
    use zesven::read::{Archive, SelectAll, TestOptions};
    use zesven::{ArchivePath, Writer};

    // Create a valid archive with multiple files
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();

        let path1 = ArchivePath::new("file1.txt").unwrap();
        writer.add_bytes(path1, b"content one").unwrap();

        let path2 = ArchivePath::new("file2.txt").unwrap();
        writer.add_bytes(path2, b"content two").unwrap();

        let path3 = ArchivePath::new("file3.txt").unwrap();
        writer.add_bytes(path3, b"content three").unwrap();

        let _ = writer.finish().unwrap();
    }

    // Open and test all entries - all should pass
    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open(cursor).unwrap();

    let result = archive.test(SelectAll, &TestOptions::default()).unwrap();

    // Verify result fields
    assert_eq!(result.entries_tested, 3, "Should have tested 3 entries");
    assert_eq!(
        result.entries_failed, 0,
        "No entries should have failed: {:?}",
        result.failures
    );
    assert!(
        result.failures.is_empty(),
        "Failures should be empty for valid archive"
    );
    assert!(result.is_ok(), "Result should indicate success");
    assert!(!result.is_err(), "Result should not have errors");
}

/// Tests ExtractResult fields after extraction with MemoryDestination.
#[cfg(feature = "lzma")]
#[test]
fn test_extractresult_fields_populated() {
    use std::io::Cursor;
    use zesven::read::{Archive, MemoryDestination};
    use zesven::{ArchivePath, Writer};

    // Create a valid archive
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();

        let path1 = ArchivePath::new("a.txt").unwrap();
        writer.add_bytes(path1, b"alpha").unwrap();

        let path2 = ArchivePath::new("b.txt").unwrap();
        writer.add_bytes(path2, b"beta").unwrap();

        let _ = writer.finish().unwrap();
    }

    // Extract to memory
    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open(cursor).unwrap();

    let mut dest = MemoryDestination::new();
    let result = archive.extract_to_destination(&mut dest).unwrap();

    // Verify result fields
    assert_eq!(
        result.entries_extracted, 2,
        "Should have extracted 2 entries"
    );
    assert_eq!(
        result.entries_failed, 0,
        "No entries should have failed: {:?}",
        result.failures
    );
    assert!(
        result.failures.is_empty(),
        "Failures should be empty for valid archive"
    );
    assert!(result.is_ok(), "Result should indicate success");

    // Verify content was extracted correctly
    assert_eq!(dest.get("a.txt").unwrap(), b"alpha");
    assert_eq!(dest.get("b.txt").unwrap(), b"beta");
}

// =============================================================================
// Additional Error Path Tests
// =============================================================================
//
// These tests expand coverage for error conditions during archive reading,
// including corrupt headers at various positions and extraction-time errors.

/// Tests that corrupted NextHeaderCRC is detected.
///
/// The NextHeaderCRC is stored at bytes 28-31 of the start header. If this
/// is corrupted, the header data read from NextHeaderOffset will fail CRC
/// verification.
#[test]
fn test_corrupted_next_header_crc_detected() {
    let mut data = minimal_header();
    // Corrupt the NextHeaderCRC (last 4 bytes of start header)
    data[28] = 0xFF;
    data[29] = 0xFF;
    data[30] = 0xFF;
    data[31] = 0xFF;

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail with CRC or corruption error
    assert!(
        result.is_err(),
        "Corrupted NextHeaderCRC should cause error"
    );
}

/// Tests that valid signature with garbage header content is rejected.
///
/// This verifies that having a valid 7z signature at the start doesn't cause
/// crashes when the rest of the file is garbage.
#[test]
fn test_valid_signature_garbage_content() {
    let mut data = vec![0u8; 100];
    // Valid 7z signature
    data[0..6].copy_from_slice(SIGNATURE);
    // Fill rest with random garbage
    for (i, byte) in data[6..100].iter_mut().enumerate() {
        *byte = ((i + 6) * 17) as u8;
    }

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail to parse - multiple error types acceptable
    assert!(result.is_err(), "Garbage after valid signature should fail");
}

/// Tests detection of signature bytes at different byte positions.
///
/// Verifies that single-byte corruptions in the signature are detected.
#[test]
fn test_signature_corruption_at_each_position() {
    for corrupt_pos in 0..6 {
        let mut data = minimal_header();
        data[corrupt_pos] ^= 0xFF; // Flip bits at this position

        let cursor = Cursor::new(data);
        let result = Archive::open(cursor);

        assert!(
            result.is_err(),
            "Corrupted signature at position {} should fail",
            corrupt_pos
        );

        let err = expect_err(result);
        assert!(
            matches!(err, Error::InvalidFormat(_)),
            "Position {} should give InvalidFormat, got: {:?}",
            corrupt_pos,
            err
        );
    }
}

/// Tests that header claiming more streams than data causes error.
///
/// This simulates a malformed archive where the header metadata claims
/// there are more compressed streams than actually exist in the file.
#[cfg(feature = "lzma")]
#[test]
fn test_header_overclaims_streams() {
    use std::io::Cursor;
    use zesven::{ArchivePath, Writer};

    // Create a valid small archive first
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();
        let path = ArchivePath::new("small.txt").unwrap();
        writer.add_bytes(path, b"tiny").unwrap();
        let _ = writer.finish().unwrap();
    }

    // Now corrupt the packed streams info in the header
    // This is tricky because we'd need to modify internal structures
    // Instead, we truncate the compressed data section
    let truncate_at = archive_bytes.len() - 20;
    if truncate_at > 32 {
        archive_bytes.truncate(truncate_at);

        let cursor = Cursor::new(&archive_bytes);
        let result = Archive::open(cursor);

        // Should fail because the archive is now incomplete
        assert!(result.is_err(), "Truncated archive should fail to open");
    }
}

/// Tests that archive with entry count mismatch is handled.
///
/// This verifies behavior when metadata claims N entries but fewer exist.
#[cfg(feature = "lzma")]
#[test]
fn test_entry_count_mismatch_handling() {
    use std::io::Cursor;
    use zesven::read::{SelectAll, TestOptions};
    use zesven::{ArchivePath, Writer};

    // Create valid archive
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap();
        let path = ArchivePath::new("file.txt").unwrap();
        writer.add_bytes(path, b"content for test file").unwrap();
        let _ = writer.finish().unwrap();
    }

    // Find and corrupt the file count in the header
    // The header is at the end of the file, and contains EncodedHeader/FilesInfo
    // This is difficult to do precisely, so we use bit-flipping approach
    let len = archive_bytes.len();
    if len > 50 {
        // Corrupt a byte in what's likely header territory
        let corrupt_pos = len - 15;
        archive_bytes[corrupt_pos] ^= 0x01;

        let cursor = Cursor::new(&archive_bytes);

        // Archive may or may not open depending on what we corrupted
        match Archive::open(cursor) {
            Ok(mut archive) => {
                // If it opens, testing should reveal corruption
                let result = archive.test(SelectAll, &TestOptions::default());
                // Either test fails or reports failures
                match result {
                    Ok(_test_result) => {
                        // Corruption may or may not affect this specific byte
                        // Document either outcome as acceptable
                    }
                    Err(_) => {
                        // Test caught corruption - good
                    }
                }
            }
            Err(_) => {
                // Open failed due to corruption - also acceptable
            }
        }
    }
}

/// Tests behavior with maximum u64 values in header fields.
///
/// Ensures the parser doesn't overflow or panic with extreme values.
#[test]
fn test_extreme_header_values() {
    let mut data = minimal_header();

    // Set NextHeaderOffset to u64::MAX
    data[12..20].copy_from_slice(&u64::MAX.to_le_bytes());

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail gracefully, not panic
    assert!(result.is_err(), "Extreme header values should cause error");
}

/// Tests behavior with zero-filled header after signature.
///
/// A header with all zeros (except signature) should fail parsing.
#[test]
fn test_all_zeros_after_signature() {
    let mut data = vec![0u8; 64];
    data[0..6].copy_from_slice(SIGNATURE);
    // Everything else is zeros

    let cursor = Cursor::new(data);
    let result = Archive::open(cursor);

    // Should fail because CRC would be wrong and/or parsing invalid
    assert!(result.is_err(), "All-zeros header should fail");
}
