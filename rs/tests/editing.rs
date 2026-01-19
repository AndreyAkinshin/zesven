//! Integration tests for archive editing operations.
//!
//! These tests verify that the editing API correctly:
//! - Removes entries from archives
//! - Renames entries within archives
//! - Updates entry content
//! - Adds new entries to existing archives
//! - Returns correct error types for invalid operations

#![cfg(feature = "lzma2")]

mod common;

use std::io::Cursor;
use zesven::edit::ArchiveEditor;
use zesven::read::Archive;
use zesven::{ArchivePath, Error};

use common::create_archive;

/// Reads an archive and returns a map of path -> content.
fn read_archive_contents(archive_bytes: &[u8]) -> zesven::Result<Vec<(String, Vec<u8>)>> {
    let mut archive = Archive::open(Cursor::new(archive_bytes))?;
    let mut contents = Vec::new();

    // Collect entry info first to avoid borrow issues
    let entry_infos: Vec<_> = archive
        .entries()
        .iter()
        .filter(|e| !e.is_directory)
        .map(|e| e.path.as_str().to_string())
        .collect();

    for path in entry_infos {
        let data = archive.extract_to_vec(&path)?;
        contents.push((path, data));
    }

    Ok(contents)
}

// ============================================================================
// Delete operation tests
// ============================================================================

#[test]
fn test_editor_delete_entry() {
    let entries = [
        ("keep.txt", b"Keep this" as &[u8]),
        ("delete.txt", b"Delete this"),
        ("also_keep.txt", b"Also keep"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    editor.delete("delete.txt").unwrap();

    assert_eq!(editor.pending_operations(), 1);

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    assert_eq!(result.entries_kept, 2);
    assert_eq!(result.entries_deleted, 1);
    assert_eq!(result.total_entries(), 2);

    // Verify the archive contents
    let contents = read_archive_contents(output.get_ref()).unwrap();
    assert_eq!(contents.len(), 2);

    let paths: Vec<_> = contents.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"keep.txt"));
    assert!(paths.contains(&"also_keep.txt"));
    assert!(!paths.contains(&"delete.txt"));
}

