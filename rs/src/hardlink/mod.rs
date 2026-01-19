//! Hard link tracking and detection for 7z archives.
//!
//! This module provides utilities for detecting and handling hard links
//! during archive creation and extraction. Hard links allow multiple
//! file entries to share the same data.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::hardlink::{HardLinkTracker, HardLinkInfo};
//! use std::path::Path;
//!
//! let mut tracker = HardLinkTracker::new();
//!
//! // Check if a file is a hard link to a previously seen file
//! if let Some(target_index) = tracker.check_file(Path::new("file1.txt"), 0)? {
//!     // This is a hard link to entry at target_index
//! }
//! ```

mod tracker;

pub use tracker::{HardLinkInfo, HardLinkTracker};

use crate::ArchivePath;

/// Information about a hard link entry in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardLinkEntry {
    /// Path of the hard link entry.
    pub path: ArchivePath,
    /// Index of the target entry this links to.
    pub target_index: usize,
}

impl HardLinkEntry {
    /// Creates a new hard link entry.
    pub fn new(path: ArchivePath, target_index: usize) -> Self {
        Self { path, target_index }
    }
}

/// Creates a hard link during extraction.
///
/// # Arguments
///
/// * `target_path` - Path to the existing file
/// * `link_path` - Path where the hard link should be created
///
/// # Errors
///
/// Returns an error if the hard link cannot be created.
#[cfg(unix)]
pub fn create_hard_link(
    target_path: impl AsRef<std::path::Path>,
    link_path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::hard_link(target_path, link_path)
}

/// Creates a hard link during extraction.
///
/// On Windows, this requires elevated privileges or developer mode.
#[cfg(windows)]
pub fn create_hard_link(
    target_path: impl AsRef<std::path::Path>,
    link_path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::hard_link(target_path, link_path)
}

/// Creates a hard link during extraction.
///
/// On non-Unix/Windows platforms, this is not supported.
#[cfg(not(any(unix, windows)))]
pub fn create_hard_link(
    _target_path: impl AsRef<std::path::Path>,
    _link_path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Hard links are not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hard_link_entry() {
        let entry = HardLinkEntry::new(ArchivePath::new("link.txt").unwrap(), 5);
        assert_eq!(entry.path.as_str(), "link.txt");
        assert_eq!(entry.target_index, 5);
    }
}
