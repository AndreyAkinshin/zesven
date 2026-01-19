//! Integration tests for async API functionality.
//!
//! These tests verify the async archive reading and writing functionality
//! with Tokio runtime.

#![cfg(feature = "async")]

use std::io::Cursor;

use zesven::format::property_id;
use zesven::{
    ArchivePath, AsyncArchive, AsyncExtractOptions, AsyncProgressCallback, AsyncWriter,
    CancellationToken, WriteOptions,
};

/// Helper to create a minimal valid 7z archive in memory.
fn make_empty_archive() -> Vec<u8> {
    let mut data = Vec::new();

    // Signature
    data.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
    // Version (0.4)
    data.extend_from_slice(&[0x00, 0x04]);

    // Start header CRC (placeholder)
    let start_header_crc_pos = data.len();
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

    // Next header offset (0 - header immediately follows)
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // Header data: HEADER marker followed by END
    let header_data = vec![property_id::HEADER, property_id::END];

    // Next header size
    let header_size = header_data.len() as u64;
    data.extend_from_slice(&header_size.to_le_bytes());

    // Next header CRC
    let header_crc = crc32fast::hash(&header_data);
    data.extend_from_slice(&header_crc.to_le_bytes());

    // Compute start header CRC (covers bytes 12-31: offset, size, crc)
    let start_header_crc = crc32fast::hash(&data[12..32]);
    data[start_header_crc_pos..start_header_crc_pos + 4]
        .copy_from_slice(&start_header_crc.to_le_bytes());

    // Append header data
    data.extend_from_slice(&header_data);

    data
}

// ============================================================================
// AsyncArchive Tests
// ============================================================================

#[tokio::test]
async fn test_async_archive_open_empty() {
    let data = make_empty_archive();
    let cursor = Cursor::new(data);
    let archive = AsyncArchive::open(cursor).await.unwrap();

    assert!(archive.is_empty());
    assert_eq!(archive.len(), 0);
}

#[tokio::test]
async fn test_async_archive_info() {
    let data = make_empty_archive();
    let cursor = Cursor::new(data);
    let archive = AsyncArchive::open(cursor).await.unwrap();

    let info = archive.info();
    assert_eq!(info.entry_count, 0);
    assert!(!info.is_solid);
    assert!(!info.has_encrypted_entries);
}

#[tokio::test]
async fn test_async_archive_entries() {
    let data = make_empty_archive();
    let cursor = Cursor::new(data);
    let archive = AsyncArchive::open(cursor).await.unwrap();

    assert!(archive.entries().is_empty());
    assert!(archive.entry("nonexistent").is_none());
}

// ============================================================================
// AsyncArchive Error Path Tests
// ============================================================================
//
// These tests verify that the async API correctly propagates error types.
// We test two key error scenarios:
// 1. Parse-time errors (invalid signature) - error during archive opening
// 2. Extract-time errors (truncated data) - error during extraction
//
// Additional error scenarios (corrupted CRC, malformed headers) are tested
// extensively in tests/malformed_archives.rs for the sync API. These tests
// serve as regression guards for the async wrapper.
// ============================================================================

#[tokio::test]
async fn test_async_archive_open_invalid_signature() {
    // Random bytes that don't form a valid 7z signature
    let data: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
    let cursor = Cursor::new(data);

    match AsyncArchive::open(cursor).await {
        Err(zesven::Error::InvalidFormat(_)) => {} // Expected
        Err(e) => panic!(
            "Expected InvalidFormat error for invalid signature, got: {:?}",
            e
        ),
        Ok(_) => panic!("Should fail for invalid signature"),
    }
}

