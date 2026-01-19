//! Roundtrip tests for BCJ filter support.
//!
//! These tests verify that archives created with pre-compression filters
//! can be read back correctly.

// Filters require the lzma feature for BCJ encoders
#![cfg(feature = "lzma")]

mod common;

use zesven::write::{WriteFilter, WriteOptions};

/// Test data that simulates x86 code patterns.
/// Real x86 code has relative jumps/calls that BCJ filters transform.
fn x86_like_data() -> Vec<u8> {
    let mut data = Vec::new();
    // Simulated x86 code with relative CALL instructions (E8 xx xx xx xx)
    for i in 0..100 {
        // CALL relative (E8)
        data.push(0xE8);
        // Relative address (4 bytes little-endian)
        let offset = (i * 0x1000) as u32;
        data.extend_from_slice(&offset.to_le_bytes());
        // Some filler instructions
        data.extend_from_slice(&[0x90, 0x90, 0x90]); // NOPs
    }
    data
}

/// Test data that simulates ARM code patterns.
fn arm_like_data() -> Vec<u8> {
    let mut data = Vec::new();
    // Simulated ARM instructions (4 bytes each)
    for i in 0..100 {
        // ARM BL instruction pattern
        let instr = 0xEB000000u32 | (i & 0xFFFFFF);
        data.extend_from_slice(&instr.to_le_bytes());
    }
    data
}

/// Simple repetitive data for Delta filter testing.
fn delta_test_data() -> Vec<u8> {
    // Audio-like data: values that differ by small amounts
    let mut data = Vec::new();
    let mut value: u16 = 32768;
    for i in 0..1000 {
        // Simulate audio samples varying around a center
        let delta = ((i as f32 * 0.1).sin() * 100.0) as i16;
        value = value.wrapping_add_signed(delta);
        data.extend_from_slice(&value.to_le_bytes());
    }
    data
}

