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

/// How much of an entry is read before it is clear which path it takes.
///
/// A read of this size, at most: an entry that fits is held whole, and one that
/// does not has its beginning here and the rest still to come.
const HEAD_CHUNK: usize = 256 * 1024;

/// Makes room for `want` more bytes without ever reserving past `ceiling`.
///
/// A `Vec` doubles when it grows, which is right when the final size is unknown
/// and wrong here, where it is known exactly: doubling at 32 MiB reserves 64,
/// and doubling at 64 reserves 128 for a buffer that stops at 64. Growth stays
/// geometric below the ceiling, so filling the buffer still costs a handful of
/// reallocations rather than one per chunk.
fn reserve_within(buffer: &mut Vec<u8>, want: usize, ceiling: usize) {
    let needed = buffer.len().saturating_add(want);
    if buffer.capacity() >= needed {
        return;
    }
    let target = buffer.capacity().saturating_mul(2).clamp(needed, ceiling);
    buffer.reserve_exact(target - buffer.len());
}

/// What an entry turned out to be, once enough of it had been read to tell.
enum EntryBytes {
    /// The whole entry. The source is at its end.
    Whole(Vec<u8>),
    /// The beginning of an entry too large to hold, and more to come.
    Streaming(Vec<u8>),
}

impl<W: Write + Seek + Send> Writer<W> {
    /// Reads enough of an entry to tell whether it is compressed as it is read.
    ///
    /// Past the streaming threshold it is, provided the options allow it: a
    /// filter, encryption or a solid block all need the compressed bytes in
    /// hand, and every codec but LZMA, LZMA2 and `Copy` hands back a buffer
    /// rather than writing through.
    ///
    /// Stops at the threshold, which is where an entry becomes one that is
    /// compressed as it is read - so reaching it is the answer, and there is
    /// no need to look one byte further to tell.
    ///
    /// The buffer never grows past the threshold either. Left to double the way
    /// a `Vec` does, a 64 MiB read reserves 128 MiB, which is a doubling of the
    /// writer's footprint that no amount of it is ever used.
    fn read_entry_head(&self, source: &mut dyn Read) -> Result<EntryBytes> {
        let limit = if super::streaming_entry::can_stream(&self.options) {
            usize::try_from(super::streaming_entry::STREAMING_THRESHOLD).unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };

        let mut head = Vec::new();
        while head.len() < limit {
            let filled = head.len();
            let want = (limit - filled).min(HEAD_CHUNK);
            reserve_within(&mut head, want, limit);
            head.resize(filled + want, 0);
            let read = super::streaming_entry::read_some(source, &mut head[filled..])?;
            head.truncate(filled + read);
            if read < want {
                return Ok(EntryBytes::Whole(head));
            }
        }

        Ok(EntryBytes::Streaming(head))
    }

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
        //
        // Which entries those are is settled by reading, not by what `meta`
        // claims. A declared length is a `stat` that may be stale, a file that
        // is still being written, or a caller's guess about a pipe - and were
        // it to decide anything, the same bytes declared differently would
        // produce different archives, and a length declared too small would
        // send a file far larger than memory down the path that buffers it.
        let added = match self.read_entry_head(source)? {
            EntryBytes::Streaming(prefix) => {
                self.compress_entry_streaming(archive_path, prefix, source, meta)
            }
            EntryBytes::Whole(data) if self.options.solid.is_solid() => {
                self.buffer_entry_solid(archive_path, data, meta)
            }
            EntryBytes::Whole(data) => self.compress_entry_non_solid(archive_path, data, meta),
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
