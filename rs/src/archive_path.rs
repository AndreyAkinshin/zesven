//! Archive path type with validation for secure path handling.

use crate::{Error, Result};
use std::fmt;

/// Maximum length for archive paths (in bytes).
///
/// This limit prevents denial-of-service attacks where a malicious archive
/// specifies extremely long paths. 32KB is well above any reasonable file
/// system path limit (e.g., Linux PATH_MAX is 4KB, Windows MAX_PATH is ~260).
const MAX_PATH_LENGTH: usize = 32768;

/// Windows reserved device names that cannot be used as filenames.
///
/// On Windows, these names refer to device drivers and cannot be used as
/// regular filenames. Creating files with these names can cause unexpected
/// behavior or denial of service. We reject them on all platforms to ensure
/// archives created on non-Windows systems can be safely extracted on Windows.
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Checks if a filename is a Windows reserved name.
///
/// Windows reserved names are case-insensitive and also reserved
/// when followed by an extension (e.g., "CON.txt" is reserved).
fn is_windows_reserved(name: &str) -> bool {
    // Get base name without extension
    let base = match name.find('.') {
        Some(pos) => &name[..pos],
        None => name,
    };

    // Case-insensitive comparison
    WINDOWS_RESERVED_NAMES
        .iter()
        .any(|reserved| base.eq_ignore_ascii_case(reserved))
}

/// A validated archive path that ensures security against path traversal attacks.
///
/// `ArchivePath` normalizes paths to use forward slashes and validates that:
/// - No NUL bytes are present
/// - The path is not absolute (does not start with `/`)
/// - No empty segments exist (no `//` or trailing `/`)
/// - No `.` or `..` segments are present (prevents path traversal)
///
/// # Examples
///
/// ```
/// use zesven::ArchivePath;
///
/// // Valid paths
/// let path = ArchivePath::new("dir/file.txt").unwrap();
/// assert_eq!(path.as_str(), "dir/file.txt");
///
/// // Invalid paths are rejected
/// assert!(ArchivePath::new("../secret").is_err());
/// assert!(ArchivePath::new("/absolute/path").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArchivePath(String);

impl ArchivePath {
    /// Creates a new `ArchivePath` from a string, validating it.
    ///
    /// # Errors
    ///
    /// Returns an error if the path:
    /// - Contains NUL bytes
    /// - Is an absolute path (starts with `/`)
    /// - Contains empty segments (e.g., `a//b`)
    /// - Contains `.` or `..` segments
    /// - Is empty
    pub fn new(s: &str) -> Result<Self> {
        Self::validate(s)?;
        Ok(Self(s.to_string()))
    }

    /// Validates an archive path string.
    fn validate(s: &str) -> Result<()> {
        // Check for NUL bytes
        if s.contains('\0') {
            return Err(Error::InvalidArchivePath("contains NUL byte".into()));
        }

        // Check for empty path
        if s.is_empty() {
            return Err(Error::InvalidArchivePath("empty path".into()));
        }

        // Check for path length limit
        if s.len() > MAX_PATH_LENGTH {
            return Err(Error::InvalidArchivePath(format!(
                "path exceeds maximum length of {} bytes",
                MAX_PATH_LENGTH
            )));
        }

        // Check for absolute path
        if s.starts_with('/') {
            return Err(Error::InvalidArchivePath(
                "absolute path not allowed".into(),
            ));
        }

        // Check for trailing slash
        if s.ends_with('/') {
            return Err(Error::InvalidArchivePath(
                "trailing slash not allowed".into(),
            ));
        }

        // Check each segment
        for segment in s.split('/') {
            if segment.is_empty() {
                return Err(Error::InvalidArchivePath(
                    "empty segment (consecutive slashes)".into(),
                ));
            }
            if segment == "." {
                return Err(Error::InvalidArchivePath("'.' segment not allowed".into()));
            }
            if segment == ".." {
                return Err(Error::InvalidArchivePath(
                    "'..' segment not allowed (path traversal)".into(),
                ));
            }

            // Check for Windows reserved names (reject on all platforms for portability)
            if is_windows_reserved(segment) {
                return Err(Error::InvalidArchivePath(format!(
                    "Windows reserved filename '{}' not allowed",
                    segment
                )));
            }
        }

        Ok(())
    }

