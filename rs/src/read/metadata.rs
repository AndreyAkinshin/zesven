//! Metadata preservation for extracted files.
//!
//! This module provides functions for applying timestamps and attributes
//! to extracted files.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::{Error, READ_BUFFER_SIZE, Result};

use super::PreserveMetadata;

/// Applies metadata to an extracted file based on options.
///
/// This sets file timestamps and attributes as configured in `PreserveMetadata`.
pub(crate) fn apply_metadata(
    path: &Path,
    options: &PreserveMetadata,
    modification_time: Option<u64>,
    creation_time: Option<u64>,
    attributes: Option<u32>,
) {
    use filetime::FileTime;

    // Convert Windows FILETIME (100-ns intervals since 1601) to Unix timestamp
    // Windows FILETIME epoch: 1601-01-01 00:00:00
    // Unix epoch: 1970-01-01 00:00:00
    // Difference: 11644473600 seconds (116444736000000000 in 100-ns intervals)
    const FILETIME_UNIX_DIFF: u64 = 116444736000000000;

    let filetime_to_unix = |ft: u64| -> Option<FileTime> {
        if ft >= FILETIME_UNIX_DIFF {
            let unix_100ns = ft - FILETIME_UNIX_DIFF;
            let secs = unix_100ns / 10_000_000;
            let nanos = ((unix_100ns % 10_000_000) * 100) as u32;
            Some(FileTime::from_unix_time(secs as i64, nanos))
        } else {
            None
        }
    };

    // Set modification time
    if options.modification_time {
        if let Some(mtime) = modification_time.and_then(filetime_to_unix) {
            if let Err(e) = filetime::set_file_mtime(path, mtime) {
                log::warn!(
                    "Failed to set modification time on '{}': {}",
                    path.display(),
                    e
                );
            }
        }
    }

    // Set creation time (platform-dependent)
    #[cfg(any(windows, target_os = "macos"))]
    if options.creation_time {
        if let Some(ctime) = creation_time.and_then(filetime_to_unix) {
            // filetime crate doesn't support setting creation time directly,
            // we'd need platform-specific code. For now, log a warning.
            // On Windows, this would use SetFileTime.
            // On macOS, this would use setattrlist.
            log::debug!(
                "Creation time preservation requested but not yet implemented for '{}': {:?}",
                path.display(),
                ctime
            );
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = creation_time;

    // Set file attributes
    if options.attributes {
        if let Some(attrs) = attributes {
            apply_file_attributes(path, attrs);
        }
    }
}

/// Applies file attributes to an extracted file.
#[cfg(unix)]
pub(crate) fn apply_file_attributes(path: &Path, attrs: u32) {
    use crate::ownership::decode_unix_mode;
    use std::os::unix::fs::PermissionsExt;

    // Try to decode Unix permissions from the attributes
    if let Some(mode) = decode_unix_mode(attrs) {
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            log::warn!("Failed to set permissions on '{}': {}", path.display(), e);
        }
    }
}

/// Applies file attributes to an extracted file (Windows).
#[cfg(windows)]
pub(crate) fn apply_file_attributes(path: &Path, attrs: u32) {
    // Windows attributes are stored in the lower 16 bits
    // Bit 0: Read-only, Bit 1: Hidden, Bit 2: System, etc.
    let windows_attrs = attrs & 0xFFFF;

    // Use Windows API to set attributes
    // For now, just handle read-only flag as it's most common
    if windows_attrs & 0x01 != 0 {
        // Read-only flag
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut perms = metadata.permissions();
            perms.set_readonly(true);
            if let Err(e) = std::fs::set_permissions(path, perms) {
                log::warn!(
                    "Failed to set read-only attribute on '{}': {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// Applies file attributes to an extracted file (fallback for other platforms).
#[cfg(not(any(unix, windows)))]
pub(crate) fn apply_file_attributes(_path: &Path, _attrs: u32) {
    // No-op for unsupported platforms
}

/// Calculates CRC32 of a file.
pub(crate) fn calculate_file_crc(path: &Path) -> Result<u32> {
    let mut file = File::open(path).map_err(Error::Io)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; READ_BUFFER_SIZE];

    loop {
        let n = file.read(&mut buf).map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize())
}
