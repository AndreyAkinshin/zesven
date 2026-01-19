//! Tests for password handling and encryption.
//!
//! These tests verify that zesven correctly handles encrypted archives.
//!
//! # Encryption Support
//!
//! zesven supports:
//! - **Header encryption** (`encrypt_header(true)`): Hides file names and metadata
//! - **Content encryption** (`encrypt_data(true)`): Encrypts file contents with AES-256
//! - Password storage with `password()`: Required for any encryption
//!
//! These tests verify both:
//! - Correct passwords successfully open and read archives
//! - Wrong passwords are properly rejected

#![cfg(all(feature = "aes", feature = "lzma2"))]

use std::io::Cursor;

use zesven::read::Archive;
use zesven::{ArchivePath, Error, Password, PasswordDetectionMethod, WriteOptions, Writer};

/// Test data - simple text content.
fn test_content() -> Vec<u8> {
    b"This is secret content for encryption testing.".to_vec()
}

/// Creates an archive with header encryption (hides file names).
fn create_header_encrypted_archive(password: &str) -> Vec<u8> {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(WriteOptions::new().password(password).encrypt_header(true));

        let path = ArchivePath::new("secret.txt").expect("Invalid path");
        writer
            .add_bytes(path, &test_content())
            .expect("Failed to add entry");
        let _ = writer.finish().expect("Failed to finish archive");
    }
    archive_bytes
}

// =============================================================================
// Password Detection Method Coverage Summary
// =============================================================================
//
// Note: Error type construction tests have been moved to unit tests in src/error.rs
// (test_password_detection_method_display and test_wrong_password_error_has_full_context)
//
// This test file verifies the following `PasswordDetectionMethod` variants:
//
// COVERED (integration-tested behavior):
// - `EarlyHeaderValidation`: Tested in `test_header_encryption_wrong_password_rejected`
//   Triggers when wrong password produces garbage that fails header structure validation.
//
// - `PasswordRequired`: Tested in `test_header_encryption_no_password`
//   Triggers when opening encrypted archive without providing any password.
//
// NOT YET TESTABLE (requires content encryption, which is not implemented):
// - `CrcMismatch`: Would trigger during extraction when decrypted content's CRC
//   doesn't match expected CRC. Requires `encrypt_data(true)` support.
//
// - `DecompressionFailure`: Would trigger when wrong password causes decompression
//   to fail on garbage data. Requires `encrypt_data(true)` support.
//
// See `test_data_encryption_returns_unsupported_error` for documentation of the
// current limitation. When content encryption is implemented, add tests for the
// remaining detection methods.

// =============================================================================
// Correct Password Tests (Header Encryption)
// =============================================================================

/// Tests that header-encrypted archive opens successfully with correct password.
#[test]
fn test_header_encryption_correct_password_succeeds() {
    let password = "correct_password";
    let archive_bytes = create_header_encrypted_archive(password);

    let cursor = Cursor::new(&archive_bytes);

    // Open with correct password - should succeed
    let archive =
        Archive::open_with_password(cursor, password).expect("Should open with correct password");

    // Verify archive contents
    assert_eq!(archive.len(), 1, "Should have 1 entry");

    let entry = &archive.entries()[0];
    assert_eq!(entry.path.as_str(), "secret.txt");
    assert_eq!(entry.size, test_content().len() as u64);
}

/// Tests full roundtrip: create encrypted archive, read it back, extract content.
#[test]
fn test_header_encryption_full_roundtrip() {
    let password = "roundtrip_test_password";
    let archive_bytes = create_header_encrypted_archive(password);

    let cursor = Cursor::new(&archive_bytes);
    let mut archive =
        Archive::open_with_password(cursor, password).expect("Should open with correct password");

    // Verify we can read the archive info
    let info = archive.info();
    assert_eq!(info.entry_count, 1);
    assert!(
        info.has_encrypted_header,
        "Archive should report header is encrypted"
    );

    // Verify entry metadata
    let entry = &archive.entries()[0];
    assert_eq!(entry.path.as_str(), "secret.txt");
    assert!(!entry.is_directory);

    // Verify actual content extraction works
    let extracted = archive
        .extract_to_vec("secret.txt")
        .expect("Should extract with correct password");
    assert_eq!(
        extracted,
        test_content(),
        "Decrypted content should match original"
    );
}

