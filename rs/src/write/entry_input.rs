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

impl<W: Write + Seek + Send> Writer<W> {
    /// Checks that entries arrive in sorted order, when that was asked for.
    ///
    /// Recorded here rather than checked at the end, so the caller learns which
    /// entry broke the order while it is still theirs to fix.
    fn check_order(&self, archive_path: &ArchivePath) -> Result<()> {
        if !self.options.deterministic {
            return Ok(());
        }

        let path = archive_path.as_str();
        if let Some(previous) = &self.last_path {
            if path < previous.as_str() {
                return Err(Error::InvalidArchivePath(format!(
                    "deterministic mode requires entries in sorted order, \
                     but '{path}' was added after '{previous}'"
                )));
            }
        }

        Ok(())
    }

    /// Records a path as added, once the entry is really in.
    ///
    /// Kept separate from the check so that an add which fails - a missing
    /// file, a rejected option - does not advance the position and reject the
    /// perfectly good entry that follows it.
    ///
    /// Recorded whether or not the setting is on, since the options can be
    /// replaced between entries: skipping this while it was off left the check
    /// comparing against whatever was last added before that, so turning it on
    /// mid-archive accepted an entry that broke the order it was meant to
    /// enforce.
    fn record_order(&mut self, archive_path: &ArchivePath) {
        match &mut self.last_path {
            Some(last) => {
                last.clear();
                last.push_str(archive_path.as_str());
            }
            None => self.last_path = Some(archive_path.as_str().to_string()),
        }
    }

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
        self.check_order(&archive_path)?;

        let disk_path = disk_path.as_ref();
        let meta = EntryMeta::from_path(disk_path)?;

        if meta.is_directory {
            return self.add_directory(archive_path, meta);
        }

        let file = File::open(disk_path).map_err(Error::Io)?;
        let mut reader = BufReader::new(file);
        self.add_stream(archive_path, &mut reader, meta)
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
        // Settled first, so anything waiting under the previous password is
        // written - and the archive keyed - before this entry is judged.
        self.settle_stale_buffers()?;
        #[cfg(feature = "aes")]
        {
            let options = self.options.clone();
            self.check_password(&options)?;
        }
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta {
                is_directory: true,
                ..meta
            },
            uncompressed_size: 0,
        };

        // Entries waiting in the batch were added before this one and have to
        // reach the entry list first, or the archive lists them out of order.
        self.flush_buffered_entries()?;

        self.entries.push(entry);
        self.record_order(&recorded);
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
        // Settled first, so anything waiting under the previous password is
        // written - and the archive keyed - before this entry is judged.
        self.settle_stale_buffers()?;
        #[cfg(feature = "aes")]
        {
            let options = self.options.clone();
            self.check_password(&options)?;
        }
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta::anti_item(),
            uncompressed_size: 0,
        };

        // Entries waiting in the batch were added before this one and have to
        // reach the entry list first, or the archive lists them out of order.
        self.flush_buffered_entries()?;

        self.entries.push(entry);
        self.record_order(&recorded);
        Ok(())
    }

    /// Adds an anti-item directory (directory marked for deletion).
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Directory path within the archive to mark for deletion
    pub fn add_anti_directory(&mut self, archive_path: ArchivePath) -> Result<()> {
        self.ensure_accepting_entries()?;
        // Settled first, so anything waiting under the previous password is
        // written - and the archive keyed - before this entry is judged.
        self.settle_stale_buffers()?;
        #[cfg(feature = "aes")]
        {
            let options = self.options.clone();
            self.check_password(&options)?;
        }
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta::anti_directory(),
            uncompressed_size: 0,
        };

        // Entries waiting in the batch were added before this one and have to
        // reach the entry list first, or the archive lists them out of order.
        self.flush_buffered_entries()?;

        self.entries.push(entry);
        self.record_order(&recorded);
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
        // Anything buffered under options that have since been replaced is
        // written out with those options before this entry is accepted.
        // Settled first, so anything waiting under the previous password is
        // written - and the archive keyed - before this entry is judged.
        self.settle_stale_buffers()?;
        #[cfg(feature = "aes")]
        {
            let options = self.options.clone();
            self.check_password(&options)?;
        }
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        // A large entry is compressed straight into the sink rather than read
        // into memory first, so that archiving a file does not require as much
        // memory as the file is long.
        let size = meta.size;
        let added = if super::streaming_entry::can_stream(&self.options, size) {
            self.compress_entry_streaming(archive_path, source, meta, size)
        } else if self.options.solid.is_solid() {
            self.buffer_entry_solid(archive_path, source, meta)
        } else {
            self.compress_entry_non_solid(archive_path, source, meta)
        };

        if added.is_ok() {
            self.record_order(&recorded);
        }
        added
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
