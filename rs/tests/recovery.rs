//! Integration tests for archive recovery functionality.
//!
//! These tests verify that the recovery module can handle damaged,
//! corrupted, or embedded archives through the public API.

#![cfg(feature = "lzma")]

mod common;

use std::io::Cursor;
use zesven::recovery::{
    RecoveryOptions, RecoveryStatus, find_all_signatures, is_valid_archive, recover_archive,
};

/// Tests that is_valid_archive correctly identifies a valid archive.
#[test]
fn test_is_valid_archive_api() {
    let entries = [("test.txt", b"Hello, World!" as &[u8])];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let mut cursor = Cursor::new(archive_bytes);
    assert!(is_valid_archive(&mut cursor).unwrap());
}

/// Tests that is_valid_archive rejects invalid data.
#[test]
fn test_is_valid_archive_rejects_invalid() {
    let invalid_data = b"This is not a 7z archive at all!";
    let mut cursor = Cursor::new(invalid_data.to_vec());

    assert!(!is_valid_archive(&mut cursor).unwrap());
}

/// Tests recovery of an archive with garbage prefix.
#[test]
fn test_recover_archive_with_garbage_prefix() {
    let entries = [("file.txt", b"Test content" as &[u8])];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Add garbage prefix
    let mut corrupted = vec![0xDE, 0xAD, 0xBE, 0xEF];
    corrupted.extend(b"Some random garbage data before the archive... ");
    corrupted.extend(&archive_bytes);

    let cursor = Cursor::new(corrupted);
    let result = recover_archive(cursor, RecoveryOptions::default()).expect("Recovery failed");

    // Should find the archive at offset > 0
    assert!(
        result.archive_offset > 0,
        "Should find archive after garbage"
    );
    assert!(
        result.archive.is_some(),
        "Should recover archive despite garbage prefix"
    );

    // Should have a warning about non-zero offset
    assert!(
        !result.warnings.is_empty(),
        "Should warn about archive offset"
    );
}

/// Tests recovery of an archive embedded in binary data (like SFX).
#[test]
fn test_recover_archive_embedded_in_binary() {
    let entries = [
        ("readme.txt", b"This is the readme" as &[u8]),
        ("data.bin", b"Binary data here"),
    ];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Simulate an SFX-like structure with executable prefix
    let mut embedded = vec![0u8; 256];
    // Add some "executable" looking bytes
    embedded[0] = b'M';
    embedded[1] = b'Z';
    embedded.extend(&archive_bytes);

    let cursor = Cursor::new(embedded);
    let result = recover_archive(cursor, RecoveryOptions::default()).expect("Recovery failed");

    assert_eq!(
        result.archive_offset, 256,
        "Should find archive at correct offset"
    );
    assert!(result.archive.is_some());
}

/// Tests that recovery extracts valid entries.
#[test]
fn test_recovery_extracts_valid_entries() {
    let content = b"Important data that must be recovered";
    let entries = [("important.txt", content.as_slice())];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let cursor = Cursor::new(archive_bytes);
    let result = recover_archive(cursor, RecoveryOptions::default()).expect("Recovery failed");

    assert!(result.archive.is_some());
    let mut archive = result.archive.unwrap();

    // Should be able to extract the entry
    let extracted = archive
        .extract_to_vec("important.txt")
        .expect("Failed to extract");
    assert_eq!(extracted, content);
}

/// Tests recovery reports correct status for partial recovery scenarios.
#[test]
fn test_recovery_reports_status() {
    let entries = [
        ("file1.txt", b"First file" as &[u8]),
        ("file2.txt", b"Second file"),
        ("dir/file3.txt", b"Third file"),
    ];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let cursor = Cursor::new(archive_bytes);
    let result = recover_archive(cursor, RecoveryOptions::default()).expect("Recovery failed");

    // Should have full recovery since archive is valid
    assert_eq!(
        result.status,
        RecoveryStatus::FullRecovery,
        "Valid archive should report full recovery"
    );

    // All entries should be recovered
    assert_eq!(result.recovered_count(), 3);
    assert_eq!(result.failed_count(), 0);
    assert!((result.recovery_rate() - 1.0).abs() < 0.001);
}

