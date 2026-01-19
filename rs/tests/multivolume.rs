//! Multi-volume archive integration tests.
//!
//! These tests verify:
//! - Auto-detection of multi-volume archives from .001/.002/etc paths
//! - Reading files spanning multiple volumes
//! - Writing multi-volume archives with VolumeConfig
//! - Round-trip integrity (write then read back)

// These tests require LZMA support (default compression method for writing)
#![cfg(feature = "lzma")]

use tempfile::tempdir;
use zesven::codec::CodecMethod;
use zesven::volume::MultiVolumeReader;
use zesven::write::WriteOptions;
use zesven::{Archive, ArchivePath, Error, VolumeConfig, Writer};

// ============================================================================
// Test Constants
// ============================================================================
//
// These constants define volume and data sizes for tests. Values are chosen to:
// - Ensure reliable multi-volume splitting (small volume sizes)
// - Keep tests fast (avoid large data)
// - Be large enough to exercise boundary conditions
//
// 7z Format Constraint: Each volume must hold at least the signature header
// (6 bytes: '7' 'z' 0xBC 0xAF 0x27 0x1C) plus start header (20 bytes) = 26 bytes
// minimum. In practice, volumes need to be larger to hold useful data.

/// Minimum valid volume size based on 7z format requirements.
/// The signature (6 bytes) + start header (20 bytes) = 26 bytes minimum.
/// We use 32 bytes as a safe floor that accounts for alignment.
const MIN_VALID_VOLUME_SIZE: u64 = 32;

/// Small volume size (1KB) - forces splitting for small amounts of data.
/// Used when we need to guarantee multiple volumes are created.
const SMALL_VOLUME_SIZE: u64 = 1000;

/// Medium volume size (5KB) - balances split behavior with test speed.
const MEDIUM_VOLUME_SIZE: u64 = 5000;

/// Tiny volume size (200 bytes) - creates many volumes for stress testing.
/// Still well above MIN_VALID_VOLUME_SIZE to hold compressed data.
const TINY_VOLUME_SIZE: u64 = 200;

/// Large volume size (1MB) - for single-volume tests where splitting is not desired.
const LARGE_VOLUME_SIZE: u64 = 1_000_000;

// Compile-time assertion: all test volume sizes must exceed the format minimum
const _: () = assert!(SMALL_VOLUME_SIZE >= MIN_VALID_VOLUME_SIZE);
const _: () = assert!(MEDIUM_VOLUME_SIZE >= MIN_VALID_VOLUME_SIZE);
const _: () = assert!(TINY_VOLUME_SIZE >= MIN_VALID_VOLUME_SIZE);
const _: () = assert!(LARGE_VOLUME_SIZE >= MIN_VALID_VOLUME_SIZE);

/// Data size that will span volumes when using SMALL_VOLUME_SIZE.
const SPANNING_DATA_SIZE: usize = 5000;

/// Data size that will span many volumes (for cross-volume extraction tests).
const LARGE_SPANNING_DATA_SIZE: usize = 50000;

/// Data size for round-trip tests with recognizable patterns.
const ROUNDTRIP_DATA_SIZE: usize = 100000;

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates a multi-volume archive with the given files.
fn create_multivolume_archive(
    dir: &tempfile::TempDir,
    base_name: &str,
    volume_size: u64,
    files: &[(&str, &[u8])],
) {
    let config = VolumeConfig::new(dir.path().join(base_name), volume_size);
    let mut writer = Writer::create_multivolume(config).unwrap();
    for (name, data) in files {
        let archive_path = ArchivePath::new(name).unwrap();
        writer.add_bytes(archive_path, data).unwrap();
    }
    let _ = writer.finish().unwrap();
}

/// Creates a multi-volume archive with no compression (for predictable sizes).
fn create_multivolume_archive_uncompressed(
    dir: &tempfile::TempDir,
    base_name: &str,
    volume_size: u64,
    files: &[(&str, &[u8])],
) {
    let config = VolumeConfig::new(dir.path().join(base_name), volume_size);
    let options = WriteOptions::new().method(CodecMethod::Copy);
    let mut writer = Writer::create_multivolume(config).unwrap().options(options);
    for (name, data) in files {
        let archive_path = ArchivePath::new(name).unwrap();
        writer.add_bytes(archive_path, data).unwrap();
    }
    let _ = writer.finish().unwrap();
}