#[tokio::test]
async fn test_async_archive_extract_truncated_data() {
    // Create a valid archive first
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    // Add file with substantial content
    let content = b"This content will be truncated in the test archive";
    writer
        .add_bytes(ArchivePath::new("test.txt").unwrap(), content)
        .await
        .unwrap();

    let (_, cursor) = writer.finish_into_inner().await.unwrap();
    let mut archive_bytes = cursor.into_inner();

    // Truncate the archive data (remove last 20 bytes to corrupt compressed data)
    if archive_bytes.len() > 30 {
        archive_bytes.truncate(archive_bytes.len() - 20);
    }

    // Opening might succeed (header may still be readable)
    // but extraction should fail
    let read_cursor = Cursor::new(archive_bytes);
    match AsyncArchive::open(read_cursor).await {
        Ok(mut archive) => {
            // If archive opened, extraction should fail
            let temp_dir = tempfile::tempdir().unwrap();
            let result = archive
                .extract(temp_dir.path(), (), &AsyncExtractOptions::default())
                .await;

            // Should get an error - either Io, InvalidFormat, or corruption-related
            assert!(
                result.is_err(),
                "Extraction of truncated archive should fail"
            );
        }
        Err(_) => {
            // Also acceptable - archive failed to open due to truncation
        }
    }
}

// =============================================================================
// Async Archive Encrypted Header Support - Missing Implementation
// =============================================================================
//
// A test for async archive opening with wrong password is not included because
// the async API does not yet support header-encrypted archives.
//
// Current limitation: `AsyncArchive::open_with_password` uses `read_archive_header`
// without password support, while header-encrypted archives require
// `read_archive_header_with_offset_and_password`.
//
// When the async API properly supports header decryption, add a test that:
// 1. Creates an encrypted archive with header encryption
// 2. Attempts to open with wrong password - should fail
// 3. Opens with correct password - should succeed
// 4. Verifies entry can be extracted
//
// See also: multivolume.rs for a related encryption limitation in multi-volume API.

/// Test that dropping an async writer without finishing doesn't panic.
///
/// This verifies that the async writer handles early termination gracefully,
/// which is important for cancellation scenarios.
#[tokio::test]
async fn test_async_writer_drop_without_finish() {
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    // Add some content but don't finish
    writer
        .add_bytes(ArchivePath::new("file.txt").unwrap(), b"content")
        .await
        .unwrap();

    // Drop the writer without calling finish()
    // This should not panic - test passes if we reach this point
    drop(writer);
}

// ============================================================================
// AsyncWriter Tests
// ============================================================================

#[tokio::test]
async fn test_async_writer_create() {
    let buffer = Cursor::new(Vec::new());
    let _writer = AsyncWriter::create(buffer).await.unwrap();
}

#[tokio::test]
async fn test_async_writer_empty_archive() {
    let buffer = Cursor::new(Vec::new());
    let writer = AsyncWriter::create(buffer).await.unwrap();

    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 0);
    assert_eq!(result.directories_written, 0);
}

#[tokio::test]
async fn test_async_writer_add_bytes() {
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    let path = ArchivePath::new("test.txt").unwrap();
    writer
        .add_bytes(path, b"Hello, async world!")
        .await
        .unwrap();

    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 1);
    assert_eq!(result.total_size, 19);
}

#[tokio::test]
async fn test_async_writer_add_multiple_entries() {
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    writer
        .add_bytes(ArchivePath::new("file1.txt").unwrap(), b"Content 1")
        .await
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("file2.txt").unwrap(), b"Content 2")
        .await
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("file3.txt").unwrap(), b"Content 3")
        .await
        .unwrap();

    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 3);
}

#[tokio::test]
async fn test_async_writer_with_directory() {
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    use zesven::write::EntryMeta;
    let dir_path = ArchivePath::new("mydir").unwrap();
    writer
        .add_directory(dir_path, EntryMeta::directory())
        .await
        .unwrap();

    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 0);
    assert_eq!(result.directories_written, 1);
}

