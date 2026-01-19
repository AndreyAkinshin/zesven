//! Comprehensive codec combination tests for zesven.
//!
//! These tests verify full roundtrip integrity for all supported codec combinations:
//! - Write archives with various compression methods
//! - Read back and verify content matches exactly
//! - Test solid archive mode with multiple codecs
//! - Test encryption combinations (when aes feature enabled)
//!
//! Note: These tests require at least one codec feature to be enabled.

// Skip compilation when no codecs are enabled
#![cfg(any(
    feature = "lzma",
    feature = "lzma2",
    feature = "deflate",
    feature = "bzip2",
    feature = "ppmd",
    feature = "lz4",
    feature = "zstd",
    feature = "brotli"
))]

mod common;

use std::io::Cursor;
use zesven::WriteOptions;
use zesven::read::Archive;

/// Test data types for different compression scenarios.
mod test_data {
    // ==========================================================================
    // Test data size constants
    //
    // These sizes are chosen to balance test thoroughness against execution time:
    // - Large enough to exercise compression algorithms beyond trivial cases
    // - Small enough for fast test execution (sub-second per test)
    // - Powers of 10 for easy reasoning about compression ratios
    // ==========================================================================

    /// 10KB - exercises compression dictionary building without excessive runtime.
    /// Large enough to see compression patterns, small enough for fast tests.
    const TEXT_DATA_SIZE: usize = 10_000;

    /// 50KB - tests dictionary-based compression efficiency on highly redundant data.
    /// Larger size amplifies compression ratio differences between algorithms.
    const REPETITIVE_DATA_SIZE: usize = 50_000;

    /// 10KB - moderately compressible binary cycle (0-255 repeating).
    /// Tests codec handling of byte patterns that don't benefit from text modeling.
    const BINARY_DATA_SIZE: usize = 10_000;

    /// 5KB - incompressible random data (seeded for reproducibility).
    /// Smaller size since random data tests overhead rather than compression.
    const RANDOM_DATA_SIZE: usize = 5_000;

    /// Highly compressible text data (repeating patterns).
    pub fn text() -> Vec<u8> {
        let pattern = b"The quick brown fox jumps over the lazy dog. ";
        pattern
            .iter()
            .cycle()
            .take(TEXT_DATA_SIZE)
            .copied()
            .collect()
    }

    /// Repetitive data (highly compressible).
    pub fn repetitive() -> Vec<u8> {
        vec![b'A'; REPETITIVE_DATA_SIZE]
    }

    /// Binary data (moderately compressible).
    pub fn binary() -> Vec<u8> {
        (0u8..=255).cycle().take(BINARY_DATA_SIZE).collect()
    }

    /// Random/incompressible data (deterministically seeded for reproducibility).
    pub fn random() -> Vec<u8> {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        // Fixed seed ensures test reproducibility across runs
        let mut rng = StdRng::seed_from_u64(0xDEAD_BEEF_CAFE_1234);
        let mut data = vec![0u8; RANDOM_DATA_SIZE];
        rng.fill(&mut data[..]);
        data
    }

    /// Small data (tests overhead).
    pub fn small() -> Vec<u8> {
        b"Hello, World!".to_vec()
    }

    /// Empty data.
    pub fn empty() -> Vec<u8> {
        vec![]
    }

    /// Executable-like binary data (benefits from BCJ filters).
    pub fn executable_like() -> Vec<u8> {
        // Create data that mimics x86 executable patterns
        let mut data = Vec::with_capacity(20000);
        for i in 0..5000u32 {
            // Mix of CALL-like patterns (0xE8) and other x86-ish bytes
            if i % 7 == 0 {
                data.push(0xE8); // CALL opcode
                data.extend_from_slice(&i.to_le_bytes());
            } else if i % 11 == 0 {
                data.push(0xE9); // JMP opcode
                data.extend_from_slice(&i.to_le_bytes());
            } else {
                data.push((i & 0xFF) as u8);
            }
        }
        data
    }
}

// Archive creation and verification use common helpers directly.
// See tests/common/mod.rs for implementation.
use common::{create_archive_with_options, verify_archive_contents};

