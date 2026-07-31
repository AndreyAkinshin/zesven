//! Integration tests for async API functionality.
//!
//! These tests verify the async archive reading and writing functionality
//! with Tokio runtime.

// Every test here writes an archive first, which needs a codec.
#![cfg(all(feature = "async", feature = "lzma2"))]

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
// Async Archive Encryption
// =============================================================================

/// Builds an archive in memory with the synchronous writer.
#[cfg(feature = "aes")]
fn build_archive(options: WriteOptions, entries: &[(&str, &[u8])]) -> Vec<u8> {
    use zesven::Writer;

    let mut bytes = Vec::new();
    {
        let mut writer = Writer::create(Cursor::new(&mut bytes))
            .expect("create writer")
            .options(options);
        for (path, data) in entries {
            writer
                .add_bytes(ArchivePath::new(path).expect("valid path"), data)
                .expect("add entry");
        }
        let _ = writer.finish().expect("finish");
    }
    bytes
}

/// Header-encrypted archives must open, list and extract through the async API.
#[cfg(feature = "aes")]
#[tokio::test]
async fn test_async_archive_header_encrypted() {
    use zesven::crypto::{NoncePolicy, Password};

    let payload = b"async encrypted payload";
    let bytes = build_archive(
        WriteOptions::new()
            .password(Password::new("hunter2"))
            .encrypt_header(true)
            .nonce_policy(NoncePolicy::random_with_params(4, 8)),
        &[("secret.txt", payload.as_slice())],
    );

    let archive = AsyncArchive::open_with_password(Cursor::new(bytes.clone()), "hunter2")
        .await
        .expect("header-encrypted archive must open with the right password");
    assert_eq!(archive.entries().len(), 1);
    assert_eq!(archive.entries()[0].path.as_str(), "secret.txt");

    // Without the password the file names are not even readable.
    assert!(
        AsyncArchive::open(Cursor::new(bytes)).await.is_err(),
        "a header-encrypted archive must not open without a password"
    );
}

/// A filtered folder has more than one coder; decoding only the first returns
/// data of the right length and the wrong contents.
#[cfg(feature = "aes")]
#[tokio::test]
async fn test_async_archive_extracts_filtered_and_encrypted() {
    use zesven::WriteFilter;
    use zesven::crypto::{NoncePolicy, Password};

    let payload: Vec<u8> = (0..4096u32)
        .map(|i| if i % 16 == 0 { 0xE8 } else { (i % 251) as u8 })
        .collect();
    let bytes = build_archive(
        WriteOptions::new()
            .password(Password::new("hunter2"))
            .filter(WriteFilter::BcjX86)
            .nonce_policy(NoncePolicy::random_with_params(4, 8)),
        &[("program.bin", payload.as_slice())],
    );

    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut archive = AsyncArchive::open_with_password(Cursor::new(bytes), "hunter2")
        .await
        .expect("open");
    let _ = archive
        .extract(temp.path(), (), &AsyncExtractOptions::default())
        .await
        .expect("extraction of a filtered encrypted folder must succeed");

    let extracted = tokio::fs::read(temp.path().join("program.bin"))
        .await
        .expect("read extracted file");
    assert_eq!(
        extracted, payload,
        "the filter and the cipher must both be applied"
    );
}

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

/// An empty entry must not swallow the entry that follows it.
///
/// The async writer gave every entry a folder, including empty ones. An empty
/// entry is recorded as kEmptyStream and carries no stream at all, so the extra
/// folder shifted the pairing: the next file came back empty and its own
/// contents were unreachable.
#[tokio::test]
async fn test_async_empty_entry_does_not_consume_the_next_one() {
    let mut writer = AsyncWriter::create(Cursor::new(Vec::new())).await.unwrap();
    writer
        .add_bytes(ArchivePath::new("empty.bin").unwrap(), b"")
        .await
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("good.bin").unwrap(), b"GOOD")
        .await
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().await.unwrap();

    let mut archive = zesven::read::Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    assert!(archive.extract_to_vec("empty.bin").unwrap().is_empty());
    assert_eq!(archive.extract_to_vec("good.bin").unwrap(), b"GOOD");
}

