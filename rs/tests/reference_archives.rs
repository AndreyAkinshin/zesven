//! Tests using reference archives from bodgit/sevenzip Go implementation.
//!
//! These tests verify that zesven can correctly read archives produced
//! by various 7-zip implementations and with different compression methods.
//!
//! # Test Data Source
//!
//! Test data is automatically cloned from <https://github.com/bodgit/sevenzip> (v1.6.1)
//! when tests are run and the testdata directory doesn't exist.

// These tests require LZMA support (reference archives use LZMA/LZMA2)
#![cfg(feature = "lzma")]

use std::path::Path;
use std::process::Command;
use std::sync::Once;

use zesven::read::{Archive, ArchiveInfo, SelectAll, TestOptions};
use zesven::volume::MultiVolumeReader;

/// Repository URL for testdata.
const TESTDATA_REPO: &str = "https://github.com/bodgit/sevenzip";

/// Version tag to clone.
const TESTDATA_VERSION: &str = "v1.6.1";

/// Path to clone the repository to.
const CLONE_DIR: &str = "testdata/sevenzip";

/// Path to reference archives within the cloned repository.
const TESTDATA_DIR: &str = "testdata/sevenzip/testdata";

/// Ensures testdata is cloned only once across all tests.
static CLONE_ONCE: Once = Once::new();

/// Clone the testdata repository if it doesn't exist.
fn ensure_testdata_cloned() {
    CLONE_ONCE.call_once(|| {
        if Path::new(TESTDATA_DIR).exists() {
            return;
        }

        eprintln!(
            "\n    Cloning testdata from {} ({})...",
            TESTDATA_REPO, TESTDATA_VERSION
        );

        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                TESTDATA_VERSION,
                TESTDATA_REPO,
                CLONE_DIR,
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                eprintln!("    Testdata cloned successfully.\n");
            }
            Ok(s) => {
                panic!(
                    "Failed to clone testdata repository (exit code: {:?}). \
                     Ensure git is installed and network is available.",
                    s.code()
                );
            }
            Err(e) => {
                panic!("Failed to run git clone: {}. Ensure git is installed.", e);
            }
        }
    });
}

/// Check if the testdata directory exists.
fn testdata_available() -> bool {
    Path::new(TESTDATA_DIR).exists()
}

/// Macro that ensures testdata is available, cloning if necessary.
macro_rules! require_testdata {
    () => {
        ensure_testdata_cloned();
        if !testdata_available() {
            panic!(
                "Testdata not available at '{}' even after clone attempt.",
                TESTDATA_DIR
            );
        }
    };
}

// =============================================================================
// Test Data Availability Check
// =============================================================================

