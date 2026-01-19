//! Integration tests for hard link detection and recreation.
//!
//! These tests verify that hard links can be detected during archive creation
//! and recreated during extraction. Tests are primarily Unix-focused since
//! hard link behavior varies by platform.

use std::fs::File;
#[cfg(unix)]
use std::io::Write;
use tempfile::TempDir;
use zesven::ArchivePath;
use zesven::hardlink::{HardLinkEntry, HardLinkTracker, create_hard_link};

/// Tests HardLinkEntry creation.
#[test]
fn test_hardlink_entry_creation() {
    let path = ArchivePath::new("path/to/link.txt").unwrap();
    let entry = HardLinkEntry::new(path.clone(), 5);

    assert_eq!(entry.path.as_str(), "path/to/link.txt");
    assert_eq!(entry.target_index, 5);
}

/// Tests HardLinkTracker basic functionality.
#[test]
fn test_hardlink_tracker_new() {
    let tracker = HardLinkTracker::new();
    assert_eq!(tracker.tracked_count(), 0);
}

/// Tests tracker with regular (non-hardlinked) files.
#[test]
fn test_hardlink_tracker_regular_files() {
    let dir = TempDir::new().unwrap();

    // Create two separate regular files
    let file1_path = dir.path().join("file1.txt");
    let file2_path = dir.path().join("file2.txt");

    File::create(&file1_path).unwrap();
    File::create(&file2_path).unwrap();

    let mut tracker = HardLinkTracker::new();

    // Regular files with nlink=1 should not be reported as hard links
    let result1 = tracker.check_file(&file1_path, 0).unwrap();
    assert!(result1.is_none(), "Regular file should not be a hard link");

    let result2 = tracker.check_file(&file2_path, 1).unwrap();
    assert!(
        result2.is_none(),
        "Second regular file should not be a hard link"
    );
}

/// Tests hard link detection with actual hard links (Unix only).
#[cfg(unix)]
#[test]
fn test_hardlink_detection_during_write() {
    let dir = TempDir::new().unwrap();

    let original = dir.path().join("original.txt");
    let link1 = dir.path().join("link1.txt");
    let link2 = dir.path().join("link2.txt");

    // Create original file with content
    {
        let mut f = File::create(&original).unwrap();
        f.write_all(b"Shared content").unwrap();
    }

    // Create hard links
    std::fs::hard_link(&original, &link1).unwrap();
    std::fs::hard_link(&original, &link2).unwrap();

    let mut tracker = HardLinkTracker::new();

    // First occurrence of the file
    let result0 = tracker.check_file(&original, 0).unwrap();
    assert!(
        result0.is_none(),
        "First occurrence should not be a hard link"
    );
    assert_eq!(tracker.tracked_count(), 1);

    // Second occurrence (link1) should be detected as hard link
    let result1 = tracker.check_file(&link1, 1).unwrap();
    assert_eq!(result1, Some(0), "link1 should link to entry 0");

    // Third occurrence (link2) should also link to first
    let result2 = tracker.check_file(&link2, 2).unwrap();
    assert_eq!(result2, Some(0), "link2 should link to entry 0");

    // Tracker count should still be 1 (only original was registered)
    assert_eq!(tracker.tracked_count(), 1);
}

/// Tests hard link recreation (Unix only).
#[cfg(unix)]
#[test]
fn test_hardlink_recreation_during_extract() {
    let dir = TempDir::new().unwrap();

    let target_path = dir.path().join("target.txt");
    let link_path = dir.path().join("link.txt");

    // Create target file
    {
        let mut f = File::create(&target_path).unwrap();
        f.write_all(b"Target content").unwrap();
    }

    // Create hard link using the library function
    create_hard_link(&target_path, &link_path).expect("Failed to create hard link");

    // Verify they're the same file
    use std::os::unix::fs::MetadataExt;
    let target_meta = std::fs::metadata(&target_path).unwrap();
    let link_meta = std::fs::metadata(&link_path).unwrap();

    assert_eq!(
        target_meta.ino(),
        link_meta.ino(),
        "Hard links should have same inode"
    );
    assert_eq!(
        target_meta.dev(),
        link_meta.dev(),
        "Hard links should be on same device"
    );
    assert!(target_meta.nlink() >= 2, "Should have multiple links");
}