#[cfg(feature = "aes")]
use common::verify_encrypted_archive;

// =============================================================================
// Copy (No Compression) Tests
// =============================================================================

#[test]
fn test_copy_roundtrip() {
    use zesven::codec::CodecMethod;

    let data = test_data::binary();
    let entries = [("test.bin", data.as_slice())];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Copy), &entries)
            .expect("Failed to create archive");

    verify_archive_contents(&archive, &entries);
}

#[test]
fn test_copy_multiple_files() {
    use zesven::codec::CodecMethod;

    let text = test_data::text();
    let binary = test_data::binary();
    let small = test_data::small();
    let entries = [
        ("text.txt", text.as_slice()),
        ("data.bin", binary.as_slice()),
        ("small.txt", small.as_slice()),
    ];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Copy), &entries)
            .expect("Failed to create archive");

    verify_archive_contents(&archive, &entries);
}

#[test]
fn test_copy_empty_file() {
    use zesven::codec::CodecMethod;

    let empty = test_data::empty();
    let entries = [("empty.txt", empty.as_slice())];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Copy), &entries)
            .expect("Failed to create archive");

    verify_archive_contents(&archive, &entries);
}

// =============================================================================
// Codec Roundtrip Test Macros
// =============================================================================
//
// These macros reduce repetition across codec tests while preserving the same
// test coverage. Each macro generates tests that verify roundtrip integrity
// for a specific data type or scenario.

/// Generates a single-file roundtrip test for a codec with specific data type.
macro_rules! codec_data_test {
    ($test_name:ident, $method:expr, $data_fn:ident, $filename:literal) => {
        #[test]
        fn $test_name() {
            let data = test_data::$data_fn();
            let entries = [($filename, data.as_slice())];

            let archive =
                create_archive_with_options(WriteOptions::new().method($method), &entries)
                    .expect("Failed to create archive");

            verify_archive_contents(&archive, &entries);
        }
    };
}

/// Generates a multi-file roundtrip test for a codec.
macro_rules! codec_multiple_files_test {
    ($test_name:ident, $method:expr) => {
        #[test]
        fn $test_name() {
            let text = test_data::text();
            let binary = test_data::binary();
            let small = test_data::small();
            let entries = [
                ("docs/readme.txt", text.as_slice()),
                ("data/archive.bin", binary.as_slice()),
                ("hello.txt", small.as_slice()),
            ];

            let archive =
                create_archive_with_options(WriteOptions::new().method($method), &entries)
                    .expect("Failed to create archive");

            verify_archive_contents(&archive, &entries);
        }
    };
}

/// Generates a compression levels test for a codec.
macro_rules! codec_levels_test {
    ($test_name:ident, $method:expr, $levels:expr) => {
        #[test]
        fn $test_name() {
            let data = test_data::text();
            let entries = [("test.txt", data.as_slice())];

            for level in $levels {
                let archive = create_archive_with_options(
                    WriteOptions::new().method($method).level(level).unwrap(),
                    &entries,
                )
                .unwrap_or_else(|_| panic!("Failed to create archive at level {}", level));

                verify_archive_contents(&archive, &entries);
            }
        }
    };
}

// =============================================================================
// LZMA Tests
// =============================================================================

#[cfg(feature = "lzma")]
mod lzma_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_lzma_text, CodecMethod::Lzma, text, "document.txt");
    codec_data_test!(test_lzma_binary, CodecMethod::Lzma, binary, "data.bin");
    codec_data_test!(
        test_lzma_repetitive,
        CodecMethod::Lzma,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_lzma_random, CodecMethod::Lzma, random, "random.bin");
    codec_data_test!(test_lzma_small, CodecMethod::Lzma, small, "small.txt");
    codec_data_test!(test_lzma_empty, CodecMethod::Lzma, empty, "empty.txt");
    codec_multiple_files_test!(test_lzma_multiple_files, CodecMethod::Lzma);
    codec_levels_test!(test_lzma_levels, CodecMethod::Lzma, [1, 5, 9]);
}

// =============================================================================
// LZMA2 Tests
// =============================================================================

