//! CLI command integration tests.
//!
//! These tests verify the core functionality that CLI commands would use.
//! Tests use library functions directly rather than subprocess execution.

// These tests require LZMA support (default compression method for writing)
#![cfg(feature = "lzma")]

use std::io::Cursor;
use std::path::PathBuf;
use tempfile::TempDir;
use zesven::read::{Archive, ExtractOptions, SelectAll, SelectByName, TestOptions};
use zesven::{ArchivePath, WriteOptions, Writer};

mod common;

/// Creates a test archive in memory with the given entries.
fn create_test_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    common::create_archive(entries).expect("Failed to create test archive")
}

/// Creates a test archive file on disk.
fn create_test_archive_file(entries: &[(&str, &[u8])]) -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let archive_path = temp_dir.path().join("test.7z");
    let archive_bytes = create_test_archive(entries);
    std::fs::write(&archive_path, &archive_bytes).expect("Failed to write archive");
    (temp_dir, archive_path)
}

// =============================================================================
// List Command Tests
// =============================================================================

#[test]
fn test_list_basic() {
    let archive_bytes = create_test_archive(&[
        ("file1.txt", b"content1"),
        ("file2.txt", b"content2"),
        ("subdir/file3.txt", b"content3"),
    ]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");

    let entries = archive.entries();
    assert_eq!(entries.len(), 3);

    let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert!(names.contains(&"file1.txt"));
    assert!(names.contains(&"file2.txt"));
    assert!(names.contains(&"subdir/file3.txt"));
}

#[test]
fn test_list_empty_archive() {
    let archive_bytes = create_test_archive(&[]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");

    assert!(archive.is_empty());
    assert_eq!(archive.len(), 0);
}

#[test]
fn test_list_archive_info() {
    let archive_bytes = create_test_archive(&[("a.txt", b"hello"), ("b.txt", b"world")]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");
    let info = archive.info();

    assert_eq!(info.entry_count, 2);
    assert_eq!(info.total_size, 10); // "hello" + "world" = 10 bytes
}

#[test]
fn test_list_nonexistent_file() {
    let result = Archive::open_path("/nonexistent/path/archive.7z");
    assert!(result.is_err());
}

// =============================================================================
// Extract Command Tests
// =============================================================================

#[test]
fn test_extract_all() {
    let (temp_dir, archive_path) = create_test_archive_file(&[
        ("file1.txt", b"hello world"),
        ("file2.txt", b"goodbye world"),
    ]);

    let output_dir = temp_dir.path().join("output");
    std::fs::create_dir(&output_dir).expect("Failed to create output dir");

    let mut archive = Archive::open_path(&archive_path).expect("Failed to open archive");
    let result = archive
        .extract(&output_dir, SelectAll, &ExtractOptions::default())
        .expect("Failed to extract");

    assert_eq!(result.entries_extracted, 2);
    assert!(output_dir.join("file1.txt").exists());
    assert!(output_dir.join("file2.txt").exists());

    let content1 = std::fs::read(output_dir.join("file1.txt")).unwrap();
    assert_eq!(content1, b"hello world");

    let content2 = std::fs::read(output_dir.join("file2.txt")).unwrap();
    assert_eq!(content2, b"goodbye world");
}

#[test]
fn test_extract_selective() {
    let (temp_dir, archive_path) = create_test_archive_file(&[
        ("keep.txt", b"keep this"),
        ("skip.txt", b"skip this"),
        ("also_keep.txt", b"also keep"),
    ]);

    let output_dir = temp_dir.path().join("output");
    std::fs::create_dir(&output_dir).expect("Failed to create output dir");

    let mut archive = Archive::open_path(&archive_path).expect("Failed to open archive");

    // Extract only files containing "keep" in the name
    let selector = zesven::read::SelectByPredicate::new(|e| e.path.as_str().contains("keep"));
    let result = archive
        .extract(&output_dir, selector, &ExtractOptions::default())
        .expect("Failed to extract");

    assert_eq!(result.entries_extracted, 2);
    assert!(output_dir.join("keep.txt").exists());
    assert!(output_dir.join("also_keep.txt").exists());
    assert!(!output_dir.join("skip.txt").exists());
}

#[test]
fn test_extract_to_directory() {
    let (temp_dir, archive_path) =
        create_test_archive_file(&[("nested/deep/file.txt", b"deep content")]);

    let output_dir = temp_dir.path().join("custom_output");
    std::fs::create_dir_all(&output_dir).expect("Failed to create output dir");

    let mut archive = Archive::open_path(&archive_path).expect("Failed to open archive");
    let result = archive
        .extract(&output_dir, SelectAll, &ExtractOptions::default())
        .expect("Failed to extract");

    assert_eq!(result.entries_extracted, 1);
    let deep_file = output_dir.join("nested/deep/file.txt");
    assert!(deep_file.exists());

    let content = std::fs::read(&deep_file).unwrap();
    assert_eq!(content, b"deep content");
}

#[test]
fn test_extract_to_vec() {
    let archive_bytes = create_test_archive(&[("test.txt", b"test content here")]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");
    let content = archive
        .extract_to_vec("test.txt")
        .expect("Failed to extract");

    assert_eq!(content, b"test content here");
}

#[test]
fn test_extract_entry_not_found() {
    let archive_bytes = create_test_archive(&[("exists.txt", b"hello")]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");
    let result = archive.extract_to_vec("nonexistent.txt");

    assert!(result.is_err());
}

#[cfg(feature = "aes")]
#[test]
fn test_extract_wrong_password() {
    use zesven::Password;

    // Create encrypted archive
    let correct_password = Password::new("correct");
    let wrong_password = Password::new("wrong");

    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).unwrap().options(
            WriteOptions::new()
                .password(correct_password)
                .encrypt_data(true),
        );

        writer
            .add_bytes(ArchivePath::new("secret.txt").unwrap(), b"secret data")
            .unwrap();

        let _result = writer.finish().unwrap();
    }

    // Try to extract with wrong password
    let mut archive = Archive::open_with_password(Cursor::new(&archive_bytes), wrong_password)
        .expect("Archive should open even with wrong password");

    let result = archive.extract_to_vec("secret.txt");
    assert!(
        result.is_err(),
        "Extraction with wrong password should fail"
    );
}

// =============================================================================
// Create Command Tests
// =============================================================================

#[test]
fn test_create_basic() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create a source file
    let source_file = temp_dir.path().join("source.txt");
    std::fs::write(&source_file, b"source content").expect("Failed to write source");

    // Create archive in memory
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).expect("Failed to create writer");

        writer
            .add_path(&source_file, ArchivePath::new("source.txt").unwrap())
            .expect("Failed to add file");

        let result = writer.finish().expect("Failed to finish");
        assert_eq!(result.entries_written, 1);
    }

    // Verify archive is readable
    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open archive");
    assert_eq!(archive.len(), 1);
}