    /// Returns the path as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Joins this path with another segment.
    ///
    /// # Errors
    ///
    /// Returns an error if the resulting path would be invalid.
    pub fn join(&self, other: &str) -> Result<Self> {
        let joined = format!("{}/{}", self.0, other);
        Self::new(&joined)
    }

    /// Returns the parent directory of this path, if any.
    ///
    /// Returns `None` if this path has no parent (i.e., is a single segment).
    pub fn parent(&self) -> Option<Self> {
        self.0.rfind('/').map(|idx| {
            // Safety: we validated during construction, so parent is also valid
            Self(self.0[..idx].to_string())
        })
    }

    /// Returns the file name (last segment) of this path.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Returns the file extension, if any.
    ///
    /// The extension is the portion of the file name after the last `.`.
    /// Returns `None` if there is no extension, or if the file name starts
    /// with a dot (e.g., `.gitignore` has no extension).
    ///
    /// # Examples
    ///
    /// ```
    /// use zesven::ArchivePath;
    ///
    /// let path = ArchivePath::new("dir/file.txt").unwrap();
    /// assert_eq!(path.extension(), Some("txt"));
    ///
    /// let path = ArchivePath::new("dir/file").unwrap();
    /// assert_eq!(path.extension(), None);
    ///
    /// let path = ArchivePath::new(".gitignore").unwrap();
    /// assert_eq!(path.extension(), None);
    /// ```
    pub fn extension(&self) -> Option<&str> {
        let file_name = self.file_name();
        // Find the last dot, but not if it's the first character
        let dot_pos = file_name.rfind('.')?;
        if dot_pos == 0 {
            // File starts with dot (e.g., .gitignore) - no extension
            None
        } else {
            Some(&file_name[dot_pos + 1..])
        }
    }

    /// Returns an iterator over the path components (segments).
    ///
    /// # Examples
    ///
    /// ```
    /// use zesven::ArchivePath;
    ///
    /// let path = ArchivePath::new("a/b/c.txt").unwrap();
    /// let components: Vec<_> = path.components().collect();
    /// assert_eq!(components, vec!["a", "b", "c.txt"]);
    /// ```
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Returns true if this path starts with the given prefix.
    ///
    /// This performs a component-wise comparison, not a string prefix match.
    /// For example, `"foo/bar"` starts with `"foo"` but not `"fo"`.
    ///
    /// # Examples
    ///
    /// ```
    /// use zesven::ArchivePath;
    ///
    /// let path = ArchivePath::new("dir/subdir/file.txt").unwrap();
    /// assert!(path.starts_with("dir"));
    /// assert!(path.starts_with("dir/subdir"));
    /// assert!(!path.starts_with("di")); // Not a component boundary
    /// assert!(!path.starts_with("other"));
    /// ```
    pub fn starts_with(&self, prefix: &str) -> bool {
        if prefix.is_empty() {
            return true;
        }
        // Component-wise comparison
        let self_components: Vec<_> = self.0.split('/').collect();
        let prefix_components: Vec<_> = prefix.split('/').collect();

        if prefix_components.len() > self_components.len() {
            return false;
        }

        self_components
            .iter()
            .zip(prefix_components.iter())
            .all(|(a, b)| a == b)
    }
}

impl AsRef<str> for ArchivePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for ArchivePath {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl TryFrom<String> for ArchivePath {
    type Error = Error;