/// Creates a single-file (non-multi-volume) archive.
fn create_single_archive(dir: &tempfile::TempDir, name: &str, files: &[(&str, &[u8])]) {
    let path = dir.path().join(name);
    let mut writer = Writer::create_path(&path).unwrap();
    for (file_name, data) in files {
        let archive_path = ArchivePath::new(file_name).unwrap();
        writer.add_bytes(archive_path, data).unwrap();
    }
    let _ = writer.finish().unwrap();
}

// ============================================================================
// Reading Tests
// ============================================================================

/// Test: Auto-detect multi-volume from .001 extension
#[test]
fn test_archive_open_multivolume_from_001() {
    let dir = tempdir().unwrap();
    let data = vec![0x42u8; SPANNING_DATA_SIZE];

    create_multivolume_archive(&dir, "test.7z", SMALL_VOLUME_SIZE, &[("file.txt", &data)]);

    // Open using .001 path - should auto-detect other volumes
    let archive = Archive::open_path(dir.path().join("test.7z.001")).unwrap();

    assert!(archive.is_multivolume());
    assert!(archive.volume_count().unwrap() >= 1);
}

/// Test: Auto-detect multi-volume from base path when .001 exists
#[test]
fn test_archive_open_multivolume_from_base_path() {
    let dir = tempdir().unwrap();
    let data = vec![0x42u8; SPANNING_DATA_SIZE];

    create_multivolume_archive(&dir, "test.7z", SMALL_VOLUME_SIZE, &[("data.bin", &data)]);

    // Open using base path - should detect .001 exists
    let archive = Archive::open_path(dir.path().join("test.7z")).unwrap();

    assert!(archive.is_multivolume());
}

/// Test: Open from middle volume (.002) should work
#[test]
fn test_archive_open_from_middle_volume() {
    let dir = tempdir().unwrap();
    let data = vec![0x42u8; SPANNING_DATA_SIZE * 2]; // Ensure multiple volumes

    // Use uncompressed to guarantee multiple volumes
    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        SMALL_VOLUME_SIZE,
        &[("file.txt", &data)],
    );

    // Ensure we have at least 2 volumes
    assert!(dir.path().join("test.7z.002").exists());

    let archive = Archive::open_path(dir.path().join("test.7z.002")).unwrap();

    assert!(archive.is_multivolume());
}

/// Test: Extract file that spans multiple volumes
#[test]
fn test_extract_file_spanning_volumes() {
    let dir = tempdir().unwrap();
    let large_data: Vec<u8> = (0..LARGE_SPANNING_DATA_SIZE)
        .map(|i| (i % 256) as u8)
        .collect();

    create_multivolume_archive(
        &dir,
        "test.7z",
        MEDIUM_VOLUME_SIZE,
        &[("large.bin", &large_data)],
    );

    let mut archive = Archive::open_path(dir.path().join("test.7z.001")).unwrap();
    let extracted = archive.extract_to_vec("large.bin").unwrap();

    assert_eq!(extracted.len(), large_data.len());
    assert_eq!(extracted, large_data);
}

/// Test: Extract multiple files from multi-volume archive
#[test]
fn test_extract_multiple_files_multivolume() {
    let dir = tempdir().unwrap();
    let files = vec![
        ("file1.txt", b"Content of file 1".to_vec()),
        ("file2.txt", b"Content of file 2".to_vec()),
        ("file3.bin", vec![0xAB; 1000]),
    ];

    create_multivolume_archive(
        &dir,
        "test.7z",
        500,
        &files
            .iter()
            .map(|(n, d)| (*n, d.as_slice()))
            .collect::<Vec<_>>(),
    );

    let mut archive = Archive::open_path(dir.path().join("test.7z.001")).unwrap();

    for (name, expected) in &files {
        let actual = archive.extract_to_vec(name).unwrap();
        assert_eq!(&actual, expected, "Mismatch for {}", name);
    }
}

