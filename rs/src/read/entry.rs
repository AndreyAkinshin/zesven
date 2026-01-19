//! Archive entry types and selectors.

use crate::ArchivePath;
use crate::ownership::UnixOwnership;
use crate::timestamp::Timestamp;

/// An entry in a 7z archive.
///
/// This struct is marked `#[non_exhaustive]` to allow adding new fields
/// in future versions without breaking downstream code. Pattern matching
/// on `Entry` requires a `..` wildcard.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Entry {
    /// The path within the archive.
    pub path: ArchivePath,
    /// Whether this entry is a directory.
    pub is_directory: bool,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// CRC-32 checksum of the entry's uncompressed data.
    ///
    /// This is the standard checksum format used in 7z archives.
    /// The field is explicitly named `crc32` to indicate the bit-width.
    ///
    /// # When this is `None`
    ///
    /// - **Directories**: Have no content to checksum
    /// - **Empty files**: May not have a stored CRC
    /// - **Archive without CRCs**: Some 7z archives omit CRC storage
    /// - **Solid archives**: CRCs may be stored per-block rather than per-file
    pub crc32: Option<u32>,
    /// CRC-64 checksum (if available).
    ///
    /// **Note:** Standard 7z archives only use CRC-32. This field is
    /// currently always `None` for archives created by 7-Zip. It is
    /// reserved for future use with archives that store CRC-64 checksums
    /// or for custom applications requiring enhanced verification.
    // Hidden from public documentation as this field is not yet populated.
    // Kept for future compatibility when CRC-64 support is added.
    #[doc(hidden)]
    pub crc64: Option<u64>,
    /// Modification time as Windows FILETIME (100-nanosecond intervals since 1601-01-01).
    ///
    /// Use [`modification_timestamp()`](Self::modification_timestamp) to get a [`Timestamp`]
    /// with convenient conversion methods, or [`modified()`](Self::modified) for `SystemTime`.
    pub modification_time: Option<u64>,
    /// Creation time as Windows FILETIME (100-nanosecond intervals since 1601-01-01).
    ///
    /// Use [`creation_timestamp()`](Self::creation_timestamp) to get a [`Timestamp`]
    /// with convenient conversion methods, or [`created()`](Self::created) for `SystemTime`.
    pub creation_time: Option<u64>,
    /// Access time as Windows FILETIME (100-nanosecond intervals since 1601-01-01).
    ///
    /// Use [`access_timestamp()`](Self::access_timestamp) to get a [`Timestamp`]
    /// with convenient conversion methods, or [`accessed()`](Self::accessed) for `SystemTime`.
    pub access_time: Option<u64>,
    /// Windows file attributes.
    pub attributes: Option<u32>,
    /// Whether this entry is encrypted.
    pub is_encrypted: bool,
    /// Whether this entry is a symbolic link.
    ///
    /// Symlinks are detected from the entry attributes:
    /// - Unix: Mode bits indicate S_IFLNK (0o120000)
    /// - Windows: REPARSE_POINT attribute (0x400)
    ///
    /// When extracting symlinks, the entry content contains the target path.
    pub is_symlink: bool,
    /// Whether this is an anti-item (marks file for deletion in incremental backups).
    pub is_anti: bool,
    /// Unix file ownership information.
    pub ownership: Option<UnixOwnership>,
    /// Index in the internal entry list.
    #[allow(dead_code)] // Used for internal tracking
    pub(crate) index: usize,
    /// Folder index for solid archives.
    pub(crate) folder_index: Option<usize>,
    /// Stream index within folder.
    pub(crate) stream_index: Option<usize>,
}

impl Entry {
    /// Returns the file name (last component of the path).
    pub fn name(&self) -> &str {
        self.path.file_name()
    }

    /// Returns true if this is a file (not a directory).
    pub fn is_file(&self) -> bool {
        !self.is_directory
    }

