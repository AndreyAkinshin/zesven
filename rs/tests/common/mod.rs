//! Shared test utilities for integration tests.
//!
//! This module provides common helper functions used across multiple test files.
//! Archive creation helpers are consolidated here to avoid duplication.
//!
//! Note: `#![allow(dead_code)]` is required because each integration test file
//! compiles as a separate crate and may only use a subset of these helpers.

#![allow(dead_code)]

use std::io::Cursor;
use zesven::{ArchivePath, WriteOptions, WriteResult, Writer};

/// Creates an in-memory archive with optional configuration.
///
/// This is the core implementation for archive creation in tests.
/// Returns both the archive bytes and the WriteResult metadata.
///
/// # Arguments
///
/// * `options` - Optional WriteOptions for customization (None = defaults)
/// * `entries` - A slice of (path, data) tuples representing files to add
///
/// # Returns
///
/// A tuple of (archive_bytes, WriteResult) on success, allowing callers
/// to verify both the raw archive and the write metadata.
///
/// # Example
///
/// ```ignore
/// use zesven::WriteOptions;
///
/// // With default options
/// let (bytes, result) = create_archive_with_result(None, &[("file.txt", b"content" as &[u8])]).unwrap();
///
/// // With custom options
/// let opts = WriteOptions::new().level(5);
/// let (bytes, result) = create_archive_with_result(Some(opts), &entries).unwrap();
/// ```
pub fn create_archive_with_result(
    options: Option<WriteOptions>,
    entries: &[(&str, &[u8])],
) -> zesven::Result<(Vec<u8>, WriteResult)> {
    let mut archive_bytes = Vec::new();
    let result = {
        let cursor = Cursor::new(&mut archive_bytes);
        let writer = Writer::create(cursor)?;

        let mut writer = match options {
            Some(opts) => writer.options(opts),
            None => writer,
        };

        for (name, data) in entries {
            let path = ArchivePath::new(name)?;
            writer.add_bytes(path, data)?;
        }

        writer.finish()?
    };
    Ok((archive_bytes, result))
}

/// Creates an in-memory archive with default options.
///
/// Convenience wrapper around [`create_archive_with_result`] that discards
/// the WriteResult metadata. Use this for simple archive creation when you
/// don't need to inspect write statistics.
///
/// Uses the default compression method (LZMA2 when enabled).
///
/// # Arguments
///
/// * `entries` - A slice of (path, data) tuples representing files to add
///
/// # Returns
///
/// The raw bytes of the created 7z archive on success.
///
/// # Example
///
/// ```ignore
/// let entries = [("file.txt", b"content" as &[u8])];
/// let archive_bytes = create_archive(&entries).unwrap();
/// ```
pub fn create_archive(entries: &[(&str, &[u8])]) -> zesven::Result<Vec<u8>> {
    create_archive_with_result(None, entries).map(|(bytes, _)| bytes)
}

/// Creates an in-memory archive with custom WriteOptions.
///
/// Convenience wrapper around [`create_archive_with_result`] that accepts
/// options but discards the WriteResult metadata.
///
/// # Arguments
///
/// * `options` - WriteOptions for configuring compression, encryption, etc.
/// * `entries` - A slice of (path, data) tuples representing files to add
///
/// # Returns
///
/// The raw bytes of the created 7z archive on success.
///
/// # Example
///
/// ```ignore
/// use zesven::WriteOptions;
/// use zesven::codec::CodecMethod;
///
/// let opts = WriteOptions::new().method(CodecMethod::Lzma2);
/// let archive = create_archive_with_options(opts, &entries).unwrap();
/// ```
pub fn create_archive_with_options(
    options: WriteOptions,
    entries: &[(&str, &[u8])],
) -> zesven::Result<Vec<u8>> {
    create_archive_with_result(Some(options), entries).map(|(bytes, _)| bytes)
}