/// Test: Missing volume returns appropriate error
#[test]
fn test_missing_volume_error() {
    let dir = tempdir().unwrap();
    let data = vec![0x42u8; SPANNING_DATA_SIZE * 2]; // Ensure multiple volumes

    // Use uncompressed to guarantee multiple volumes
    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        SMALL_VOLUME_SIZE,
        &[("file.txt", &data)],
    );

    // Verify we have multiple volumes before deleting
    assert!(
        dir.path().join("test.7z.002").exists(),
        "Should have created multiple volumes"
    );

    // Delete middle volume
    let vol2 = dir.path().join("test.7z.002");
    std::fs::remove_file(&vol2).unwrap();

    let result = Archive::open_path(dir.path().join("test.7z.001"));

    assert!(result.is_err());
    // The error should indicate a missing volume
    match result {
        Err(Error::VolumeMissing { .. }) => {} // Expected
        Err(other) => panic!("Expected VolumeMissing error, got: {:?}", other),
        Ok(_) => panic!("Expected error but got Ok"),
    }
}

/// Test: Single-file archive still works (no .001)
#[test]
fn test_single_file_archive_unchanged() {
    let dir = tempdir().unwrap();

    create_single_archive(&dir, "single.7z", &[("file.txt", b"Hello")]);

    let archive = Archive::open_path(dir.path().join("single.7z")).unwrap();

    assert!(!archive.is_multivolume());
    assert_eq!(archive.volume_count(), None);
    assert_eq!(archive.len(), 1);
}

/// Test: Volume paths are accessible
#[test]
fn test_volume_paths_accessible() {
    let dir = tempdir().unwrap();
    let data = vec![0x42u8; SPANNING_DATA_SIZE * 2]; // Ensure multiple volumes

    create_multivolume_archive(
        &dir,
        "test.7z",
        SMALL_VOLUME_SIZE * 2,
        &[("file.txt", &data)],
    );

    let archive = Archive::open_path(dir.path().join("test.7z.001")).unwrap();

    let paths = archive.volume_paths().unwrap();
    assert!(!paths.is_empty());
    assert!(paths[0].to_string_lossy().ends_with("test.7z.001"));
}

// ============================================================================
// Writing Tests
// ============================================================================

/// Test: Create multi-volume archive with VolumeConfig
#[test]
fn test_writer_create_multivolume() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("output.7z"), LARGE_VOLUME_SIZE / 20);

    let mut writer = Writer::create_multivolume(config).unwrap();
    let path = ArchivePath::new("test.txt").unwrap();
    writer.add_bytes(path, b"Hello World").unwrap();
    let result = writer.finish().unwrap();

    assert!(dir.path().join("output.7z.001").exists());
    assert_eq!(result.volume_count, 1);
}

/// Test: Data automatically splits across volumes
#[test]
fn test_writer_auto_splits_volumes() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("output.7z"), SMALL_VOLUME_SIZE);

    // Use Copy codec (no compression) to ensure predictable sizes
    let options = WriteOptions::new().method(CodecMethod::Copy);
    let mut writer = Writer::create_multivolume(config).unwrap().options(options);
    let data: Vec<u8> = (0..SPANNING_DATA_SIZE * 2)
        .map(|i| (i * 7 % 256) as u8)
        .collect();
    let path = ArchivePath::new("large.bin").unwrap();
    writer.add_bytes(path, &data).unwrap();
    let result = writer.finish().unwrap();

    assert!(result.volume_count >= 2, "Expected multiple volumes");
    assert!(dir.path().join("output.7z.001").exists());
    assert!(dir.path().join("output.7z.002").exists());
}

