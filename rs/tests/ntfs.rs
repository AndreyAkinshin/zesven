//! Integration tests for NTFS alternate data streams support.
//!
//! These tests verify the ADS path parsing, construction, and detection
//! functionality. Windows-specific tests that interact with actual ADS
//! are conditionally compiled.

use std::path::PathBuf;
use zesven::ntfs::{
    ADS_SEPARATOR, AltStream, discover_alt_streams, is_ads_path, make_ads_path, parse_ads_path,
    read_alt_stream, write_alt_stream,
};

/// Tests ADS path parsing with simple paths.
#[test]
fn test_ads_path_parsing_simple() {
    let result = parse_ads_path("file.txt:Zone.Identifier");
    assert_eq!(result, Some(("file.txt", "Zone.Identifier")));
}

/// Tests ADS path parsing with nested paths.
#[test]
fn test_ads_path_parsing_nested() {
    let result = parse_ads_path("path/to/document.docx:metadata");
    assert_eq!(result, Some(("path/to/document.docx", "metadata")));
}

/// Tests ADS path parsing returns None for regular paths.
#[test]
fn test_ads_path_parsing_no_stream() {
    assert_eq!(parse_ads_path("regular_file.txt"), None);
    assert_eq!(parse_ads_path("path/to/file.txt"), None);
    assert_eq!(parse_ads_path(""), None);
}

/// Tests ADS path parsing with empty stream name.
#[test]
fn test_ads_path_parsing_empty_stream() {
    // Trailing colon with no stream name should return None
    assert_eq!(parse_ads_path("file.txt:"), None);
}

/// Tests ADS path parsing with multiple colons.
#[test]
fn test_ads_path_parsing_multiple_colons() {
    // Should use the last colon as separator
    let result = parse_ads_path("file:with:colons:stream");
    assert_eq!(result, Some(("file:with:colons", "stream")));
}

/// Tests ADS path construction.
#[test]
fn test_ads_path_construction() {
    let path = make_ads_path("document.docx", "Zone.Identifier");
    assert_eq!(path, "document.docx:Zone.Identifier");

    let path2 = make_ads_path("path/to/file.txt", "custom_stream");
    assert_eq!(path2, "path/to/file.txt:custom_stream");
}

/// Tests is_ads_path detection.
#[test]
fn test_is_ads_path_detection() {
    assert!(is_ads_path("file.txt:stream"));
    assert!(is_ads_path("path/to/file:Zone.Identifier"));

    assert!(!is_ads_path("file.txt"));
    assert!(!is_ads_path("path/to/file.txt"));
    assert!(!is_ads_path("file.txt:"));
    assert!(!is_ads_path(""));
}

/// Tests ADS separator constant.
#[test]
fn test_ads_separator() {
    assert_eq!(ADS_SEPARATOR, ':');
}

/// Tests AltStream full_path method.
#[test]
fn test_alt_stream_full_path() {
    let stream = AltStream {
        base_path: PathBuf::from("path/to/file.txt"),
        stream_name: "Zone.Identifier".to_string(),
        size: 100,
    };

    let full = stream.full_path();
    let full_str = full.to_string_lossy();
    assert!(full_str.ends_with("file.txt:Zone.Identifier"));
}

/// Tests AltStream archive_path method.
#[test]
fn test_alt_stream_archive_path() {
    let stream = AltStream {
        base_path: PathBuf::from("documents/report.docx"),
        stream_name: "metadata".to_string(),
        size: 50,
    };

    let path = stream.archive_path();
    assert!(path.contains("documents"));
    assert!(path.ends_with(":metadata"));
}

/// Tests parsing and reconstructing ADS paths roundtrip.
#[test]
fn test_ads_path_roundtrip() {
    let test_cases = [
        ("file.txt", "stream1"),
        ("path/to/document.docx", "Zone.Identifier"),
        ("deeply/nested/path/file.bin", "custom_data"),
    ];

    for (base, stream) in test_cases {
        let constructed = make_ads_path(base, stream);
        let (parsed_base, parsed_stream) =
            parse_ads_path(&constructed).expect("Should parse constructed path");

        assert_eq!(parsed_base, base);
        assert_eq!(parsed_stream, stream);
    }
}