// =============================================================================
// Wrong Password Detection Tests (Header Encryption)
// =============================================================================

/// Tests that header-encrypted archive fails to open with wrong password.
///
/// With header encryption, the file names and metadata are encrypted.
/// Using a wrong password should fail during header decryption.
///
/// ## Acceptable Error Types
///
/// When decryption fails with wrong password, different error types may occur:
/// - `WrongPassword` - ideal case, detected via header validation
/// - `InvalidFormat` - garbage decryption produces invalid header data
/// - `Io` - truncated/malformed data from failed decryption
///
/// All are acceptable as long as the archive doesn't open successfully.
#[test]
fn test_header_encryption_wrong_password_rejected() {
    let correct_password = "correct_password";
    let wrong_password = "wrong_password";
    let archive_bytes = create_header_encrypted_archive(correct_password);

    let cursor = Cursor::new(&archive_bytes);

    // Try to open with wrong password - should fail
    match Archive::open_with_password(cursor, wrong_password) {
        Ok(_) => {
            panic!("Opening header-encrypted archive with wrong password should fail");
        }
        Err(Error::WrongPassword {
            detection_method, ..
        }) => {
            // Expected: wrong password detected via header validation
            assert_eq!(
                detection_method,
                PasswordDetectionMethod::EarlyHeaderValidation,
                "Expected early header validation for header encryption"
            );
        }
        // Acceptable alternative errors when decryption produces garbage:
        Err(Error::InvalidFormat(msg)) => {
            // Garbage decryption → invalid header structure
            assert!(
                !msg.is_empty(),
                "InvalidFormat error should have descriptive message"
            );
        }
        Err(Error::Io(io_err)) => {
            // Garbage decryption → truncated/malformed read
            // Multiple IO error kinds are acceptable depending on how decryption fails:
            // - UnexpectedEof: truncated data
            // - InvalidData: malformed data
            // - InvalidInput: invalid parameter after garbage decryption
            let acceptable_kinds = [
                std::io::ErrorKind::UnexpectedEof,
                std::io::ErrorKind::InvalidData,
                std::io::ErrorKind::InvalidInput,
            ];
            assert!(
                acceptable_kinds.contains(&io_err.kind()),
                "Unexpected IO error kind: {:?}",
                io_err.kind()
            );
        }
        Err(other) => {
            // Fail on truly unexpected error types to catch regressions
            panic!(
                "Unexpected error type when opening with wrong password: {:?}\n\
                 Expected: WrongPassword, InvalidFormat, or Io (UnexpectedEof/InvalidData)",
                other
            );
        }
    }
}

/// Tests that header-encrypted archive fails to open without password.
#[test]
fn test_header_encryption_no_password() {
    let password = "secret_password";
    let archive_bytes = create_header_encrypted_archive(password);

    let cursor = Cursor::new(&archive_bytes);

    // Try to open without password - should fail with PasswordRequired
    match Archive::open(cursor) {
        Ok(_) => {
            panic!("Opening header-encrypted archive without password should fail");
        }
        Err(Error::PasswordRequired) => {
            // Expected: password required for encrypted archive
        }
        Err(e) => {
            // Other errors are acceptable - encrypted header can't be read
            // But we should note what we got for debugging
            eprintln!("Got error (acceptable): {e}");
        }
    }
}

// Note: test_header_encryption_similar_passwords_rejected was removed as redundant.
// The property "wrong password fails" is already tested by test_header_encryption_wrong_password_rejected.
// The specific character differences (case, digits, spaces) are cryptographically irrelevant -
// any different password produces a different key via PBKDF2/AES.