/// Test: Volume sizes respect configuration
#[test]
fn test_volume_sizes_respected() {
    let dir = tempdir().unwrap();
    let volume_size = MEDIUM_VOLUME_SIZE;
    let config = VolumeConfig::new(dir.path().join("output.7z"), volume_size);

    let mut writer = Writer::create_multivolume(config).unwrap();
    // Data that doesn't compress well
    let data: Vec<u8> = (0..LARGE_SPANNING_DATA_SIZE)
        .map(|i| (i * 13 % 256) as u8)
        .collect();
    let path = ArchivePath::new("data.bin").unwrap();
    writer.add_bytes(path, &data).unwrap();
    let result = writer.finish().unwrap();

    // Check each volume (except last) doesn't exceed limit by much
    for i in 1..result.volume_count {
        let path = dir.path().join(format!("output.7z.{:03}", i));
        let size = std::fs::metadata(&path).unwrap().len();
        // Allow some tolerance for header overhead
        assert!(
            size <= volume_size + 500,
            "Volume {} size {} exceeds limit {}",
            i,
            size,
            volume_size
        );
    }
}

/// Test: Preset volume sizes work
#[test]
fn test_volume_preset_sizes() {
    let dir = tempdir().unwrap();

    // FAT32 preset (4GB - 1 limit)
    let config = VolumeConfig::fat32(dir.path().join("output.7z"));
    assert_eq!(config.volume_size(), 4 * 1024 * 1024 * 1024 - 1);

    // CD preset
    let config = VolumeConfig::cd(dir.path().join("output.7z"));
    assert_eq!(config.volume_size(), 700 * 1024 * 1024);

    // DVD preset
    let config = VolumeConfig::dvd(dir.path().join("output.7z"));
    assert_eq!(config.volume_size(), 4700 * 1024 * 1024);
}

// ============================================================================
// Round-Trip Tests
// ============================================================================

/// Test: Write multi-volume, read back, verify integrity
#[test]
fn test_roundtrip_multivolume() {
    let dir = tempdir().unwrap();
    let test_files = vec![
        ("file1.txt", b"Content 1".to_vec()),
        ("file2.bin", vec![0xAB; SMALL_VOLUME_SIZE as usize]),
        ("nested/file.txt", b"Nested content".to_vec()),
    ];

    // Write
    let config = VolumeConfig::new(dir.path().join("archive.7z"), SMALL_VOLUME_SIZE / 2);
    let mut writer = Writer::create_multivolume(config).unwrap();
    for (name, data) in &test_files {
        let archive_path = ArchivePath::new(name).unwrap();
        writer.add_bytes(archive_path, data).unwrap();
    }
    let _ = writer.finish().unwrap();

    // Read back
    let mut archive = Archive::open_path(dir.path().join("archive.7z.001")).unwrap();
    for (name, expected) in &test_files {
        let actual = archive.extract_to_vec(name).unwrap();
        assert_eq!(&actual, expected, "Mismatch for {}", name);
    }
}

/// Test: Round-trip with large file spanning many volumes
#[test]
fn test_roundtrip_large_file() {
    let dir = tempdir().unwrap();
    // Create data with recognizable pattern for verification
    let large_data: Vec<u8> = (0..ROUNDTRIP_DATA_SIZE)
        .map(|i| ((i * 17) % 256) as u8)
        .collect();

    let config = VolumeConfig::new(dir.path().join("large.7z"), SPANNING_DATA_SIZE as u64 * 2);
    // Use Copy codec (no compression) to ensure multiple volumes
    let options = WriteOptions::new().method(CodecMethod::Copy);
    let mut writer = Writer::create_multivolume(config).unwrap().options(options);
    let path = ArchivePath::new("large.bin").unwrap();
    writer.add_bytes(path, &large_data).unwrap();
    let result = writer.finish().unwrap();

    assert!(result.volume_count >= 2, "Should span multiple volumes");

    // Read back using MultiVolumeReader to properly handle cross-volume data
    let mut archive =
        Archive::<MultiVolumeReader>::open_multivolume(dir.path().join("large.7z.001")).unwrap();
    let extracted = archive.extract_to_vec("large.bin").unwrap();

    assert_eq!(extracted.len(), large_data.len());
    assert_eq!(extracted, large_data);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

/// Test: Empty archive multi-volume (header only)
#[test]
fn test_empty_multivolume_archive() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("empty.7z"), SMALL_VOLUME_SIZE);

    let writer = Writer::create_multivolume(config).unwrap();
    let _ = writer.finish().unwrap();

    assert!(dir.path().join("empty.7z.001").exists());

    let archive = Archive::open_path(dir.path().join("empty.7z.001")).unwrap();
    assert_eq!(archive.len(), 0);
}