#[cfg(feature = "lzma2")]
mod lzma2_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_lzma2_text, CodecMethod::Lzma2, text, "document.txt");
    codec_data_test!(test_lzma2_binary, CodecMethod::Lzma2, binary, "data.bin");
    codec_data_test!(
        test_lzma2_repetitive,
        CodecMethod::Lzma2,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_lzma2_random, CodecMethod::Lzma2, random, "random.bin");
    codec_data_test!(test_lzma2_small, CodecMethod::Lzma2, small, "small.txt");
    codec_data_test!(test_lzma2_empty, CodecMethod::Lzma2, empty, "empty.txt");
    codec_data_test!(
        test_lzma2_executable_like,
        CodecMethod::Lzma2,
        executable_like,
        "program.exe"
    );
    codec_multiple_files_test!(test_lzma2_multiple_files, CodecMethod::Lzma2);
    codec_levels_test!(test_lzma2_levels, CodecMethod::Lzma2, [1, 5, 9]);
}

// =============================================================================
// Deflate Tests
// =============================================================================

#[cfg(feature = "deflate")]
mod deflate_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(
        test_deflate_text,
        CodecMethod::Deflate,
        text,
        "document.txt"
    );
    codec_data_test!(
        test_deflate_binary,
        CodecMethod::Deflate,
        binary,
        "data.bin"
    );
    codec_data_test!(
        test_deflate_repetitive,
        CodecMethod::Deflate,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(
        test_deflate_random,
        CodecMethod::Deflate,
        random,
        "random.bin"
    );
    codec_data_test!(test_deflate_small, CodecMethod::Deflate, small, "small.txt");
    codec_data_test!(test_deflate_empty, CodecMethod::Deflate, empty, "empty.txt");
    codec_levels_test!(test_deflate_levels, CodecMethod::Deflate, [1, 6, 9]);

    // Deflate uses a simpler multiple_files test with only 2 files (historical test structure)
    #[test]
    fn test_deflate_multiple_files() {
        let text = test_data::text();
        let binary = test_data::binary();
        let entries = [
            ("readme.txt", text.as_slice()),
            ("data.bin", binary.as_slice()),
        ];

        let archive =
            create_archive_with_options(WriteOptions::new().method(CodecMethod::Deflate), &entries)
                .expect("Failed to create archive");

        verify_archive_contents(&archive, &entries);
    }
}

// =============================================================================
// BZip2 Tests
// =============================================================================

#[cfg(feature = "bzip2")]
mod bzip2_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_bzip2_text, CodecMethod::BZip2, text, "document.txt");
    codec_data_test!(test_bzip2_binary, CodecMethod::BZip2, binary, "data.bin");
    codec_data_test!(
        test_bzip2_repetitive,
        CodecMethod::BZip2,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_bzip2_small, CodecMethod::BZip2, small, "small.txt");
    codec_data_test!(test_bzip2_empty, CodecMethod::BZip2, empty, "empty.txt");
    codec_multiple_files_test!(test_bzip2_multiple_files, CodecMethod::BZip2);
    codec_levels_test!(test_bzip2_levels, CodecMethod::BZip2, [1, 5, 9]);
}

// =============================================================================
// PPMd Tests
// =============================================================================

#[cfg(feature = "ppmd")]
mod ppmd_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_ppmd_text, CodecMethod::PPMd, text, "document.txt");
    codec_data_test!(test_ppmd_binary, CodecMethod::PPMd, binary, "data.bin");
    codec_data_test!(
        test_ppmd_repetitive,
        CodecMethod::PPMd,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_ppmd_random, CodecMethod::PPMd, random, "random.bin");
    codec_data_test!(test_ppmd_small, CodecMethod::PPMd, small, "small.txt");
    codec_data_test!(test_ppmd_empty, CodecMethod::PPMd, empty, "empty.txt");
    codec_multiple_files_test!(test_ppmd_multiple_files, CodecMethod::PPMd);
    codec_levels_test!(test_ppmd_levels, CodecMethod::PPMd, [1, 5, 9]);
}

// =============================================================================
// LZ4 Tests
// =============================================================================

