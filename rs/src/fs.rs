//! Filesystem-style API for archive access.
//!
//! This module provides a filesystem-like interface for accessing 7z archive
//! contents, inspired by Go's `fs.FS` interface. It enables convenient
//! directory traversal, file lookup, and metadata access.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::fs::ArchiveFS;
//!
//! let archive = ArchiveFS::open("archive.7z")?;
//!
//! // List root directory
//! for entry in archive.read_dir("/")? {
//!     println!("{}", entry.name());
//! }
//!
//! // Get file metadata
//! if let Some(meta) = archive.metadata("docs/readme.txt") {
//!     println!("Size: {} bytes", meta.size());
//! }
//!
//! // Read a file
//! let contents = archive.read_to_vec("config.json")?;
//! ```
//!
//! # Path Handling
//!
//! Paths use forward slashes (`/`) as separators, regardless of the platform.
//! Both absolute paths (`/file.txt`) and relative paths (`file.txt`) are
//! supported and treated equivalently.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use crate::read::Entry;
use crate::streaming::{StreamingArchive, StreamingConfig};
use crate::{Error, Result};

#[cfg(feature = "aes")]
use crate::Password;

/// Filesystem-like interface for 7z archive access.
///
/// This struct provides a convenient API for navigating and accessing archive
/// contents using familiar filesystem operations like `read_dir`, `metadata`,
/// and file reading.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::fs::ArchiveFS;
///
/// // Open archive
/// let archive = ArchiveFS::open("data.7z")?;
///
/// // Check if path exists
/// if archive.exists("important/data.bin") {
///     let meta = archive.metadata("important/data.bin").unwrap();
///     println!("File size: {} bytes", meta.size());
/// }
///
/// // List directory contents
/// for entry in archive.read_dir("important")? {
///     println!("  {}", entry.name());
/// }
/// ```
pub struct ArchiveFS<R> {
    /// Internal streaming archive
    archive: StreamingArchive<R>,
    /// Path index for fast lookups
    path_index: HashMap<String, usize>,
    /// Directory tree (path -> list of child names)
    dir_tree: HashMap<String, Vec<String>>,
}

impl ArchiveFS<BufReader<File>> {
    /// Opens an archive from a file path.
    #[cfg(feature = "aes")]
    pub fn open(path: impl AsRef<Path>, password: impl Into<Password>) -> Result<Self> {
        let archive =
            StreamingArchive::open_path_with_config(path, password, StreamingConfig::default())?;
        Self::from_archive(archive)
    }

    /// Opens an archive from a file path (without password).
    #[cfg(not(feature = "aes"))]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let archive = StreamingArchive::open_path_with_config(path, StreamingConfig::default())?;
        Self::from_archive(archive)
    }
}

impl<R: Read + Seek + Send> ArchiveFS<R> {
    /// Creates an ArchiveFS from a reader.
    #[cfg(feature = "aes")]
    pub fn from_reader(reader: R, password: impl Into<Password>) -> Result<Self> {
        let archive = StreamingArchive::open(reader, password)?;
        Self::from_archive(archive)
    }

    /// Creates an ArchiveFS from a reader (without password).
    #[cfg(not(feature = "aes"))]
    pub fn from_reader(reader: R) -> Result<Self> {
        let archive = StreamingArchive::open(reader)?;
        Self::from_archive(archive)
    }

    /// Creates an ArchiveFS from an existing StreamingArchive.
    pub fn from_archive(archive: StreamingArchive<R>) -> Result<Self> {
        let mut path_index = HashMap::new();
        let mut dir_tree: HashMap<String, Vec<String>> = HashMap::new();

        // Build indexes
        for (idx, entry) in archive.entries_list().iter().enumerate() {
            let path = normalize_path(entry.path.as_str());
            path_index.insert(path.clone(), idx);

            // Build directory tree
            let parent = parent_path(&path);
            let name = file_name(&path);

            dir_tree.entry(parent).or_default().push(name);
        }

        // Ensure root directory exists
        dir_tree.entry(String::new()).or_default();

        Ok(Self {
            archive,
            path_index,
            dir_tree,
        })
    }

    /// Returns the number of entries in the archive.
    pub fn len(&self) -> usize {
        self.archive.len()
    }

