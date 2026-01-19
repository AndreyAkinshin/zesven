//! Archive opening methods.
//!
//! This module provides methods for opening 7z archives from various sources
//! including files, readers, and encrypted archives.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::format::header::detect_sfx;
use crate::format::parser::ArchiveHeader;
#[cfg(not(feature = "aes"))]
use crate::format::parser::read_archive_header_with_offset;
#[cfg(feature = "aes")]
use crate::format::parser::read_archive_header_with_offset_and_password;
use crate::format::streams::ResourceLimits;
use crate::safety::LimitedReader;
use crate::{Error, Result};

#[cfg(feature = "aes")]
use crate::Password;

use super::multivolume::{detect_multivolume_base, open_multivolume_as_single};
use super::{Archive, ArchiveInfo, Entry, entries};

/// Result of opening an archive (internal helper to avoid CFG duplication).
pub(crate) struct OpenResult<R> {
    pub reader: R,
    pub header: ArchiveHeader,
    pub entries: Vec<Entry>,
    pub info: ArchiveInfo,
    pub sfx_offset: u64,
}

/// Context for extraction with resource limit enforcement.
///
/// This struct is passed through the extraction call chain to enforce
/// limits on entry size, total size, and compression ratio.
pub(crate) struct ExtractionLimits {
    /// Maximum bytes for a single entry.
    pub max_entry_bytes: u64,
    /// Maximum total bytes across all entries.
    pub max_total_bytes: u64,
    /// Maximum compression ratio allowed.
    pub max_ratio: Option<u32>,
    /// Shared counter for total bytes extracted (across entries).
    pub total_tracker: Arc<AtomicU64>,
}

