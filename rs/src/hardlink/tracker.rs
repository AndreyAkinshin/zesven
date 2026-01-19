//! Hard link tracking during archive creation.
//!
//! This module provides the `HardLinkTracker` which detects hard links
//! by tracking inode/file ID information during compression.

use std::collections::HashMap;
use std::path::Path;

/// Information about a hard link relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardLinkInfo {
    /// Index of the entry this is linked to.
    pub target_index: usize,
}

/// Tracker for detecting hard links during archive creation.
///
/// The tracker maintains a mapping of file identifiers (device + inode on Unix,
/// volume serial + file index on Windows) to entry indices. When a file
/// with multiple hard links is encountered, subsequent occurrences are
/// detected as links to the first occurrence.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::hardlink::HardLinkTracker;
///
/// let mut tracker = HardLinkTracker::new();
///
/// for (index, path) in paths.iter().enumerate() {
///     if let Some(target_index) = tracker.check_file(path, index)? {
///         // This is a hard link to entry at target_index
///         // Store reference instead of data
///     } else {
///         // First occurrence, store the file data
///     }
/// }
/// ```
#[derive(Debug, Default)]
pub struct HardLinkTracker {
    /// Maps file identifier to the entry index of the first occurrence.
    /// Key: (device_id, inode) on Unix, (volume_serial, file_index) on Windows
    seen_files: HashMap<FileId, usize>,
}

/// Platform-independent file identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FileId {
    device: u64,
    inode: u64,
}

impl HardLinkTracker {
    /// Creates a new hard link tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if a file is a hard link to a previously seen file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file to check
    /// * `entry_index` - Index this entry will have in the archive
    ///
    /// # Returns
    ///
    /// * `Ok(Some(target_index))` - File is a hard link to entry at target_index
    /// * `Ok(None)` - File is not a hard link (or is the first occurrence)
    /// * `Err(e)` - Error reading file metadata
    ///
    /// # Note
    ///
    /// This method only detects hard links for files with more than one link.
    /// Regular files (nlink == 1) are never reported as hard links.
    #[cfg(unix)]
    pub fn check_file(
        &mut self,
        path: impl AsRef<Path>,
        entry_index: usize,
    ) -> std::io::Result<Option<usize>> {
        use std::os::unix::fs::MetadataExt;

        let path = path.as_ref();
        let metadata = std::fs::metadata(path)?;

        // Only consider files with multiple hard links
        if metadata.nlink() <= 1 {
            return Ok(None);
        }

        let file_id = FileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        };

        if let Some(&target_index) = self.seen_files.get(&file_id) {
            // Found a hard link to a previous entry
            Ok(Some(target_index))
        } else {
            // First occurrence of this file, register it
            self.seen_files.insert(file_id, entry_index);
            Ok(None)
        }
    }

    /// Checks if a file is a hard link to a previously seen file.
    ///
    /// On Windows, this uses file index information from GetFileInformationByHandle.
    #[cfg(windows)]
    pub fn check_file(
        &mut self,
        path: impl AsRef<Path>,
        entry_index: usize,
    ) -> std::io::Result<Option<usize>> {
        use std::fs::File;
        use std::os::windows::io::AsRawHandle;

        // BY_HANDLE_FILE_INFORMATION structure
        #[repr(C)]
        #[allow(non_snake_case)]
        struct BY_HANDLE_FILE_INFORMATION {
            dwFileAttributes: u32,
            ftCreationTime: [u32; 2],
            ftLastAccessTime: [u32; 2],
            ftLastWriteTime: [u32; 2],
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                hFile: *mut std::ffi::c_void,
                lpFileInformation: *mut BY_HANDLE_FILE_INFORMATION,
            ) -> i32;
        }

        let path = path.as_ref();
        let file = File::open(path)?;
        let handle = file.as_raw_handle();

        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let result = unsafe { GetFileInformationByHandle(handle as *mut _, &mut info) };

        if result == 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Combine high and low parts into 64-bit file index
        let file_index = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
        let volume_serial = info.dwVolumeSerialNumber;

        // On Windows, we can't easily check nlink, so we check all files
        // and rely on file_index matching
        let file_id = FileId {
            device: volume_serial as u64,
            inode: file_index,
        };

        // Check if we've seen this file before
        if file_index != 0 {
            if let Some(&target_index) = self.seen_files.get(&file_id) {
                return Ok(Some(target_index));
            }
            self.seen_files.insert(file_id, entry_index);
        }

        Ok(None)
    }

    /// Checks if a file is a hard link to a previously seen file.
    ///
    /// On platforms without hard link support, always returns None.
    #[cfg(not(any(unix, windows)))]
    pub fn check_file(
        &mut self,
        _path: impl AsRef<Path>,
        _entry_index: usize,
    ) -> std::io::Result<Option<usize>> {
        Ok(None)
    }

    /// Returns the number of tracked files.
    pub fn tracked_count(&self) -> usize {
        self.seen_files.len()
    }

    /// Clears all tracked files.
    pub fn clear(&mut self) {
        self.seen_files.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    #[test]
    fn test_tracker_new() {
        let tracker = HardLinkTracker::new();
        assert_eq!(tracker.tracked_count(), 0);
    }

    #[test]
    fn test_tracker_regular_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("regular.txt");

        // Create a regular file
        File::create(&file_path).unwrap();

        let mut tracker = HardLinkTracker::new();
        let result = tracker.check_file(&file_path, 0).unwrap();

        // Regular files with nlink=1 should not be reported as hard links
        assert!(result.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn test_tracker_hard_link_detection() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("original.txt");
        let link = dir.path().join("link.txt");

        // Create original file
        File::create(&original).unwrap();

        // Create hard link
        std::fs::hard_link(&original, &link).unwrap();

        let mut tracker = HardLinkTracker::new();

        // First occurrence should not be a hard link
        let result1 = tracker.check_file(&original, 0).unwrap();
        assert!(result1.is_none());
        assert_eq!(tracker.tracked_count(), 1);

        // Second occurrence should be detected as hard link
        let result2 = tracker.check_file(&link, 1).unwrap();
        assert_eq!(result2, Some(0));
    }

    #[test]
    fn test_tracker_clear() {
        let mut tracker = HardLinkTracker::new();
        tracker.seen_files.insert(
            FileId {
                device: 1,
                inode: 100,
            },
            0,
        );
        tracker.seen_files.insert(
            FileId {
                device: 1,
                inode: 200,
            },
            1,
        );

        assert_eq!(tracker.tracked_count(), 2);

        tracker.clear();
        assert_eq!(tracker.tracked_count(), 0);
    }
}