    /// Returns true if the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.archive.is_empty()
    }

    /// Returns true if the given path exists in the archive.
    pub fn exists(&self, path: impl AsRef<str>) -> bool {
        let path = normalize_path(path.as_ref());
        self.path_index.contains_key(&path) || self.dir_tree.contains_key(&path)
    }

    /// Returns true if the given path is a directory.
    pub fn is_dir(&self, path: impl AsRef<str>) -> bool {
        let path = normalize_path(path.as_ref());

        // Check if it's an explicit directory entry
        if let Some(&idx) = self.path_index.get(&path) {
            if self.archive.entries_list()[idx].is_directory {
                return true;
            }
        }

        // Check if it's an implicit directory (has children)
        self.dir_tree.contains_key(&path)
    }

    /// Returns true if the given path is a regular file.
    pub fn is_file(&self, path: impl AsRef<str>) -> bool {
        let path = normalize_path(path.as_ref());
        if let Some(&idx) = self.path_index.get(&path) {
            !self.archive.entries_list()[idx].is_directory
        } else {
            false
        }
    }

    /// Returns metadata for the given path.
    pub fn metadata(&self, path: impl AsRef<str>) -> Option<FileMetadata<'_>> {
        let path = normalize_path(path.as_ref());
        let &idx = self.path_index.get(&path)?;
        let entry = &self.archive.entries_list()[idx];
        Some(FileMetadata { entry })
    }

    /// Returns the entry at the given path.
    pub fn entry(&self, path: impl AsRef<str>) -> Option<&Entry> {
        let path = normalize_path(path.as_ref());
        let &idx = self.path_index.get(&path)?;
        Some(&self.archive.entries_list()[idx])
    }

    /// Reads directory contents at the given path.
    ///
    /// Returns an iterator over directory entries. Use an empty string or "/"
    /// for the root directory.
    pub fn read_dir(&self, path: impl AsRef<str>) -> Result<impl Iterator<Item = DirEntry<'_>>> {
        let path = normalize_path(path.as_ref());

        let children = self
            .dir_tree
            .get(&path)
            .ok_or_else(|| Error::InvalidFormat(format!("not a directory: {}", path)))?;

        let entries: Vec<DirEntry<'_>> = children
            .iter()
            .map(|name| {
                let full_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", path, name)
                };
                let entry = self
                    .path_index
                    .get(&full_path)
                    .map(|&idx| &self.archive.entries_list()[idx]);
                let is_dir = self.is_dir(&full_path);
                DirEntry {
                    name: name.clone(),
                    entry,
                    is_dir,
                }
            })
            .collect();

        Ok(entries.into_iter())
    }

    /// Walks the archive tree, starting from the given path.
    ///
    /// Yields all entries under the given path, including the path itself
    /// if it's a file or explicit directory.
    pub fn walk(&self, path: impl AsRef<str>) -> impl Iterator<Item = &Entry> {
        let path = normalize_path(path.as_ref());
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path)
        };

        self.archive.entries_list().iter().filter(move |entry| {
            let entry_path = normalize_path(entry.path.as_str());
            entry_path == path || entry_path.starts_with(&prefix)
        })
    }

    /// Returns all file entries (non-directories) in the archive.
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.archive
            .entries_list()
            .iter()
            .filter(|e| !e.is_directory)
    }

    /// Returns all directory entries in the archive.
    pub fn directories(&self) -> impl Iterator<Item = &Entry> {
        self.archive
            .entries_list()
            .iter()
            .filter(|e| e.is_directory)
    }

    /// Finds entries matching a glob pattern.
    ///
    /// Supports `*` (any characters except `/`) and `**` (any characters including `/`).
    pub fn glob(&self, pattern: impl AsRef<str>) -> impl Iterator<Item = &Entry> {
        let pattern = pattern.as_ref().to_string();
        self.archive
            .entries_list()
            .iter()
            .filter(move |entry| glob_match(&pattern, entry.path.as_str()))
    }

    /// Returns a reference to the underlying archive.
    pub fn archive(&self) -> &StreamingArchive<R> {
        &self.archive
    }

    /// Returns a mutable reference to the underlying archive.
    pub fn archive_mut(&mut self) -> &mut StreamingArchive<R> {
        &mut self.archive
    }

    /// Consumes self and returns the underlying archive.
    pub fn into_inner(self) -> StreamingArchive<R> {
        self.archive
    }
}

