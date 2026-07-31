//! Adding entries to an existing archive.
//!
//! The archive is rebuilt beside the old one and moved onto it at the end,
//! rather than extended where it is: a 7z archive keeps its index at the end,
//! so adding to it means writing that index again over entries it describes.
//! A failure at any point therefore leaves the original exactly as it was.
//!
//! It costs memory in proportion to the largest entry involved, not to the
//! archive: every existing entry is decompressed into memory and recompressed
//! on the way into the new file, and every entry being added is held until
//! [`ArchiveAppender::finish`] runs. Appending to an archive that holds a
//! multi-gigabyte entry needs room for that entry. The write-through path that
//! bounds this for [`crate::write::Writer`] needs a reader that can hand an
//! entry over a piece at a time, which this crate does not expose.
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
/// Adds new files to an existing archive.
///
/// Every existing entry is decompressed and recompressed on the way into the
/// new archive, which is then moved onto the old one - the archive is not
/// extended in place, and existing entries do not keep their original bytes.
/// It costs memory in proportion to the largest entry involved.
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

    /// Sets the write options the archive is rebuilt with.
    ///
    /// They apply to every entry, not only the new ones: appending decompresses
    /// what is already there and compresses it again into a new archive, so an
    /// entry written at level 1 last week comes out at whatever level is set
    /// here.
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

        // The new archive is built beside the old one and moved onto it at the
        // end. Named for this process and this call, and created rather than
        // opened: a fixed name meant two appends to the same archive built in
        // one file, and opening it meant truncating whatever was there.
        let temp_path = {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            self.path.with_extension(format!(
                "7z.tmp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ))
        };
        let temp_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(Error::Io)?;

        // The mode goes on now rather than at the end. Rebuilding a
        // multi-gigabyte archive takes minutes, and for all of them the
        // contents of an archive kept at 0600 would be sitting in a file
        // anyone on the machine could read.
        if let Ok(existing) = std::fs::metadata(&self.path) {
            if let Err(e) = std::fs::set_permissions(&temp_path, existing.permissions()) {
                let _ = std::fs::remove_file(&temp_path);
                return Err(Error::Io(e));
            }
        }
        let temp_writer = BufWriter::new(temp_file);

        // Use the editor approach: copy existing entries + add new ones.
        //
        // Written into a closure so that every failure below leaves through one
        // place, where the half-built file is removed. A `?` straight out of
        // here left it on disk next to the archive, named for it, holding
        // however much had been written when whatever went wrong went wrong.
        let build = || {
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

            writer.finish()
        };

        let result = match build() {
            Ok(result) => result,
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(e);
            }
        };

        // The mode was put on when the file was created, so the rename is all
        // that is left: it replaces the inode, and the archive that appears at
        // that path is already the one the caller had.
        if let Err(e) = std::fs::rename(&temp_path, &self.path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Error::Io(e));
        }

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