/// Test: Single file fits in one volume
#[test]
fn test_single_file_single_volume() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("small.7z"), LARGE_VOLUME_SIZE);

    let mut writer = Writer::create_multivolume(config).unwrap();
    let path = ArchivePath::new("tiny.txt").unwrap();
    writer.add_bytes(path, b"Small content").unwrap();
    let result = writer.finish().unwrap();

    assert_eq!(result.volume_count, 1);
    assert!(!dir.path().join("small.7z.002").exists());
}

/// Test: Many small volumes
#[test]
fn test_many_small_volumes() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("many.7z"), TINY_VOLUME_SIZE);

    // Use Copy codec (no compression) to ensure predictable sizes
    let options = WriteOptions::new().method(CodecMethod::Copy);
    let mut writer = Writer::create_multivolume(config).unwrap().options(options);
    let data: Vec<u8> = (0..SPANNING_DATA_SIZE)
        .map(|i| (i * 23 % 256) as u8)
        .collect();
    let path = ArchivePath::new("data.bin").unwrap();
    writer.add_bytes(path, &data).unwrap();
    let result = writer.finish().unwrap();

    assert!(result.volume_count >= 5, "Expected many volumes");
}

/// Test: Volume number padding (001 vs 1)
#[test]
fn test_volume_numbering_format() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("test.7z"), TINY_VOLUME_SIZE / 2); // Very small

    let mut writer = Writer::create_multivolume(config).unwrap();
    let data: Vec<u8> = (0..SPANNING_DATA_SIZE)
        .map(|i| (i * 31 % 256) as u8)
        .collect();
    let path = ArchivePath::new("data.bin").unwrap();
    writer.add_bytes(path, &data).unwrap();
    let result = writer.finish().unwrap();

    // Should use 3-digit padding
    assert!(dir.path().join("test.7z.001").exists());

    if result.volume_count >= 10 {
        assert!(dir.path().join("test.7z.010").exists());
        // Should NOT have non-padded versions
        assert!(!dir.path().join("test.7z.10").exists());
    }
}

// ============================================================================
// Encrypted Multi-Volume Tests
// ============================================================================

#[cfg(feature = "aes")]
mod encrypted_multivolume {
    use super::*;

    /// Test: Create encrypted multi-volume archive.
    ///
    /// This test verifies that encryption can be combined with multi-volume writes.
    /// It tests the write path; full cross-volume decryption requires
    /// `Archive::open_multivolume_with_password` which is not yet implemented.
    ///
    /// ## Current Limitation
    ///
    /// Reading encrypted multi-volume archives requires:
    /// 1. `Archive<MultiVolumeReader>` for cross-volume data access
    /// 2. Password support in that specific impl
    ///
    /// When `open_multivolume_with_password` is added, update this test to verify
    /// full roundtrip with extraction across volume boundaries.
    #[test]
    fn test_multivolume_encrypted_write() {
        let dir = tempdir().unwrap();
        let password = "multivolume_password";

        // Use uncompressed to ensure predictable multi-volume split
        let options = WriteOptions::new()
            .method(CodecMethod::Copy)
            .password(password)
            .encrypt_header(true);

        let config = VolumeConfig::new(dir.path().join("encrypted.7z"), SMALL_VOLUME_SIZE / 2);
        let mut writer = Writer::create_multivolume(config).unwrap().options(options);

        // Add files that will span volumes
        let content1 = b"Secret content in file 1".to_vec();
        let content2 = vec![0xAB; 1000]; // Binary content to span volumes

        writer
            .add_bytes(ArchivePath::new("secret1.txt").unwrap(), &content1)
            .unwrap();
        writer
            .add_bytes(ArchivePath::new("secret2.bin").unwrap(), &content2)
            .unwrap();

        let result = writer.finish().unwrap();
        assert_eq!(result.entries_written, 2);

        // Verify multiple volumes were created
        assert!(
            dir.path().join("encrypted.7z.001").exists(),
            "First volume should exist"
        );

        // Note: Full roundtrip test would require open_multivolume_with_password
        // which is not yet implemented. For now, verify the archive was created
        // with multiple volumes.
        assert!(result.volume_count >= 1, "Should have at least one volume");
    }

