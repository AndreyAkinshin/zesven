//! Property-based tests using proptest.
//!
//! These tests verify invariants and properties of the zesven library
//! using randomly generated inputs.

use proptest::prelude::*;
#[allow(unused_imports)]
use std::io::Cursor;
use zesven::ArchivePath;

/// Windows reserved device names (case-insensitive) that cannot be used as filenames.
/// These are rejected by ArchivePath to maintain cross-platform compatibility.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Checks if a path segment is a Windows reserved name (case-insensitive).
fn is_windows_reserved(segment: &str) -> bool {
    // Check the base name (before any extension)
    let base = segment.split('.').next().unwrap_or(segment);
    WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(base))
}

/// Strategy for generating valid archive path strings.
///
/// This strategy generates paths that will be accepted by ArchivePath::new():
/// - 1-4 path components separated by '/'
/// - Each component is 1-10 alphanumeric characters with optional underscores/dots/dashes
/// - Excludes "." and ".." segments (path traversal)
/// - Excludes Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9)
fn valid_path_strategy() -> impl Strategy<Value = String> {
    // Generate 1-4 path components, each 1-10 alphanumeric chars
    proptest::collection::vec("[a-zA-Z0-9][a-zA-Z0-9_.-]{0,9}", 1..4)
        .prop_map(|parts| parts.join("/"))
        .prop_filter("must not contain invalid segments", |s| {
            !s.split('/')
                .any(|seg| seg == "." || seg == ".." || is_windows_reserved(seg))
        })
}

proptest! {
    /// Valid paths should always parse successfully.
    #[test]
    fn valid_paths_parse_successfully(path in valid_path_strategy()) {
        let result = ArchivePath::new(&path);
        prop_assert!(result.is_ok(), "Valid path '{}' failed to parse: {:?}", path, result);

        // Round-trip: parsed path should have same string representation
        let parsed = result.unwrap();
        prop_assert_eq!(parsed.as_str(), &path);
    }

    /// Paths with NUL bytes should always be rejected.
    #[test]
    fn nul_bytes_rejected(
        prefix in "[a-zA-Z0-9]{0,5}",
        suffix in "[a-zA-Z0-9]{0,5}"
    ) {
        let path = format!("{}\0{}", prefix, suffix);
        let result = ArchivePath::new(&path);
        prop_assert!(result.is_err(), "Path with NUL byte should be rejected");
    }

    /// Absolute paths should always be rejected.
    #[test]
    fn absolute_paths_rejected(path in "/[a-zA-Z0-9/]+") {
        let result = ArchivePath::new(&path);
        prop_assert!(result.is_err(), "Absolute path '{}' should be rejected", path);
    }

    /// Paths with ".." as a complete segment should always be rejected.
    #[test]
    fn traversal_paths_rejected(
        prefix in "[a-zA-Z0-9]{1,5}",
        suffix in "[a-zA-Z0-9]{1,5}"
    ) {
        // ".." as a complete path segment must be rejected (path traversal attack)
        let dotdot_path = format!("{}/../{}", prefix, suffix);
        let dotdot_result = ArchivePath::new(&dotdot_path);
        prop_assert!(dotdot_result.is_err(), "Traversal path '{}' should be rejected", dotdot_path);
    }

    /// Empty segments (double slashes) should be rejected.
    #[test]
    fn empty_segments_rejected(
        part1 in "[a-zA-Z0-9]{1,5}",
        part2 in "[a-zA-Z0-9]{1,5}"
    ) {
        let path = format!("{}//{}", part1, part2);
        let result = ArchivePath::new(&path);
        prop_assert!(result.is_err(), "Path with empty segment '{}' should be rejected", path);
    }
}

// =============================================================================
// Compression Round-Trip Tests
// =============================================================================
//
// Note: General round-trip testing (small, medium, large data with various
// content types) is covered exhaustively by tests/codec_combinations.rs which
// tests all codecs with text, binary, random, and repetitive data patterns.
//
// The proptest compression tests here focus on:
// 1. Boundary conditions (LZMA2 chunk boundary at 64KB)
// 2. Random filename generation (tests path handling with varied inputs)
//
// Removed redundant tests:
// - round_trip_small_data: covered by codec_combinations test_*_small()
// - round_trip_medium_data: covered by codec_combinations test_*_text/binary()
// =============================================================================