    /// Returns the modification time as a SystemTime (if available).
    ///
    /// For higher precision access, use [`modification_timestamp()`](Self::modification_timestamp).
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        self.modification_time
            .map(|ft| Timestamp::from_filetime(ft).as_system_time())
    }

    /// Returns the creation time as a SystemTime (if available).
    ///
    /// For higher precision access, use [`creation_timestamp()`](Self::creation_timestamp).
    pub fn created(&self) -> Option<std::time::SystemTime> {
        self.creation_time
            .map(|ft| Timestamp::from_filetime(ft).as_system_time())
    }

    /// Returns the access time as a SystemTime (if available).
    ///
    /// For higher precision access, use [`access_timestamp()`](Self::access_timestamp).
    pub fn accessed(&self) -> Option<std::time::SystemTime> {
        self.access_time
            .map(|ft| Timestamp::from_filetime(ft).as_system_time())
    }

    /// Returns the modification timestamp with full 100-nanosecond precision.
    ///
    /// The [`Timestamp`] type provides access to the raw FILETIME value and
    /// various conversion methods while preserving maximum precision.
    pub fn modification_timestamp(&self) -> Option<Timestamp> {
        self.modification_time.map(Timestamp::from_filetime)
    }

    /// Returns the creation timestamp with full 100-nanosecond precision.
    ///
    /// The [`Timestamp`] type provides access to the raw FILETIME value and
    /// various conversion methods while preserving maximum precision.
    pub fn creation_timestamp(&self) -> Option<Timestamp> {
        self.creation_time.map(Timestamp::from_filetime)
    }

    /// Returns the access timestamp with full 100-nanosecond precision.
    ///
    /// The [`Timestamp`] type provides access to the raw FILETIME value and
    /// various conversion methods while preserving maximum precision.
    pub fn access_timestamp(&self) -> Option<Timestamp> {
        self.access_time.map(Timestamp::from_filetime)
    }

    /// Returns the Unix file mode (if stored in attributes).
    pub fn unix_mode(&self) -> Option<u32> {
        self.attributes.and_then(crate::ownership::decode_unix_mode)
    }

    /// Returns the owner UID (if available).
    pub fn owner_uid(&self) -> Option<u32> {
        self.ownership.as_ref().and_then(|o| o.uid)
    }

    /// Returns the owner GID (if available).
    pub fn owner_gid(&self) -> Option<u32> {
        self.ownership.as_ref().and_then(|o| o.gid)
    }

    /// Returns the owner name (if available).
    pub fn owner_name(&self) -> Option<&str> {
        self.ownership.as_ref().and_then(|o| o.user_name.as_deref())
    }

    /// Returns the group name (if available).
    pub fn group_name(&self) -> Option<&str> {
        self.ownership
            .as_ref()
            .and_then(|o| o.group_name.as_deref())
    }
}

/// A selector for filtering entries during extraction or testing.
///
/// # Built-in Implementations
///
/// | Type | Behavior |
/// |------|----------|
/// | `()` | Selects all entries (most concise) |
/// | [`SelectAll`] | Selects all entries (explicit) |
/// | `&[&str]` | Selects entries matching any of the paths |
/// | `Vec<String>` | Selects entries matching any of the paths |
/// | `Fn(&Entry) -> bool` | Custom predicate function |
/// | [`SelectByName`] | Selects by exact path match |
/// | [`SelectByPredicate`] | Wraps a predicate closure |
/// | [`SelectFilesOnly`] | Selects only files (excludes directories) |
/// | `SelectByRegex` | Regex-based selection (requires `regex` feature) |
///
/// # Example
///
/// ```rust,ignore
/// // Using () for select-all (most common)
/// archive.extract("./output", (), &options)?;
///
/// // Using a closure for custom filtering
/// archive.extract("./output", |e: &Entry| e.size < 1024, &options)?;
/// ```
pub trait EntrySelector {
    /// Returns true if the entry should be selected.
    fn select(&self, entry: &Entry) -> bool;
}

/// Selector that matches all entries.
#[derive(Debug, Clone, Copy, Default)]
pub struct SelectAll;