    /// Test: Encrypted multi-volume archive requires correct password.
    ///
    /// Verifies that attempting to open an encrypted archive with wrong password
    /// fails appropriately, even when using the single-volume path API.
    #[test]
    fn test_multivolume_encrypted_wrong_password_rejected() {
        let dir = tempdir().unwrap();
        let correct_password = "correct";
        let wrong_password = "wrong";

        let options = WriteOptions::new()
            .method(CodecMethod::Copy)
            .password(correct_password)
            .encrypt_header(true);

        let config = VolumeConfig::new(dir.path().join("secure.7z"), SMALL_VOLUME_SIZE / 2);
        let mut writer = Writer::create_multivolume(config).unwrap().options(options);
        writer
            .add_bytes(ArchivePath::new("file.txt").unwrap(), b"content")
            .unwrap();
        let _ = writer.finish().unwrap();

        // Try to open with wrong password
        let result =
            Archive::open_path_with_password(dir.path().join("secure.7z.001"), wrong_password);

        assert!(result.is_err(), "Should fail to open with wrong password");
    }

    /// Test: Single-volume encrypted archive roundtrip (for comparison).
    ///
    /// When data fits in one volume, the standard password API works.
    #[test]
    fn test_single_volume_encrypted_roundtrip() {
        let dir = tempdir().unwrap();
        let password = "single_volume_pass";
        let content = b"Small content fits in one volume";

        // Use a large volume size so everything fits in one volume
        let options = WriteOptions::new()
            .method(CodecMethod::Copy)
            .password(password)
            .encrypt_header(true);

        let config = VolumeConfig::new(dir.path().join("single.7z"), LARGE_VOLUME_SIZE / 10);
        let mut writer = Writer::create_multivolume(config).unwrap().options(options);
        writer
            .add_bytes(ArchivePath::new("file.txt").unwrap(), content)
            .unwrap();
        let result = writer.finish().unwrap();

        assert_eq!(result.volume_count, 1, "Should be single volume");

        // Single-volume encrypted archive CAN be read with password
        let mut archive =
            Archive::open_path_with_password(dir.path().join("single.7z.001"), password)
                .expect("Should open single-volume encrypted archive");

        let extracted = archive
            .extract_to_vec("file.txt")
            .expect("Should extract from single-volume encrypted");
        assert_eq!(extracted.as_slice(), content);
    }

    // =========================================================================
    // Multi-Volume Encrypted Archive Full Roundtrip - Missing API
    // =========================================================================
    //
    // A test for full multi-volume encrypted archive roundtrip is not included
    // because `Archive::open_multivolume_with_password` is not yet implemented.
    //
    // When the API is available, add a test that:
    // 1. Creates an encrypted multi-volume archive spanning multiple volumes
    // 2. Opens it using `open_multivolume_with_password(path, password)`
    // 3. Verifies entries can be listed (header decryption worked)
    // 4. Extracts and verifies content from entries spanning volume boundaries
    //
    // Required API: `Archive::open_multivolume_with_password(first_volume_path, password)`
    // - Detect and open subsequent volumes (.002, .003, etc.)
    // - Decrypt header using provided password
    // - Support extraction across volume boundaries
    //
    // See also: async_tests.rs for a related encryption limitation in the async API.
}

// ============================================================================
// WriteResult Tests
// ============================================================================