/// Sentinel test that ensures testdata is available (cloning if necessary).
///
/// Run with `cargo test --test reference_archives -- --nocapture` to see output.
#[test]
fn test_reference_archives_availability_check() {
    ensure_testdata_cloned();

    if testdata_available() {
        println!("\n=== Reference Archives: AVAILABLE ===");
        println!("    Path: {}", TESTDATA_DIR);
        println!("    All reference archive tests will execute.\n");
    } else {
        panic!(
            "Testdata not available at '{}' even after clone attempt.",
            TESTDATA_DIR
        );
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Helper to open and verify an archive can be read.
fn verify_archive_readable(archive_path: &str) -> zesven::Result<ArchiveInfo> {
    let path = Path::new(TESTDATA_DIR).join(archive_path);
    let archive = Archive::open_path(&path)?;
    Ok(archive.info().clone())
}

/// Helper to test all entries in an archive (verifies decompression and CRC).
fn verify_archive_testable(archive_path: &str) -> zesven::Result<()> {
    let path = Path::new(TESTDATA_DIR).join(archive_path);
    let mut archive = Archive::open_path(&path)?;

    // Test all entries - this decompresses and verifies CRC
    let result = archive.test(SelectAll, &TestOptions::default())?;

    // Check that all tested entries passed
    if result.entries_failed > 0 {
        return Err(zesven::Error::InvalidFormat(format!(
            "Test failed for {} entries: {:?}",
            result.entries_failed, result.failures
        )));
    }

    Ok(())
}

/// Helper to list entries in an archive.
fn list_entries(archive_path: &str) -> zesven::Result<Vec<String>> {
    let path = Path::new(TESTDATA_DIR).join(archive_path);
    let archive = Archive::open_path(&path)?;

    let mut entry_names = Vec::new();
    for entry in archive.entries() {
        entry_names.push(entry.path.as_str().to_string());
    }

    Ok(entry_names)
}

// =============================================================================
// Basic Compression Method Tests
// =============================================================================

#[test]
fn test_read_lzma() {
    require_testdata!();
    let info = verify_archive_readable("lzma.7z").expect("Failed to read lzma.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("lzma.7z").expect("Failed to test lzma.7z");
}

#[test]
fn test_read_lzma2() {
    require_testdata!();
    let info = verify_archive_readable("lzma2.7z").expect("Failed to read lzma2.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("lzma2.7z").expect("Failed to test lzma2.7z");
}

#[test]
fn test_read_deflate() {
    require_testdata!();
    let info = verify_archive_readable("deflate.7z").expect("Failed to read deflate.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("deflate.7z").expect("Failed to test deflate.7z");
}

#[test]
fn test_read_bzip2() {
    require_testdata!();
    let info = verify_archive_readable("bzip2.7z").expect("Failed to read bzip2.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("bzip2.7z").expect("Failed to test bzip2.7z");
}

#[test]
fn test_read_copy() {
    require_testdata!();
    let info = verify_archive_readable("copy.7z").expect("Failed to read copy.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("copy.7z").expect("Failed to test copy.7z");
}

#[test]
#[cfg(feature = "ppmd")]
fn test_read_ppmd() {
    require_testdata!();
    // ppmd.7z is not included in bodgit/sevenzip v1.6.1 testdata
    // Skip test if file doesn't exist
    let path = Path::new(TESTDATA_DIR).join("ppmd.7z");
    if !path.exists() {
        eprintln!("Skipping test_read_ppmd: ppmd.7z not found in testdata");
        return;
    }
    let info = verify_archive_readable("ppmd.7z").expect("Failed to read ppmd.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("ppmd.7z").expect("Failed to test ppmd.7z");
}

// =============================================================================
// Modern Codec Tests (Optional Features)
// =============================================================================

#[test]
#[cfg(feature = "lz4")]
fn test_read_lz4() {
    require_testdata!();
    let info = verify_archive_readable("lz4.7z").expect("Failed to read lz4.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("lz4.7z").expect("Failed to test lz4.7z");
}

#[test]
#[cfg(feature = "zstd")]
fn test_read_zstd() {
    require_testdata!();
    let info = verify_archive_readable("zstd.7z").expect("Failed to read zstd.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("zstd.7z").expect("Failed to test zstd.7z");
}

// =============================================================================
// Filter Tests
// =============================================================================

#[test]
fn test_read_bcj_x86() {
    require_testdata!();
    let info = verify_archive_readable("bcj.7z").expect("Failed to read bcj.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("bcj.7z").expect("Failed to test bcj.7z");
}

#[test]
fn test_read_bcj2() {
    require_testdata!();
    let info = verify_archive_readable("bcj2.7z").expect("Failed to read bcj2.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("bcj2.7z").expect("Failed to test bcj2.7z");
}

#[test]
fn test_read_arm() {
    require_testdata!();
    let info = verify_archive_readable("arm.7z").expect("Failed to read arm.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("arm.7z").expect("Failed to test arm.7z");
}

#[test]
fn test_read_ppc() {
    require_testdata!();
    let info = verify_archive_readable("ppc.7z").expect("Failed to read ppc.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("ppc.7z").expect("Failed to test ppc.7z");
}

#[test]
fn test_read_sparc() {
    require_testdata!();
    let info = verify_archive_readable("sparc.7z").expect("Failed to read sparc.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("sparc.7z").expect("Failed to test sparc.7z");
}

#[test]
fn test_read_delta() {
    require_testdata!();
    let info = verify_archive_readable("delta.7z").expect("Failed to read delta.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
    verify_archive_testable("delta.7z").expect("Failed to test delta.7z");
}

// =============================================================================
// Empty Archive Tests
// =============================================================================

#[test]
fn test_read_empty() {
    require_testdata!();
    // Empty archives should be readable
    let _info = verify_archive_readable("empty.7z").expect("Failed to read empty.7z");
}

#[test]
fn test_read_empty2() {
    require_testdata!();
    let _info = verify_archive_readable("empty2.7z").expect("Failed to read empty2.7z");
}

#[test]
fn test_read_file_and_empty() {
    require_testdata!();
    let info =
        verify_archive_readable("file_and_empty.7z").expect("Failed to read file_and_empty.7z");
    assert!(info.entry_count > 0, "Archive should have entries");
}

// =============================================================================
// Self-Extracting Archive Tests
// =============================================================================

#[test]
fn test_read_sfx() {
    require_testdata!();
    // Self-extracting archives are PE executables with 7z data appended
    let info = verify_archive_readable("sfx.exe").expect("Failed to read sfx.exe");
    assert!(info.entry_count > 0, "SFX archive should have entries");
    verify_archive_testable("sfx.exe").expect("Failed to test sfx.exe");
}

// =============================================================================
// Special Case Tests
// =============================================================================

#[test]
fn test_read_t_series() {
    require_testdata!();
    // Test the t0-t5 archives (various edge cases)
    // t2.7z uses an unsupported/experimental codec
    // t3.7z has LZMA format issues
    let supported = [0, 1, 4, 5];
    for i in supported {
        let name = format!("t{}.7z", i);
        let result = verify_archive_readable(&name);
        assert!(
            result.is_ok(),
            "Failed to read {}: {:?}",
            name,
            result.err()
        );
    }
}

#[test]
fn test_read_compress_492() {
    require_testdata!();
    // Regression test for Apache Commons Compress issue 492
    let _info = verify_archive_readable("COMPRESS-492.7z").expect("Failed to read COMPRESS-492.7z");
}

#[test]
fn test_read_issue87() {
    require_testdata!();
    // Regression test for sevenzip issue 87
    let _info = verify_archive_readable("issue87.7z").expect("Failed to read issue87.7z");
}

// =============================================================================
// Archive Information Tests
// =============================================================================

#[test]
fn test_archive_info_lzma2() {
    require_testdata!();
    let path = Path::new(TESTDATA_DIR).join("lzma2.7z");
    let archive = Archive::open_path(&path).expect("Failed to open archive");
    let info = archive.info();

    // Verify basic info is populated
    assert!(info.entry_count > 0);
    assert!(info.total_size > 0);
    assert!(info.packed_size > 0);

    // LZMA2 archives should report compression methods
    assert!(
        !info.compression_methods.is_empty(),
        "Should have at least one compression method"
    );
}

#[test]
fn test_archive_entry_metadata() {
    require_testdata!();
    let path = Path::new(TESTDATA_DIR).join("lzma2.7z");
    let archive = Archive::open_path(&path).expect("Failed to open archive");

    for entry in archive.entries() {
        // All entries should have a name
        assert!(
            !entry.path.as_str().is_empty(),
            "Entry name should not be empty"
        );

        // Files should have a size (can be 0 for empty files)
        if !entry.is_directory {
            let _ = entry.size;
        }
    }
}

// =============================================================================
// Encryption Tests (require password)
// =============================================================================

#[test]
#[cfg(feature = "aes")]
fn test_read_aes_encrypted_without_password() {
    require_testdata!();
    // Reading encrypted archive without password should fail during test
    let path = Path::new(TESTDATA_DIR).join("aes7z.7z");
    let result = Archive::open_path(&path);

    // Opening might succeed (headers might not be encrypted)
    if let Ok(mut archive) = result {
        let info = archive.info();
        // Archive should indicate encryption
        if info.has_encrypted_entries {
            // Testing should fail without password
            let test_result = archive.test(SelectAll, &TestOptions::default());
            // Either test returns error or reports failures
            if let Ok(result) = test_result {
                assert!(
                    result.entries_failed > 0,
                    "Testing encrypted entries without password should fail"
                );
            }
        }
    }
}

// =============================================================================
// Multi-Volume Archive Tests
// =============================================================================

#[test]
fn test_read_multivolume() {
    require_testdata!();
    // Multi-volume archives: multi.7z.001 through multi.7z.006
    // Must use open_multivolume for archives with data spanning volumes
    let path = Path::new(TESTDATA_DIR).join("multi.7z.001");
    let mut archive =
        Archive::<MultiVolumeReader>::open_multivolume(&path).expect("Failed to open multi.7z.001");
    let info = archive.info();
    assert!(
        info.entry_count > 0,
        "Multi-volume archive should have entries"
    );

    // Test all entries - this decompresses and verifies CRC
    let result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Failed to test multi.7z.001");

    assert!(
        result.entries_failed == 0,
        "Test failed for {} entries: {:?}",
        result.entries_failed,
        result.failures
    );
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_read_all_reference_archives() {
    require_testdata!();
    // Test that all supported reference archives can at least be opened
    // Excludes:
    // - t2.7z: uses unsupported/experimental codec
    // - t3.7z: LZMA format edge case
    let archives = [
        "lzma.7z",
        "lzma2.7z",
        "deflate.7z",
        "bzip2.7z",
        "copy.7z",
        "bcj.7z",
        "bcj2.7z",
        "arm.7z",
        "ppc.7z",
        "sparc.7z",
        "delta.7z",
        "empty.7z",
        "empty2.7z",
        "file_and_empty.7z",
        "t0.7z",
        "t1.7z",
        // "t2.7z", // Unsupported codec
        // "t3.7z", // LZMA edge case
        "t4.7z",
        "t5.7z",
        "COMPRESS-492.7z",
        "issue87.7z",
        "sfx.exe",
    ];

    let mut failed = Vec::new();
    for archive in &archives {
        if let Err(e) = verify_archive_readable(archive) {
            failed.push((archive.to_string(), format!("{:?}", e)));
        }
    }

    assert!(failed.is_empty(), "Failed to read archives: {:?}", failed);
}

// =============================================================================
// Entry Listing Tests
// =============================================================================

#[test]
fn test_list_entries_lzma() {
    require_testdata!();
    let entries = list_entries("lzma.7z").expect("Failed to list entries");
    assert!(!entries.is_empty(), "Archive should have entries");

    // Verify all entries have valid paths
    for entry in &entries {
        assert!(!entry.is_empty(), "Entry path should not be empty");
        assert!(
            !entry.starts_with('/'),
            "Entry path should be relative: {}",
            entry
        );
        assert!(
            !entry.contains(".."),
            "Entry path should not contain '..': {}",
            entry
        );
    }
}

#[test]
fn test_compression_methods_reported() {
    require_testdata!();
    // Test that different archives report their compression methods correctly
    let path = Path::new(TESTDATA_DIR).join("lzma2.7z");
    let archive = Archive::open_path(&path).expect("Failed to open lzma2.7z");
    let info = archive.info();

    assert!(
        !info.compression_methods.is_empty(),
        "Should report at least one compression method"
    );

    // Check that we have an LZMA2 method
    let has_lzma2 = info
        .compression_methods
        .iter()
        .any(|m| matches!(m, zesven::codec::CodecMethod::Lzma2));

    assert!(has_lzma2, "LZMA2 archive should use LZMA2 compression");
}