#[tokio::test]
async fn test_async_writer_with_options() {
    use zesven::codec::CodecMethod;

    let buffer = Cursor::new(Vec::new());
    let writer = AsyncWriter::create(buffer).await.unwrap().options(
        WriteOptions::new()
            .method(CodecMethod::Copy)
            .level(0)
            .unwrap(),
    );

    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 0);
}

// ============================================================================
// AsyncExtractOptions Tests
// ============================================================================

#[tokio::test]
async fn test_async_extract_options_default() {
    let options = AsyncExtractOptions::default();
    assert!(!options.is_cancelled());
}

#[tokio::test]
async fn test_async_extract_options_cancellation() {
    let token = CancellationToken::new();
    let options = AsyncExtractOptions::new().cancel_token(token.clone());

    assert!(!options.is_cancelled());
    token.cancel();
    assert!(options.is_cancelled());
}

#[tokio::test]
async fn test_async_extract_options_builder() {
    use std::num::NonZeroUsize;
    use zesven::read::{OverwritePolicy, PathSafety, Threads};

    let options = AsyncExtractOptions::new()
        .overwrite(OverwritePolicy::Skip)
        .path_safety(PathSafety::Relaxed)
        .threads(Threads::Count(NonZeroUsize::new(4).unwrap()));

    assert_eq!(options.overwrite, OverwritePolicy::Skip);
    assert_eq!(options.path_safety, PathSafety::Relaxed);
    assert_eq!(
        options.threads,
        Threads::Count(NonZeroUsize::new(4).unwrap())
    );
}

// ============================================================================
// Round-Trip Tests
// ============================================================================

#[tokio::test]
async fn test_async_round_trip_single_file() {
    // Write archive
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    let content = b"Hello, async round-trip test!";
    writer
        .add_bytes(ArchivePath::new("test.txt").unwrap(), content)
        .await
        .unwrap();

    // Use finish_into_inner to get access to the archive bytes
    let (result, cursor) = writer.finish_into_inner().await.unwrap();
    assert!(result.total_size > 0);
    assert_eq!(result.entries_written, 1);

    // Get the archive bytes from the cursor
    let archive_bytes = cursor.into_inner();
    assert!(!archive_bytes.is_empty());

    // Read back and verify content
    let read_cursor = Cursor::new(archive_bytes);
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    // Verify entry exists
    let entries = archive.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.as_str(), "test.txt");

    // Extract to temp directory and verify content
    let temp_dir = tempfile::tempdir().unwrap();
    let _ = archive
        .extract(temp_dir.path(), (), &AsyncExtractOptions::default())
        .await
        .unwrap();

    // Read extracted file and verify content
    let extracted_content = tokio::fs::read(temp_dir.path().join("test.txt"))
        .await
        .unwrap();
    assert_eq!(extracted_content, content);
}

#[tokio::test]
async fn test_async_round_trip_multiple_files() {
    // Write archive with multiple files
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    let files = [
        ("file1.txt", b"First file content".as_slice()),
        ("file2.txt", b"Second file content".as_slice()),
        ("subdir/file3.txt", b"Third file in subdirectory".as_slice()),
    ];

    for (path, content) in &files {
        writer
            .add_bytes(ArchivePath::new(path).unwrap(), content)
            .await
            .unwrap();
    }

    // Finish and get archive bytes
    let (result, cursor) = writer.finish_into_inner().await.unwrap();
    assert_eq!(result.entries_written, 3);

    // Read back
    let archive_bytes = cursor.into_inner();
    let read_cursor = Cursor::new(archive_bytes);
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    // Verify entries
    let entries = archive.entries();
    assert_eq!(entries.len(), 3);

    // Extract and verify each file
    let temp_dir = tempfile::tempdir().unwrap();
    let _ = archive
        .extract(temp_dir.path(), (), &AsyncExtractOptions::default())
        .await
        .unwrap();

    for (path, expected_content) in &files {
        let file_path = temp_dir.path().join(path);
        let actual_content = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(
            actual_content.as_slice(),
            *expected_content,
            "Content mismatch for {}",
            path
        );
    }
}