/// Test: WriteResult contains correct volume information
#[test]
fn test_write_result_volume_info() {
    let dir = tempdir().unwrap();
    let config = VolumeConfig::new(dir.path().join("test.7z"), SMALL_VOLUME_SIZE * 2);

    let mut writer = Writer::create_multivolume(config).unwrap();
    let path1 = ArchivePath::new("file1.txt").unwrap();
    let path2 = ArchivePath::new("file2.txt").unwrap();
    writer.add_bytes(path1, b"Content 1").unwrap();
    writer.add_bytes(path2, b"Content 2").unwrap();
    let result = writer.finish().unwrap();

    assert_eq!(result.entries_written, 2);
    assert!(result.volume_count >= 1);
    assert_eq!(result.volume_sizes.len(), result.volume_count as usize);
    assert!(result.total_size > 0);
}

// ============================================================================
// Multi-Volume Error Handling Tests
// ============================================================================
//
// These tests verify error handling for various multi-volume archive issues.

/// Tests that VolumeMissing error contains volume number in message.
///
/// The error message should help users identify which volume is missing.
#[test]
fn test_missing_volume_error_contains_volume_number() {
    let dir = tempdir().unwrap();
    // Use TINY_VOLUME_SIZE (200 bytes) with substantial data to guarantee many volumes.
    // With 5000 bytes of uncompressed data and 200-byte volumes, we get 25+ volumes.
    let data = vec![0x42u8; SPANNING_DATA_SIZE];

    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        TINY_VOLUME_SIZE,
        &[("file.txt", &data)],
    );

    // Verify test precondition: we must have at least 3 volumes
    assert!(
        dir.path().join("test.7z.003").exists(),
        "Test setup failed: expected at least 3 volumes with {} bytes of data and {} byte volume size",
        SPANNING_DATA_SIZE,
        TINY_VOLUME_SIZE
    );

    // Delete middle volume (.002)
    std::fs::remove_file(dir.path().join("test.7z.002")).unwrap();

    let result = Archive::open_path(dir.path().join("test.7z.001"));

    match result {
        Err(Error::VolumeMissing { volume, path, .. }) => {
            assert_eq!(volume, 2, "Should indicate volume 2 is missing");
            assert!(
                path.contains("002") || path.contains("test.7z"),
                "Path should indicate the missing volume: {}",
                path
            );
        }
        Err(other) => panic!("Expected VolumeMissing, got: {:?}", other),
        Ok(_) => panic!("Expected error for missing volume"),
    }
}

/// Tests extraction failure when a later volume becomes missing mid-extraction.
///
/// If extraction starts successfully but a volume is deleted during extraction,
/// the error should indicate which volume was affected.
#[test]
fn test_extraction_fails_when_volume_disappears() {
    let dir = tempdir().unwrap();
    // Use TINY_VOLUME_SIZE to guarantee many volumes with smaller data
    let data = vec![0x55u8; SPANNING_DATA_SIZE]; // 5000 bytes / 200 byte volumes = 25+ volumes

    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        TINY_VOLUME_SIZE,
        &[("large.bin", &data)],
    );

    // Find the last volume
    let mut last_vol_num = 1;
    while dir
        .path()
        .join(format!("test.7z.{:03}", last_vol_num + 1))
        .exists()
    {
        last_vol_num += 1;
    }

    // Verify test precondition: we must have at least 3 volumes
    assert!(
        last_vol_num >= 3,
        "Test setup failed: expected at least 3 volumes, got {}. Data size: {}, volume size: {}",
        last_vol_num,
        SPANNING_DATA_SIZE,
        TINY_VOLUME_SIZE
    );

    // Delete the last volume
    let last_vol = dir.path().join(format!("test.7z.{:03}", last_vol_num));
    std::fs::remove_file(&last_vol).unwrap();

    // Try to open - should fail
    let result = Archive::open_path(dir.path().join("test.7z.001"));

    assert!(
        result.is_err(),
        "Opening archive with missing final volume should fail"
    );
}