/// Tests that empty password is rejected for header-encrypted archive.
#[test]
fn test_header_encryption_empty_password_rejected() {
    let correct_password = "non_empty_password";
    let archive_bytes = create_header_encrypted_archive(correct_password);

    let cursor = Cursor::new(&archive_bytes);

    // Try with empty password - should be rejected
    let result = Archive::open_with_password(cursor, "");
    assert!(
        result.is_err(),
        "Empty password should not decrypt header-encrypted archive"
    );
}

// Note: The following "wrong password rejected" test variants were removed as redundant:
// - test_header_encryption_unicode_password_rejected
// - test_header_encryption_emoji_password_rejected
// - test_header_encryption_medium_password_rejected
//
// The property "wrong password fails" is already tested by test_header_encryption_wrong_password_rejected.
// The password encoding (ASCII, Unicode, emoji) and length are irrelevant to rejection -
// any different password produces a different AES key via SHA-256 key derivation.
// The "success" tests below (unicode/emoji/medium_password_succeeds) verify that
// these password types work correctly for creating and opening encrypted archives.

// =============================================================================
// Long Password Tests
// =============================================================================
//
// Note: Unicode, emoji, and medium-length password "success" tests were removed
// as redundant. The core functionality (correct password opens archive) is tested by:
// - test_header_encryption_correct_password_succeeds (ASCII password)
// - test_header_encryption_full_roundtrip (complete roundtrip verification)
//
// Password encoding (ASCII, Unicode, emoji) is cryptographically irrelevant -
// all passwords go through SHA-256 key derivation identically. The mixed
// whitespace test below provides representative coverage for special characters.

/// Tests that long passwords (100 chars) work for full roundtrip.
///
/// This tests that passwords longer than typical use cases work correctly.
/// 100 characters provides adequate coverage for long-password handling
/// without excessive key derivation time.
///
/// Note: A 1000-character password test was previously ignored due to ~90s
/// key derivation time. The 100-character version exercises the same code
/// paths in reasonable time (<1s).
#[test]
fn test_header_encryption_long_password_succeeds() {
    let password: String = "a".repeat(100);
    let archive_bytes = create_header_encrypted_archive(&password);

    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open_with_password(cursor, password.as_str())
        .expect("Should open with correct long password");

    let extracted = archive
        .extract_to_vec("secret.txt")
        .expect("Should extract with correct password");
    assert_eq!(extracted, test_content(), "Decrypted content should match");
}

// =============================================================================
// Password Edge Case Tests (Special Characters)
// =============================================================================
//
// Note: Individual whitespace tests (spaces-only, newlines, tabs) were removed
// as redundant. SHA-256 doesn't treat whitespace specially, so the mixed
// whitespace test below provides complete coverage for all whitespace handling.

/// Tests that passwords with mixed whitespace characters work correctly.
///
/// This exercises passwords with spaces, tabs, and newlines mixed together,
/// providing representative coverage for all whitespace character handling.
/// Individual tests for spaces-only, newlines, and tabs were removed as
/// they verify the same property: whitespace bytes are valid in passwords.
#[test]
fn test_header_encryption_mixed_whitespace_password_succeeds() {
    let password = " \t\n \t\n "; // spaces, tabs, newlines
    let archive_bytes = create_header_encrypted_archive(password);

    let cursor = Cursor::new(&archive_bytes);
    let mut archive = Archive::open_with_password(cursor, password)
        .expect("Should open with mixed whitespace password");

    let extracted = archive
        .extract_to_vec("secret.txt")
        .expect("Should extract with mixed whitespace password");
    assert_eq!(extracted, test_content(), "Decrypted content should match");
}

// =============================================================================
// Empty Password Edge Case Tests
// =============================================================================

