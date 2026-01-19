//! Fuzz target for ArchivePath::new with arbitrary string input.
//!
//! This target exercises the path validation logic with potentially malformed
//! or adversarial path strings. The goal is to find panics or logic errors
//! in path normalization and security checks.
//!
//! Run with: cargo +nightly fuzz run archive_path
//!
//! Key security properties being tested:
//! - Path traversal rejection (../)
//! - Absolute path rejection
//! - NUL byte handling
//! - Unicode normalization edge cases

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Try to interpret bytes as UTF-8 string
    if let Ok(path_str) = std::str::from_utf8(data) {
        // Attempt to create an ArchivePath
        let result = zesven::ArchivePath::new(path_str);

        // If creation succeeded, verify security invariants
        if let Ok(path) = result {
            let normalized = path.as_str();

            // Must not contain path traversal sequences after normalization
            assert!(
                !normalized.contains(".."),
                "Path traversal found in normalized path: {:?}",
                normalized
            );

            // Must not be absolute
            assert!(
                !normalized.starts_with('/'),
                "Absolute path accepted: {:?}",
                normalized
            );

            // Must not contain NUL bytes
            assert!(
                !normalized.contains('\0'),
                "NUL byte in normalized path: {:?}",
                normalized
            );
        }
    }
});