#[cfg(feature = "lzma2")]
mod compression_tests {
    use super::*;
    use zesven::Writer;
    use zesven::read::Archive;

    /// Strategy for generating arbitrary byte data.
    fn data_strategy(max_size: usize) -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(any::<u8>(), 0..max_size)
    }

    /// LZMA2 processes data in chunks of up to 64KB (2^16 = 65536 bytes).
    /// Testing at chunk boundaries exercises edge cases in the decompression path.
    ///
    /// 1-chunk boundary: 65535-65537 bytes
    /// - 65535 bytes: just under one chunk
    /// - 65536 bytes: exactly one chunk
    /// - 65537 bytes: first byte of second chunk
    const ONE_CHUNK_BOUNDARY_MIN: usize = 65535;
    const ONE_CHUNK_BOUNDARY_MAX: usize = 65537;

    /// 2-chunk boundary: 131071-131073 bytes (2 × 65536 = 131072)
    /// - 131071 bytes: just under two chunks
    /// - 131072 bytes: exactly two chunks
    /// - 131073 bytes: first byte of third chunk
    const TWO_CHUNK_BOUNDARY_MIN: usize = 131071;
    const TWO_CHUNK_BOUNDARY_MAX: usize = 131073;

    proptest! {
        // Reduced from 100 to 20: random filename testing doesn't need high iteration
        // count since path handling is deterministic. Each case exercises different
        // random filenames, but 20 is sufficient to catch edge cases.
        #![proptest_config(ProptestConfig::with_cases(20))]

        /// Multiple files with random names should round-trip correctly.
        ///
        /// This test uses property-based testing to verify path handling with
        /// varied inputs. Unlike codec_combinations which uses fixed filenames,
        /// this exercises the archive path validation with random strings.
        ///
        /// Verifies full round-trip: write -> read -> extract -> content comparison
        #[test]
        fn multiple_files_stored(
            data1 in data_strategy(512),
            data2 in data_strategy(512),
            name1 in "[a-zA-Z0-9]{1,8}",
            name2 in "[a-zA-Z0-9]{1,8}"
        ) {
            // Ensure names are different (case-insensitive to avoid collisions on
            // case-insensitive filesystems like macOS HFS+/APFS)
            prop_assume!(name1.to_lowercase() != name2.to_lowercase());

            let file1 = format!("{}.bin", name1);
            let file2 = format!("{}.txt", name2);

            let mut archive_bytes = Vec::new();
            {
                let cursor = Cursor::new(&mut archive_bytes);
                let mut writer = Writer::create(cursor).unwrap();

                let path1 = ArchivePath::new(&file1).unwrap();
                let path2 = ArchivePath::new(&file2).unwrap();

                writer.add_bytes(path1, &data1).unwrap();
                writer.add_bytes(path2, &data2).unwrap();

                let result = writer.finish().unwrap();
                prop_assert_eq!(result.entries_written, 2);
            }

            // Verify content round-trips correctly
            let mut archive = Archive::open(Cursor::new(&archive_bytes))
                .expect("Failed to open archive");

            // Extract and verify both files
            let extracted1 = archive.extract_to_vec(&file1)
                .expect("Failed to extract file1");
            prop_assert_eq!(&extracted1, &data1, "Content mismatch for {}", file1);

            let extracted2 = archive.extract_to_vec(&file2)
                .expect("Failed to extract file2");
            prop_assert_eq!(&extracted2, &data2, "Content mismatch for {}", file2);
        }
    }

    proptest! {
        // Reduced from 30 to 10: boundary tests cover only 3 sizes (65535-65537 and
        // 131071-131073), so we only need a few iterations per size. The boundary
        // behavior is deterministic; different random seeds don't change the chunking.
        #![proptest_config(ProptestConfig::with_cases(10))]

        /// Tests data sizes at LZMA2 1-chunk boundary (65535-65537 bytes).
        ///
        /// LZMA2 processes data in independent chunks of up to 64KB. This test
        /// exercises the boundary conditions at the first chunk boundary:
        /// - 65535 bytes: just under one chunk
        /// - 65536 bytes: exactly one chunk
        /// - 65537 bytes: spans two chunks
        ///
        /// This is unique coverage not provided by codec_combinations.rs which
        /// uses fixed data sizes (10KB text, 50KB repetitive, 5KB random).
        #[test]
        fn round_trip_one_chunk_boundary(
            size in ONE_CHUNK_BOUNDARY_MIN..=ONE_CHUNK_BOUNDARY_MAX,
            seed in any::<u64>()
        ) {
            // Generate deterministic data of exact size
            let data: Vec<u8> = (0..size)
                .map(|i| ((i as u64).wrapping_mul(seed.wrapping_add(17))) as u8)
                .collect();

            let mut archive_bytes = Vec::new();
            {
                let cursor = Cursor::new(&mut archive_bytes);
                let mut writer = Writer::create(cursor).unwrap();

                let path = ArchivePath::new("boundary.bin").unwrap();
                writer.add_bytes(path, &data).unwrap();
                let _ = writer.finish().unwrap();
            }

            // Verify content round-trips correctly
            let mut archive = Archive::open(Cursor::new(&archive_bytes))
                .expect("Failed to open archive");
            let extracted = archive.extract_to_vec("boundary.bin")
                .expect("Failed to extract");
            prop_assert_eq!(extracted.len(), size, "Size mismatch at 1-chunk boundary");
            prop_assert_eq!(extracted, data, "Content mismatch at 1-chunk boundary");
        }

        /// Tests data sizes at LZMA2 2-chunk boundary (131071-131073 bytes).
        ///
        /// This exercises the boundary between 2 and 3 chunks:
        /// - 131071 bytes: just under two full chunks
        /// - 131072 bytes: exactly two chunks (2 × 64KB)
        /// - 131073 bytes: two full chunks + 1 byte
        ///
        /// Multi-chunk scenarios test that chunk sequencing and state reset
        /// work correctly during decompression.
        #[test]
        fn round_trip_two_chunk_boundary(
            size in TWO_CHUNK_BOUNDARY_MIN..=TWO_CHUNK_BOUNDARY_MAX,
            seed in any::<u64>()
        ) {
            // Generate deterministic data of exact size
            let data: Vec<u8> = (0..size)
                .map(|i| ((i as u64).wrapping_mul(seed.wrapping_add(31))) as u8)
                .collect();

            let mut archive_bytes = Vec::new();
            {
                let cursor = Cursor::new(&mut archive_bytes);
                let mut writer = Writer::create(cursor).unwrap();

                let path = ArchivePath::new("two_chunk.bin").unwrap();
                writer.add_bytes(path, &data).unwrap();
                let _ = writer.finish().unwrap();
            }

            // Verify content round-trips correctly
            let mut archive = Archive::open(Cursor::new(&archive_bytes))
                .expect("Failed to open archive");
            let extracted = archive.extract_to_vec("two_chunk.bin")
                .expect("Failed to extract");
            prop_assert_eq!(extracted.len(), size, "Size mismatch at 2-chunk boundary");
            prop_assert_eq!(extracted, data, "Content mismatch at 2-chunk boundary");
        }
    }
}

// =============================================================================
// Resource Limits Proptest Module (removed)
// =============================================================================
//
// The resource_limits_tests proptest module was removed as it tested deterministic
// behavior with property-based testing, which adds overhead without finding
// additional bugs:
//
// - resource_limits_builder: ResourceLimits builder is deterministic; unit tests
//   with specific values in src/format/streams.rs provide equivalent coverage
//   (test_resource_limits_default, test_resource_limits_unlimited,
//   test_resource_limits_builder_methods)
//
// - ratio_limit_check: RatioLimit::check is deterministic arithmetic; unit tests
//   with boundary values in src/format/streams.rs provide equivalent coverage
//   (test_ratio_limit_normal_ratio, test_ratio_limit_exceeds_limit,
//   test_ratio_limit_no_truncation, etc.)