#[test]
fn test_editor_delete_nonexistent_returns_error() {
    let entries = [("existing.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let result = editor.delete("nonexistent.txt");

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::EntryNotFound { path } => {
            assert_eq!(path, "nonexistent.txt");
        }
        e => panic!("Expected EntryNotFound, got: {:?}", e),
    }
}

// ============================================================================
// Rename operation tests
// ============================================================================

#[test]
fn test_editor_rename_entry() {
    let entries = [
        ("old_name.txt", b"Content to rename" as &[u8]),
        ("other.txt", b"Other content"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    editor.rename("old_name.txt", "new_name.txt").unwrap();

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    assert_eq!(result.entries_renamed, 1);
    assert_eq!(result.entries_kept, 1);
    assert_eq!(result.total_entries(), 2);

    // Verify the archive contents
    let contents = read_archive_contents(output.get_ref()).unwrap();
    let paths: Vec<_> = contents.iter().map(|(p, _)| p.as_str()).collect();

    assert!(paths.contains(&"new_name.txt"));
    assert!(paths.contains(&"other.txt"));
    assert!(!paths.contains(&"old_name.txt"));

    // Verify content is preserved
    let renamed_content = contents.iter().find(|(p, _)| p == "new_name.txt").unwrap();
    assert_eq!(renamed_content.1, b"Content to rename");
}

#[test]
fn test_editor_rename_nonexistent_returns_error() {
    let entries = [("existing.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let result = editor.rename("nonexistent.txt", "new.txt");

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::EntryNotFound { path } => {
            assert_eq!(path, "nonexistent.txt");
        }
        e => panic!("Expected EntryNotFound, got: {:?}", e),
    }
}

#[test]
fn test_editor_rename_to_existing_returns_error() {
    let entries = [
        ("source.txt", b"Source content" as &[u8]),
        ("target.txt", b"Target content"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let result = editor.rename("source.txt", "target.txt");

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::EntryExists { path } => {
            assert_eq!(path, "target.txt");
        }
        e => panic!("Expected EntryExists, got: {:?}", e),
    }
}

// ============================================================================
// Update operation tests
// ============================================================================

#[test]
fn test_editor_update_entry() {
    let entries = [
        ("update_me.txt", b"Original content" as &[u8]),
        ("untouched.txt", b"Untouched content"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    editor
        .update("update_me.txt", b"New content here!".to_vec())
        .unwrap();

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    assert_eq!(result.entries_updated, 1);
    assert_eq!(result.entries_kept, 1);
    assert_eq!(result.total_entries(), 2);

    // Verify the archive contents
    let contents = read_archive_contents(output.get_ref()).unwrap();

    let updated = contents.iter().find(|(p, _)| p == "update_me.txt").unwrap();
    assert_eq!(updated.1, b"New content here!");

    let untouched = contents.iter().find(|(p, _)| p == "untouched.txt").unwrap();
    assert_eq!(untouched.1, b"Untouched content");
}

#[test]
fn test_editor_update_nonexistent_returns_error() {
    let entries = [("existing.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let result = editor.update("nonexistent.txt", b"new data".to_vec());

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::EntryNotFound { path } => {
            assert_eq!(path, "nonexistent.txt");
        }
        e => panic!("Expected EntryNotFound, got: {:?}", e),
    }
}

// ============================================================================
// Add operation tests
// ============================================================================

#[test]
fn test_editor_add_entry() {
    let entries = [("existing.txt", b"Existing content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let new_path = ArchivePath::new("new_file.txt").unwrap();
    editor.add(new_path, b"Brand new content".to_vec()).unwrap();

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    assert_eq!(result.entries_kept, 1);
    assert_eq!(result.entries_added, 1);
    assert_eq!(result.total_entries(), 2);

    // Verify the archive contents
    let contents = read_archive_contents(output.get_ref()).unwrap();
    assert_eq!(contents.len(), 2);

    let new_content = contents.iter().find(|(p, _)| p == "new_file.txt").unwrap();
    assert_eq!(new_content.1, b"Brand new content");
}

#[test]
fn test_editor_add_duplicate_returns_error() {
    let entries = [("existing.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);
    let path = ArchivePath::new("existing.txt").unwrap();
    let result = editor.add(path, b"duplicate".to_vec());

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::EntryExists { path } => {
            assert_eq!(path, "existing.txt");
        }
        e => panic!("Expected EntryExists, got: {:?}", e),
    }
}

// ============================================================================
// Combined operations tests
// ============================================================================

#[test]
fn test_editor_multiple_operations() {
    let entries = [
        ("keep.txt", b"Keep this" as &[u8]),
        ("delete.txt", b"Delete this"),
        ("rename.txt", b"Rename this"),
        ("update.txt", b"Update this"),
    ];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);

    editor.delete("delete.txt").unwrap();
    editor.rename("rename.txt", "renamed.txt").unwrap();
    editor
        .update("update.txt", b"Updated content".to_vec())
        .unwrap();
    let new_path = ArchivePath::new("added.txt").unwrap();
    editor.add(new_path, b"Added content".to_vec()).unwrap();

    assert_eq!(editor.pending_operations(), 4);

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    assert_eq!(result.entries_kept, 1);
    assert_eq!(result.entries_deleted, 1);
    assert_eq!(result.entries_renamed, 1);
    assert_eq!(result.entries_updated, 1);
    assert_eq!(result.entries_added, 1);
    assert_eq!(result.total_entries(), 4);

    // Verify final archive state
    let contents = read_archive_contents(output.get_ref()).unwrap();
    let paths: Vec<_> = contents.iter().map(|(p, _)| p.as_str()).collect();

    assert!(paths.contains(&"keep.txt"));
    assert!(paths.contains(&"renamed.txt"));
    assert!(paths.contains(&"update.txt"));
    assert!(paths.contains(&"added.txt"));
    assert!(!paths.contains(&"delete.txt"));
    assert!(!paths.contains(&"rename.txt"));
}

// ============================================================================
// Clear operations test
// ============================================================================

#[test]
fn test_editor_clear_operations() {
    let entries = [("file.txt", b"content" as &[u8])];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let mut editor = ArchiveEditor::new(archive);

    editor.delete("file.txt").unwrap();
    assert!(editor.has_pending_operations());
    assert_eq!(editor.pending_operations(), 1);

    editor.clear_operations();
    assert!(!editor.has_pending_operations());
    assert_eq!(editor.pending_operations(), 0);
}

// ============================================================================
// Encrypted Archive Editing Tests
// ============================================================================

/// Tests editing operations on header-encrypted archives.
///
/// This module verifies that the ArchiveEditor correctly handles encrypted
/// archives opened with a password. The expected behavior is:
/// - Archive can be opened with correct password
/// - Edit operations can be performed
/// - Output archive preserves encryption (requires password option on writer)
#[cfg(feature = "aes")]
mod encrypted_editing {
    use super::*;
    use zesven::WriteOptions;

    /// Helper to create a header-encrypted archive.
    fn create_encrypted_archive(password: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive_bytes = Vec::new();
        {
            use zesven::Writer;
            let cursor = Cursor::new(&mut archive_bytes);
            let mut writer = Writer::create(cursor)
                .expect("Failed to create writer")
                .options(WriteOptions::new().password(password).encrypt_header(true));

            for (name, data) in entries {
                let path = ArchivePath::new(name).expect("Invalid path");
                writer.add_bytes(path, data).expect("Failed to add entry");
            }
            let _ = writer.finish().expect("Failed to finish archive");
        }
        archive_bytes
    }

    #[test]
    fn test_editor_delete_from_encrypted_archive() {
        let password = "test_password";
        let entries = [
            ("keep.txt", b"Keep this content" as &[u8]),
            ("delete.txt", b"Delete this content"),
        ];

        let archive_bytes = create_encrypted_archive(password, &entries);

        // Open with password and create editor
        let archive = Archive::open_with_password(Cursor::new(archive_bytes), password)
            .expect("Should open with correct password");

        let mut editor = ArchiveEditor::new(archive);
        editor.delete("delete.txt").expect("Delete should queue");

        // Apply changes
        let mut output = Cursor::new(Vec::new());
        let result = editor.apply(&mut output).expect("Apply should succeed");

        assert_eq!(result.entries_deleted, 1);
        assert_eq!(result.entries_kept, 1);

        // Verify the output archive can be read (note: output is NOT encrypted
        // unless writer options specify encryption)
        let contents = read_archive_contents(output.get_ref()).expect("Should read edited archive");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].0, "keep.txt");
    }

    #[test]
    fn test_editor_rename_in_encrypted_archive() {
        let password = "rename_test";
        let entries = [("original.txt", b"Content to rename" as &[u8])];

        let archive_bytes = create_encrypted_archive(password, &entries);

        let archive = Archive::open_with_password(Cursor::new(archive_bytes), password)
            .expect("Should open with correct password");

        let mut editor = ArchiveEditor::new(archive);
        editor
            .rename("original.txt", "renamed.txt")
            .expect("Rename should queue");

        let mut output = Cursor::new(Vec::new());
        let result = editor.apply(&mut output).expect("Apply should succeed");

        assert_eq!(result.entries_renamed, 1);

        let contents = read_archive_contents(output.get_ref()).expect("Should read edited archive");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].0, "renamed.txt");
        assert_eq!(contents[0].1, b"Content to rename");
    }

    #[test]
    fn test_editor_update_in_encrypted_archive() {
        let password = "update_test";
        let entries = [("file.txt", b"Original content" as &[u8])];

        let archive_bytes = create_encrypted_archive(password, &entries);

        let archive = Archive::open_with_password(Cursor::new(archive_bytes), password)
            .expect("Should open with correct password");

        let mut editor = ArchiveEditor::new(archive);
        editor
            .update("file.txt", b"Updated content".to_vec())
            .expect("Update should queue");

        let mut output = Cursor::new(Vec::new());
        let result = editor.apply(&mut output).expect("Apply should succeed");

        assert_eq!(result.entries_updated, 1);

        let contents = read_archive_contents(output.get_ref()).expect("Should read edited archive");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].1, b"Updated content");
    }
}

// ============================================================================
// Writer Error Handling Tests
// ============================================================================

/// Tests that ArchiveEditor::apply() propagates writer errors correctly.
///
/// When the output writer fails mid-operation, apply() should return an error
/// rather than silently producing corrupt output. This tests I/O error handling
/// during the archive rewriting process.
#[test]
fn test_editor_apply_propagates_writer_error() {
    use std::io::{self, Write};

    /// A writer that fails after writing a specified number of bytes.
    struct FailingWriter {
        bytes_until_failure: usize,
        bytes_written: usize,
    }

    impl FailingWriter {
        fn new(bytes_until_failure: usize) -> Self {
            Self {
                bytes_until_failure,
                bytes_written: 0,
            }
        }
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let remaining = self.bytes_until_failure.saturating_sub(self.bytes_written);
            if remaining == 0 {
                return Err(io::Error::other("simulated write failure"));
            }
            let to_write = buf.len().min(remaining);
            self.bytes_written += to_write;
            Ok(to_write)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.bytes_written >= self.bytes_until_failure {
                Err(io::Error::other("simulated flush failure"))
            } else {
                Ok(())
            }
        }
    }

    impl io::Seek for FailingWriter {
        fn seek(&mut self, _pos: io::SeekFrom) -> io::Result<u64> {
            // Minimal seek implementation - return current position
            Ok(self.bytes_written as u64)
        }
    }

    // Create a valid archive to edit
    let entries = [
        ("file1.txt", b"Content for file 1" as &[u8]),
        ("file2.txt", b"Content for file 2"),
    ];
    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let editor = ArchiveEditor::new(archive);

    // Try to apply with a writer that fails after 50 bytes
    // This should be enough to start writing but fail mid-stream
    let mut failing_writer = FailingWriter::new(50);
    let result = editor.apply(&mut failing_writer);

    // The apply operation should return an error, not silently fail
    assert!(
        result.is_err(),
        "apply() should return an error when the writer fails"
    );

    // Verify it's an I/O error
    match result.unwrap_err() {
        Error::Io(io_err) => {
            assert!(
                io_err.to_string().contains("simulated"),
                "Error should contain our simulated error message: {}",
                io_err
            );
        }
        other => panic!("Expected Io error, got: {:?}", other),
    }
}

// ============================================================================
// EditResult tests
// ============================================================================

#[test]
fn test_edit_result_compression_ratio() {
    let entries = [(
        "file.txt",
        b"This is some compressible content that should compress well. " as &[u8],
    )];

    let archive_bytes = create_archive(&entries).unwrap();
    let archive = Archive::open(Cursor::new(archive_bytes)).unwrap();

    let editor = ArchiveEditor::new(archive);

    let mut output = Cursor::new(Vec::new());
    let result = editor.apply(&mut output).unwrap();

    // Compression ratio should be between 0 and 1 (compressed/total)
    let ratio = result.compression_ratio();
    assert!(ratio > 0.0);
    assert!(ratio <= 1.0);
}