#[test]
fn test_bcj_x86_roundtrip() {
    let data = x86_like_data();
    let entries = [("test.exe", data.as_slice())];

    let options = WriteOptions::new().filter(WriteFilter::BcjX86);
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create BCJ x86 filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj_arm_roundtrip() {
    let data = arm_like_data();
    let entries = [("test.bin", data.as_slice())];

    let options = WriteOptions::new().filter(WriteFilter::BcjArm);
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create BCJ ARM filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj_arm64_roundtrip() {
    let data = arm_like_data();
    let entries = [("test.bin", data.as_slice())];

    let options = WriteOptions::new().filter(WriteFilter::BcjArm64);
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create BCJ ARM64 filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_delta_roundtrip() {
    let data = delta_test_data();
    let entries = [("audio.raw", data.as_slice())];

    // Delta filter with distance=2 (16-bit samples)
    let options = WriteOptions::new().filter(WriteFilter::Delta { distance: 2 });
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create Delta filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_delta_distance_4_roundtrip() {
    let data = delta_test_data();
    let entries = [("data.raw", data.as_slice())];

    // Delta filter with distance=4 (32-bit samples)
    let options = WriteOptions::new().delta(4);
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create Delta filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_filter_with_multiple_files() {
    let exe_data = x86_like_data();
    let text_data = b"Hello, World! This is some text content.";

    let entries = [
        ("program.exe", exe_data.as_slice()),
        ("readme.txt", text_data.as_slice()),
    ];

    let options = WriteOptions::new().bcj_x86();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create multi-file filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

// Solid archives with filters use a decoder chain (e.g., BCJ + LZMA2).
// Both the writer and reader support this combination via codec::build_decoder_chain().
#[test]
fn test_filter_with_solid_archive() {
    use zesven::write::SolidOptions;

    let exe_data = x86_like_data();
    let text_data = b"Some text content for testing solid mode.";

    let entries = [
        ("app.exe", exe_data.as_slice()),
        ("config.txt", text_data.as_slice()),
    ];

    let options = WriteOptions::new()
        .filter(WriteFilter::BcjX86)
        .solid_options(SolidOptions::enabled());
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create solid filtered archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_filter_builder_methods() {
    let data = x86_like_data();
    let entries = [("test.exe", data.as_slice())];

    // Test bcj_x86() convenience method
    let options = WriteOptions::new().bcj_x86();
    assert_eq!(options.filter, WriteFilter::BcjX86);
    assert!(options.has_filter());

    let archive =
        common::create_archive_with_options(options, &entries).expect("bcj_x86() method failed");
    common::verify_archive_contents(&archive, &entries);

    // Test bcj_arm() convenience method
    let options = WriteOptions::new().bcj_arm();
    assert_eq!(options.filter, WriteFilter::BcjArm);

    // Test bcj_arm64() convenience method
    let options = WriteOptions::new().bcj_arm64();
    assert_eq!(options.filter, WriteFilter::BcjArm64);

    // Test delta() convenience method
    let options = WriteOptions::new().delta(2);
    assert_eq!(options.filter, WriteFilter::Delta { distance: 2 });
}

#[test]
fn test_no_filter_default() {
    let options = WriteOptions::new();
    assert_eq!(options.filter, WriteFilter::None);
    assert!(!options.has_filter());
}

#[test]
fn test_filter_with_empty_file() {
    let entries: [(&str, &[u8]); 1] = [("empty.txt", b"")];

    let options = WriteOptions::new().bcj_x86();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create filtered archive with empty file");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_filter_with_small_file() {
    // File smaller than BCJ alignment (BCJ x86 works on 5-byte chunks)
    let entries = [("tiny.txt", b"abc" as &[u8])];

    let options = WriteOptions::new().bcj_x86();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create filtered archive with small file");

    common::verify_archive_contents(&archive, &entries);
}

// Test with encryption if AES feature is enabled
#[cfg(feature = "aes")]
mod encrypted_filter_tests {
    use super::*;

    #[test]
    fn test_bcj_with_encryption_roundtrip() {
        let data = super::x86_like_data();
        let entries = [("secure.exe", data.as_slice())];
        let password = "test_password_123";

        let options = WriteOptions::new()
            .filter(WriteFilter::BcjX86)
            .password(password)
            .encrypt_data(true);

        let archive = common::create_archive_with_options(options, &entries)
            .expect("Failed to create encrypted + filtered archive");

        common::verify_encrypted_archive(&archive, password, &entries);
    }

    #[test]
    fn test_delta_with_encryption_roundtrip() {
        let data = super::delta_test_data();
        let entries = [("secure_audio.raw", data.as_slice())];
        let password = "audio_password";

        let options = WriteOptions::new()
            .delta(2)
            .password(password)
            .encrypt_data(true);

        let archive = common::create_archive_with_options(options, &entries)
            .expect("Failed to create encrypted + delta filtered archive");

        common::verify_encrypted_archive(&archive, password, &entries);
    }

    #[test]
    fn test_filter_with_header_encryption() {
        let data = super::x86_like_data();
        let entries = [("hidden.exe", data.as_slice())];
        let password = "header_enc_test";

        let options = WriteOptions::new()
            .bcj_x86()
            .password(password)
            .encrypt_data(true)
            .encrypt_header(true);

        let archive = common::create_archive_with_options(options, &entries)
            .expect("Failed to create archive with filter + header encryption");

        common::verify_encrypted_archive(&archive, password, &entries);
    }
}

#[test]
fn test_all_bcj_filters() {
    // Test all BCJ filter variants work
    let data = x86_like_data();

    let filters = [
        WriteFilter::BcjX86,
        WriteFilter::BcjArm,
        WriteFilter::BcjArm64,
        WriteFilter::BcjArmThumb,
        WriteFilter::BcjPpc,
        WriteFilter::BcjSparc,
        WriteFilter::BcjIa64,
        WriteFilter::BcjRiscv,
    ];

    for filter in filters {
        let entries = [("test.bin", data.as_slice())];
        let options = WriteOptions::new().filter(filter);
        let archive = common::create_archive_with_options(options, &entries)
            .unwrap_or_else(|e| panic!("Failed to create archive with {:?}: {}", filter, e));

        common::verify_archive_contents(&archive, &entries);
    }
}

#[test]
fn test_write_filter_method_id() {
    use zesven::codec::method;

    // Verify method IDs match expected values
    assert_eq!(WriteFilter::BcjX86.method_id(), Some(method::BCJ_X86));
    assert_eq!(WriteFilter::BcjArm.method_id(), Some(method::BCJ_ARM));
    assert_eq!(WriteFilter::BcjArm64.method_id(), Some(method::BCJ_ARM64));
    assert_eq!(
        WriteFilter::BcjArmThumb.method_id(),
        Some(method::BCJ_ARM_THUMB)
    );
    assert_eq!(WriteFilter::BcjPpc.method_id(), Some(method::BCJ_PPC));
    assert_eq!(WriteFilter::BcjSparc.method_id(), Some(method::BCJ_SPARC));
    assert_eq!(WriteFilter::BcjIa64.method_id(), Some(method::BCJ_IA64));
    assert_eq!(WriteFilter::BcjRiscv.method_id(), Some(method::BCJ_RISCV));
    assert_eq!(
        WriteFilter::Delta { distance: 1 }.method_id(),
        Some(method::DELTA)
    );
    assert_eq!(WriteFilter::None.method_id(), None);
}

#[test]
fn test_write_filter_properties() {
    // BCJ filters have no properties
    assert_eq!(WriteFilter::BcjX86.properties(), None);
    assert_eq!(WriteFilter::BcjArm.properties(), None);
    assert_eq!(WriteFilter::None.properties(), None);

    // Delta filter has 1-byte property (distance - 1)
    assert_eq!(
        WriteFilter::Delta { distance: 1 }.properties(),
        Some(vec![0])
    );
    assert_eq!(
        WriteFilter::Delta { distance: 2 }.properties(),
        Some(vec![1])
    );
    assert_eq!(
        WriteFilter::Delta { distance: 4 }.properties(),
        Some(vec![3])
    );
}

// ==========================================================================
// BCJ2 4-Stream Filter Tests
// ==========================================================================

#[test]
fn test_bcj2_roundtrip() {
    // x86-like data with CALL instructions
    let data = x86_like_data();
    let entries = [("test.exe", data.as_slice())];

    let options = WriteOptions::new().filter(WriteFilter::Bcj2);
    let archive = common::create_archive_with_options(options, &entries)
        .expect("Failed to create BCJ2 archive");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj2_empty_file() {
    let entries: [(&str, &[u8]); 1] = [("empty.bin", b"")];

    let options = WriteOptions::new().bcj2();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("BCJ2 should handle empty files");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj2_multiple_files() {
    let exe_data = x86_like_data();
    let text_data = b"Hello, World!";

    let entries = [
        ("app.exe", exe_data.as_slice()),
        ("readme.txt", text_data.as_slice()),
    ];

    let options = WriteOptions::new().bcj2();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("BCJ2 should handle multiple files");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj2_small_file() {
    // Very small file (less than typical x86 instruction size)
    let entries = [("tiny.bin", b"abc" as &[u8])];

    let options = WriteOptions::new().bcj2();
    let archive = common::create_archive_with_options(options, &entries)
        .expect("BCJ2 should handle small files");

    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj2_builder_method() {
    let data = x86_like_data();
    let entries = [("test.exe", data.as_slice())];

    // Test bcj2() convenience method
    let options = WriteOptions::new().bcj2();
    assert_eq!(options.filter, WriteFilter::Bcj2);
    assert!(options.has_filter());

    let archive =
        common::create_archive_with_options(options, &entries).expect("bcj2() method failed");
    common::verify_archive_contents(&archive, &entries);
}

#[test]
fn test_bcj2_is_bcj2_method() {
    // Test the is_bcj2() method
    assert!(WriteFilter::Bcj2.is_bcj2());
    assert!(!WriteFilter::BcjX86.is_bcj2());
    assert!(!WriteFilter::None.is_bcj2());
    assert!(!WriteFilter::Delta { distance: 1 }.is_bcj2());
}

#[test]
fn test_bcj2_method_id() {
    use zesven::codec::method;

    // Verify BCJ2 method ID
    assert_eq!(WriteFilter::Bcj2.method_id(), Some(method::BCJ2));
}
