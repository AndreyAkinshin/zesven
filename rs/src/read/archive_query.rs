//! Archive query methods.
//!
//! This module provides methods for querying archive information, entries,
//! and metadata without extraction.

use std::io::{Read, Seek};
use std::path::PathBuf;

use super::{Archive, ArchiveInfo, Entry};

impl<R: Read + Seek> Archive<R> {
    /// Returns information about the archive.
    pub fn info(&self) -> &ArchiveInfo {
        &self.info
    }

    /// Returns all entries in the archive.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Returns the archive comment, if any.
    ///
    /// Archive comments are optional metadata stored in the archive header.
    /// They are typically used for descriptions or notes about the archive contents.
    pub fn comment(&self) -> Option<&str> {
        self.header
            .files_info
            .as_ref()
            .and_then(|fi| fi.comment.as_deref())
    }

    /// Returns the number of entries in the archive.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the archive has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finds an entry by path.
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path.as_str() == path)
    }

    /// Returns whether this is a multi-volume archive.
    pub fn is_multivolume(&self) -> bool {
        self.volume_info.is_some()
    }

    /// Returns the number of volumes in a multi-volume archive.
    ///
    /// Returns `None` for single-file archives.
    pub fn volume_count(&self) -> Option<u32> {
        self.volume_info.as_ref().map(|v| v.count)
    }

    /// Returns the paths to all volume files.
    ///
    /// Returns `None` for single-file archives.
    pub fn volume_paths(&self) -> Option<&[PathBuf]> {
        self.volume_info.as_ref().map(|v| v.paths.as_slice())
    }
}