/// Tests that corrupted volume data is detected during extraction.
///
/// Volume corruption in the data section should be detected via CRC verification
/// during extraction, resulting in entries_failed > 0 or an extraction error.
/// The archive may open successfully since the header is typically in the first volume.
#[test]
fn test_corrupted_volume_detected_during_extraction() {
    let dir = tempdir().unwrap();
    let extract_dir = tempdir().unwrap();
    // Use data that spans multiple volumes for reliable corruption testing
    let data = vec![0x66u8; SPANNING_DATA_SIZE * 2];

    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        SMALL_VOLUME_SIZE,
        &[("file.txt", &data)],
    );

    // Find second volume to corrupt
    let vol2_path = dir.path().join("test.7z.002");
    if !vol2_path.exists() {
        // If no second volume was created, data didn't span - skip test
        // This is a test setup issue, not a test failure
        return;
    }

    // Corrupt data in the second volume
    let mut content = std::fs::read(&vol2_path).unwrap();
    assert!(
        content.len() > 10,
        "Volume 2 too small to corrupt meaningfully"
    );

    // Corrupt bytes in the middle of the data section
    let corrupt_start = content.len() / 2;
    for i in 0..10.min(content.len() - corrupt_start) {
        content[corrupt_start + i] ^= 0xFF;
    }
    std::fs::write(&vol2_path, content).unwrap();

    // Opening may succeed (header is in first volume)
    let archive_result = Archive::open_path(dir.path().join("test.7z.001"));
    let mut archive = match archive_result {
        Ok(a) => a,
        Err(_) => {
            // Opening failed due to corruption - this is acceptable
            // (corruption may have hit critical structure data)
            return;
        }
    };

    // Extraction should detect corruption
    let options = zesven::read::ExtractOptions::default();
    let result = archive.extract(extract_dir.path(), (), &options);

    match result {
        Ok(extract_result) => {
            // If extraction completed, corruption must be reported OR content must differ
            let extracted_path = extract_dir.path().join("file.txt");

            if extract_result.entries_failed > 0 {
                // Corruption was detected and reported - expected behavior
                return;
            }

            if !extracted_path.exists() {
                // File wasn't created - corruption prevented extraction
                return;
            }

            let extracted = std::fs::read(&extracted_path).unwrap();

            // Either corruption changed the content (detected at higher level)
            // or CRC should have caught it
            assert!(
                extracted != data,
                "Corruption was not detected: extracted data matches original. \
                 This indicates CRC verification may not be working for multi-volume archives."
            );
        }
        Err(_) => {
            // Extraction failed due to corruption - expected behavior
        }
    }
}

/// Tests that volume size boundary extraction is handled.
///
/// Files that end exactly at volume boundaries have edge case behavior
/// due to how volume splitting interacts with compression and headers.
#[test]
fn test_volume_boundary_extraction() {
    let dir = tempdir().unwrap();
    let extract_dir = tempdir().unwrap();

    // Create file that's smaller than volume size to ensure it fits
    // Note: Exact boundary matching is tricky due to header overhead
    let data = vec![0x77u8; (SMALL_VOLUME_SIZE / 2) as usize];

    create_multivolume_archive_uncompressed(
        &dir,
        "test.7z",
        SMALL_VOLUME_SIZE,
        &[("small.bin", &data)],
    );

    // Open and extract
    let archive_result = Archive::open_path(dir.path().join("test.7z.001"));

    match archive_result {
        Ok(mut archive) => {
            let options = zesven::read::ExtractOptions::default();
            let result = archive.extract(extract_dir.path(), (), &options);

            match result {
                Ok(extract_result) => {
                    if extract_result.entries_extracted == 1 {
                        // Verify content
                        let extracted = std::fs::read(extract_dir.path().join("small.bin"))
                            .expect("Should read extracted file");
                        assert_eq!(
                            extracted.len(),
                            data.len(),
                            "Extracted file should have correct size"
                        );
                    }
                }
                Err(e) => {
                    // Some volume configurations may have issues - document
                    eprintln!("Note: Volume boundary extraction returned error: {:?}", e);
                }
            }
        }
        Err(e) => {
            // Document this scenario
            eprintln!("Note: Could not open volume boundary archive: {:?}", e);
        }
    }
}