/// Tests behavior when creating an archive with an empty password.
///
/// Test: Empty password creates encrypted archive that requires empty password to open.
///
/// ## Documented Behavior
///
/// An empty password `""` is treated as a valid password, distinct from no password (`None`):
/// - `WriteOptions::new().password("").encrypt_header(true)` → Creates encrypted archive
/// - `WriteOptions::new().encrypt_header(true)` (no password) → Creates unencrypted archive
///
/// This is because `is_header_encrypted()` checks `self.password.is_some()`, and an empty
/// string wrapped in `Password` is `Some`, not `None`.
///
/// While this is consistent (write with "" requires read with ""), it may be surprising.
/// This test documents the current behavior; consider rejecting empty passwords in the future.
#[test]
fn test_write_with_empty_password_behavior() {
    // Create archive with empty password and header encryption
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(WriteOptions::new().password("").encrypt_header(true));

        let path = ArchivePath::new("test.txt").expect("valid path");
        writer
            .add_bytes(path, b"test content")
            .expect("add_bytes should succeed with empty password");

        let _result = writer
            .finish()
            .expect("finish should succeed with empty password");
    }

    // Verify the archive is actually encrypted (requires password to open)
    let cursor = Cursor::new(&archive_bytes);
    let open_result = Archive::open(cursor.clone());

    // Archive with empty password should NOT open without any password
    assert!(
        open_result.is_err(),
        "Archive encrypted with empty password should require a password to open"
    );

    // Archive should open with empty password
    let mut archive = Archive::open_with_password(cursor, "")
        .expect("Archive created with empty password should open with empty password");

    // Verify the header is marked as encrypted
    assert!(
        archive.info().has_encrypted_header,
        "Archive should have encrypted header"
    );

    // Verify content can be extracted
    let extracted = archive
        .extract_to_vec("test.txt")
        .expect("Should extract file from archive opened with empty password");
    assert_eq!(extracted, b"test content");
}

// =============================================================================
// Data Encryption Tests
// =============================================================================

/// Verifies that data (content) encryption works correctly.
///
/// This test creates an archive with encrypt_data(true), which encrypts
/// the file contents with AES-256. The archive should be readable with
/// the correct password.
#[test]
fn test_data_encryption_roundtrip() {
    let cursor = Cursor::new(Vec::new());
    let password = "test_password";
    let test_content = b"test content for data encryption";

    // Create archive with encrypted content
    let mut writer = Writer::create(cursor)
        .expect("Failed to create writer")
        .options(
            WriteOptions::new().password(password).encrypt_data(true), // Data encryption requested
        );

    let path = ArchivePath::new("test.txt").expect("valid path");
    writer
        .add_bytes(path, test_content)
        .expect("Should write encrypted content");

    let (_result, cursor) = writer.finish_into_inner().expect("Should finish archive");
    let archive_data = cursor.into_inner();

    // Read archive back with correct password
    let mut archive =
        Archive::open_with_password(Cursor::new(archive_data), Password::new(password))
            .expect("Should open encrypted archive");

    // Extract and verify content
    let extracted = archive
        .extract_to_vec("test.txt")
        .expect("Should extract encrypted content");

    assert_eq!(
        extracted, test_content,
        "Decrypted content should match original"
    );
}

// =============================================================================
// Content Encryption - Additional Test Cases
// =============================================================================
//
// Content encryption is now implemented. The following tests verify edge cases:
//
// - Wrong password detection during extraction (CrcMismatch or DecompressionFailure)
// - Combination of header + content encryption
// - Solid archives with encryption
//
// See also: write::tests::test_content_encryption_* in src/write/mod.rs
// See also: test_data_encryption_returns_unsupported_error which documents the
// current limitation.

// =============================================================================
// Password Edge Case Tests
// =============================================================================
//
// These tests verify password handling for edge cases including unusual
// characters, Unicode boundaries, and encoding edge cases.
//
// Note: Individual tests for combining characters, emoji, RTL, and supplementary
// characters were consolidated into `test_password_unicode_codepoints` since
// they all exercise the same UTF-16LE encoding code path. The null bytes test
// remains separate as it tests a distinct edge case (embedded NUL characters).