/// Deterministic mode in the async writer checks order rather than sorting.
///
/// Sorting the file list at the end rearranged names over streams that had
/// already been written, exactly as on the blocking path.
#[tokio::test]
async fn test_async_deterministic_mode_requires_sorted_entries() {
    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().deterministic(true));

    writer
        .add_bytes(ArchivePath::new("z.txt").unwrap(), b"CONTENT-Z")
        .await
        .unwrap();
    let out_of_order = writer
        .add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A")
        .await;

    assert!(
        out_of_order.is_err(),
        "adding an earlier-sorting path must fail rather than reorder the archive",
    );
}

/// Deterministic mode checks directories too, not only files.
///
/// The check lived in the byte-adding path alone, so a directory could be
/// added out of order and the entry after it was measured against the wrong
/// name.
#[tokio::test]
async fn test_async_deterministic_mode_checks_directories() {
    use zesven::write::EntryMeta;

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().deterministic(true));

    writer
        .add_directory(ArchivePath::new("z").unwrap(), EntryMeta::directory())
        .await
        .unwrap();
    let out_of_order = writer
        .add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A")
        .await;

    assert!(
        out_of_order.is_err(),
        "a directory must advance the order like any other entry",
    );
}

/// Options the async writer does not implement are refused, not ignored.
///
/// It emits its own header and applies none of these, but accepted them and
/// wrote an archive without them: a caller who asked for a filtered, solid or
/// commented archive got a plain one and no indication of it.
#[tokio::test]
async fn test_async_writer_refuses_options_it_cannot_apply() {
    use zesven::WriteFilter;

    let cases = [
        ("filter", WriteOptions::new().filter(WriteFilter::delta(4))),
        ("solid", WriteOptions::new().solid()),
        ("comment", WriteOptions::new().comment("hello")),
    ];

    for (name, options) in cases {
        let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
            .await
            .unwrap()
            .options(options);

        assert!(
            writer
                .add_bytes(ArchivePath::new("data.bin").unwrap(), b"DATA")
                .await
                .is_err(),
            "{name} was accepted and then not applied",
        );
    }
}

/// Encryption is refused on the flag, not on the presence of a password.
///
/// The check tested `is_encrypted()`, which asks whether a password is set, so
/// `encrypt_data(true)` on its own passed and the data went out in the clear.
#[cfg(feature = "aes")]
#[tokio::test]
async fn test_async_writer_refuses_encryption_without_a_password() {
    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().encrypt_data(true));

    let refused = writer
        .add_bytes(ArchivePath::new("secret.bin").unwrap(), b"SECRET")
        .await;

    assert!(
        refused.is_err(),
        "encryption was requested, unimplemented, and written in the clear",
    );
}

/// Changing the method mid-archive must not corrupt the entries already written.
///
/// The async writer described every folder with whichever method was set last,
/// so an entry written with LZMA2 and followed by a switch to `Copy` was
/// declared as stored and failed its checksum. It kept its own folder model,
/// which is why fixing the blocking writer left this standing.
#[tokio::test]
async fn test_async_changing_the_method_does_not_corrupt_earlier_entries() {
    use zesven::codec::CodecMethod;

    let first = vec![b'A'; 256 * 1024];

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), &first)
        .await
        .unwrap();

    writer = writer.options(
        WriteOptions::new()
            .level(1)
            .unwrap()
            .method(CodecMethod::Copy),
    );
    writer
        .add_bytes(ArchivePath::new("b.bin").unwrap(), b"SMALL")
        .await
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().await.unwrap();

    let mut archive = zesven::read::Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    assert_eq!(archive.extract_to_vec("a.bin").unwrap(), first);
    assert_eq!(archive.extract_to_vec("b.bin").unwrap(), b"SMALL");
}