#[test]
fn test_create_with_compression_level() {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let options = WriteOptions::new().level(9).expect("Invalid level");
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(options);

        writer
            .add_bytes(ArchivePath::new("file.txt").unwrap(), b"content")
            .expect("Failed to add file");

        let result = writer.finish().expect("Failed to finish");
        assert_eq!(result.entries_written, 1);
    }

    // Verify it's readable
    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    assert_eq!(archive.len(), 1);
}

#[cfg(feature = "aes")]
#[test]
fn test_create_encrypted() {
    use zesven::Password;

    let password = Password::new("test_password");
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let options = WriteOptions::new()
            .password(password.clone())
            .encrypt_data(true)
            .encrypt_header(true);

        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(options);

        writer
            .add_bytes(ArchivePath::new("secret.txt").unwrap(), b"secret data")
            .expect("Failed to add file");

        let result = writer.finish().expect("Failed to finish");
        assert_eq!(result.entries_written, 1);
    }

    // Verify it's readable with correct password
    let mut archive = Archive::open_with_password(Cursor::new(&archive_bytes), password)
        .expect("Failed to open encrypted archive");

    let content = archive
        .extract_to_vec("secret.txt")
        .expect("Failed to extract");
    assert_eq!(content, b"secret data");
}

#[test]
fn test_create_multiple_files() {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).expect("Failed to create writer");

        for i in 1..=5 {
            let name = format!("file{}.txt", i);
            let content = format!("content of file {}", i);
            writer
                .add_bytes(ArchivePath::new(&name).unwrap(), content.as_bytes())
                .expect("Failed to add file");
        }

        let result = writer.finish().expect("Failed to finish");
        assert_eq!(result.entries_written, 5);
    }

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    assert_eq!(archive.len(), 5);
}

#[test]
fn test_create_with_directories() {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let mut writer = Writer::create(cursor).expect("Failed to create writer");

        // Add directory
        writer
            .add_directory(
                ArchivePath::new("mydir").unwrap(),
                zesven::write::EntryMeta::directory(),
            )
            .expect("Failed to add directory");

        // Add file in directory
        writer
            .add_bytes(
                ArchivePath::new("mydir/file.txt").unwrap(),
                b"file in directory",
            )
            .expect("Failed to add file");

        let result = writer.finish().expect("Failed to finish");
        assert_eq!(result.entries_written, 1);
        assert_eq!(result.directories_written, 1);
    }
}