// ============================================================================
// Cancellation Tests
// ============================================================================

#[tokio::test]
async fn test_cancellation_before_extract() {
    // Create an archive with content to extract
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    // Add multiple files to increase chance of cancellation being checked
    for i in 0..10 {
        writer
            .add_bytes(
                ArchivePath::new(&format!("file{}.txt", i)).unwrap(),
                format!("Content for file {}", i).as_bytes(),
            )
            .await
            .unwrap();
    }

    let (_, cursor) = writer.finish_into_inner().await.unwrap();
    let archive_bytes = cursor.into_inner();

    let read_cursor = Cursor::new(archive_bytes);
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    let token = CancellationToken::new();
    token.cancel(); // Cancel before extraction

    let options = AsyncExtractOptions::new().cancel_token(token);
    let temp_dir = tempfile::tempdir().unwrap();

    let result = archive.extract(temp_dir.path(), (), &options).await;

    // With a pre-cancelled token, extraction should return Cancelled error
    assert!(
        matches!(result, Err(zesven::Error::Cancelled)),
        "Expected Cancelled error with pre-cancelled token, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_extract_with_cancellation_precancelled() {
    // Tests extract_with_cancellation with a pre-cancelled token for deterministic behavior.
    // This verifies the cancellation check in the select! macro works correctly.

    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    for i in 0..10 {
        writer
            .add_bytes(
                ArchivePath::new(&format!("file{}.txt", i)).unwrap(),
                format!("Content for file {}", i).as_bytes(),
            )
            .await
            .unwrap();
    }

    let (_, cursor) = writer.finish_into_inner().await.unwrap();
    let archive_bytes = cursor.into_inner();

    let read_cursor = Cursor::new(archive_bytes);
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    let token = CancellationToken::new();
    token.cancel(); // Pre-cancel for deterministic test

    let options = AsyncExtractOptions::default();
    let temp_dir = tempfile::tempdir().unwrap();

    let result = archive
        .extract_with_cancellation(temp_dir.path(), (), &options, token)
        .await;

    // With pre-cancelled token, extract_with_cancellation should return Cancelled
    assert!(
        matches!(result, Err(zesven::Error::Cancelled)),
        "Expected Cancelled error with pre-cancelled token, got: {:?}",
        result
    );
}

// ============================================================================
// Progress Callback Tests
// ============================================================================

#[tokio::test]
async fn test_channel_progress_reporter() {
    use std::sync::Arc;
    use zesven::{ChannelProgressReporter, ProgressEvent};

    let (reporter, mut rx) = ChannelProgressReporter::new(10);
    let reporter = Arc::new(reporter);

    // Test sending events
    reporter.on_entry_start("test.txt", 100).await;
    reporter.on_progress(50, 100).await;
    reporter.on_entry_complete("test.txt", true).await;

    // Verify events received
    let event1 = rx.recv().await.unwrap();
    assert!(matches!(
        event1,
        ProgressEvent::EntryStart {
            name,
            size: 100
        } if name == "test.txt"
    ));

    let event2 = rx.recv().await.unwrap();
    assert!(matches!(
        event2,
        ProgressEvent::Progress {
            bytes_extracted: 50,
            total_bytes: 100
        }
    ));

    let event3 = rx.recv().await.unwrap();
    assert!(matches!(
        event3,
        ProgressEvent::EntryComplete {
            name,
            success: true
        } if name == "test.txt"
    ));
}

/// Tests that progress events are correctly reported during actual extraction.
///
/// This integration test verifies that the AsyncProgressCallback implementation
/// receives all expected events (EntryStart, EntryComplete) during a real
/// extraction operation, not just in isolated callback testing.
#[tokio::test]
async fn test_async_extraction_with_progress_callback() {
    use std::sync::Arc;
    use zesven::{ChannelProgressReporter, ProgressEvent};

    // Create archive with multiple files
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    let files = [
        ("file1.txt", b"Content for file one".as_slice()),
        ("file2.txt", b"Content for file two".as_slice()),
        ("subdir/file3.txt", b"Content in subdirectory".as_slice()),
    ];

    for (path, content) in &files {
        writer
            .add_bytes(ArchivePath::new(path).unwrap(), content)
            .await
            .unwrap();
    }

    let (_, cursor) = writer.finish_into_inner().await.unwrap();
    let archive_bytes = cursor.into_inner();

    // Use the built-in ChannelProgressReporter to collect events
    let (reporter, mut rx) = ChannelProgressReporter::new(100);
    let reporter = Arc::new(reporter);

    // Open and extract with progress callback
    let read_cursor = Cursor::new(archive_bytes);
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    let options = AsyncExtractOptions::new().progress(reporter);
    let temp_dir = tempfile::tempdir().unwrap();

    let result = archive
        .extract(temp_dir.path(), (), &options)
        .await
        .unwrap();
    assert_eq!(result.entries_extracted, 3);

    // Collect all events from the channel
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Count event types
    let start_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProgressEvent::EntryStart { .. }))
        .collect();
    let complete_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProgressEvent::EntryComplete { .. }))
        .collect();

    // Should have EntryStart and EntryComplete for each file
    assert_eq!(
        start_events.len(),
        3,
        "Expected 3 EntryStart events, got {}",
        start_events.len()
    );
    assert_eq!(
        complete_events.len(),
        3,
        "Expected 3 EntryComplete events, got {}",
        complete_events.len()
    );

    // Verify all entries completed successfully
    for event in &complete_events {
        if let ProgressEvent::EntryComplete { success, .. } = event {
            assert!(success, "All entries should complete successfully");
        }
    }

    // Verify the files we expected were reported
    let reported_names: Vec<_> = start_events
        .iter()
        .filter_map(|e| {
            if let ProgressEvent::EntryStart { name, .. } = e {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();

    for (expected_path, _) in &files {
        assert!(
            reported_names.contains(expected_path),
            "Expected '{}' in progress events, got {:?}",
            expected_path,
            reported_names
        );
    }
}

// ============================================================================
// Password Provider Tests (requires aes feature)
// ============================================================================

// Note: AsyncCopyDecoder and build_async_decoder tests are in src/async_codec.rs
// as unit tests, since they test internal implementation details.

#[cfg(feature = "aes")]
mod password_tests {
    use zesven::Password;
    use zesven::async_password::{
        AsyncPassword, AsyncPasswordProvider, InteractivePasswordProvider,
    };

    #[tokio::test]
    async fn test_async_password_with_value() {
        let provider = AsyncPassword::new("test_password");
        let password = provider.get_password().await;
        assert!(password.is_some());
        assert_eq!(password.unwrap().as_str(), "test_password");
    }

    #[tokio::test]
    async fn test_async_password_none() {
        let provider = AsyncPassword::none();
        let password = provider.get_password().await;
        assert!(password.is_none());
    }

    #[tokio::test]
    async fn test_interactive_password_provider() {
        let (tx, provider) = InteractivePasswordProvider::new();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(Some(Password::new("interactive_password"))).ok();
        });

        let password = provider.get_password().await;
        assert!(password.is_some());
        assert_eq!(password.unwrap().as_str(), "interactive_password");
    }

    #[tokio::test]
    async fn test_interactive_password_provider_cancelled() {
        let (tx, provider) = InteractivePasswordProvider::new();
        drop(tx); // Drop sender to simulate cancellation

        let password = provider.get_password().await;
        assert!(password.is_none());
    }
}