/// Tests finding all signatures with multiple embedded archives.
#[test]
fn test_find_all_signatures_multiple_archives() {
    let entries1 = [("archive1.txt", b"First archive" as &[u8])];
    let entries2 = [("archive2.txt", b"Second archive" as &[u8])];

    let archive1 = common::create_archive(&entries1).expect("Failed to create archive 1");
    let archive2 = common::create_archive(&entries2).expect("Failed to create archive 2");

    // Concatenate with some padding
    let mut combined = archive1.clone();
    combined.extend(vec![0u8; 64]); // Some padding
    combined.extend(&archive2);

    let mut cursor = Cursor::new(combined);
    let signatures = find_all_signatures(&mut cursor, None).expect("Failed to scan");

    // Should find at least the first signature
    assert!(!signatures.is_empty(), "Should find at least one signature");
    assert_eq!(signatures[0], 0, "First signature should be at offset 0");

    // If the scanner finds both, check the second offset
    if signatures.len() >= 2 {
        assert_eq!(
            signatures[1] as usize,
            archive1.len() + 64,
            "Second signature should be after first archive + padding"
        );
    }
}

/// Tests that recovery handles empty/corrupted data gracefully.
#[test]
fn test_recovery_handles_no_signature() {
    let no_archive_data = b"This data contains no 7z signature anywhere.";
    let cursor = Cursor::new(no_archive_data.to_vec());

    let result =
        recover_archive(cursor, RecoveryOptions::default()).expect("Recovery should not panic");

    assert_eq!(result.status, RecoveryStatus::Failed);
    assert!(result.archive.is_none());
    assert!(
        !result.warnings.is_empty(),
        "Should have warning about no signature"
    );
}

/// Tests recovery options builder.
#[test]
fn test_recovery_options_configuration() {
    let options = RecoveryOptions::new()
        .search_limit(512 * 1024)
        .validate_crcs(false)
        .skip_corrupt_entries(true)
        .try_multiple_headers(true);

    assert_eq!(options.search_limit, 512 * 1024);
    assert!(!options.validate_crcs);
    assert!(options.skip_corrupt_entries);
    assert!(options.try_multiple_headers);
}

/// Tests recovery with search limit.
#[test]
fn test_recovery_with_search_limit() {
    let entries = [("test.txt", b"Content" as &[u8])];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Put archive far into the data
    let mut far_away = vec![0u8; 2048]; // 2KB of zeros
    far_away.extend(&archive_bytes);

    // Try with small search limit (should fail)
    let cursor1 = Cursor::new(far_away.clone());
    let result1 = recover_archive(cursor1, RecoveryOptions::new().search_limit(1024))
        .expect("Recovery failed");

    assert_eq!(
        result1.status,
        RecoveryStatus::Failed,
        "Should not find archive with small search limit"
    );

    // Try with large search limit (should succeed)
    let cursor2 = Cursor::new(far_away);
    let result2 = recover_archive(cursor2, RecoveryOptions::new().search_limit(4096))
        .expect("Recovery failed");

    assert!(
        result2.archive.is_some(),
        "Should find archive with larger search limit"
    );
}

/// Tests recovery result metrics.
#[test]
fn test_recovery_result_metrics() {
    let entries = [
        ("a.txt", b"A" as &[u8]),
        ("b.txt", b"B"),
        ("c.txt", b"C"),
        ("d.txt", b"D"),
    ];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let cursor = Cursor::new(archive_bytes);
    let result = recover_archive(cursor, RecoveryOptions::default()).expect("Recovery failed");

    assert_eq!(result.total_entries(), 4);
    assert_eq!(result.recovered_count(), 4);
    assert_eq!(result.failed_count(), 0);
    assert!((result.recovery_rate() - 1.0).abs() < f64::EPSILON);
}