/// Tests hard link roundtrip (create links, detect them, recreate) (Unix only).
#[cfg(unix)]
#[test]
fn test_hardlink_roundtrip_preserves_links() {
    let source_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    // Setup: Create files with hard links
    let original = source_dir.path().join("data.txt");
    let link = source_dir.path().join("data_link.txt");

    {
        let mut f = File::create(&original).unwrap();
        f.write_all(b"Shared data").unwrap();
    }
    std::fs::hard_link(&original, &link).unwrap();

    // Detect hard links
    let mut tracker = HardLinkTracker::new();

    // Simulate adding files to archive
    let original_result = tracker.check_file(&original, 0).unwrap();
    let link_result = tracker.check_file(&link, 1).unwrap();

    assert!(original_result.is_none());
    assert_eq!(link_result, Some(0), "Link should reference original");

    // Simulate extraction: create target first, then hard link
    let dest_original = dest_dir.path().join("data.txt");
    let dest_link = dest_dir.path().join("data_link.txt");

    // Write original file
    {
        let mut f = File::create(&dest_original).unwrap();
        f.write_all(b"Shared data").unwrap();
    }

    // Recreate hard link (as would happen during extraction)
    create_hard_link(&dest_original, &dest_link).expect("Failed to recreate hard link");

    // Verify hard link was recreated
    use std::os::unix::fs::MetadataExt;
    let orig_meta = std::fs::metadata(&dest_original).unwrap();
    let link_meta = std::fs::metadata(&dest_link).unwrap();

    assert_eq!(orig_meta.ino(), link_meta.ino());
    assert!(orig_meta.nlink() >= 2);
}

/// Tests tracker with multiple distinct files that happen to have links (Unix only).
#[cfg(unix)]
#[test]
fn test_hardlink_tracker_with_multiple_link_groups() {
    let dir = TempDir::new().unwrap();

    // Group 1: file_a and its link
    let file_a = dir.path().join("group1_a.txt");
    let link_a = dir.path().join("group1_link.txt");
    {
        let mut f = File::create(&file_a).unwrap();
        f.write_all(b"Group 1").unwrap();
    }
    std::fs::hard_link(&file_a, &link_a).unwrap();

    // Group 2: file_b and its links
    let file_b = dir.path().join("group2_b.txt");
    let link_b1 = dir.path().join("group2_link1.txt");
    let link_b2 = dir.path().join("group2_link2.txt");
    {
        let mut f = File::create(&file_b).unwrap();
        f.write_all(b"Group 2").unwrap();
    }
    std::fs::hard_link(&file_b, &link_b1).unwrap();
    std::fs::hard_link(&file_b, &link_b2).unwrap();

    let mut tracker = HardLinkTracker::new();

    // Check files in mixed order
    let r_a = tracker.check_file(&file_a, 0).unwrap();
    let r_b = tracker.check_file(&file_b, 1).unwrap();
    let r_la = tracker.check_file(&link_a, 2).unwrap();
    let r_lb1 = tracker.check_file(&link_b1, 3).unwrap();
    let r_lb2 = tracker.check_file(&link_b2, 4).unwrap();

    // First occurrences should not be hard links
    assert!(r_a.is_none());
    assert!(r_b.is_none());

    // Links should reference their originals
    assert_eq!(r_la, Some(0), "link_a should reference file_a (index 0)");
    assert_eq!(r_lb1, Some(1), "link_b1 should reference file_b (index 1)");
    assert_eq!(r_lb2, Some(1), "link_b2 should reference file_b (index 1)");

    // Should have tracked 2 distinct files
    assert_eq!(tracker.tracked_count(), 2);
}

/// Tests tracker clear functionality.
#[test]
fn test_hardlink_tracker_clear() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("file.txt");
    File::create(&file_path).unwrap();

    let mut tracker = HardLinkTracker::new();

    // Check a file (even though it's not a hard link, on some platforms
    // it may still be tracked)
    let _ = tracker.check_file(&file_path, 0);

    // Clear tracker
    tracker.clear();
    assert_eq!(tracker.tracked_count(), 0);
}

/// Tests create_hard_link error handling (link to non-existent file).
#[test]
fn test_create_hardlink_error() {
    let dir = TempDir::new().unwrap();
    let non_existent = dir.path().join("does_not_exist.txt");
    let link_path = dir.path().join("link.txt");

    let result = create_hard_link(&non_existent, &link_path);
    assert!(result.is_err(), "Should fail to link to non-existent file");
}

/// Tests that hard links share content modifications (Unix only).
#[cfg(unix)]
#[test]
fn test_hardlinks_share_modifications() {
    let dir = TempDir::new().unwrap();
    let original = dir.path().join("original.txt");
    let link = dir.path().join("link.txt");

    // Create and link
    {
        let mut f = File::create(&original).unwrap();
        f.write_all(b"Initial content").unwrap();
    }
    std::fs::hard_link(&original, &link).unwrap();

    // Modify through the link
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&link)
            .unwrap();
        f.write_all(b"Modified content").unwrap();
    }

    // Read through original - should see modification
    let content = std::fs::read_to_string(&original).unwrap();
    assert_eq!(content, "Modified content");
}