/// A sink that fails once must leave the async writer unusable.
///
/// A transient failure - a full disk that is then freed, a socket that
/// reconnects - left bytes in the sink belonging to no folder. The writer went
/// on accepting entries and `finish` produced an archive that opened and then
/// failed to extract.
#[tokio::test]
async fn test_async_partial_write_poisons_the_writer() {
    use std::io::SeekFrom;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncSeek, AsyncWrite};

    /// Accepts a fixed number of bytes, fails once, then accepts again.
    struct FailsOnce {
        inner: Cursor<Vec<u8>>,
        budget: usize,
        recovered: bool,
    }

    impl AsyncWrite for FailsOnce {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let me = self.get_mut();
            if me.budget == 0 && !me.recovered {
                me.recovered = true;
                return Poll::Ready(Err(std::io::Error::other("disk full")));
            }
            if me.recovered {
                return std::io::Write::write(&mut me.inner, buf).into();
            }
            let n = buf.len().min(me.budget);
            me.budget -= n;
            std::io::Write::write(&mut me.inner, &buf[..n]).into()
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncSeek for FailsOnce {
        fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            std::io::Seek::seek(&mut self.get_mut().inner, position).map(|_| ())
        }
        fn poll_complete(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(std::io::Seek::stream_position(
                &mut self.get_mut().inner,
            )
            .unwrap()))
        }
    }

    // Incompressible, so the entry cannot shrink inside the budget.
    let mut data = vec![0u8; 1 << 20];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for byte in data.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }

    let mut writer = AsyncWriter::create(FailsOnce {
        inner: Cursor::new(Vec::new()),
        budget: 32 + 4096,
        recovered: false,
    })
    .await
    .unwrap()
    .options(WriteOptions::new().level(1).unwrap());

    assert!(
        writer
            .add_bytes(ArchivePath::new("big.bin").unwrap(), &data)
            .await
            .is_err(),
        "the sink failed, so the add must fail",
    );
    assert!(
        writer
            .add_bytes(ArchivePath::new("after.bin").unwrap(), b"AFTER")
            .await
            .is_err(),
        "the writer kept accepting entries after a partial write",
    );
    assert!(
        writer.finish_into_inner().await.is_err(),
        "an archive was produced from a failed write",
    );
}

/// Turning deterministic mode on mid-archive enforces the order from there on.
#[tokio::test]
async fn test_async_deterministic_mode_enabled_midway_enforces_order() {
    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().deterministic(false));
    writer
        .add_bytes(ArchivePath::new("z.txt").unwrap(), b"CONTENT-Z")
        .await
        .unwrap();

    writer = writer.options(WriteOptions::new().deterministic(true));
    assert!(
        writer
            .add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A")
            .await
            .is_err(),
        "'a.txt' sorts before the entry already written, and the setting is on",
    );
}

/// Writing to a path must produce a file this crate can open again.
///
/// Every other case here writes into a `Cursor`, which holds no buffer of its
/// own. `create_path` wraps the file in a buffered async sink, and dropping one
/// of those discards whatever it still holds rather than flushing it - so the
/// archive was left with 32 zero bytes where its signature belongs.
#[tokio::test]
async fn test_async_writer_to_a_path_produces_a_readable_archive() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("archive.7z");

    let payload = b"the signature is written last, after a seek to the start\n".repeat(32);
    let mut writer = AsyncWriter::create_path(&path)
        .await
        .unwrap()
        .options(WriteOptions::new());
    writer
        .add_bytes(ArchivePath::new("data.txt").unwrap(), &payload)
        .await
        .unwrap();
    let result = writer.finish().await.unwrap();
    assert_eq!(result.entries_written, 1);

    let mut archive = zesven::read::Archive::open_path(&path).unwrap();
    assert_eq!(archive.extract_to_vec("data.txt").unwrap(), payload);
}