#[cfg(feature = "lz4")]
mod lz4_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_lz4_text, CodecMethod::Lz4, text, "document.txt");
    codec_data_test!(test_lz4_binary, CodecMethod::Lz4, binary, "data.bin");
    codec_data_test!(
        test_lz4_repetitive,
        CodecMethod::Lz4,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_lz4_random, CodecMethod::Lz4, random, "random.bin");
    codec_data_test!(test_lz4_empty, CodecMethod::Lz4, empty, "empty.txt");
    codec_data_test!(test_lz4_small, CodecMethod::Lz4, small, "small.txt");
    codec_multiple_files_test!(test_lz4_multiple_files, CodecMethod::Lz4);

    // Note: LZ4 does not have configurable compression levels in the lz4_flex crate.
    // The Lz4EncoderOptions struct has no level field, so no levels test is needed.
}

// =============================================================================
// Zstd Tests
// =============================================================================

#[cfg(feature = "zstd")]
mod zstd_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_zstd_text, CodecMethod::Zstd, text, "document.txt");
    codec_data_test!(test_zstd_binary, CodecMethod::Zstd, binary, "data.bin");
    codec_data_test!(
        test_zstd_repetitive,
        CodecMethod::Zstd,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(test_zstd_random, CodecMethod::Zstd, random, "random.bin");
    codec_data_test!(test_zstd_small, CodecMethod::Zstd, small, "small.txt");
    codec_data_test!(test_zstd_empty, CodecMethod::Zstd, empty, "empty.txt");
    codec_multiple_files_test!(test_zstd_multiple_files, CodecMethod::Zstd);
    codec_levels_test!(test_zstd_levels, CodecMethod::Zstd, [1, 5, 9]);
}

// =============================================================================
// Brotli Tests
// =============================================================================

#[cfg(feature = "brotli")]
mod brotli_roundtrip {
    use super::*;
    use zesven::codec::CodecMethod;

    codec_data_test!(test_brotli_text, CodecMethod::Brotli, text, "document.txt");
    codec_data_test!(test_brotli_binary, CodecMethod::Brotli, binary, "data.bin");
    codec_data_test!(
        test_brotli_repetitive,
        CodecMethod::Brotli,
        repetitive,
        "repeated.txt"
    );
    codec_data_test!(
        test_brotli_random,
        CodecMethod::Brotli,
        random,
        "random.bin"
    );
    codec_data_test!(test_brotli_small, CodecMethod::Brotli, small, "small.txt");
    codec_data_test!(test_brotli_empty, CodecMethod::Brotli, empty, "empty.txt");
    codec_multiple_files_test!(test_brotli_multiple_files, CodecMethod::Brotli);
    codec_levels_test!(test_brotli_levels, CodecMethod::Brotli, [1, 5, 9]);
}

// =============================================================================
// Solid Archive Tests
// =============================================================================

#[cfg(feature = "lzma2")]
mod solid_archives {
    use super::*;
    use zesven::codec::CodecMethod;

    #[test]
    fn test_solid_lzma2_multiple_files() {
        let text = test_data::text();
        let binary = test_data::binary();
        let small = test_data::small();
        let entries = [
            ("file1.txt", text.as_slice()),
            ("file2.bin", binary.as_slice()),
            ("file3.txt", small.as_slice()),
        ];

        let archive = create_archive_with_options(
            WriteOptions::new().method(CodecMethod::Lzma2).solid(),
            &entries,
        )
        .expect("Failed to create solid archive");

        verify_archive_contents(&archive, &entries);
    }

    #[test]
    fn test_solid_many_small_files() {
        let small = test_data::small();
        let entries: Vec<(String, &[u8])> = (0..20)
            .map(|i| (format!("file{:02}.txt", i), small.as_slice()))
            .collect();

        let entry_refs: Vec<(&str, &[u8])> =
            entries.iter().map(|(s, d)| (s.as_str(), *d)).collect();

        let archive = create_archive_with_options(
            WriteOptions::new().method(CodecMethod::Lzma2).solid(),
            &entry_refs,
        )
        .expect("Failed to create solid archive");

        verify_archive_contents(&archive, &entry_refs);
    }

