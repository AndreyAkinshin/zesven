//! Multi-volume archive support.
//!
//! This module provides functions for detecting and opening multi-volume archives.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[cfg(not(feature = "aes"))]
use crate::format::parser::read_archive_header;
use crate::format::streams::ResourceLimits;
use crate::volume::{MultiVolumeReader, VolumeReader};
use crate::{Error, Result};

use super::entries;
use super::{Archive, VolumeInfo};

/// Detects if a path points to a multi-volume archive.
///
/// Returns the base path (without .NNN extension) if this is a multi-volume archive.
pub(crate) fn detect_multivolume_base(path: &Path) -> Option<PathBuf> {
    let path_str = path.to_string_lossy();

    // Check .7z.NNN suffix (e.g., archive.7z.001)
    if let Some(pos) = path_str.rfind(".7z.") {
        let suffix = &path_str[pos + 4..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            // This is a volume path, return the base
            return Some(PathBuf::from(&path_str[..pos + 3]));
        }
    }

    // Check if base.7z.001 exists for a plain .7z path
    if path_str.ends_with(".7z") {
        let first_volume = PathBuf::from(format!("{}.001", path_str));
        if first_volume.exists() {
            return Some(path.to_path_buf());
        }
    }

    None
}

impl Archive<MultiVolumeReader> {
    /// Opens a multi-volume archive.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to any volume file or the base path
    ///
    /// # Errors
    ///
    /// Returns an error if the archive cannot be opened.
    pub fn open_multivolume(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_multivolume_internal(
            path.as_ref(),
            #[cfg(feature = "aes")]
            None,
        )
    }

    /// Opens a multi-volume archive whose header is encrypted.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to any volume file or the base path
    /// * `password` - Password the header was encrypted with
    ///
    /// # Errors
    ///
    /// Returns an error if the archive cannot be opened or the password is wrong.
    #[cfg(feature = "aes")]
    pub fn open_multivolume_with_password(
        path: impl AsRef<Path>,
        password: impl Into<crate::crypto::Password>,
    ) -> Result<Self> {
        Self::open_multivolume_internal(path.as_ref(), Some(password.into()))
    }

    fn open_multivolume_internal(
        path: &Path,
        #[cfg(feature = "aes")] password: Option<crate::crypto::Password>,
    ) -> Result<Self> {
        let reader = MultiVolumeReader::open(path)?;

        // Collect volume info
        let volume_count = reader.volume_count();
        let volume_paths: Vec<PathBuf> = (1..=volume_count)
            .map(|n| reader.get_volume_path(n))
            .collect();

        // Read archive header. The password is needed here, not only later: a
        // header-encrypted archive cannot be listed without it.
        let limits = ResourceLimits::default();
        #[cfg(feature = "aes")]
        let (_start_header, header) = crate::format::parser::read_archive_header_with_password(
            &mut { reader },
            Some(limits),
            password.clone(),
        )?;
        #[cfg(not(feature = "aes"))]
        let (_start_header, header) = read_archive_header(&mut { reader }, Some(limits))?;

        // Build entries
        let entries = entries::build_entries(&header);
        let info = entries::build_info(&header, &entries);

        // Re-open for extraction operations
        let reader = MultiVolumeReader::open(path)?;

        Ok(Self {
            reader,
            header,
            entries,
            info,
            #[cfg(feature = "aes")]
            password,
            volume_info: Some(VolumeInfo {
                count: volume_count,
                paths: volume_paths,
            }),
            sfx_offset: 0, // Multi-volume archives don't have SFX stubs
        })
    }
}

/// Helper: Opens a multi-volume archive and returns Archive<BufReader<File>>.
///
/// This function reads the header using `MultiVolumeReader` but returns an
/// `Archive<BufReader<File>>` using only the first volume. This is a type-system
/// workaround to allow `open_path` to return a consistent type.
///
/// **Limitation**: This approach only works reliably when all compressed data
/// fits within the first volume. For archives with data spanning multiple
/// volumes, use [`Archive::open_multivolume`] instead which properly handles
/// cross-volume reads.
pub(crate) fn open_multivolume_as_single(
    base_path: &Path,
    #[cfg(feature = "aes")] password: Option<crate::crypto::Password>,
) -> Result<Archive<BufReader<File>>> {
    // Read header using MultiVolumeReader
    let reader = MultiVolumeReader::open(base_path)?;
    let volume_count = reader.volume_count();
    let volume_paths: Vec<PathBuf> = (1..=volume_count)
        .map(|n| reader.get_volume_path(n))
        .collect();

    let limits = ResourceLimits::default();
    #[cfg(feature = "aes")]
    let (_start_header, header) = crate::format::parser::read_archive_header_with_password(
        &mut { reader },
        Some(limits),
        password.clone(),
    )?;
    #[cfg(not(feature = "aes"))]
    let (_start_header, header) = read_archive_header(&mut { reader }, Some(limits))?;

    let entries = entries::build_entries(&header);
    let info = entries::build_info(&header, &entries);

    // Use single-file reader from first volume
    // Note: For cross-volume extraction, use Archive::open_multivolume instead
    let first_volume_path = format!("{}.001", base_path.display());
    let file = File::open(&first_volume_path).map_err(Error::Io)?;

    Ok(Archive {
        reader: BufReader::new(file),
        header,
        entries,
        info,
        #[cfg(feature = "aes")]
        password,
        volume_info: Some(VolumeInfo {
            count: volume_count,
            paths: volume_paths,
        }),
        sfx_offset: 0, // Multi-volume archives don't have SFX stubs
    })
}
