//! Integration tests for self-extracting archive (SFX) functionality.
//!
//! These tests verify SFX creation, extraction, and roundtrip functionality
//! using the public API.

#![cfg(feature = "lzma")]

mod common;

use std::io::Cursor;
use zesven::read::Archive;
use zesven::sfx::{
    SfxBuilder, SfxConfig, SfxFormat, SfxResult, SfxStub, create_sfx, extract_archive_from_sfx,
};

/// Creates a minimal fake PE stub for testing.
fn create_fake_pe_stub() -> Vec<u8> {
    let mut stub = vec![0u8; 256];
    // MZ signature
    stub[0] = b'M';
    stub[1] = b'Z';
    // PE header offset at 0x3C
    stub[0x3C] = 64;
    // PE signature at offset 64
    stub[64] = b'P';
    stub[65] = b'E';
    stub[66] = 0;
    stub[67] = 0;
    stub
}

/// Creates a minimal fake ELF stub for testing.
fn create_fake_elf_stub() -> Vec<u8> {
    let mut stub = vec![0u8; 64];
    // ELF magic
    stub[0..4].copy_from_slice(b"\x7FELF");
    stub[4] = 2; // 64-bit
    stub[5] = 1; // Little endian
    stub[16] = 2; // Executable type
    stub[17] = 0;
    stub
}