/// Metadata for a file or directory entry.
#[derive(Debug)]
pub struct FileMetadata<'a> {
    entry: &'a Entry,
}

impl<'a> FileMetadata<'a> {
    /// Returns the entry path.
    pub fn path(&self) -> &str {
        self.entry.path.as_str()
    }

    /// Returns true if this is a directory.
    pub fn is_dir(&self) -> bool {
        self.entry.is_directory
    }

    /// Returns true if this is a regular file.
    pub fn is_file(&self) -> bool {
        !self.entry.is_directory
    }

    /// Returns the uncompressed size in bytes.
    pub fn size(&self) -> u64 {
        self.entry.size
    }

    /// Returns the modification time as Windows FILETIME, if available.
    ///
    /// Use [`crate::timestamp::Timestamp::from_filetime`] to convert to a more
    /// convenient type.
    pub fn modified(&self) -> Option<u64> {
        self.entry.modification_time
    }

    /// Returns the creation time as Windows FILETIME, if available.
    pub fn created(&self) -> Option<u64> {
        self.entry.creation_time
    }

    /// Returns the access time as Windows FILETIME, if available.
    pub fn accessed(&self) -> Option<u64> {
        self.entry.access_time
    }

    /// Returns true if the entry is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.entry.is_encrypted
    }

    /// Returns the CRC-32 checksum, if available.
    pub fn crc32(&self) -> Option<u32> {
        self.entry.crc32
    }

    /// Returns the underlying entry.
    pub fn entry(&self) -> &Entry {
        self.entry
    }
}

/// A directory entry returned by `read_dir`.
#[derive(Debug)]
pub struct DirEntry<'a> {
    name: String,
    entry: Option<&'a Entry>,
    is_dir: bool,
}

impl<'a> DirEntry<'a> {
    /// Returns the file/directory name (not the full path).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns true if this is a directory.
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns true if this is a file.
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }

    /// Returns the underlying entry, if this is an explicit entry.
    ///
    /// Returns `None` for implicit directories (directories that exist
    /// only because they contain files, but have no explicit entry).
    pub fn entry(&self) -> Option<&Entry> {
        self.entry
    }

    /// Returns the file size (0 for directories).
    pub fn size(&self) -> u64 {
        self.entry.map(|e| e.size).unwrap_or(0)
    }
}

/// Normalizes a path for consistent lookups.
fn normalize_path(path: &str) -> String {
    let path = path.trim_start_matches('/').trim_end_matches('/');
    // Replace backslashes with forward slashes
    path.replace('\\', "/")
}

/// Returns the parent directory of a path.
fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(idx) => path[..idx].to_string(),
        None => String::new(), // Root directory
    }
}

/// Returns the file name component of a path.
fn file_name(path: &str) -> String {
    match path.rfind('/') {
        Some(idx) => path[idx + 1..].to_string(),
        None => path.to_string(),
    }
}

