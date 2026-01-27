//! Tests for zstdmt skippable frame format support.
//!
//! These tests verify that zesven can correctly decompress archives created
//! with 7-Zip forks (7-Zip-zstd, NanaZip) that use zstdmt internally.
//!
//! # Background
//!
//! Archives created with these tools wrap LZ4 and Brotli compressed data in
//! "skippable frames" - a format from zstd that allows embedding metadata.
//! Standard decoders don't understand this format and fail with errors like
//! `SkippableFrame(4)` for LZ4 or `Invalid Data` for Brotli.
//!
//! # Test Files
//!
//! - `zstdmt-lz4.7z`: Archive with LZ4-compressed LICENSE file using skippable frames
//! - `zstdmt-brotli.7z`: Archive with Brotli-compressed LICENSE file using skippable frames
//!
//! Source: <https://github.com/hasenbanck/sevenz-rust2/tree/main/tests/resources>
//!
//! # References
//!
//! - Issue: <https://github.com/AndreyAkinshin/zesven/issues/4>
//! - zstdmt format: <https://github.com/mcmilk/zstdmt>

// Only compile this test module when at least one relevant feature is enabled
#![cfg(any(feature = "lz4", feature = "brotli"))]

use std::path::PathBuf;

use zesven::read::{Archive, SelectAll, TestOptions};

/// Returns the path to the test fixtures directory.
fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Tests decompression of LZ4 archive with zstdmt skippable frames.
///
/// The archive contains a LICENSE file compressed with LZ4 using the zstdmt
/// frame format. Without skippable frame support, this fails with:
/// `I/O error: SkippableFrame(4)`
#[test]
#[cfg(feature = "lz4")]
fn decompress_lz4_with_skippable_frames() {
    let path = fixtures_path().join("zstdmt-lz4.7z");
    let mut archive = Archive::open_path(&path).expect("Failed to open archive");

    let result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Failed to test archive");

    assert!(
        result.failures.is_empty(),
        "LZ4 with skippable frames should decompress successfully: {:?}",
        result.failures
    );
}

/// Tests decompression of Brotli archive with zstdmt skippable frames.
///
/// The archive contains a LICENSE file compressed with Brotli using the zstdmt
/// frame format. Without skippable frame support, this fails with:
/// `I/O error: Invalid Data`
#[test]
#[cfg(feature = "brotli")]
fn decompress_brotli_with_skippable_frames() {
    let path = fixtures_path().join("zstdmt-brotli.7z");
    let mut archive = Archive::open_path(&path).expect("Failed to open archive");

    let result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Failed to test archive");

    assert!(
        result.failures.is_empty(),
        "Brotli with skippable frames should decompress successfully: {:?}",
        result.failures
    );
}

/// Tests that extracted content from LZ4 skippable frame archive is correct.
#[test]
#[cfg(feature = "lz4")]
fn extract_lz4_with_skippable_frames() {
    let path = fixtures_path().join("zstdmt-lz4.7z");
    let mut archive = Archive::open_path(&path).expect("Failed to open archive");

    let content = archive
        .extract_to_vec("LICENSE")
        .expect("Failed to extract LICENSE");

    // The LICENSE file should contain Apache License text
    let text = String::from_utf8_lossy(&content);
    assert!(
        text.contains("Apache License"),
        "LICENSE should contain 'Apache License', got: {}...",
        &text[..text.len().min(100)]
    );
}

/// Tests that extracted content from Brotli skippable frame archive is correct.
#[test]
#[cfg(feature = "brotli")]
fn extract_brotli_with_skippable_frames() {
    let path = fixtures_path().join("zstdmt-brotli.7z");
    let mut archive = Archive::open_path(&path).expect("Failed to open archive");

    let content = archive
        .extract_to_vec("LICENSE")
        .expect("Failed to extract LICENSE");

    // The LICENSE file should contain Apache License text
    let text = String::from_utf8_lossy(&content);
    assert!(
        text.contains("Apache License"),
        "LICENSE should contain 'Apache License', got: {}...",
        &text[..text.len().min(100)]
    );
}