/// Extracts the error from a Result, panicking if it's Ok.
///
/// This helper is useful for tests that expect an error. It provides a cleaner
/// alternative to `unwrap_err()` when the Ok type doesn't implement Debug.
///
/// # Panics
///
/// Panics if the result is `Ok(_)`.
///
/// # Example
///
/// ```ignore
/// use zesven::Error;
///
/// let result: Result<(), Error> = Err(Error::InvalidFormat("test".into()));
/// let error = expect_err(result);
/// assert!(matches!(error, Error::InvalidFormat(_)));
/// ```
pub fn expect_err<T, E>(result: Result<T, E>) -> E {
    match result {
        Ok(_) => panic!("Expected error but got Ok"),
        Err(e) => e,
    }
}

/// Verifies archive integrity and content by opening, testing, and extracting.
///
/// Performs three-phase verification:
/// 1. **CRC Test**: Runs `archive.test()` on all entries to verify decompression and checksums
/// 2. **Entry Count**: Verifies the archive contains exactly the expected number of files
/// 3. **Content Comparison**: Extracts each entry and compares against expected data byte-by-byte
///
/// # Arguments
///
/// * `archive_bytes` - The raw 7z archive bytes to verify
/// * `expected_entries` - Slice of (path, data) tuples representing expected contents
///
/// # Panics
///
/// Panics if:
/// - Archive opening or extraction fails
/// - CRC verification fails
/// - Entry count doesn't match
/// - Content doesn't match expected
pub fn verify_archive_contents(archive_bytes: &[u8], expected_entries: &[(&str, &[u8])]) {
    use std::io::Cursor;
    use zesven::read::{Archive, SelectAll, TestOptions};

    let cursor = Cursor::new(archive_bytes);
    let mut archive = Archive::open(cursor).expect("Failed to open archive for verification");

    // Test all entries (verifies decompression and CRC)
    let test_result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Archive test failed");
    assert_eq!(
        test_result.entries_failed, 0,
        "CRC verification failed: {:?}",
        test_result.failures
    );

    // Verify entry count
    let file_count = expected_entries.len();
    let archive_file_count = archive.entries().iter().filter(|e| !e.is_directory).count();
    assert_eq!(
        archive_file_count, file_count,
        "Entry count mismatch: expected {}, got {}",
        file_count, archive_file_count
    );

    // Verify content for each entry
    for (name, expected_data) in expected_entries {
        let mut found = false;
        for entry in archive.entries() {
            if entry.path.as_str() == *name {
                found = true;
                let extracted = archive
                    .extract_to_vec(name)
                    .unwrap_or_else(|e| panic!("Failed to extract '{}': {}", name, e));
                assert_eq!(
                    &extracted[..],
                    *expected_data,
                    "Content mismatch for '{}'",
                    name
                );
                break;
            }
        }
        assert!(found, "Entry '{}' not found in archive", name);
    }
}

/// Verifies an encrypted archive can be opened with the correct password.
///
/// Similar to [`verify_archive_contents`] but handles password-protected archives.
///
/// # Arguments
///
/// * `archive_bytes` - The raw encrypted 7z archive bytes
/// * `password` - The password used to create the archive
/// * `expected_entries` - Slice of (path, data) tuples representing expected contents
///
/// # Panics
///
/// Panics if:
/// - Archive opening fails (including wrong password)
/// - Decryption fails
/// - Extraction fails
/// - CRC verification fails
/// - Content doesn't match
#[cfg(feature = "aes")]
pub fn verify_encrypted_archive(
    archive_bytes: &[u8],
    password: &str,
    expected_entries: &[(&str, &[u8])],
) {
    use std::io::Cursor;
    use zesven::read::{Archive, SelectAll, TestOptions};

    let cursor = Cursor::new(archive_bytes);
    let mut archive =
        Archive::open_with_password(cursor, password).expect("Failed to open encrypted archive");

    // Test with password
    let test_result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Archive test failed");
    assert_eq!(
        test_result.entries_failed, 0,
        "Decryption failed with correct password: {:?}",
        test_result.failures
    );

    // Verify content matches
    for (name, expected_data) in expected_entries {
        let extracted = archive
            .extract_to_vec(name)
            .unwrap_or_else(|e| panic!("Failed to extract '{}': {}", name, e));
        assert_eq!(
            &extracted[..],
            *expected_data,
            "Content mismatch for '{}' after decryption",
            name
        );
    }
}