/// Simple glob pattern matching.
///
/// Supports:
/// - `*` matches any characters except `/`
/// - `**` matches any characters including `/`
/// - Literal characters match themselves
fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    let path = path.trim_start_matches('/');

    // Handle common patterns efficiently

    // Pattern: ** alone matches everything
    if pattern == "**" {
        return true;
    }

    // Pattern: prefix/** (matches anything under prefix)
    if pattern.ends_with("**") && !pattern[..pattern.len() - 2].contains('*') {
        let prefix = pattern[..pattern.len() - 2].trim_end_matches('/');
        if prefix.is_empty() {
            return true; // ** matches everything
        }
        if path == prefix {
            return true;
        }
        let prefix_with_slash = format!("{}/", prefix);
        return path.starts_with(&prefix_with_slash);
    }

    // Pattern: **/*.ext or **/name (matches paths ending with pattern after **/)
    if let Some(suffix_pattern) = pattern.strip_prefix("**/") {
        // Check if the suffix has wildcards
        if suffix_pattern.contains('*') {
            // Handle **/pattern with wildcards using recursive logic
            // For **/*.ext, check any directory level
            if let Some(star_pos) = suffix_pattern.find('*') {
                let pre = &suffix_pattern[..star_pos];
                let post = &suffix_pattern[star_pos + 1..];
                // Match at any level
                for (i, _) in path.char_indices() {
                    let candidate = &path[i..];
                    if candidate.starts_with(pre) && candidate.ends_with(post) {
                        let middle = &candidate[pre.len()..candidate.len() - post.len()];
                        if !middle.contains('/') {
                            // Make sure we're at a path boundary
                            if i == 0 || path.as_bytes()[i - 1] == b'/' {
                                return true;
                            }
                        }
                    }
                }
                return false;
            }
        } else {
            // No wildcards in suffix, exact filename match at any level
            if path == suffix_pattern {
                return true;
            }
            return path.ends_with(&format!("/{}", suffix_pattern));
        }
    }

    // Pattern: *.ext (matches files with extension, no subdirs)
    if pattern.starts_with('*')
        && !pattern.contains('/')
        && pattern.chars().filter(|&c| c == '*').count() == 1
    {
        let suffix = &pattern[1..];
        return path.ends_with(suffix)
            && !path[..path.len().saturating_sub(suffix.len())].contains('/');
    }

    // Pattern: dir/* (matches files directly in dir)
    if pattern.contains('/') && pattern.chars().filter(|&c| c == '*').count() == 1 {
        if let Some(star_pos) = pattern.find('*') {
            let prefix = &pattern[..star_pos];
            let suffix = &pattern[star_pos + 1..];

            if path.starts_with(prefix) && path.ends_with(suffix) {
                let middle_start = prefix.len();
                let middle_end = path.len().saturating_sub(suffix.len());
                if middle_end > middle_start {
                    let middle = &path[middle_start..middle_end];
                    return !middle.contains('/');
                } else {
                    return middle_start == middle_end;
                }
            }
            return false;
        }
    }

    // Exact match
    pattern == path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("/foo/bar"), "foo/bar");
        assert_eq!(normalize_path("foo/bar/"), "foo/bar");
        assert_eq!(normalize_path("/foo/bar/"), "foo/bar");
        assert_eq!(normalize_path("foo\\bar"), "foo/bar");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn test_parent_path() {
        assert_eq!(parent_path("foo/bar/baz"), "foo/bar");
        assert_eq!(parent_path("foo/bar"), "foo");
        assert_eq!(parent_path("foo"), "");
        assert_eq!(parent_path(""), "");
    }

    #[test]
    fn test_file_name() {
        assert_eq!(file_name("foo/bar/baz.txt"), "baz.txt");
        assert_eq!(file_name("foo/bar"), "bar");
        assert_eq!(file_name("file.txt"), "file.txt");
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("foo.txt", "foo.txt"));
        assert!(!glob_match("foo.txt", "bar.txt"));
    }

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*.txt", "foo.txt"));
        assert!(glob_match("*.txt", "bar.txt"));
        assert!(!glob_match("*.txt", "foo/bar.txt"));
        assert!(glob_match("foo/*", "foo/bar"));
        assert!(!glob_match("foo/*", "foo/bar/baz"));
    }

    #[test]
    fn test_glob_match_double_star() {
        // **/*.txt matches *.txt at any directory level
        assert!(glob_match("**/*.txt", "foo/bar.txt"));
        assert!(glob_match("**/*.txt", "foo/bar/baz.txt"));
        // foo.txt at root also matches (starts at position 0)
        assert!(glob_match("**/*.txt", "foo.txt"));
        // Simple ** at end
        assert!(glob_match("src/**", "src/foo"));
        assert!(glob_match("src/**", "src/foo/bar"));
        // ** alone matches anything
        assert!(glob_match("**", "anything"));
        assert!(glob_match("**", "foo/bar/baz"));
    }

    #[test]
    fn test_dir_entry() {
        let entry = DirEntry {
            name: "test.txt".to_string(),
            entry: None,
            is_dir: false,
        };
        assert_eq!(entry.name(), "test.txt");
        assert!(!entry.is_dir());
        assert!(entry.is_file());
        assert_eq!(entry.size(), 0);
    }
}