impl EntrySelector for SelectAll {
    fn select(&self, _entry: &Entry) -> bool {
        true
    }
}

/// Selector that matches entries by exact names.
#[derive(Debug, Clone)]
pub struct SelectByName {
    names: Vec<String>,
}

impl SelectByName {
    /// Creates a selector for the given names.
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }
}

impl EntrySelector for SelectByName {
    fn select(&self, entry: &Entry) -> bool {
        self.names.iter().any(|name| entry.path.as_str() == name)
    }
}

/// Selector that matches entries by a predicate function.
pub struct SelectByPredicate<F> {
    predicate: F,
}

impl<F: Fn(&Entry) -> bool> SelectByPredicate<F> {
    /// Creates a selector with the given predicate.
    pub fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

impl<F: Fn(&Entry) -> bool> EntrySelector for SelectByPredicate<F> {
    fn select(&self, entry: &Entry) -> bool {
        (self.predicate)(entry)
    }
}

/// Selector that matches only files (not directories).
#[derive(Debug, Clone, Copy, Default)]
pub struct SelectFilesOnly;

impl EntrySelector for SelectFilesOnly {
    fn select(&self, entry: &Entry) -> bool {
        entry.is_file()
    }
}

/// The unit type `()` implements `EntrySelector` to select all entries.
///
/// This is the most concise way to extract all entries from an archive:
///
/// ```rust,ignore
/// // Extract all entries - the () is the selector
/// archive.extract("./output", (), &ExtractOptions::default())?;
///
/// // Equivalent to using SelectAll explicitly:
/// archive.extract("./output", SelectAll, &ExtractOptions::default())?;
/// ```
///
/// Using `()` is idiomatic when you don't need any filtering. For explicit
/// code or when storing selectors in variables, prefer [`SelectAll`].
impl EntrySelector for () {
    fn select(&self, _entry: &Entry) -> bool {
        true
    }
}

// Implement for closures
impl<F: Fn(&Entry) -> bool> EntrySelector for F {
    fn select(&self, entry: &Entry) -> bool {
        self(entry)
    }
}

// Implement for slice of strings
impl EntrySelector for &[&str] {
    fn select(&self, entry: &Entry) -> bool {
        self.iter().any(|name| entry.path.as_str() == *name)
    }
}

// Implement for Vec of strings
impl EntrySelector for Vec<String> {
    fn select(&self, entry: &Entry) -> bool {
        self.iter().any(|name| entry.path.as_str() == name)
    }
}

/// Selector that matches entries by regular expression pattern.
///
/// This selector matches against the full entry path within the archive.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::read::SelectByRegex;
///
/// // Match all .txt files
/// let selector = SelectByRegex::new(r"\.txt$").unwrap();
///
/// // Match files in a specific directory
/// let selector = SelectByRegex::new(r"^src/.*\.rs$").unwrap();
/// ```
#[cfg(feature = "regex")]
#[derive(Debug, Clone)]
pub struct SelectByRegex {
    pattern: regex::Regex,
}

#[cfg(feature = "regex")]
impl SelectByRegex {
    /// Creates a selector with the given regex pattern.
    ///
    /// # Arguments
    ///
    /// * `pattern` - A regular expression pattern to match against entry paths
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRegex`] if the pattern is not a valid regular expression.
    ///
    /// [`Error::InvalidRegex`]: crate::Error::InvalidRegex
    pub fn new(pattern: &str) -> crate::Result<Self> {
        let regex = regex::Regex::new(pattern).map_err(|e| crate::Error::InvalidRegex {
            pattern: pattern.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self { pattern: regex })
    }

    /// Returns the underlying regex pattern.
    pub fn pattern(&self) -> &regex::Regex {
        &self.pattern
    }
}

#[cfg(feature = "regex")]
impl EntrySelector for SelectByRegex {
    fn select(&self, entry: &Entry) -> bool {
        self.pattern.is_match(entry.path.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(path: &str, is_dir: bool) -> Entry {
        Entry {
            path: ArchivePath::new(path).unwrap(),
            is_directory: is_dir,
            size: 100,
            crc32: Some(0x12345678),
            crc64: None,
            modification_time: None,
            creation_time: None,
            access_time: None,
            attributes: None,
            is_encrypted: false,
            is_symlink: false,
            is_anti: false,
            ownership: None,
            index: 0,
            folder_index: None,
            stream_index: None,
        }
    }

    #[test]
    fn test_entry_is_file() {
        let file = make_entry("test.txt", false);
        assert!(file.is_file());
        assert!(!file.is_directory);

        let dir = make_entry("subdir", true);
        assert!(!dir.is_file());
        assert!(dir.is_directory);
    }

    #[test]
    fn test_entry_name() {
        let entry = make_entry("path/to/file.txt", false);
        assert_eq!(entry.name(), "file.txt");
    }

    #[test]
    fn test_select_all() {
        let entry = make_entry("test.txt", false);
        assert!(SelectAll.select(&entry));
        assert!(().select(&entry));
    }

    #[test]
    fn test_select_by_name() {
        let entry1 = make_entry("file1.txt", false);
        let entry2 = make_entry("file2.txt", false);
        let entry3 = make_entry("other.txt", false);

        let selector = SelectByName::new(["file1.txt", "file2.txt"]);
        assert!(selector.select(&entry1));
        assert!(selector.select(&entry2));
        assert!(!selector.select(&entry3));
    }

    #[test]
    fn test_select_by_predicate() {
        let file = make_entry("test.txt", false);
        let dir = make_entry("subdir", true);

        let selector = SelectByPredicate::new(|e: &Entry| e.is_file());
        assert!(selector.select(&file));
        assert!(!selector.select(&dir));
    }

    #[test]
    fn test_select_files_only() {
        let file = make_entry("test.txt", false);
        let dir = make_entry("subdir", true);

        assert!(SelectFilesOnly.select(&file));
        assert!(!SelectFilesOnly.select(&dir));
    }

    #[test]
    fn test_select_closure() {
        let entry = make_entry("test.txt", false);
        let selector = |e: &Entry| e.size > 50;
        assert!(selector.select(&entry));
    }

    #[test]
    fn test_select_slice() {
        let entry1 = make_entry("file1.txt", false);
        let entry2 = make_entry("other.txt", false);

        let names: &[&str] = &["file1.txt", "file2.txt"];
        assert!(names.select(&entry1));
        assert!(!names.select(&entry2));
    }

    #[cfg(feature = "regex")]
    #[test]
    fn test_select_by_regex() {
        let txt_file = make_entry("path/to/file.txt", false);
        let rs_file = make_entry("src/main.rs", false);
        let other_file = make_entry("README.md", false);

        // Match .txt files
        let txt_selector = SelectByRegex::new(r"\.txt$").unwrap();
        assert!(txt_selector.select(&txt_file));
        assert!(!txt_selector.select(&rs_file));
        assert!(!txt_selector.select(&other_file));

        // Match files in src/ directory
        let src_selector = SelectByRegex::new(r"^src/").unwrap();
        assert!(!src_selector.select(&txt_file));
        assert!(src_selector.select(&rs_file));
        assert!(!src_selector.select(&other_file));

        // Match Rust files
        let rs_selector = SelectByRegex::new(r"\.rs$").unwrap();
        assert!(!rs_selector.select(&txt_file));
        assert!(rs_selector.select(&rs_file));
        assert!(!rs_selector.select(&other_file));
    }

    #[cfg(feature = "regex")]
    #[test]
    fn test_select_by_regex_invalid_pattern() {
        // Invalid regex pattern should return an InvalidRegex error
        let result = SelectByRegex::new(r"[invalid");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::Error::InvalidRegex { .. }));
        let err_str = err.to_string();
        assert!(
            err_str.contains("[invalid"),
            "Error should contain the pattern"
        );
    }
}
