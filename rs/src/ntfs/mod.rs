//! NTFS Alternate Data Streams support.
//!
//! This module provides utilities for working with NTFS Alternate Data Streams (ADS),
//! which are additional named data streams that can be attached to files on NTFS
//! file systems.
//!
//! # Overview
//!
//! On NTFS, every file has at least one stream - the default unnamed stream that
//! contains the file's data. Files can also have additional named streams, which
//! are commonly used for:
//!
//! - Zone.Identifier: Tracks where a file was downloaded from (security feature)
//! - SummaryInformation: Document metadata
//! - Custom application data
//!
//! # Platform Support
//!
//! This functionality is only available on Windows. On other platforms, the
//! functions in this module will return empty results or no-op.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::ntfs::{AltStream, discover_alt_streams};
//!
//! // Discover alternate streams on a file
//! let streams = discover_alt_streams("document.docx")?;
//!
//! for stream in streams {
//!     println!("Found stream: {} ({} bytes)", stream.stream_name, stream.size);
//! }
//! ```

mod streams;

pub use streams::{AltStream, discover_alt_streams, read_alt_stream, write_alt_stream};

/// Naming convention for alternate data stream paths in archives.
///
/// Alternate data streams are stored using the format:
/// `base_path:stream_name`
///
/// For example:
/// - `document.docx:Zone.Identifier`
/// - `file.txt:custom_stream`
pub const ADS_SEPARATOR: char = ':';

/// Parses an archive path to extract base path and stream name.
///
/// # Arguments
///
/// * `archive_path` - The path from the archive entry
///
/// # Returns
///
/// * `Some((base_path, stream_name))` if the path contains an ADS separator
/// * `None` if the path is a regular file path
///
/// # Example
///
/// ```
/// use zesven::ntfs::parse_ads_path;
///
/// let result = parse_ads_path("file.txt:Zone.Identifier");
/// assert_eq!(result, Some(("file.txt", "Zone.Identifier")));
///
/// let result = parse_ads_path("regular_file.txt");
/// assert_eq!(result, None);
/// ```
pub fn parse_ads_path(archive_path: &str) -> Option<(&str, &str)> {
    // Find the last colon that separates base path from stream name
    // Note: On Windows, paths can start with drive letters (C:), so we need
    // to handle that case. We look for colons after the first character.
    let path_without_drive = if archive_path.len() > 2 && archive_path.as_bytes()[1] == b':' {
        &archive_path[2..]
    } else {
        archive_path
    };

    if let Some(pos) = path_without_drive.rfind(ADS_SEPARATOR) {
        let abs_pos = if archive_path.len() > path_without_drive.len() {
            pos + 2
        } else {
            pos
        };

        if abs_pos > 0 && abs_pos < archive_path.len() - 1 {
            let base = &archive_path[..abs_pos];
            let stream = &archive_path[abs_pos + 1..];
            if !stream.is_empty() {
                return Some((base, stream));
            }
        }
    }

    None
}

/// Creates an archive path from a base path and stream name.
///
/// # Example
///
/// ```
/// use zesven::ntfs::make_ads_path;
///
/// let path = make_ads_path("file.txt", "Zone.Identifier");
/// assert_eq!(path, "file.txt:Zone.Identifier");
/// ```
pub fn make_ads_path(base_path: &str, stream_name: &str) -> String {
    format!("{}{}{}", base_path, ADS_SEPARATOR, stream_name)
}

/// Checks if an archive path represents an alternate data stream.
///
/// # Example
///
/// ```
/// use zesven::ntfs::is_ads_path;
///
/// assert!(is_ads_path("file.txt:Zone.Identifier"));
/// assert!(!is_ads_path("regular_file.txt"));
/// ```
pub fn is_ads_path(archive_path: &str) -> bool {
    parse_ads_path(archive_path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ads_path_simple() {
        let result = parse_ads_path("file.txt:Zone.Identifier");
        assert_eq!(result, Some(("file.txt", "Zone.Identifier")));
    }

    #[test]
    fn test_parse_ads_path_nested() {
        let result = parse_ads_path("path/to/file.txt:custom_stream");
        assert_eq!(result, Some(("path/to/file.txt", "custom_stream")));
    }

    #[test]
    fn test_parse_ads_path_no_stream() {
        let result = parse_ads_path("regular_file.txt");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_ads_path_empty_stream() {
        let result = parse_ads_path("file.txt:");
        assert_eq!(result, None);
    }

    #[test]
    fn test_make_ads_path() {
        let path = make_ads_path("document.docx", "Zone.Identifier");
        assert_eq!(path, "document.docx:Zone.Identifier");
    }

    #[test]
    fn test_is_ads_path() {
        assert!(is_ads_path("file.txt:stream"));
        assert!(!is_ads_path("file.txt"));
        assert!(!is_ads_path("file.txt:"));
    }
}