    // Note: test_solid_with_empty_files_at_end and test_solid_with_empty_entry_in_middle
    // were removed as redundant. The test below covers all empty file positions:
    // start, middle, and end. This provides complete coverage for empty file handling
    // in solid archives.

    /// Tests solid archive with multiple empty files interspersed.
    ///
    /// This comprehensive test covers all empty file edge cases in solid archives:
    /// - Empty file at start: Tests boundary at block start
    /// - Empty file in middle: Tests zero bytes between files' data
    /// - Empty file at end: Tests boundary at block end
    ///
    /// Empty files have no data stream (marked as EmptyStream/EmptyFile in
    /// FilesInfo) so they don't increment the substream counter. The decoder
    /// must correctly track offsets to extract all files.
    #[test]
    fn test_solid_with_multiple_empty_entries_interspersed() {
        let text = test_data::text();
        let small = test_data::small();
        let empty = test_data::empty();
        let entries = [
            ("empty_start.txt", empty.as_slice()),
            ("file1.txt", text.as_slice()),
            ("empty_middle.txt", empty.as_slice()),
            ("file2.txt", small.as_slice()),
            ("empty_end.txt", empty.as_slice()),
        ];

        let archive = create_archive_with_options(
            WriteOptions::new().method(CodecMethod::Lzma2).solid(),
            &entries,
        )
        .expect("Failed to create solid archive with interspersed empty entries");

        verify_archive_contents(&archive, &entries);
    }

    /// Tests random access extraction from solid archives.
    ///
    /// Solid archives compress all file data as a single stream, which requires
    /// sequential decompression for streaming access. However, the non-streaming
    /// Archive API should allow extracting any entry by path without requiring
    /// the caller to explicitly extract preceding entries first.
    ///
    /// This test verifies that:
    /// 1. Extracting the last entry directly works correctly
    /// 2. Content matches the original data
    /// 3. The extraction is independent (doesn't require extracting earlier entries)
    #[test]
    fn test_solid_archive_random_entry_extraction() {
        use std::io::Cursor;
        use zesven::read::Archive;

        // Create content that's distinguishable for each entry
        let content_a = b"Content for file A - first entry in the solid block";
        let content_b = b"Content for file B - second entry in the solid block";
        let content_c = b"Content for file C - third entry in the solid block";
        let entries = [
            ("first.txt", content_a.as_slice()),
            ("second.txt", content_b.as_slice()),
            ("third.txt", content_c.as_slice()),
        ];

        let archive_bytes = create_archive_with_options(
            WriteOptions::new().method(CodecMethod::Lzma2).solid(),
            &entries,
        )
        .expect("Failed to create solid archive");

        // Open the archive
        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open(cursor).expect("Failed to open solid archive");

        // Note: Solid archives are created by WriteOptions::solid(), verified by the
        // create_archive_with_options call above. The archive format determines solid
        // behavior, not a per-entry flag.

        // Extract ONLY the third entry without touching the first two
        // This tests random access within a solid block
        let extracted_c = archive
            .extract_to_vec("third.txt")
            .expect("Failed to extract third entry from solid archive");

        assert_eq!(
            extracted_c.as_slice(),
            content_c,
            "Third entry content should match original"
        );

        // Also verify we can extract the first entry after extracting the third
        // (ensures no state corruption from previous extraction)
        let extracted_a = archive
            .extract_to_vec("first.txt")
            .expect("Failed to extract first entry after third");

        assert_eq!(
            extracted_a.as_slice(),
            content_a,
            "First entry content should match original"
        );
    }
}

#[cfg(feature = "deflate")]
mod solid_deflate {
    use super::*;
    use zesven::codec::CodecMethod;

    #[test]
    fn test_solid_deflate_multiple_files() {
        let text = test_data::text();
        let binary = test_data::binary();
        let entries = [
            ("doc.txt", text.as_slice()),
            ("data.bin", binary.as_slice()),
        ];

        let archive = create_archive_with_options(
            WriteOptions::new().method(CodecMethod::Deflate).solid(),
            &entries,
        )
        .expect("Failed to create solid archive");

        verify_archive_contents(&archive, &entries);
    }
}

