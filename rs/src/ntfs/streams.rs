//! NTFS Alternate Data Stream operations.
//!
//! This module provides functions for discovering, reading, and writing
//! NTFS alternate data streams.

use std::path::{Path, PathBuf};

/// Information about an alternate data stream on a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltStream {
    /// Path to the base file.
    pub base_path: PathBuf,
    /// Name of the stream (without the `:$DATA` suffix).
    pub stream_name: String,
    /// Size of the stream in bytes.
    pub size: u64,
}

impl AltStream {
    /// Returns the full path to the stream including the stream name.
    ///
    /// On Windows, this returns a path like `file.txt:stream_name`.
    /// On other platforms, this returns the same format for compatibility.
    pub fn full_path(&self) -> PathBuf {
        let mut path = self.base_path.clone();
        if let Some(file_name) = path.file_name() {
            let new_name = format!("{}:{}", file_name.to_string_lossy(), self.stream_name);
            path.set_file_name(new_name);
        }
        path
    }

    /// Returns the archive path representation of this stream.
    ///
    /// This is suitable for storing in an archive with the format:
    /// `path/to/file.txt:stream_name`
    pub fn archive_path(&self) -> String {
        format!("{}:{}", self.base_path.display(), self.stream_name)
    }
}

/// Discovers all alternate data streams on a file.
///
/// # Arguments
///
/// * `path` - Path to the file to scan for alternate streams
///
/// # Returns
///
/// A vector of `AltStream` entries, one for each alternate stream found.
/// The default unnamed stream (`::$DATA`) is not included.
///
/// # Platform Support
///
/// - Windows: Uses `FindFirstStreamW`/`FindNextStreamW` to enumerate streams
/// - Other platforms: Returns an empty vector
///
/// # Example
///
/// ```rust,ignore
/// use zesven::ntfs::discover_alt_streams;
///
/// let streams = discover_alt_streams("downloaded_file.exe")?;
/// for stream in streams {
///     println!("{}: {} bytes", stream.stream_name, stream.size);
/// }
/// ```
#[cfg(windows)]
pub fn discover_alt_streams(path: impl AsRef<Path>) -> std::io::Result<Vec<AltStream>> {
    use std::os::windows::ffi::OsStrExt;

    // Windows API types
    #[repr(C)]
    struct WIN32_FIND_STREAM_DATA {
        stream_size: i64,
        stream_name: [u16; 296], // MAX_PATH + stream name
    }

    type HANDLE = *mut std::ffi::c_void;
    const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FindFirstStreamW(
            lpFileName: *const u16,
            InfoLevel: u32,
            lpFindStreamData: *mut WIN32_FIND_STREAM_DATA,
            dwFlags: u32,
        ) -> HANDLE;
        fn FindNextStreamW(
            hFindStream: HANDLE,
            lpFindStreamData: *mut WIN32_FIND_STREAM_DATA,
        ) -> i32;
        fn FindClose(hFindFile: HANDLE) -> i32;
        fn GetLastError() -> u32;
    }

    const FIND_STREAM_INFO_STANDARD: u32 = 0;
    const ERROR_HANDLE_EOF: u32 = 38;

    let path = path.as_ref();
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut streams = Vec::new();

    unsafe {
        let mut find_data: WIN32_FIND_STREAM_DATA = std::mem::zeroed();

        let handle = FindFirstStreamW(
            wide_path.as_ptr(),
            FIND_STREAM_INFO_STANDARD,
            &mut find_data,
            0,
        );

        if handle == INVALID_HANDLE_VALUE {
            let error = GetLastError();
            if error == ERROR_HANDLE_EOF {
                // No streams found
                return Ok(streams);
            }
            return Err(std::io::Error::last_os_error());
        }

        loop {
            // Parse stream name
            let stream_name_end = find_data
                .stream_name
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(find_data.stream_name.len());
            let stream_name_wide = &find_data.stream_name[..stream_name_end];
            let stream_name = String::from_utf16_lossy(stream_name_wide);

            // Stream names are in format `:stream_name:$DATA`
            // Skip the default unnamed stream (`::$DATA`)
            if stream_name != "::$DATA" && stream_name.ends_with(":$DATA") {
                // Extract just the stream name (remove leading : and trailing :$DATA)
                if let Some(name) = stream_name
                    .strip_prefix(':')
                    .and_then(|s| s.strip_suffix(":$DATA"))
                {
                    if !name.is_empty() {
                        streams.push(AltStream {
                            base_path: path.to_path_buf(),
                            stream_name: name.to_string(),
                            size: find_data.stream_size as u64,
                        });
                    }
                }
            }

            // Try to find next stream
            if FindNextStreamW(handle, &mut find_data) == 0 {
                let error = GetLastError();
                if error == ERROR_HANDLE_EOF {
                    break;
                }
                FindClose(handle);
                return Err(std::io::Error::last_os_error());
            }
        }

        FindClose(handle);
    }

    Ok(streams)
}