    fn try_from(s: String) -> Result<Self> {
        Self::validate(&s)?;
        Ok(Self(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_valid_simple_file() {
        let path = ArchivePath::new("file.txt").unwrap();
        assert_eq!(path.as_str(), "file.txt");
    }

    #[test]
    fn test_valid_nested_path() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        assert_eq!(path.as_str(), "dir/file.txt");
    }

    #[test]
    fn test_valid_deeply_nested() {
        let path = ArchivePath::new("a/b/c/d.txt").unwrap();
        assert_eq!(path.as_str(), "a/b/c/d.txt");
    }

    #[test]
    fn test_valid_single_char_names() {
        let path = ArchivePath::new("a/b/c").unwrap();
        assert_eq!(path.as_str(), "a/b/c");
    }

    #[test]
    fn test_valid_unicode() {
        let path = ArchivePath::new("日本語/файл.txt").unwrap();
        assert_eq!(path.as_str(), "日本語/файл.txt");
    }

    #[test]
    fn test_invalid_empty() {
        let err = ArchivePath::new("").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
    }

    #[test]
    fn test_invalid_nul_byte() {
        let err = ArchivePath::new("file\0.txt").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("NUL"));
    }

    #[test]
    fn test_invalid_absolute_path() {
        let err = ArchivePath::new("/etc/passwd").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("absolute"));
    }

    #[test]
    fn test_invalid_empty_segment() {
        let err = ArchivePath::new("a//b").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("empty segment"));
    }