// ============================================================================
// Concurrency Tests
// ============================================================================

#[tokio::test]
async fn test_concurrent_writes() {
    // Test that multiple async writers can work concurrently
    let handles: Vec<_> = (0..4)
        .map(|i| {
            tokio::spawn(async move {
                let buffer = Cursor::new(Vec::new());
                let mut writer = AsyncWriter::create(buffer).await.unwrap();

                writer
                    .add_bytes(
                        ArchivePath::new(&format!("file{}.txt", i)).unwrap(),
                        format!("Content from task {}", i).as_bytes(),
                    )
                    .await
                    .unwrap();

                writer.finish().await.unwrap()
            })
        })
        .collect();

    for handle in handles {
        let result = handle.await.unwrap();
        assert_eq!(result.entries_written, 1);
    }
}

#[tokio::test]
async fn test_concurrent_reads() {
    // Create test archives
    let archives: Vec<_> = (0..4).map(|_| make_empty_archive()).collect();

    // Read them concurrently
    let handles: Vec<_> = archives
        .into_iter()
        .map(|data| {
            tokio::spawn(async move {
                let cursor = Cursor::new(data);
                let archive = AsyncArchive::open(cursor).await.unwrap();
                archive.len()
            })
        })
        .collect();

    for handle in handles {
        let count = handle.await.unwrap();
        assert_eq!(count, 0); // Empty archives
    }
}