/// Discovers all alternate data streams on a file.
///
/// On non-Windows platforms, this always returns an empty vector.
#[cfg(not(windows))]
pub fn discover_alt_streams(_path: impl AsRef<Path>) -> std::io::Result<Vec<AltStream>> {
    Ok(Vec::new())
}

/// Reads the contents of an alternate data stream.
///
/// # Arguments
///
/// * `base_path` - Path to the base file
/// * `stream_name` - Name of the stream to read
///
/// # Returns
///
/// The contents of the stream as a byte vector.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::ntfs::read_alt_stream;
///
/// let zone_info = read_alt_stream("file.exe", "Zone.Identifier")?;
/// let content = String::from_utf8_lossy(&zone_info);
/// println!("Zone info: {}", content);
/// ```
#[cfg(windows)]
pub fn read_alt_stream(base_path: impl AsRef<Path>, stream_name: &str) -> std::io::Result<Vec<u8>> {
    let full_path = format!("{}:{}", base_path.as_ref().display(), stream_name);
    std::fs::read(&full_path)
}

/// Reads the contents of an alternate data stream.
///
/// On non-Windows platforms, this returns an error.
#[cfg(not(windows))]
pub fn read_alt_stream(
    _base_path: impl AsRef<Path>,
    _stream_name: &str,
) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Alternate data streams are only supported on Windows NTFS",
    ))
}

/// Writes data to an alternate data stream.
///
/// # Arguments
///
/// * `base_path` - Path to the base file
/// * `stream_name` - Name of the stream to write
/// * `data` - Data to write to the stream
///
/// # Note
///
/// This will create the stream if it doesn't exist, or overwrite it if it does.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::ntfs::write_alt_stream;
///
/// // Write custom metadata to a stream
/// write_alt_stream("document.docx", "custom_metadata", b"some data")?;
/// ```
#[cfg(windows)]
pub fn write_alt_stream(
    base_path: impl AsRef<Path>,
    stream_name: &str,
    data: &[u8],
) -> std::io::Result<()> {
    let full_path = format!("{}:{}", base_path.as_ref().display(), stream_name);
    std::fs::write(&full_path, data)
}

/// Writes data to an alternate data stream.
///
/// On non-Windows platforms, this returns an error.
#[cfg(not(windows))]
pub fn write_alt_stream(
    _base_path: impl AsRef<Path>,
    _stream_name: &str,
    _data: &[u8],
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Alternate data streams are only supported on Windows NTFS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alt_stream_full_path() {
        let stream = AltStream {
            base_path: PathBuf::from("path/to/file.txt"),
            stream_name: "Zone.Identifier".to_string(),
            size: 100,
        };

        let full = stream.full_path();
        assert!(full.to_string_lossy().ends_with("file.txt:Zone.Identifier"));
    }

    #[test]
    fn test_alt_stream_archive_path() {
        let stream = AltStream {
            base_path: PathBuf::from("documents/report.docx"),
            stream_name: "metadata".to_string(),
            size: 50,
        };

        let path = stream.archive_path();
        assert!(path.contains("documents"));
        assert!(path.contains("report.docx:metadata"));
    }

    #[test]
    fn test_discover_alt_streams_non_windows() {
        // On non-Windows platforms, this should return an empty vector
        #[cfg(not(windows))]
        {
            let result = discover_alt_streams("/some/path").unwrap();
            assert!(result.is_empty());
        }
    }
}