    #[test]
    fn test_invalid_trailing_slash() {
        let err = ArchivePath::new("dir/").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("trailing slash"));
    }

    #[test]
    fn test_invalid_dot_segment() {
        let err = ArchivePath::new("./file").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("'.'"));
    }

    #[test]
    fn test_invalid_dot_in_middle() {
        let err = ArchivePath::new("a/./b").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
    }

    #[test]
    fn test_invalid_dotdot_traversal() {
        let err = ArchivePath::new("../secret").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn test_invalid_dotdot_in_middle() {
        let err = ArchivePath::new("a/../b").unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
    }

    #[test]
    fn test_file_name_simple() {
        let path = ArchivePath::new("file.txt").unwrap();
        assert_eq!(path.file_name(), "file.txt");
    }

    #[test]
    fn test_file_name_nested() {
        let path = ArchivePath::new("dir/subdir/file.txt").unwrap();
        assert_eq!(path.file_name(), "file.txt");
    }

    #[test]
    fn test_parent_simple() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.as_str(), "dir");
    }

    #[test]
    fn test_parent_nested() {
        let path = ArchivePath::new("a/b/c").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.as_str(), "a/b");
    }

    #[test]
    fn test_parent_none() {
        let path = ArchivePath::new("file.txt").unwrap();
        assert!(path.parent().is_none());
    }

    #[test]
    fn test_join() {
        let path = ArchivePath::new("dir").unwrap();
        let joined = path.join("file.txt").unwrap();
        assert_eq!(joined.as_str(), "dir/file.txt");
    }

    #[test]
    fn test_join_invalid() {
        let path = ArchivePath::new("dir").unwrap();
        assert!(path.join("..").is_err());
    }

    #[test]
    fn test_ordering() {
        let a = ArchivePath::new("a").unwrap();
        let b = ArchivePath::new("b").unwrap();
        let aa = ArchivePath::new("aa").unwrap();

        assert!(a < b);
        assert!(a < aa);
        assert!(aa < b);
    }

    #[test]
    fn test_hash_consistency() {
        let path1 = ArchivePath::new("dir/file.txt").unwrap();
        let path2 = ArchivePath::new("dir/file.txt").unwrap();

        let mut set = HashSet::new();
        set.insert(path1.clone());

        assert!(set.contains(&path2));
        assert_eq!(path1, path2);
    }

    #[test]
    fn test_display() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        assert_eq!(format!("{}", path), "dir/file.txt");
    }

    #[test]
    fn test_as_ref() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        let s: &str = path.as_ref();
        assert_eq!(s, "dir/file.txt");
    }

    #[test]
    fn test_try_from_str() {
        let path: ArchivePath = "dir/file.txt".try_into().unwrap();
        assert_eq!(path.as_str(), "dir/file.txt");
    }

    #[test]
    fn test_try_from_string() {
        let path: ArchivePath = String::from("dir/file.txt").try_into().unwrap();
        assert_eq!(path.as_str(), "dir/file.txt");
    }

    #[test]
    fn test_clone() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        let cloned = path.clone();
        assert_eq!(path, cloned);
    }

    #[test]
    fn test_debug() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        let debug = format!("{:?}", path);
        assert!(debug.contains("dir/file.txt"));
    }

    // Edge cases: files that look like traversal but aren't
    #[test]
    fn test_valid_dotfile() {
        let path = ArchivePath::new(".gitignore").unwrap();
        assert_eq!(path.as_str(), ".gitignore");
    }

    #[test]
    fn test_valid_double_dots_in_name() {
        let path = ArchivePath::new("file..txt").unwrap();
        assert_eq!(path.as_str(), "file..txt");
    }

    #[test]
    fn test_valid_triple_dots() {
        let path = ArchivePath::new("...").unwrap();
        assert_eq!(path.as_str(), "...");
    }

    #[test]
    fn test_invalid_too_long() {
        // Create a path that exceeds MAX_PATH_LENGTH
        let long_path = "a".repeat(MAX_PATH_LENGTH + 1);
        let err = ArchivePath::new(&long_path).unwrap_err();
        assert!(matches!(err, Error::InvalidArchivePath(_)));
        assert!(err.to_string().contains("maximum length"));
    }

    // ============================================================================
    // extension() tests
    // ============================================================================

    #[test]
    fn test_extension_simple() {
        let path = ArchivePath::new("file.txt").unwrap();
        assert_eq!(path.extension(), Some("txt"));
    }

    #[test]
    fn test_extension_nested() {
        let path = ArchivePath::new("dir/subdir/file.rs").unwrap();
        assert_eq!(path.extension(), Some("rs"));
    }

    #[test]
    fn test_extension_multiple_dots() {
        let path = ArchivePath::new("archive.tar.gz").unwrap();
        assert_eq!(path.extension(), Some("gz"));
    }

    #[test]
    fn test_extension_none_no_dot() {
        let path = ArchivePath::new("README").unwrap();
        assert_eq!(path.extension(), None);
    }

    #[test]
    fn test_extension_none_dotfile() {
        let path = ArchivePath::new(".gitignore").unwrap();
        assert_eq!(path.extension(), None);
    }

    #[test]
    fn test_extension_dotfile_with_ext() {
        let path = ArchivePath::new(".config.json").unwrap();
        assert_eq!(path.extension(), Some("json"));
    }

    #[test]
    fn test_extension_empty_extension() {
        let path = ArchivePath::new("file.").unwrap();
        assert_eq!(path.extension(), Some(""));
    }

    // ============================================================================
    // components() tests
    // ============================================================================

    #[test]
    fn test_components_single() {
        let path = ArchivePath::new("file.txt").unwrap();
        let components: Vec<_> = path.components().collect();
        assert_eq!(components, vec!["file.txt"]);
    }

    #[test]
    fn test_components_multiple() {
        let path = ArchivePath::new("a/b/c/d.txt").unwrap();
        let components: Vec<_> = path.components().collect();
        assert_eq!(components, vec!["a", "b", "c", "d.txt"]);
    }

    #[test]
    fn test_components_two() {
        let path = ArchivePath::new("dir/file").unwrap();
        let components: Vec<_> = path.components().collect();
        assert_eq!(components, vec!["dir", "file"]);
    }

    // ============================================================================
    // starts_with() tests
    // ============================================================================

    #[test]
    fn test_starts_with_empty() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        assert!(path.starts_with(""));
    }

    #[test]
    fn test_starts_with_single_component() {
        let path = ArchivePath::new("dir/subdir/file.txt").unwrap();
        assert!(path.starts_with("dir"));
    }

    #[test]
    fn test_starts_with_multiple_components() {
        let path = ArchivePath::new("a/b/c/d.txt").unwrap();
        assert!(path.starts_with("a/b"));
        assert!(path.starts_with("a/b/c"));
    }

    #[test]
    fn test_starts_with_full_path() {
        let path = ArchivePath::new("dir/file.txt").unwrap();
        assert!(path.starts_with("dir/file.txt"));
    }

    #[test]
    fn test_starts_with_partial_component() {
        let path = ArchivePath::new("directory/file.txt").unwrap();
        // "dir" is not a full component match
        assert!(!path.starts_with("dir"));
    }

    #[test]
    fn test_starts_with_wrong_prefix() {
        let path = ArchivePath::new("foo/bar/baz").unwrap();
        assert!(!path.starts_with("other"));
        assert!(!path.starts_with("fo"));
    }

    #[test]
    fn test_starts_with_longer_prefix() {
        let path = ArchivePath::new("a/b").unwrap();
        assert!(!path.starts_with("a/b/c/d"));
    }

    #[test]
    fn test_starts_with_single_file() {
        let path = ArchivePath::new("file.txt").unwrap();
        assert!(path.starts_with("file.txt"));
        assert!(!path.starts_with("file"));
    }

    // ============================================================================
    // Windows Reserved Filename Tests
    // ============================================================================
    //
    // Windows reserved names (CON, PRN, AUX, NUL, COM1-9, LPT1-9) are rejected
    // on all platforms to ensure archives can be safely extracted on Windows.

    /// Tests that Windows reserved filenames are rejected.
    #[test]
    fn test_windows_reserved_filenames_rejected() {
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];

        for name in reserved_names {
            let err = ArchivePath::new(name).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArchivePath(_)),
                "Windows reserved name '{}' should be rejected",
                name
            );
            assert!(
                err.to_string().contains("reserved"),
                "Error for '{}' should mention 'reserved'",
                name
            );
        }
    }

    /// Tests that reserved names with extensions are also rejected.
    #[test]
    fn test_windows_reserved_with_extension_rejected() {
        // Windows also treats CON.txt, NUL.any as reserved
        let reserved_with_ext = ["CON.txt", "NUL.log", "PRN.doc", "AUX.dat"];

        for name in reserved_with_ext {
            let err = ArchivePath::new(name).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArchivePath(_)),
                "Windows reserved name '{}' with extension should be rejected",
                name
            );
        }
    }

    /// Tests that reserved names in path segments are also rejected.
    #[test]
    fn test_windows_reserved_in_path_rejected() {
        let paths_with_reserved = ["dir/CON/file.txt", "data/NUL", "output/PRN.log"];

        for path in paths_with_reserved {
            let err = ArchivePath::new(path).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArchivePath(_)),
                "Path with Windows reserved name '{}' should be rejected",
                path
            );
        }
    }

    /// Tests that case variations of reserved names are also rejected.
    #[test]
    fn test_windows_reserved_case_insensitive_rejected() {
        // Windows reserved names are case-insensitive
        let case_variations = [
            "con", "Con", "CON", "nul", "Nul", "NUL", "com1", "Com1", "COM1",
        ];

        for name in case_variations {
            let err = ArchivePath::new(name).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArchivePath(_)),
                "Case variant '{}' of Windows reserved name should be rejected",
                name
            );
        }
    }

    /// Tests that names containing reserved names as substrings are allowed.
    #[test]
    fn test_windows_reserved_lookalikes_allowed() {
        // These should be allowed - they contain reserved names but aren't reserved
        let valid_names = [
            "CONNIE",
            "CONNECTOR",
            "PRNT",
            "AUXILIARY",
            "NULL",
            "NULLIFY",
            "COM10",
            "COM1a",
            "LPT10",
            "CONSOLE",
        ];

        for name in valid_names {
            let result = ArchivePath::new(name);
            assert!(
                result.is_ok(),
                "Valid name '{}' should be allowed (not a reserved name)",
                name
            );
        }
    }
}