/// Tests that SfxBuilder requires a stub.
#[test]
fn test_sfx_builder_requires_stub() {
    let entries = [("test.txt", b"Content" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let builder = SfxBuilder::new();
    let mut output = Vec::new();

    let result = builder.build(&mut output, &archive);
    assert!(result.is_err(), "Should fail without stub");
}

/// Tests SfxBuilder creates executable with PE stub.
#[test]
fn test_sfx_builder_creates_executable_pe() {
    let entries = [("readme.txt", b"Installation instructions" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub_data = create_fake_pe_stub();
    let stub = SfxStub::with_format(stub_data.clone(), SfxFormat::WindowsPe);

    let mut output = Vec::new();
    let result = SfxBuilder::new()
        .stub(stub)
        .validate_stub(false)
        .build(&mut output, &archive)
        .expect("Failed to build SFX");

    assert_eq!(result.stub_size, stub_data.len() as u64);
    assert_eq!(result.archive_size, archive.len() as u64);
    assert_eq!(result.total_size, output.len() as u64);

    // Output should start with MZ
    assert_eq!(&output[0..2], b"MZ");
}

/// Tests SfxBuilder roundtrip with PE stub.
#[test]
fn test_sfx_roundtrip_with_pe_stub() {
    let content = b"Content that must survive roundtrip";
    let entries = [("important.txt", content.as_slice())];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = SfxStub::with_format(create_fake_pe_stub(), SfxFormat::WindowsPe);

    // Build SFX
    let mut sfx_data = Vec::new();
    let _result = SfxBuilder::new()
        .stub(stub)
        .validate_stub(false)
        .build(&mut sfx_data, &archive)
        .expect("Failed to build SFX");

    // Extract archive from SFX
    let (extracted_archive, info) =
        extract_archive_from_sfx(&sfx_data).expect("Failed to extract from SFX");

    assert_eq!(info.format, Some(SfxFormat::WindowsPe));
    assert!(info.archive_offset > 0);

    // Verify we can open the extracted archive
    let cursor = Cursor::new(extracted_archive);
    let mut archive = Archive::open(cursor).expect("Failed to open extracted archive");

    let extracted_content = archive
        .extract_to_vec("important.txt")
        .expect("Failed to extract file");
    assert_eq!(extracted_content, content);
}

/// Tests SfxBuilder roundtrip with ELF stub.
#[test]
fn test_sfx_roundtrip_with_elf_stub() {
    let content = b"Linux executable content";
    let entries = [("data.txt", content.as_slice())];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = SfxStub::with_format(create_fake_elf_stub(), SfxFormat::LinuxElf);

    // Build SFX
    let mut sfx_data = Vec::new();
    let _result = SfxBuilder::new()
        .stub(stub)
        .validate_stub(false)
        .build(&mut sfx_data, &archive)
        .expect("Failed to build SFX");

    // Extract archive from SFX
    let (extracted_archive, info) =
        extract_archive_from_sfx(&sfx_data).expect("Failed to extract from SFX");

    assert_eq!(info.format, Some(SfxFormat::LinuxElf));

    // Verify content
    let cursor = Cursor::new(extracted_archive);
    let mut archive = Archive::open(cursor).expect("Failed to open");
    let extracted = archive
        .extract_to_vec("data.txt")
        .expect("Failed to extract");
    assert_eq!(extracted, content);
}

/// Tests SfxBuilder with config options.
#[test]
fn test_sfx_with_config_options() {
    let entries = [("setup.exe", b"Setup program data" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = SfxStub::with_format(create_fake_pe_stub(), SfxFormat::WindowsPe);
    let config = SfxConfig::new()
        .title("My Installer")
        .run_program("setup.exe")
        .progress(true);

    let mut sfx_data = Vec::new();
    let result = SfxBuilder::new()
        .stub(stub)
        .config(config)
        .validate_stub(false)
        .build(&mut sfx_data, &archive)
        .expect("Failed to build SFX");

    // Config should add bytes
    assert!(result.config_size > 0, "Config should add data");
    assert!(result.total_size > result.stub_size + result.archive_size);

    // Check config is embedded
    let sfx_str = String::from_utf8_lossy(&sfx_data);
    assert!(sfx_str.contains(";!@Install@!UTF-8!"));
    assert!(sfx_str.contains("Title=\"My Installer\""));
    assert!(sfx_str.contains("RunProgram=\"setup.exe\""));
}

/// Tests extract_archive_from_sfx function.
#[test]
fn test_extract_archive_from_sfx() {
    let entries = [("file.txt", b"File content" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    // Create SFX manually
    let stub = create_fake_pe_stub();
    let mut sfx_data = Vec::new();
    create_sfx(&mut sfx_data, &stub, None, &archive).expect("Failed to create SFX");

    // Extract
    let (extracted, info) = extract_archive_from_sfx(&sfx_data).expect("Failed to extract");

    assert_eq!(info.archive_offset, stub.len() as u64);
    assert_eq!(info.stub_size, stub.len() as u64);
    assert_eq!(extracted, archive);
}

/// Tests create_sfx function directly.
#[test]
fn test_create_sfx_function() {
    let entries = [("test.txt", b"Test" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = create_fake_elf_stub();
    let config = SfxConfig::new().title("Test");

    let mut output = Vec::new();
    let total =
        create_sfx(&mut output, &stub, Some(&config), &archive).expect("Failed to create SFX");

    assert_eq!(total, output.len() as u64);
    assert!(total > (stub.len() + archive.len()) as u64); // Config adds bytes
}

/// Tests create_sfx without config.
#[test]
fn test_create_sfx_no_config() {
    let entries = [("test.txt", b"Test" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = vec![1, 2, 3, 4, 5]; // Minimal stub

    let mut output = Vec::new();
    let total = create_sfx(&mut output, &stub, None, &archive).expect("Failed to create SFX");

    assert_eq!(total, (stub.len() + archive.len()) as u64);
}

/// Tests SfxInfo detection.
#[test]
fn test_sfx_info_detection() {
    let entries = [("file.txt", b"Data" as &[u8])];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    // Test PE detection
    let pe_stub = create_fake_pe_stub();
    let mut pe_sfx = Vec::new();
    let _result = SfxBuilder::new()
        .stub(SfxStub::with_format(pe_stub.clone(), SfxFormat::WindowsPe))
        .validate_stub(false)
        .build(&mut pe_sfx, &archive)
        .unwrap();

    let (_, pe_info) = extract_archive_from_sfx(&pe_sfx).unwrap();
    assert_eq!(pe_info.format, Some(SfxFormat::WindowsPe));

    // Test ELF detection
    let elf_stub = create_fake_elf_stub();
    let mut elf_sfx = Vec::new();
    let _result = SfxBuilder::new()
        .stub(SfxStub::with_format(elf_stub.clone(), SfxFormat::LinuxElf))
        .validate_stub(false)
        .build(&mut elf_sfx, &archive)
        .unwrap();

    let (_, elf_info) = extract_archive_from_sfx(&elf_sfx).unwrap();
    assert_eq!(elf_info.format, Some(SfxFormat::LinuxElf));
}

/// Tests SfxResult overhead calculation.
#[test]
fn test_sfx_result_overhead_percent() {
    let result = SfxResult {
        total_size: 1000,
        stub_size: 100,
        config_size: 50,
        archive_size: 850,
    };

    let overhead = result.overhead_percent();
    assert!((overhead - 15.0).abs() < 0.001, "Overhead should be 15%");
}

/// Tests that SFX can be opened directly by Archive.
#[test]
fn test_sfx_can_be_opened_by_archive() {
    let content = b"Archive content inside SFX";
    let entries = [("nested.txt", content.as_slice())];
    let archive = common::create_archive(&entries).expect("Failed to create archive");

    let stub = SfxStub::with_format(create_fake_pe_stub(), SfxFormat::WindowsPe);

    let mut sfx_data = Vec::new();
    let _result = SfxBuilder::new()
        .stub(stub)
        .validate_stub(false)
        .build(&mut sfx_data, &archive)
        .expect("Failed to build SFX");

    // Archive::open should handle SFX detection automatically
    let cursor = Cursor::new(sfx_data);
    let mut archive = Archive::open(cursor).expect("Should open SFX directly");

    let extracted = archive
        .extract_to_vec("nested.txt")
        .expect("Should extract from SFX");
    assert_eq!(extracted, content);
}
