//! Integration tests for I/O statistics tracking.
//!
//! These tests verify that the stats module correctly tracks read and seek
//! operations during actual archive operations.

#![cfg(feature = "lzma")]

mod common;

use std::io::Cursor;
use zesven::read::Archive;
use zesven::stats::{StatsConfig, StatsReader, WithStats};

/// Tests that StatsReader tracks bytes read when opening an archive.
#[test]
fn test_stats_reader_with_archive_open() {
    let entries = [
        ("file1.txt", b"Hello, World!" as &[u8]),
        ("file2.txt", b"Test content here"),
    ];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Wrap the archive bytes with stats tracking
    let cursor = Cursor::new(archive_bytes.clone());
    let (reader, stats) = StatsReader::new(cursor, StatsConfig::default());

    // Open the archive
    let archive = Archive::open(reader).expect("Failed to open archive");

    // Verify stats were collected
    let stats = stats.lock().unwrap();
    assert!(
        stats.bytes_read > 0,
        "Should have read bytes when opening archive"
    );
    assert!(
        stats.read_count > 0,
        "Should have performed read operations"
    );

    // The archive should have our entries
    assert_eq!(archive.len(), 2);
}

/// Tests that stats track extraction bytes.
#[test]
fn test_stats_tracks_extraction_bytes() {
    let content = b"This is test content for extraction tracking.";
    let entries = [("tracked.txt", content.as_slice())];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Use with_stats trait method
    let cursor = Cursor::new(archive_bytes.clone());
    let (reader, stats) = cursor.with_stats_default();

    let mut archive = Archive::open(reader).expect("Failed to open archive");

    // Get initial read count
    let initial_bytes = stats.lock().unwrap().bytes_read;

    // Extract the file
    let extracted = archive
        .extract_to_vec("tracked.txt")
        .expect("Failed to extract");

    assert_eq!(extracted, content);

    // Verify more bytes were read during extraction
    let final_stats = stats.lock().unwrap();
    assert!(
        final_stats.bytes_read > initial_bytes,
        "Should read more bytes during extraction"
    );
}

/// Tests detailed mode records individual operations.
#[test]
fn test_stats_detailed_mode_records_operations() {
    let entries = [
        ("a.txt", b"File A content" as &[u8]),
        ("b.txt", b"File B content"),
        ("c.txt", b"File C content"),
    ];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let cursor = Cursor::new(archive_bytes);
    let (reader, stats) = cursor.with_detailed_stats();

    let mut archive = Archive::open(reader).expect("Failed to open archive");

    // Extract all files
    for (name, _) in &entries {
        let _ = archive.extract_to_vec(name);
    }

    // Check detailed stats
    let stats = stats.lock().unwrap();

    // Should have recorded individual read operations
    assert!(
        !stats.read_ops.is_empty(),
        "Detailed mode should record read operations"
    );

    // Each read op should have valid data
    for op in &stats.read_ops {
        assert!(
            op.actual > 0 || op.requested > 0,
            "Read ops should have size info"
        );
    }

    // Should also have seek operations
    assert!(
        !stats.seek_ops.is_empty(),
        "Should have seek operations from archive reading"
    );
}

/// Tests throughput calculation with real data.
#[test]
fn test_stats_throughput_calculation() {
    // Create an archive with enough data to get meaningful throughput
    let large_data = vec![b'X'; 10000];
    let entries = [("large.txt", large_data.as_slice())];

    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    let cursor = Cursor::new(archive_bytes);
    let (reader, stats) = StatsReader::new(cursor, StatsConfig::default());

    let mut archive = Archive::open(reader).expect("Failed to open archive");
    let _ = archive.extract_to_vec("large.txt");

    let stats = stats.lock().unwrap();

    // Throughput should be calculable (may be 0 if read is instant)
    let throughput = stats.throughput_bytes_per_sec();
    // Just verify it doesn't panic and returns a reasonable value
    assert!(throughput >= 0.0, "Throughput should be non-negative");

    // Average read size should be calculable
    let avg_size = stats.avg_read_size();
    assert!(avg_size > 0.0, "Should have positive average read size");
}

/// Tests WithStats trait on File-like readers.
#[test]
fn test_with_stats_trait_on_cursor() {
    let entries = [("test.txt", b"Testing WithStats trait" as &[u8])];
    let archive_bytes = common::create_archive(&entries).expect("Failed to create archive");

    // Use the trait methods
    let cursor = Cursor::new(archive_bytes);

    // Test with_stats_default
    let (reader, stats) = cursor.with_stats(StatsConfig::summary_only());

    let archive = Archive::open(reader).expect("Failed to open archive");
    assert_eq!(archive.len(), 1);

    let stats = stats.lock().unwrap();
    assert!(stats.bytes_read > 0);
    // In summary mode, ops vectors should be empty
    assert!(
        stats.read_ops.is_empty(),
        "Summary mode should not record individual ops"
    );
}

/// Tests stats merging functionality.
#[test]
fn test_stats_merge_from_multiple_archives() {
    use zesven::stats::ReadStats;

    let entries1 = [("file1.txt", b"Content 1" as &[u8])];
    let entries2 = [("file2.txt", b"Content 2" as &[u8])];

    let archive1 = common::create_archive(&entries1).expect("Failed to create archive 1");
    let archive2 = common::create_archive(&entries2).expect("Failed to create archive 2");

    // Read first archive
    let (reader1, stats1) = Cursor::new(archive1).with_stats_default();
    let _ = Archive::open(reader1).expect("Failed to open archive 1");

    // Read second archive
    let (reader2, stats2) = Cursor::new(archive2).with_stats_default();
    let _ = Archive::open(reader2).expect("Failed to open archive 2");

    // Merge stats
    let mut combined = ReadStats::default();
    combined.merge(&stats1.lock().unwrap());
    combined.merge(&stats2.lock().unwrap());

    // Combined stats should have data from both
    let stats1_bytes = stats1.lock().unwrap().bytes_read;
    let stats2_bytes = stats2.lock().unwrap().bytes_read;
    assert_eq!(combined.bytes_read, stats1_bytes + stats2_bytes);

    let stats1_reads = stats1.lock().unwrap().read_count;
    let stats2_reads = stats2.lock().unwrap().read_count;
    assert_eq!(combined.read_count, stats1_reads + stats2_reads);
}
