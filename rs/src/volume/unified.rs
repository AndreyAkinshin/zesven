//! Unified reader that handles both single-file and multi-volume archives.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{MultiVolumeReader, VolumeReader};
use crate::{Error, Result};

/// A unified reader that transparently handles both single-file and multi-volume archives.
///
/// This type automatically detects whether an archive is multi-volume based on the
/// file path and uses the appropriate reader internally.
pub enum UnifiedReader {
    /// Single-file archive reader.
    Single(BufReader<File>),
    /// Multi-volume archive reader.
    MultiVolume(MultiVolumeReader),
}

impl UnifiedReader {
    /// Opens an archive from a file path, auto-detecting multi-volume format.
    ///
    /// If the path ends in `.7z.NNN` (volume number) or a `.7z.001` file exists
    /// for the base path, opens as multi-volume. Otherwise opens as single-file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Check if this looks like a multi-volume archive
        if let Some(base_path) = detect_multivolume_base(path) {
            // Open as multi-volume
            let reader = MultiVolumeReader::open(&base_path)?;
            return Ok(UnifiedReader::MultiVolume(reader));
        }

        // Open as single file
        let file = File::open(path).map_err(Error::Io)?;
        Ok(UnifiedReader::Single(BufReader::new(file)))
    }

    /// Returns true if this is a multi-volume archive.
    pub fn is_multivolume(&self) -> bool {
        matches!(self, UnifiedReader::MultiVolume(_))
    }

    /// Returns the volume count if this is a multi-volume archive.
    pub fn volume_count(&self) -> Option<u32> {
        match self {
            UnifiedReader::Single(_) => None,
            UnifiedReader::MultiVolume(r) => Some(r.volume_count()),
        }
    }

    /// Returns the volume paths if this is a multi-volume archive.
    pub fn volume_paths(&self) -> Option<Vec<PathBuf>> {
        match self {
            UnifiedReader::Single(_) => None,
            UnifiedReader::MultiVolume(r) => {
                let count = r.volume_count();
                Some((1..=count).map(|n| r.get_volume_path(n)).collect())
            }
        }
    }
}

impl Read for UnifiedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            UnifiedReader::Single(r) => r.read(buf),
            UnifiedReader::MultiVolume(r) => r.read(buf),
        }
    }
}

impl Seek for UnifiedReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            UnifiedReader::Single(r) => r.seek(pos),
            UnifiedReader::MultiVolume(r) => r.seek(pos),
        }
    }
}

impl std::fmt::Debug for UnifiedReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnifiedReader::Single(_) => f.debug_struct("UnifiedReader::Single").finish(),
            UnifiedReader::MultiVolume(r) => f
                .debug_struct("UnifiedReader::MultiVolume")
                .field("volume_count", &r.volume_count())
                .finish(),
        }
    }
}

/// Detects if a path refers to a multi-volume archive.
///
/// Returns the base path (without volume extension) if multi-volume is detected.
fn detect_multivolume_base(path: &Path) -> Option<PathBuf> {
    let path_str = path.to_string_lossy();

    // Check .7z.NNN suffix (volume number)
    if let Some(pos) = path_str.rfind(".7z.") {
        let suffix = &path_str[pos + 4..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return Some(PathBuf::from(&path_str[..pos + 3]));
        }
    }

    // Check if base.7z.001 exists
    if path_str.ends_with(".7z") {
        let vol001 = PathBuf::from(format!("{}.001", path_str));
        if vol001.exists() {
            return Some(path.to_path_buf());
        }
    }

    None
}
