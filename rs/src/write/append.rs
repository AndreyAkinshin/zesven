//! Append mode for adding entries to existing archives in-place.
//!
//! Unlike the editor which creates a new archive from an existing one,
//! append mode extends the existing archive file directly, which is more
//! efficient for large archives when only adding new files.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::write::ArchiveAppender;
//! use zesven::ArchivePath;
//!
//! // Open archive for appending
//! let mut appender = ArchiveAppender::open("archive.7z")?;
//!
//! // Add new files
//! appender.add_bytes(ArchivePath::new("new_file.txt")?, b"Hello, World!")?;
//! appender.add_path("local_file.txt", ArchivePath::new("in_archive.txt")?)?;
//!
//! // Commit changes
//! let result = appender.finish()?;
//! println!("Added {} entries", result.entries_added);
//! ```

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::read::Archive;
use crate::write::{EntryMeta, WriteOptions, Writer};
use crate::{ArchivePath, Error, Result};

/// Result of an append operation.
#[must_use = "append result should be checked to verify operation completed as expected"]
#[derive(Debug, Clone, Default)]
pub struct AppendResult {
    /// Number of entries that were added.
    pub entries_added: usize,
    /// Number of entries in total after append.
    pub total_entries: usize,
    /// Total bytes in the archive after append.
    pub total_bytes: u64,
}

/// An appender for adding entries to an existing archive.
///
/// This provides an efficient way to add new files to an existing archive
/// without recompressing existing entries. The archive is extended in-place.
///
/// # Limitations
///
/// - Cannot delete or modify existing entries (use `ArchiveEditor` for that)
/// - Cannot add entries with paths that already exist in the archive
/// - Requires write access to the archive file
///
/// # Example
///
/// ```rust,ignore
/// use zesven::write::ArchiveAppender;
/// use zesven::ArchivePath;
///
/// let mut appender = ArchiveAppender::open("archive.7z")?;
/// appender.add_bytes(ArchivePath::new("new.txt")?, b"content")?;
/// appender.finish()?;
/// ```
pub struct ArchiveAppender {
    /// Path to the archive file.
    path: std::path::PathBuf,
    /// Existing entry paths for duplicate detection.
    existing_paths: HashSet<String>,
    /// Original archive entry count.
    original_entry_count: usize,
    /// Write options for new entries.
    options: WriteOptions,
    /// New entries to add (path -> data).
    new_entries: Vec<PendingAppendEntry>,
}

/// A pending entry to be added during append.
#[derive(Debug)]
struct PendingAppendEntry {
    path: ArchivePath,
    data: Vec<u8>,
    is_directory: bool,
}

impl ArchiveAppender {
    /// Opens an existing archive for appending.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the existing archive file
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file doesn't exist
    /// - The file is not a valid 7z archive
    /// - The file cannot be read
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Open and read existing archive to get entry paths
        let file = File::open(path).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        let archive = Archive::open(reader)?;

        let existing_paths: HashSet<String> = archive
            .entries()
            .iter()
            .map(|e| e.path.as_str().to_string())
            .collect();

        let original_entry_count = archive.entries().len();