// =============================================================================
// BCJ/Filter Write Tests - NOT YET AVAILABLE
// =============================================================================
//
// BCJ filter chains during *writing* are not yet exposed through WriteOptions.
//
// **Reading BCJ archives**: Fully supported (tested in reference_archives.rs)
// **Writing BCJ archives**: Not yet implemented
//
// Note: test_data::executable_like() is already used by test_lzma2_executable_like
// (line 287) to verify LZMA2 compression of x86-like binary patterns. When BCJ
// filter writing is implemented, the same test data can be used to verify that
// BCJ pre-filtering improves compression ratios for executable content.
//
// Required API changes to enable BCJ writing:
// 1. Add `WriteOptions::filter(Filter)` method
// 2. Wire filter into compression pipeline in src/write/mod.rs
// 3. Add `Filter` enum to public API

// =============================================================================
// Encryption Tests
// =============================================================================

#[cfg(feature = "aes")]
mod encrypted_archives {
    use super::*;
    use zesven::codec::CodecMethod;

    #[test]
    fn test_encrypted_lzma2() {
        let data = test_data::text();
        let entries = [("secret.txt", data.as_slice())];
        let password = "test_password_123";

        let archive = create_archive_with_options(
            WriteOptions::new()
                .method(CodecMethod::Lzma2)
                .password(password),
            &entries,
        )
        .expect("Failed to create encrypted archive");

        verify_encrypted_archive(&archive, password, &entries);
    }

    #[cfg(feature = "deflate")]
    #[test]
    fn test_encrypted_deflate() {
        let data = test_data::binary();
        let entries = [("secret.bin", data.as_slice())];
        let password = "another_password";

        let archive = create_archive_with_options(
            WriteOptions::new()
                .method(CodecMethod::Deflate)
                .password(password),
            &entries,
        )
        .expect("Failed to create encrypted archive");

        verify_encrypted_archive(&archive, password, &entries);
    }

    #[test]
    fn test_encrypted_solid() {
        let text = test_data::text();
        let binary = test_data::binary();
        let entries = [
            ("doc.txt", text.as_slice()),
            ("data.bin", binary.as_slice()),
        ];
        let password = "solid_password";

        let archive = create_archive_with_options(
            WriteOptions::new()
                .method(CodecMethod::Lzma2)
                .solid()
                .password(password),
            &entries,
        )
        .expect("Failed to create encrypted solid archive");

        verify_encrypted_archive(&archive, password, &entries);
    }

    #[test]
    fn test_encrypted_with_unicode_password() {
        let data = test_data::small();
        let entries = [("file.txt", data.as_slice())];
        let password = "密码测试🔐";

        let archive = create_archive_with_options(
            WriteOptions::new()
                .method(CodecMethod::Lzma2)
                .password(password),
            &entries,
        )
        .expect("Failed to create encrypted archive");

        verify_encrypted_archive(&archive, password, &entries);
    }
}

// =============================================================================
// Archive Comments Tests
// =============================================================================

#[test]
fn test_archive_with_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("test.txt", data.as_slice())];
    let comment = "This is a test archive";

    let archive = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Copy)
            .comment(comment),
        &entries,
    )
    .expect("Failed to create archive with comment");

    // Verify we can read it back
    let cursor = Cursor::new(&archive);
    let archive_reader = Archive::open(cursor).expect("Failed to open archive");
    let info = archive_reader.info();

    assert_eq!(info.comment.as_deref(), Some(comment));
}

#[test]
fn test_archive_with_unicode_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("test.txt", data.as_slice())];
    let comment = "日本語コメント 🎉";

    let archive = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Copy)
            .comment(comment),
        &entries,
    )
    .expect("Failed to create archive with unicode comment");

    let cursor = Cursor::new(&archive);
    let archive_reader = Archive::open(cursor).expect("Failed to open archive");
    let info = archive_reader.info();

    assert_eq!(info.comment.as_deref(), Some(comment));
}