// =============================================================================
// Test Command Tests
// =============================================================================

#[test]
fn test_test_valid_archive() {
    let archive_bytes =
        create_test_archive(&[("file1.txt", b"content1"), ("file2.txt", b"content2")]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Test failed");

    assert_eq!(result.entries_tested, 2);
    assert_eq!(result.entries_passed, 2);
    assert_eq!(result.entries_failed, 0);
    assert!(result.failures.is_empty());
}

#[test]
fn test_test_empty_archive() {
    let archive_bytes = create_test_archive(&[]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let result = archive
        .test(SelectAll, &TestOptions::default())
        .expect("Test failed");

    assert_eq!(result.entries_tested, 0);
    assert_eq!(result.entries_passed, 0);
}

#[test]
fn test_test_selective() {
    let archive_bytes = create_test_archive(&[
        ("test_this.txt", b"test content"),
        ("skip_this.txt", b"skip content"),
    ]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let result = archive
        .test(
            SelectByName::new(["test_this.txt"]),
            &TestOptions::default(),
        )
        .expect("Test failed");

    assert_eq!(result.entries_tested, 1);
    assert_eq!(result.entries_passed, 1);
}

// =============================================================================
// Info Command Tests
// =============================================================================

#[test]
fn test_info_basic() {
    let archive_bytes =
        create_test_archive(&[("file1.txt", b"hello world"), ("file2.txt", b"goodbye")]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let info = archive.info();

    assert_eq!(info.entry_count, 2);
    assert_eq!(info.total_size, 18); // "hello world" (11) + "goodbye" (7) = 18
    assert!(!info.is_solid);
}

#[test]
fn test_info_solid_archive() {
    // Create a solid archive
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let options = WriteOptions::new().solid();
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(options);

        writer
            .add_bytes(ArchivePath::new("file1.txt").unwrap(), b"content1")
            .expect("Failed to add file");
        writer
            .add_bytes(ArchivePath::new("file2.txt").unwrap(), b"content2")
            .expect("Failed to add file");

        let _result = writer.finish().expect("Failed to finish");
    }

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let info = archive.info();

    assert_eq!(info.entry_count, 2);
    assert!(info.is_solid);
}

#[test]
fn test_info_with_comment() {
    let mut archive_bytes = Vec::new();
    {
        let cursor = Cursor::new(&mut archive_bytes);
        let options = WriteOptions::new().comment("Test archive comment");
        let mut writer = Writer::create(cursor)
            .expect("Failed to create writer")
            .options(options);

        writer
            .add_bytes(ArchivePath::new("file.txt").unwrap(), b"content")
            .expect("Failed to add file");

        let _result = writer.finish().expect("Failed to finish");
    }

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let comment = archive.comment();

    assert!(comment.is_some());
    assert_eq!(comment.unwrap(), "Test archive comment");
}

// =============================================================================
// Entry Details Tests
// =============================================================================

#[test]
fn test_entry_details() {
    let archive_bytes = create_test_archive(&[("file.txt", b"test content")]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let entry = archive.entry("file.txt");

    assert!(entry.is_some());
    let entry = entry.unwrap();
    assert_eq!(entry.path.as_str(), "file.txt");
    assert_eq!(entry.size, 12); // "test content" = 12 bytes
    assert!(!entry.is_directory);
}

#[test]
fn test_entry_not_found() {
    let archive_bytes = create_test_archive(&[("exists.txt", b"hello")]);

    let archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let entry = archive.entry("nonexistent.txt");

    assert!(entry.is_none());
}

// =============================================================================
// Destination Tests
// =============================================================================

#[test]
fn test_extract_to_memory_destination() {
    use zesven::read::MemoryDestination;

    let archive_bytes =
        create_test_archive(&[("file1.txt", b"content1"), ("file2.txt", b"content2")]);

    let mut archive = Archive::open(Cursor::new(&archive_bytes)).expect("Failed to open");
    let mut dest = MemoryDestination::new();

    let result = archive
        .extract_to_destination(&mut dest)
        .expect("Failed to extract");

    assert_eq!(result.entries_extracted, 2);

    let files = dest.files();
    assert!(files.contains_key("file1.txt"));
    assert!(files.contains_key("file2.txt"));
    assert_eq!(files.get("file1.txt").unwrap().as_slice(), b"content1");
    assert_eq!(files.get("file2.txt").unwrap().as_slice(), b"content2");
}