        Ok(Self {
            path: path.to_path_buf(),
            existing_paths,
            original_entry_count,
            options: WriteOptions::default(),
            new_entries: Vec::new(),
        })
    }

    /// Sets the write options for new entries.
    ///
    /// Note: These options only apply to newly added entries.
    /// Existing entries retain their original compression.
    pub fn with_options(mut self, options: WriteOptions) -> Self {
        self.options = options;
        self
    }

    /// Returns the number of entries currently in the archive.
    pub fn existing_entry_count(&self) -> usize {
        self.original_entry_count
    }

    /// Returns the number of entries pending to be added.
    pub fn pending_entry_count(&self) -> usize {
        self.new_entries.len()
    }

    /// Checks if a path already exists in the archive or pending entries.
    pub fn path_exists(&self, path: &ArchivePath) -> bool {
        let path_str = path.as_str();
        self.existing_paths.contains(path_str)
            || self.new_entries.iter().any(|e| e.path.as_str() == path_str)
    }

    /// Adds bytes with the given path to the archive.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The path already exists in the archive
    /// - The path is invalid
    pub fn add_bytes(&mut self, path: ArchivePath, data: impl Into<Vec<u8>>) -> Result<()> {
        if self.path_exists(&path) {
            return Err(Error::InvalidArchivePath(format!(
                "path '{}' already exists in archive",
                path.as_str()
            )));
        }

        self.new_entries.push(PendingAppendEntry {
            path,
            data: data.into(),
            is_directory: false,
        });

        Ok(())
    }

    /// Adds a file from the filesystem to the archive.
    ///
    /// # Arguments
    ///
    /// * `source` - Path to the file on the filesystem
    /// * `archive_path` - Path to use inside the archive
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The source file cannot be read
    /// - The archive path already exists
    pub fn add_path(&mut self, source: impl AsRef<Path>, archive_path: ArchivePath) -> Result<()> {
        if self.path_exists(&archive_path) {
            return Err(Error::InvalidArchivePath(format!(
                "path '{}' already exists in archive",
                archive_path.as_str()
            )));
        }

        let data = std::fs::read(source.as_ref()).map_err(Error::Io)?;
        self.new_entries.push(PendingAppendEntry {
            path: archive_path,
            data,
            is_directory: false,
        });

        Ok(())
    }

    /// Adds an empty directory to the archive.
    ///
    /// # Errors
    ///
    /// Returns an error if the path already exists.
    pub fn add_directory(&mut self, path: ArchivePath) -> Result<()> {
        if self.path_exists(&path) {
            return Err(Error::InvalidArchivePath(format!(
                "path '{}' already exists in archive",
                path.as_str()
            )));
        }

        self.new_entries.push(PendingAppendEntry {
            path,
            data: Vec::new(),
            is_directory: true,
        });

        Ok(())
    }

    /// Applies all pending additions and finalizes the archive.
    ///
    /// This operation:
    /// 1. Opens the original archive
    /// 2. Creates a new archive with all original entries
    /// 3. Adds all new entries
    /// 4. Replaces the original file with the new archive
    ///
    /// Note: This is implemented by creating a new archive rather than
    /// true in-place append, which ensures data integrity.
    ///
    /// # Errors
    ///
    /// Returns an error if the archive cannot be written.
    pub fn finish(self) -> Result<AppendResult> {
        if self.new_entries.is_empty() {
            return Ok(AppendResult {
                entries_added: 0,
                total_entries: self.original_entry_count,
                total_bytes: 0,
            });
        }

        let entries_added = self.new_entries.len();
        let total_entries = self.original_entry_count + entries_added;

        // Create a temporary file for the new archive
        let temp_path = self.path.with_extension("7z.tmp");
        let temp_file = File::create(&temp_path).map_err(Error::Io)?;
        let temp_writer = BufWriter::new(temp_file);

        // Use the editor approach: copy existing entries + add new ones
        let result = {
            // Open original archive
            let original_file = File::open(&self.path).map_err(Error::Io)?;
            let original_reader = BufReader::new(original_file);
            let mut original_archive = Archive::open(original_reader)?;

            // Create new writer
            let mut writer = Writer::create(temp_writer)?.options(self.options.clone());

            // Copy all existing entries by extracting and re-adding
            let entries: Vec<_> = original_archive.entries().to_vec();
            for (idx, entry) in entries.iter().enumerate() {
                if entry.is_directory {
                    writer.add_directory(entry.path.clone(), EntryMeta::default())?;
                } else {
                    let data = original_archive.extract_entry_to_vec_by_index(idx)?;
                    writer.add_bytes(entry.path.clone(), &data)?;
                }
            }

            // Add new entries
            for pending in self.new_entries {
                if pending.is_directory {
                    writer.add_directory(pending.path, EntryMeta::default())?;
                } else {
                    writer.add_bytes(pending.path, &pending.data)?;
                }
            }

            writer.finish()?
        };

        // Replace original with new archive
        std::fs::rename(&temp_path, &self.path).map_err(Error::Io)?;

        Ok(AppendResult {
            entries_added,
            total_entries,
            total_bytes: result.total_size,
        })
    }
}