/// Tests that archive comments work correctly with encryption.
///
/// This verifies that:
/// - Comments can be set on encrypted archives
/// - Comments are accessible after decryption
/// - Header encryption doesn't corrupt the comment
#[cfg(feature = "aes")]
#[test]
fn test_encrypted_archive_with_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("secret.txt", data.as_slice())];
    let comment = "Encrypted archive comment";
    let password = "comment_test_password";

    let archive = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Lzma2)
            .password(password)
            .encrypt_header(true)
            .comment(comment),
        &entries,
    )
    .expect("Failed to create encrypted archive with comment");

    // Verify we can read the comment after decryption
    let cursor = Cursor::new(&archive);
    let archive_reader =
        Archive::open_with_password(cursor, password).expect("Failed to open encrypted archive");
    let info = archive_reader.info();

    assert_eq!(
        info.comment.as_deref(),
        Some(comment),
        "Comment should be preserved in encrypted archive"
    );
    assert!(
        info.has_encrypted_header,
        "Archive should report header is encrypted"
    );
}

/// Tests that multiline archive comments are preserved correctly.
///
/// Newlines in comments should be preserved during round-trip.
#[test]
fn test_archive_with_multiline_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("test.txt", data.as_slice())];
    let comment = "Line 1: Introduction\nLine 2: Details\nLine 3: Conclusion";

    let archive = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Copy)
            .comment(comment),
        &entries,
    )
    .expect("Failed to create archive with multiline comment");

    let cursor = Cursor::new(&archive);
    let archive_reader = Archive::open(cursor).expect("Failed to open archive");
    let info = archive_reader.info();

    assert_eq!(
        info.comment.as_deref(),
        Some(comment),
        "Multiline comment should be preserved exactly"
    );
}

/// Tests that long archive comments (10KB) are supported.
///
/// This verifies that reasonably large comments work without truncation.
/// We use 10KB as a practical size that's large but not extreme.
#[test]
fn test_archive_with_long_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("test.txt", data.as_slice())];
    // Create a 10KB comment with recognizable content
    let comment: String = (0..500)
        .map(|i| format!("Line {:04}: This is comment content for testing.\n", i))
        .collect();
    assert!(comment.len() > 10_000, "Comment should be > 10KB");

    let archive = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Copy)
            .comment(&comment),
        &entries,
    )
    .expect("Failed to create archive with long comment");

    let cursor = Cursor::new(&archive);
    let archive_reader = Archive::open(cursor).expect("Failed to open archive");
    let info = archive_reader.info();

    assert_eq!(
        info.comment.as_deref(),
        Some(comment.as_str()),
        "Long comment should be preserved exactly"
    );
}

/// Tests behavior when no comment is set vs empty comment.
///
/// Verifies that:
/// - No comment set: `info.comment` is `None`
/// - Empty string comment: behavior may vary (documented here)
#[test]
fn test_archive_without_comment() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [("test.txt", data.as_slice())];

    // Archive without any comment
    let archive = create_archive_with_options(
        WriteOptions::new().method(CodecMethod::Copy),
        // Note: no .comment() call
        &entries,
    )
    .expect("Failed to create archive without comment");

    let cursor = Cursor::new(&archive);
    let archive_reader = Archive::open(cursor).expect("Failed to open archive");
    let info = archive_reader.info();

    assert_eq!(
        info.comment, None,
        "Archive without comment should have None"
    );
}

// =============================================================================
// Mixed Data Type Tests
// =============================================================================

#[cfg(feature = "lzma2")]
#[test]
fn test_mixed_data_types() {
    use zesven::codec::CodecMethod;

    let text = test_data::text();
    let binary = test_data::binary();
    let repetitive = test_data::repetitive();
    let random = test_data::random();

    // Note: Empty files and very small files may have CRC handling differences
    // in non-solid archives due to how the 7z format handles them
    let entries = [
        ("docs/readme.txt", text.as_slice()),
        ("data/binary.bin", binary.as_slice()),
        ("data/repeated.dat", repetitive.as_slice()),
        ("data/random.bin", random.as_slice()),
    ];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Lzma2), &entries)
            .expect("Failed to create mixed archive");

    verify_archive_contents(&archive, &entries);
}

// =============================================================================
// Stress Tests
// =============================================================================