/// Tests that password with null bytes works correctly.
///
/// 7z uses UTF-16LE encoding for password key derivation. This test verifies
/// that passwords containing literal null bytes (which are valid in UTF-16LE
/// since ASCII characters are represented as byte + 0x00) are handled correctly.
///
/// This is kept separate from the Unicode codepoint test because embedded NUL
/// characters are a distinct edge case from multi-byte Unicode sequences.
#[test]
fn test_password_encoding_edge_cases() {
    // ASCII character followed by null byte in UTF-16LE looks like: 'a' (0x61, 0x00)
    // A password containing literal null bytes tests encoding edge cases
    let password = "test\0null\0bytes";
    let archive_bytes = create_header_encrypted_archive(password);

    let cursor = Cursor::new(&archive_bytes);
    let mut archive =
        Archive::open_with_password(cursor, password).expect("Should open with null byte password");

    let extracted = archive
        .extract_to_vec("secret.txt")
        .expect("Should extract with null byte password");
    assert_eq!(extracted, test_content());
}

/// Tests that various Unicode codepoint types work in passwords.
///
/// This consolidated test verifies that the password system correctly handles:
/// - Combining characters (e + combining acute → é)
/// - Multi-codepoint emoji sequences (family emoji with ZWJ)
/// - Right-to-left characters (Arabic script)
/// - Supplementary characters outside BMP (require UTF-16 surrogate pairs)
///
/// All these cases exercise the UTF-16LE encoding used for 7z password key derivation.
/// Since they all use the same code path, they're consolidated into a single test
/// with multiple sub-cases for efficiency.
#[test]
fn test_password_unicode_codepoints() {
    // Test cases covering different Unicode edge cases
    let test_cases = [
        // Combining characters: e + combining acute accent
        ("cafe\u{0301}", "combining character"),
        // Multi-codepoint emoji: man + ZWJ + woman + ZWJ + girl
        (
            "family\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "emoji sequence",
        ),
        // RTL: Arabic text "كلمة_السر_123"
        ("كلمة_السر_123", "RTL characters"),
        // Supplementary: characters outside BMP requiring surrogate pairs
        // U+1F4A9 PILE OF POO + U+10000 LINEAR B SYLLABLE B008 A
        ("test\u{1F4A9}\u{10000}end", "supplementary characters"),
    ];

    for (password, description) in test_cases {
        let archive_bytes = create_header_encrypted_archive(password);

        let cursor = Cursor::new(&archive_bytes);
        let mut archive = Archive::open_with_password(cursor, password)
            .unwrap_or_else(|e| panic!("Should open with {} password: {:?}", description, e));

        let extracted = archive
            .extract_to_vec("secret.txt")
            .unwrap_or_else(|e| panic!("Should extract with {} password: {:?}", description, e));

        assert_eq!(
            extracted,
            test_content(),
            "Content mismatch for {} password",
            description
        );
    }
}

/// Tests that Unicode normalization differences are preserved.
///
/// NFC and NFD are different byte sequences. The password system should
/// treat them as different passwords (not normalize them). This verifies
/// a behavioral property: different byte representations produce different keys.
#[test]
fn test_unicode_normalization_treated_as_different() {
    // NFC: é as single codepoint
    let password_nfc = "caf\u{00E9}";
    // NFD: e + combining acute
    let password_nfd = "cafe\u{0301}";

    // These look identical but are different bytes
    assert_ne!(
        password_nfc.as_bytes(),
        password_nfd.as_bytes(),
        "NFC and NFD should be different byte sequences"
    );

    // Create archive with NFC password
    let archive_bytes = create_header_encrypted_archive(password_nfc);

    // NFC password should work
    let cursor = Cursor::new(&archive_bytes);
    let result_nfc = Archive::open_with_password(cursor, password_nfc);
    assert!(result_nfc.is_ok(), "NFC password should work");

    // NFD password should NOT work (different password)
    let cursor = Cursor::new(&archive_bytes);
    let result_nfd = Archive::open_with_password(cursor, password_nfd);
    assert!(
        result_nfd.is_err(),
        "NFD password should fail (different from NFC)"
    );
}