/// A cancelled write must leave the writer unusable, like a failed one.
///
/// Cancellation is not an error path: a future dropped by `timeout` or a losing
/// `select!` branch simply never resumes, so nothing after the await runs. With
/// the state set from the error path only, a write cancelled with part of the
/// entry already in the sink left the writer accepting entries, and `finish`
/// produced an archive whose next entry failed its checksum.
#[tokio::test]
async fn test_async_cancelled_write_poisons_the_writer() {
    use std::io::SeekFrom;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncSeek, AsyncWrite};
    use tokio::sync::oneshot;

    /// Takes one short write, announces it, and then stalls until told to heal.
    ///
    /// The announcement is what cancels the write, so the test does not depend
    /// on any duration: no waker is registered for the stall, so the only way
    /// out of that future is the losing `select!` branch being dropped.
    struct StallsUntilHealed {
        inner: Cursor<Vec<u8>>,
        announce: Option<oneshot::Sender<()>>,
        healthy: Arc<AtomicBool>,
    }

    impl AsyncWrite for StallsUntilHealed {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let me = self.get_mut();
            if me.healthy.load(Ordering::SeqCst) {
                // Recovered, as a reconnected socket or a freed disk would be.
                return std::io::Write::write(&mut me.inner, buf).into();
            }
            if let Some(announce) = me.announce.take() {
                // Part of the entry lands in the sink...
                let n = buf.len().min(64);
                let written = std::io::Write::write(&mut me.inner, &buf[..n]);
                let _ = announce.send(());
                return written.into();
            }
            // ...and the rest of it never does.
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncSeek for StallsUntilHealed {
        fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            std::io::Seek::seek(&mut self.get_mut().inner, position).map(|_| ())
        }
        fn poll_complete(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(std::io::Seek::stream_position(
                &mut self.get_mut().inner,
            )
            .unwrap()))
        }
    }

    // Incompressible, so the entry is longer than the sink's first short write.
    let mut data = vec![0u8; 256 * 1024];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for byte in data.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }

    let (announce, announced) = oneshot::channel();
    let healthy = Arc::new(AtomicBool::new(false));
    let mut writer = AsyncWriter::create(StallsUntilHealed {
        inner: Cursor::new(Vec::new()),
        announce: Some(announce),
        healthy: healthy.clone(),
    })
    .await
    .unwrap()
    .options(WriteOptions::new().level(1).unwrap());

    tokio::select! {
        _ = writer.add_bytes(ArchivePath::new("big.bin").unwrap(), &data) => {
            panic!("the sink stalled, so the write cannot have completed")
        }
        _ = announced => {}
    }
    healthy.store(true, Ordering::SeqCst);

    assert!(
        writer
            .add_bytes(ArchivePath::new("after.bin").unwrap(), b"AFTER")
            .await
            .is_err(),
        "the writer kept accepting entries after a cancelled write",
    );
    assert!(
        writer.finish_into_inner().await.is_err(),
        "an archive was produced after a cancelled write",
    );
}

