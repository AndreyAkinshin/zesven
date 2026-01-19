//! Entry input methods.
//!
//! This module provides methods for adding entries to an archive from
//! various sources: files, streams, and byte slices.

use std::fs::File;
use std::io::{BufReader, Read, Seek, Write};
use std::path::Path;

use crate::{ArchivePath, Error, Result};

use super::options::EntryMeta;
use super::{PendingEntry, Writer};

impl<W: Write + Seek> Writer<W> {
    /// Adds a file from a filesystem path.
    ///
    /// # Arguments
    ///
    /// * `disk_path` - Path to the file on disk
    /// * `archive_path` - Path within the archive
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the writer is in an invalid state.
    pub fn add_path(
        &mut self,
        disk_path: impl AsRef<Path>,
        archive_path: ArchivePath,
    ) -> Result<()> {
        self.ensure_accepting_entries()?;

        let disk_path = disk_path.as_ref();
        let meta = EntryMeta::from_path(disk_path)?;

        if meta.is_directory {
            self.add_directory(archive_path, meta)
        } else {
            let file = File::open(disk_path).map_err(Error::Io)?;
            let mut reader = BufReader::new(file);
            self.add_stream(archive_path, &mut reader, meta)
        }
    }

    /// Adds a directory entry.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `meta` - Entry metadata
    ///
    /// # Errors
    ///
    /// Returns an error if the writer is in an invalid state.
    pub fn add_directory(&mut self, archive_path: ArchivePath, meta: EntryMeta) -> Result<()> {
        self.ensure_accepting_entries()?;

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta {
                is_directory: true,
                ..meta
            },
            uncompressed_size: 0,
        };

        self.entries.push(entry);
        Ok(())
    }

    /// Adds an anti-item entry (file marked for deletion in incremental backups).
    ///
    /// Anti-items are empty entries that indicate a file or directory should
    /// be deleted when the incremental archive is applied. This is useful for
    /// incremental backup systems.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive to mark for deletion
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut writer = Writer::create_file("incremental.7z")?;
    /// writer.add_anti_item(ArchivePath::new("deleted_file.txt")?)?;
    /// writer.finish()?;
    /// ```
    pub fn add_anti_item(&mut self, archive_path: ArchivePath) -> Result<()> {
        self.ensure_accepting_entries()?;

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta::anti_item(),
            uncompressed_size: 0,
        };

        self.entries.push(entry);
        Ok(())
    }

    /// Adds an anti-item directory (directory marked for deletion).
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Directory path within the archive to mark for deletion
    pub fn add_anti_directory(&mut self, archive_path: ArchivePath) -> Result<()> {
        self.ensure_accepting_entries()?;

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta::anti_directory(),
            uncompressed_size: 0,
        };

        self.entries.push(entry);
        Ok(())
    }

    /// Adds data from a stream.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `source` - Reader providing the data
    /// * `meta` - Entry metadata
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails or if the writer is in an invalid state.
    pub fn add_stream(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        self.ensure_accepting_entries()?;

        if self.options.solid.is_solid() {
            self.buffer_entry_solid(archive_path, source, meta)
        } else {
            self.compress_entry_non_solid(archive_path, source, meta)
        }
    }

    /// Adds data from a byte slice.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `data` - The data to add
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails or if the writer is in an invalid state.
    pub fn add_bytes(&mut self, archive_path: ArchivePath, data: &[u8]) -> Result<()> {
        let meta = EntryMeta::file(data.len() as u64);
        let mut cursor = std::io::Cursor::new(data);
        self.add_stream(archive_path, &mut cursor, meta)
    }
}