/// Tests discover_alt_streams on non-Windows (should return empty).
#[cfg(not(windows))]
#[test]
fn test_discover_alt_streams_non_windows() {
    use std::fs::File;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    File::create(&file_path).unwrap();

    let streams = discover_alt_streams(&file_path).unwrap();
    assert!(
        streams.is_empty(),
        "Non-Windows should return empty streams"
    );
}

/// Tests read_alt_stream error on non-Windows.
#[cfg(not(windows))]
#[test]
fn test_read_alt_stream_non_windows_error() {
    let result = read_alt_stream("/some/path", "stream");
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}

/// Tests write_alt_stream error on non-Windows.
#[cfg(not(windows))]
#[test]
fn test_write_alt_stream_non_windows_error() {
    let result = write_alt_stream("/some/path", "stream", b"data");
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}

/// Tests AltStream with various stream names.
#[test]
fn test_alt_stream_various_names() {
    let common_streams = [
        "Zone.Identifier",
        "SummaryInformation",
        "DocumentSummaryInformation",
        "favicon",
        "thumbnail",
    ];

    for name in common_streams {
        let stream = AltStream {
            base_path: PathBuf::from("file.dat"),
            stream_name: name.to_string(),
            size: 0,
        };

        // Should be able to get archive path
        let archive_path = stream.archive_path();
        assert!(archive_path.contains(name));

        // Should be detected as ADS path
        assert!(is_ads_path(&archive_path));
    }
}

/// Tests path handling with special characters in stream names.
#[test]
fn test_ads_path_special_characters() {
    // Stream names with dots
    let path1 = make_ads_path("file", "stream.with.dots");
    let (base, stream) = parse_ads_path(&path1).unwrap();
    assert_eq!(base, "file");
    assert_eq!(stream, "stream.with.dots");

    // Stream names with underscores and numbers
    let path2 = make_ads_path("data.bin", "backup_2024_01_15");
    assert!(is_ads_path(&path2));
}

// Windows-specific tests that interact with actual ADS
#[cfg(windows)]
mod windows_tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    /// Tests discovering ADS on a real file (Windows only).
    #[test]
    fn test_discover_ads_on_real_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test_file.txt");

        // Create base file
        {
            let mut f = File::create(&file_path).unwrap();
            f.write_all(b"Main content").unwrap();
        }

        // Write an alternate stream
        write_alt_stream(&file_path, "custom_stream", b"Stream content")
            .expect("Failed to write ADS");

        // Discover streams
        let streams = discover_alt_streams(&file_path).expect("Failed to discover");

        // Should find our custom stream
        let found = streams.iter().any(|s| s.stream_name == "custom_stream");
        assert!(found, "Should find the custom stream");
    }

    /// Tests ADS read/write roundtrip (Windows only).
    #[test]
    fn test_read_write_ads_roundtrip() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("roundtrip.txt");

        // Create base file
        File::create(&file_path).unwrap();

        let stream_content = b"This is alternate stream content";

        // Write stream
        write_alt_stream(&file_path, "test_stream", stream_content).expect("Failed to write");

        // Read back
        let read_back = read_alt_stream(&file_path, "test_stream").expect("Failed to read");

        assert_eq!(read_back, stream_content);
    }

    /// Tests multiple ADS on same file (Windows only).
    #[test]
    fn test_multiple_ads_on_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("multi_stream.txt");

        File::create(&file_path).unwrap();

        // Write multiple streams
        write_alt_stream(&file_path, "stream1", b"Content 1").unwrap();
        write_alt_stream(&file_path, "stream2", b"Content 2").unwrap();
        write_alt_stream(&file_path, "stream3", b"Content 3").unwrap();

        // Discover all
        let streams = discover_alt_streams(&file_path).unwrap();

        assert!(streams.len() >= 3, "Should find all streams");
    }
}