impl ExtractionLimits {
    /// Creates extraction limits from ResourceLimits.
    pub fn from_resource_limits(limits: &ResourceLimits) -> Self {
        Self {
            max_entry_bytes: limits.max_entry_unpacked,
            max_total_bytes: limits.max_total_unpacked,
            max_ratio: limits.ratio_limit.as_ref().map(|r| r.max_ratio),
            total_tracker: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Creates a LimitedReader wrapping the given reader.
    pub fn wrap_reader<RD: Read>(&self, reader: RD, compressed_size: u64) -> LimitedReader<RD> {
        let mut limited = LimitedReader::new(reader)
            .max_entry_bytes(self.max_entry_bytes)
            .compressed_size(compressed_size)
            .total_tracker(self.total_tracker.clone(), self.max_total_bytes);

        if let Some(ratio) = self.max_ratio {
            limited = limited.max_ratio(ratio);
        }

        limited
    }

    /// Creates unlimited extraction limits (no enforcement).
    pub fn unlimited() -> Self {
        Self {
            max_entry_bytes: u64::MAX,
            max_total_bytes: u64::MAX,
            max_ratio: None,
            total_tracker: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Converts an IO error to a Result, extracting ResourceLimitExceeded if present.
///
/// The LimitedReader wraps ResourceLimitExceeded errors in io::Error. This function
/// extracts them back to the proper Error type for consistent error handling.
pub(crate) fn map_io_error(e: std::io::Error) -> Error {
    // Try to extract a boxed Error from the io::Error
    if let Some(inner) = e.get_ref() {
        if let Some(Error::ResourceLimitExceeded(msg)) = inner.downcast_ref::<Error>() {
            return Error::ResourceLimitExceeded(msg.clone());
        }
    }
    // Check if it's an io::Error::other() containing our error
    if e.kind() == std::io::ErrorKind::Other {
        let msg = e.to_string();
        if msg.contains("exceeds limit") || msg.contains("Compression ratio") {
            return Error::ResourceLimitExceeded(msg);
        }
    }
    Error::Io(e)
}

impl Archive<BufReader<File>> {
    /// Opens an archive from a file path.
    ///
    /// This method auto-detects multi-volume archives:
    /// - If the path is `.7z.001`, `.7z.002`, etc., opens as multi-volume
    /// - If the path is `.7z` and `.7z.001` exists, opens as multi-volume
    /// - Otherwise opens as a single-file archive
    ///
    /// # Multi-Volume Note
    ///
    /// For multi-volume archives where compressed data spans multiple volume files,
    /// use [`Archive::open_multivolume`] instead. This method returns an
    /// `Archive<MultiVolumeReader>` that properly handles cross-volume reads.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or the archive is invalid.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Try to detect multi-volume archive
        if let Some(base_path) = detect_multivolume_base(path) {
            // This is a multi-volume archive
            return open_multivolume_as_single(&base_path);
        }

        // Single-file archive
        let file = File::open(path).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open(reader)
    }

    /// Opens an archive from a file path with custom resource limits.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file
    /// * `limits` - Custom resource limits for parsing
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened, the archive is invalid,
    /// or the specified resource limits are violated.
    pub fn open_path_with_limits(path: impl AsRef<Path>, limits: ResourceLimits) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open_with_limits(reader, limits)
    }

    /// Opens an encrypted archive from a file path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file
    /// * `password` - Password for decryption
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened, the archive is invalid,
    /// or the password is incorrect.
    #[cfg(feature = "aes")]
    pub fn open_path_with_password(
        path: impl AsRef<Path>,
        password: impl Into<Password>,
    ) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open_with_password(reader, password)
    }
}

impl<R: Read + Seek> Archive<R> {
    /// Opens an archive from a reader.
    ///
    /// # Arguments
    ///
    /// * `reader` - A reader providing the archive data
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid or cannot be read.
    pub fn open(reader: R) -> Result<Self> {
        Self::open_internal(reader, None, None)
    }

    /// Opens an archive from a reader with custom resource limits.
    ///
    /// This allows configuring protection against malicious archives (zip bombs)
    /// by setting limits on entry count, header size, compression ratios, etc.
    ///
    /// # Arguments
    ///
    /// * `reader` - A reader providing the archive data
    /// * `limits` - Custom resource limits for parsing
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid, cannot be read, or violates
    /// the specified resource limits.
    pub fn open_with_limits(reader: R, limits: ResourceLimits) -> Result<Self> {
        Self::open_internal(reader, None, Some(limits))
    }

    /// Opens an encrypted archive from a reader.
    ///
    /// # Arguments
    ///
    /// * `reader` - A reader providing the archive data
    /// * `password` - Password for decryption
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid, cannot be read,
    /// or the password is incorrect.
    #[cfg(feature = "aes")]
    pub fn open_with_password(reader: R, password: impl Into<Password>) -> Result<Self> {
        Self::open_internal(reader, Some(password.into()), None)
    }

    /// Opens an encrypted archive from a reader with custom resource limits.
    ///
    /// This combines password-based decryption with custom resource limit
    /// enforcement for parsing.
    ///
    /// # Arguments
    ///
    /// * `reader` - A reader providing the archive data
    /// * `password` - Password for decryption
    /// * `limits` - Custom resource limits for parsing
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid, cannot be read,
    /// the password is incorrect, or the specified resource limits are violated.
    #[cfg(feature = "aes")]
    pub fn open_with_password_and_limits(
        reader: R,
        password: impl Into<Password>,
        limits: ResourceLimits,
    ) -> Result<Self> {
        Self::open_internal(reader, Some(password.into()), Some(limits))
    }

    /// Common archive opening logic (AES version).
    #[cfg(feature = "aes")]
    fn open_common(
        mut reader: R,
        password: Option<Password>,
        limits: Option<ResourceLimits>,
    ) -> Result<OpenResult<R>> {
        // Detect if this is an SFX archive (7z data not at offset 0)
        let sfx_offset = match detect_sfx(&mut reader)? {
            Some(sfx_info) => {
                // Seek to the archive start
                reader
                    .seek(SeekFrom::Start(sfx_info.archive_offset))
                    .map_err(Error::Io)?;
                sfx_info.archive_offset
            }
            None => 0,
        };

        // Read main header (also parses start header internally)
        // Use provided limits or fall back to defaults
        let limits = limits.unwrap_or_default();
        let (_start_header, header) = read_archive_header_with_offset_and_password(
            &mut reader,
            Some(limits),
            sfx_offset,
            password,
        )?;

        // Build entries from files info
        let entries = entries::build_entries(&header);

        // Build archive info
        let info = entries::build_info(&header, &entries);

        Ok(OpenResult {
            reader,
            header,
            entries,
            info,
            sfx_offset,
        })
    }

    /// Common archive opening logic (non-AES version).
    #[cfg(not(feature = "aes"))]
    fn open_common(
        mut reader: R,
        _password: Option<()>,
        limits: Option<ResourceLimits>,
    ) -> Result<OpenResult<R>> {
        // Detect if this is an SFX archive (7z data not at offset 0)
        let sfx_offset = match detect_sfx(&mut reader)? {
            Some(sfx_info) => {
                // Seek to the archive start
                reader
                    .seek(SeekFrom::Start(sfx_info.archive_offset))
                    .map_err(Error::Io)?;
                sfx_info.archive_offset
            }
            None => 0,
        };

        // Read main header (also parses start header internally)
        // Use provided limits or fall back to defaults
        let limits = limits.unwrap_or_default();
        let (_start_header, header) =
            read_archive_header_with_offset(&mut reader, Some(limits), sfx_offset)?;

        // Build entries from files info
        let entries = entries::build_entries(&header);

        // Build archive info
        let info = entries::build_info(&header, &entries);

        Ok(OpenResult {
            reader,
            header,
            entries,
            info,
            sfx_offset,
        })
    }

    #[cfg(feature = "aes")]
    fn open_internal(
        reader: R,
        password: Option<Password>,
        limits: Option<ResourceLimits>,
    ) -> Result<Self> {
        let result = Self::open_common(reader, password.clone(), limits)?;
        Ok(Self {
            reader: result.reader,
            header: result.header,
            entries: result.entries,
            info: result.info,
            password,
            volume_info: None,
            sfx_offset: result.sfx_offset,
        })
    }

    #[cfg(not(feature = "aes"))]
    fn open_internal(
        reader: R,
        _password: Option<()>,
        limits: Option<ResourceLimits>,
    ) -> Result<Self> {
        let result = Self::open_common(reader, None, limits)?;
        Ok(Self {
            reader: result.reader,
            header: result.header,
            entries: result.entries,
            info: result.info,
            volume_info: None,
            sfx_offset: result.sfx_offset,
        })
    }
}