#[cfg(feature = "lzma2")]
#[test]
fn test_large_file() {
    use zesven::codec::CodecMethod;

    // 1MB of compressible data
    let data: Vec<u8> = test_data::text()
        .into_iter()
        .cycle()
        .take(1_000_000)
        .collect();
    let entries = [("large.txt", data.as_slice())];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Lzma2), &entries)
            .expect("Failed to create large archive");

    verify_archive_contents(&archive, &entries);
}

#[cfg(feature = "lzma2")]
#[test]
fn test_many_files() {
    use zesven::codec::CodecMethod;

    let small = test_data::small();
    let entries: Vec<(String, &[u8])> = (0..100)
        .map(|i| (format!("dir{}/file{:03}.txt", i / 10, i), small.as_slice()))
        .collect();

    let entry_refs: Vec<(&str, &[u8])> = entries.iter().map(|(s, d)| (s.as_str(), *d)).collect();

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Lzma2), &entry_refs)
            .expect("Failed to create many files archive");

    verify_archive_contents(&archive, &entry_refs);
}

// =============================================================================
// Unicode Path Tests
// =============================================================================

/// Tests Unicode paths in various scripts.
#[test]
fn test_unicode_paths() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();
    let entries = [
        ("日本語/ファイル.txt", data.as_slice()),
        ("中文/文件.txt", data.as_slice()),
        ("Ελληνικά/αρχείο.txt", data.as_slice()),
        ("🎉/emoji.txt", data.as_slice()),
    ];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Copy), &entries)
            .expect("Failed to create unicode path archive");

    verify_archive_contents(&archive, &entries);
}

/// Tests Unicode normalization forms (NFC vs NFD) in file paths.
///
/// Unicode has multiple ways to represent the same visual character:
/// - NFC (Composed): "café" uses é (U+00E9, single codepoint)
/// - NFD (Decomposed): "café" uses e + ́ (U+0065 + U+0301, two codepoints)
///
/// Different operating systems use different normalization forms:
/// - macOS HFS+/APFS: Uses NFD for file paths
/// - Windows/Linux: Typically preserve the original form
///
/// This test verifies that both normalization forms round-trip correctly.
/// The archive should preserve the exact bytes used in the path, allowing
/// the caller to handle normalization as needed.
#[test]
fn test_unicode_normalization_roundtrip() {
    use zesven::codec::CodecMethod;

    let data = test_data::small();

    // NFC (precomposed): é is U+00E9
    let nfc_path = "caf\u{00E9}.txt"; // café.txt

    // NFD (decomposed): e + combining acute accent
    let nfd_path = "cafe\u{0301}.txt"; // café.txt (visually same, different bytes)

    // Verify these are different byte sequences but same visual appearance
    assert_ne!(
        nfc_path.as_bytes(),
        nfd_path.as_bytes(),
        "NFC and NFD should have different byte representations"
    );

    // Create archive with both normalization forms
    let entries = [(nfc_path, data.as_slice()), (nfd_path, data.as_slice())];

    let archive =
        create_archive_with_options(WriteOptions::new().method(CodecMethod::Copy), &entries)
            .expect("Failed to create unicode normalization archive");

    // Verify both paths round-trip correctly with exact byte preservation
    verify_archive_contents(&archive, &entries);
}

// =============================================================================
// Deterministic Mode Tests
// =============================================================================

#[cfg(feature = "lzma2")]
#[test]
fn test_deterministic_archives_identical() {
    use zesven::codec::CodecMethod;

    let text = test_data::text();
    let binary = test_data::binary();
    let entries = [
        ("file_b.txt", text.as_slice()),
        ("file_a.bin", binary.as_slice()),
    ];

    let archive1 = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Lzma2)
            .deterministic(true),
        &entries,
    )
    .expect("Failed to create first archive");

    let archive2 = create_archive_with_options(
        WriteOptions::new()
            .method(CodecMethod::Lzma2)
            .deterministic(true),
        &entries,
    )
    .expect("Failed to create second archive");

    assert_eq!(
        archive1, archive2,
        "Deterministic archives should be identical"
    );
}

// Note: WriteOptions validation tests (level validation, level_clamped) are in
// src/write/options.rs as unit tests, which is the appropriate layer for testing
// API validation behavior.