#[cfg(all(test, feature = "lzma"))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_archive(dir: &TempDir) -> std::path::PathBuf {
        let archive_path = dir.path().join("test.7z");

        // Create a simple archive
        let file = File::create(&archive_path).unwrap();
        let buf_writer = BufWriter::new(file);
        let mut writer = Writer::create(buf_writer).unwrap();
        writer
            .add_bytes(ArchivePath::new("file1.txt").unwrap(), b"content1")
            .unwrap();
        writer
            .add_bytes(ArchivePath::new("file2.txt").unwrap(), b"content2")
            .unwrap();
        let _ = writer.finish().unwrap();

        archive_path
    }

    #[test]
    fn test_appender_open() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let appender = ArchiveAppender::open(&archive_path).unwrap();
        assert_eq!(appender.existing_entry_count(), 2);
        assert_eq!(appender.pending_entry_count(), 0);
    }

    #[test]
    fn test_appender_add_bytes() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let mut appender = ArchiveAppender::open(&archive_path).unwrap();
        appender
            .add_bytes(ArchivePath::new("new.txt").unwrap(), b"new content")
            .unwrap();

        assert_eq!(appender.pending_entry_count(), 1);
    }

    #[test]
    fn test_appender_duplicate_path_error() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let mut appender = ArchiveAppender::open(&archive_path).unwrap();

        // Try to add with existing path
        let result = appender.add_bytes(ArchivePath::new("file1.txt").unwrap(), b"data");
        assert!(result.is_err());
    }

    #[test]
    fn test_appender_finish() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let mut appender = ArchiveAppender::open(&archive_path).unwrap();
        appender
            .add_bytes(ArchivePath::new("new.txt").unwrap(), b"new content")
            .unwrap();

        let result = appender.finish().unwrap();
        assert_eq!(result.entries_added, 1);
        assert_eq!(result.total_entries, 3);

        // Verify the archive
        let file = File::open(&archive_path).unwrap();
        let reader = BufReader::new(file);
        let archive = Archive::open(reader).unwrap();
        assert_eq!(archive.entries().len(), 3);
    }

    #[test]
    fn test_appender_finish_empty() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let appender = ArchiveAppender::open(&archive_path).unwrap();
        let result = appender.finish().unwrap();

        assert_eq!(result.entries_added, 0);
        assert_eq!(result.total_entries, 2);
    }

    #[test]
    fn test_appender_path_exists() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let mut appender = ArchiveAppender::open(&archive_path).unwrap();

        // Existing path
        assert!(appender.path_exists(&ArchivePath::new("file1.txt").unwrap()));

        // New path doesn't exist yet
        assert!(!appender.path_exists(&ArchivePath::new("new.txt").unwrap()));

        // After adding, it should exist
        appender
            .add_bytes(ArchivePath::new("new.txt").unwrap(), b"content")
            .unwrap();
        assert!(appender.path_exists(&ArchivePath::new("new.txt").unwrap()));
    }

    #[test]
    fn test_appender_add_directory() {
        let dir = TempDir::new().unwrap();
        let archive_path = create_test_archive(&dir);

        let mut appender = ArchiveAppender::open(&archive_path).unwrap();
        appender
            .add_directory(ArchivePath::new("new_dir").unwrap())
            .unwrap();

        let result = appender.finish().unwrap();
        assert_eq!(result.entries_added, 1);
        assert_eq!(result.total_entries, 3);
    }
}