/// Finishing must not hold the runtime while the header is built.
///
/// Header encoding is a synchronous loop over every entry: a hundred thousand
/// of them take tens of milliseconds, and run inline they block a
/// current-thread runtime for all of it - no other task is polled, no timer
/// fires, no connection is accepted. It belongs on the blocking pool, like
/// compression.
#[tokio::test]
async fn test_async_finish_leaves_the_runtime_free() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use zesven::write::EntryMeta;

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new());

    // Directories, so the archive costs nothing to compress and the header is
    // the only expensive part of finishing.
    for i in 0..50_000 {
        writer
            .add_directory(
                ArchivePath::new(&format!("d{i:06}")).unwrap(),
                EntryMeta::directory(),
            )
            .await
            .unwrap();
    }

    let polls = Arc::new(AtomicU64::new(0));
    // When the ticker was last polled, in microseconds since the run began.
    //
    // A gap the ticker measures for itself is not enough: it can only see a
    // stall that some later poll ends, and the last expensive thing `finish`
    // does has nothing after it that yields on an in-memory sink. The stall
    // that mattered was the one nobody was left to observe - which is why the
    // first version of this test passed with the fix reverted and a 2.75
    // second stall in the call. So it is worked out afterwards, from when the
    // ticker was last seen alive.
    let last_seen = Arc::new(AtomicU64::new(0));
    let counter = polls.clone();
    let seen = last_seen.clone();
    let start = std::time::Instant::now();
    let ticker = tokio::spawn(async move {
        loop {
            counter.fetch_add(1, Ordering::Relaxed);
            seen.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
            tokio::task::yield_now().await;
        }
    });

    // Let the ticker reach its loop before anything is measured.
    tokio::task::yield_now().await;
    let before = polls.load(Ordering::Relaxed);
    let result = writer.finish().await.unwrap();
    let finished_at = start.elapsed().as_micros() as u64;
    let during = polls.load(Ordering::Relaxed) - before;
    let trailing = finished_at.saturating_sub(last_seen.load(Ordering::Relaxed));
    ticker.abort();

    assert_eq!(result.directories_written, 50_000);
    // Off the runtime the ticker takes hundreds of thousands of turns while
    // the header is built; on it this whole call is one stall and the count
    // has been observed at zero. The threshold sits far below the healthy
    // figure and far above the broken one, which is all a threshold has to do.
    assert!(
        during > 10_000,
        "the runtime was blocked while the header was built: {during} turns",
    );
    assert!(
        trailing < 50_000,
        "the runtime went {trailing}us without a turn before finish returned at \
         {finished_at}us: something synchronous ran on it",
    );
}

/// The write result must count what the blocking writer counts.
///
/// Anti-items are removals rather than files, and the async writer counted them
/// as files: the same archive was reported as holding one more entry than the
/// blocking writer reported for it. The archives were correct either way -
/// both this crate's reader and 7-Zip see the anti-items - so only the figures
/// handed back to the caller were wrong.
#[tokio::test]
async fn test_async_write_result_matches_the_blocking_writer() {
    use zesven::write::{EntryMeta, Writer};

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new());
    writer
        .add_bytes(ArchivePath::new("kept.txt").unwrap(), b"KEPT")
        .await
        .unwrap();
    writer
        .add_directory(ArchivePath::new("dir").unwrap(), EntryMeta::directory())
        .await
        .unwrap();
    writer
        .add_stream(
            ArchivePath::new("gone.txt").unwrap(),
            &mut &b""[..],
            EntryMeta::anti_item(),
        )
        .await
        .unwrap();
    writer
        .add_directory(
            ArchivePath::new("gone-dir").unwrap(),
            EntryMeta::anti_directory(),
        )
        .await
        .unwrap();
    let (asynchronous, _sink) = writer.finish_into_inner().await.unwrap();

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new());
    writer
        .add_bytes(ArchivePath::new("kept.txt").unwrap(), b"KEPT")
        .unwrap();
    writer
        .add_directory(ArchivePath::new("dir").unwrap(), EntryMeta::directory())
        .unwrap();
    writer
        .add_anti_item(ArchivePath::new("gone.txt").unwrap())
        .unwrap();
    writer
        .add_anti_directory(ArchivePath::new("gone-dir").unwrap())
        .unwrap();
    let (blocking, _sink) = writer.finish_into_inner().unwrap();

    assert_eq!(
        (
            asynchronous.entries_written,
            asynchronous.directories_written,
            asynchronous.total_size,
            asynchronous.volume_count,
        ),
        (
            blocking.entries_written,
            blocking.directories_written,
            blocking.total_size,
            blocking.volume_count,
        ),
        "the two writers disagree about what they wrote",
    );
    // Sizes rather than equality: the two archives are not byte-identical, but
    // each must report the length of the one it actually produced.
    assert_eq!(
        asynchronous.volume_sizes.len(),
        blocking.volume_sizes.len(),
        "one writer reported volume sizes and the other did not",
    );
}