// ============================================================================
// No-blocking Verification Tests
// ============================================================================

#[tokio::test]
async fn test_async_operations_dont_block() {
    // Run with current thread runtime to detect blocking
    // If any operation blocks, this will timeout or hang

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // Create and open empty archive
        let data = make_empty_archive();
        let cursor = Cursor::new(data);
        let _archive = AsyncArchive::open(cursor).await.unwrap();

        // Create writer and write content
        let buffer = Cursor::new(Vec::new());
        let mut writer = AsyncWriter::create(buffer).await.unwrap();
        writer
            .add_bytes(ArchivePath::new("test.txt").unwrap(), b"test content")
            .await
            .unwrap();
        let _ = writer.finish().await.unwrap();

        true
    })
    .await;

    assert!(result.is_ok());
}

// ============================================================================
// Cancellation During Active Extraction Tests
// ============================================================================

#[tokio::test]
async fn test_cancellation_during_larger_extraction() {
    // Create an archive with larger content to give cancellation time to trigger
    let buffer = Cursor::new(Vec::new());
    let mut writer = AsyncWriter::create(buffer).await.unwrap();

    // Add multiple files with substantial content (10 files provides adequate
    // cancellation opportunity while keeping test execution fast)
    for i in 0..10 {
        let content = format!(
            "File {} content with some padding to make it larger: {}",
            i,
            "x".repeat(1000)
        );
        writer
            .add_bytes(
                ArchivePath::new(&format!("file{:02}.txt", i)).unwrap(),
                content.as_bytes(),
            )
            .await
            .unwrap();
    }

    let (_, cursor) = writer.finish_into_inner().await.unwrap();
    let archive_bytes = cursor.into_inner();

    // Test 1: Cancel token triggered during extraction
    let read_cursor = Cursor::new(archive_bytes.clone());
    let mut archive = AsyncArchive::open(read_cursor).await.unwrap();

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let options = AsyncExtractOptions::new().cancel_token(token_clone);
    let temp_dir = tempfile::tempdir().unwrap();

    // Spawn task that cancels after a tiny delay
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_micros(100)).await;
        token.cancel();
    });

    let result = archive.extract(temp_dir.path(), (), &options).await;

    // Wait for cancel task to complete
    let _ = cancel_handle.await;

    // Either extraction completed before cancellation, or cancellation was processed
    match result {
        Ok(extract_result) => {
            // Extraction completed successfully - valid outcome
            assert!(extract_result.entries_extracted > 0 || extract_result.entries_failed == 0);
        }
        Err(zesven::Error::Cancelled) => {
            // Cancellation was detected - valid outcome
        }
        Err(e) => {
            panic!("Expected Ok or Cancelled, got unexpected error: {:?}", e);
        }
    }
}
