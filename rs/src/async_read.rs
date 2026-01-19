//! Async archive reading API for 7z archives.
//!
//! This module provides the async API for reading 7z archives, including
//! listing entries, extracting files, and verifying integrity.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::async_read::{AsyncArchive, AsyncExtractOptions};
//!
//! #[tokio::main]
//! async fn main() -> zesven::Result<()> {
//!     // Open an archive
//!     let mut archive = AsyncArchive::open_path("archive.7z").await?;
//!
//!     // List entries
//!     for entry in archive.entries() {
//!         println!("{}: {} bytes", entry.path.as_str(), entry.size);
//!     }
//!
//!     // Extract all files
//!     archive.extract("output_dir", (), &AsyncExtractOptions::default()).await?;
//!     Ok(())
//! }
//! ```

use std::path::Path;

use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, BufReader};
use tokio_util::sync::CancellationToken;

use std::io::{Cursor, Read, Write};

use crate::async_options::{AsyncExtractOptions, AsyncTestOptions};
use crate::format::SIGNATURE_HEADER_SIZE;
use crate::format::header::StartHeader;
use crate::format::parser::{ArchiveHeader, read_archive_header};
use crate::format::streams::ResourceLimits;
use crate::read::{
    ArchiveInfo, Entry, EntrySelector, ExtractResult, OverwritePolicy, PathSafety, TestResult,
};
use crate::streaming::Crc32Sink;
use crate::{Error, READ_BUFFER_SIZE, Result, codec};

#[cfg(feature = "aes")]
use crate::Password;

/// Result of async archive opening (internal helper to avoid CFG duplication).
struct AsyncOpenResult<R> {
    reader: R,
    start_header: StartHeader,
    header: ArchiveHeader,
    entries: Vec<Entry>,
    info: ArchiveInfo,
    archive_data: Vec<u8>,
}

/// An async 7z archive reader.
///
/// This provides the same functionality as the sync `Archive` but with
/// async/await support for non-blocking I/O operations.
pub struct AsyncArchive<R> {
    #[allow(dead_code)] // Reader stored for potential future streaming operations
    reader: R,
    #[allow(dead_code)] // Header metadata preserved for archive info queries
    start_header: StartHeader,
    header: ArchiveHeader,
    entries: Vec<Entry>,
    info: ArchiveInfo,
    /// Raw archive data for extraction (we read it all during open)
    archive_data: Vec<u8>,
    #[cfg(feature = "aes")]
    #[allow(dead_code)] // Reserved for future encrypted extraction support
    password: Option<Password>,
}

impl AsyncArchive<BufReader<File>> {
    /// Opens an archive from a file path asynchronously.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or the archive is invalid.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let archive = AsyncArchive::open_path("archive.7z").await?;
    /// ```
    pub async fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref()).await.map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open(reader).await
    }

    /// Opens an encrypted archive from a file path asynchronously.
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
    pub async fn open_path_with_password(
        path: impl AsRef<Path>,
        password: impl Into<Password>,
    ) -> Result<Self> {
        let file = File::open(path.as_ref()).await.map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open_with_password(reader, password).await
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send> AsyncArchive<R> {
    /// Opens an archive from an async reader.
    ///
    /// # Arguments
    ///
    /// * `reader` - An async reader providing the archive data
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid or cannot be read.
    pub async fn open(reader: R) -> Result<Self> {
        Self::open_internal(reader, None).await
    }

    /// Opens an encrypted archive from an async reader.
    ///
    /// # Arguments
    ///
    /// * `reader` - An async reader providing the archive data
    /// * `password` - Password for decryption
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is invalid, cannot be read,
    /// or the password is incorrect.
    #[cfg(feature = "aes")]
    pub async fn open_with_password(reader: R, password: impl Into<Password>) -> Result<Self> {
        Self::open_internal(reader, Some(password.into())).await
    }

    /// Common async archive opening logic shared between AES and non-AES builds.
    async fn open_common(mut reader: R) -> Result<AsyncOpenResult<R>> {
        // Read the archive data into memory for sync parsing
        // This is necessary because the format parsing code is synchronous
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).await.map_err(Error::Io)?;

        // Clone buffer for later extraction use
        let archive_data = buffer.clone();

        // Parse using sync code in a blocking task
        let (start_header, header, entries, info) = tokio::task::spawn_blocking(move || {
            let mut cursor = std::io::Cursor::new(buffer);
            let limits = ResourceLimits::default();
            let (start_header, header) = read_archive_header(&mut cursor, Some(limits))?;
            let entries = crate::read::entries::build_entries(&header);
            let info = crate::read::entries::build_info(&header, &entries);
            Ok::<_, Error>((start_header, header, entries, info))
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))??;

        Ok(AsyncOpenResult {
            reader,
            start_header,
            header,
            entries,
            info,
            archive_data,
        })
    }

    #[cfg(feature = "aes")]
    async fn open_internal(reader: R, password: Option<Password>) -> Result<Self> {
        let result = Self::open_common(reader).await?;
        Ok(Self {
            reader: result.reader,
            start_header: result.start_header,
            header: result.header,
            entries: result.entries,
            info: result.info,
            archive_data: result.archive_data,
            password,
        })
    }

    #[cfg(not(feature = "aes"))]
    async fn open_internal(reader: R, _password: Option<()>) -> Result<Self> {
        let result = Self::open_common(reader).await?;
        Ok(Self {
            reader: result.reader,
            start_header: result.start_header,
            header: result.header,
            entries: result.entries,
            info: result.info,
            archive_data: result.archive_data,
        })
    }

    /// Returns information about the archive.
    pub fn info(&self) -> &ArchiveInfo {
        &self.info
    }

    /// Returns all entries in the archive.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Returns the number of entries in the archive.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the archive has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finds an entry by path.
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path.as_str() == path)
    }

    /// Tests the archive for integrity asynchronously.
    ///
    /// This decompresses all selected entries and verifies their CRC checksums
    /// without writing any files.
    ///
    /// # Arguments
    ///
    /// * `selector` - Selects which entries to test
    /// * `options` - Test options including cancellation token
    ///
    /// # Returns
    ///
    /// A TestResult containing the results of the integrity check.
    pub async fn test(
        &mut self,
        selector: impl EntrySelector,
        options: &AsyncTestOptions,
    ) -> Result<TestResult> {
        // Check cancellation
        if options.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let mut result = TestResult {
            entries_tested: 0,
            entries_passed: 0,
            entries_failed: 0,
            failures: Vec::new(),
        };

        // Collect entries to test
        let entries_to_test: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selector.select(e) && !e.is_directory)
            .map(|(idx, e)| (idx, e.clone()))
            .collect();

        for (_entry_idx, entry) in entries_to_test {
            // Check cancellation before each entry
            if options.is_cancelled() {
                return Err(Error::Cancelled);
            }

            result.entries_tested += 1;

            // Test the entry
            let archive_data = self.archive_data.clone();
            let header = self.header.clone();
            let entry_clone = entry.clone();

            let test_result = tokio::task::spawn_blocking(move || {
                Self::test_entry_sync(&archive_data, &header, &entry_clone)
            })
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;

            match test_result {
                Ok(()) => {
                    result.entries_passed += 1;
                }
                Err(e) => {
                    result.entries_failed += 1;
                    result
                        .failures
                        .push((entry.path.as_str().to_string(), e.to_string()));
                }
            }
        }

        Ok(result)
    }

    /// Tests a single entry for integrity (sync helper for spawn_blocking).
    fn test_entry_sync(archive_data: &[u8], header: &ArchiveHeader, entry: &Entry) -> Result<()> {
        // Empty files and directories always pass
        let folder_idx = match entry.folder_index {
            Some(idx) => idx,
            None => return Ok(()),
        };

        // Get folder and pack info
        let unpack_info = header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        let folder = unpack_info.folders.get(folder_idx).ok_or_else(|| {
            Error::InvalidFormat(format!("folder index {} out of range", folder_idx))
        })?;

        let pack_info = header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        // Calculate pack position
        let mut pack_pos = SIGNATURE_HEADER_SIZE + pack_info.pack_pos;
        for i in 0..folder_idx {
            if i < pack_info.pack_sizes.len() {
                pack_pos += pack_info.pack_sizes[i];
            }
        }

        // Get pack size
        let pack_size = pack_info
            .pack_sizes
            .get(folder_idx)
            .copied()
            .ok_or_else(|| Error::InvalidFormat("missing pack size".into()))?;

        // Read packed data
        let pack_end = pack_pos as usize + pack_size as usize;
        if pack_end > archive_data.len() {
            return Err(Error::InvalidFormat(
                "pack data extends beyond archive".into(),
            ));
        }
        let packed_data = archive_data[pack_pos as usize..pack_end].to_vec();

        // Decompress to CRC sink (no file I/O)
        let mut sink = Crc32Sink::new();

        // Check if this is a solid block
        let is_solid_block = header
            .substreams_info
            .as_ref()
            .and_then(|ss| ss.num_unpack_streams_in_folders.get(folder_idx))
            .map(|&count| count > 1)
            .unwrap_or(false);

        if is_solid_block {
            Self::extract_from_solid_block_sync(
                packed_data,
                folder,
                header,
                folder_idx,
                entry.stream_index.unwrap_or(0),
                &mut sink,
            )?;
        } else {
            Self::extract_non_solid_sync(packed_data, folder, entry.size, &mut sink)?;
        }

        // Verify CRC
        if let Some(expected_crc) = entry.crc32 {
            let actual_crc = sink.finalize();
            if actual_crc != expected_crc {
                return Err(Error::CrcMismatch {
                    entry_index: entry.index,
                    entry_name: Some(entry.path.as_str().to_string()),
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
        }

        Ok(())
    }

    /// Extracts entries to a destination directory asynchronously.
    ///
    /// # Arguments
    ///
    /// * `dest` - Destination directory
    /// * `selector` - Selects which entries to extract
    /// * `options` - Extraction options including cancellation token
    ///
    /// # Returns
    ///
    /// An ExtractResult containing extraction statistics.
    pub async fn extract(
        &mut self,
        dest: impl AsRef<Path>,
        selector: impl EntrySelector,
        options: &AsyncExtractOptions,
    ) -> Result<ExtractResult> {
        let dest = dest.as_ref().to_path_buf();
        let mut result = ExtractResult::default();

        // Create destination directory if it doesn't exist
        if !dest.exists() {
            tokio::fs::create_dir_all(&dest).await.map_err(Error::Io)?;
        }

        // Collect entries to extract
        let entries_to_extract: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selector.select(e))
            .map(|(idx, e)| (idx, e.clone()))
            .collect();

        for (idx, entry) in entries_to_extract {
            // Check cancellation before each entry
            if options.is_cancelled() {
                return Err(Error::Cancelled);
            }

            // Report progress if callback is set
            if let Some(progress) = &options.progress {
                progress
                    .on_entry_start(entry.path.as_str(), entry.size)
                    .await;
            }

            if entry.is_directory {
                // Create directory
                let dir_path = dest.join(entry.path.as_str());
                match tokio::fs::create_dir_all(&dir_path).await {
                    Ok(_) => {
                        result.entries_extracted += 1;
                        if let Some(progress) = &options.progress {
                            progress.on_entry_complete(entry.path.as_str(), true).await;
                        }
                    }
                    Err(e) => {
                        result.entries_failed += 1;
                        result
                            .failures
                            .push((entry.path.as_str().to_string(), e.to_string()));
                        if let Some(progress) = &options.progress {
                            progress.on_entry_complete(entry.path.as_str(), false).await;
                        }
                    }
                }
            } else {
                // Extract file
                let entry_path = entry.path.as_str().to_string();
                match self.extract_entry_async(idx, &dest, options).await {
                    Ok(bytes) => {
                        result.entries_extracted += 1;
                        result.bytes_extracted += bytes;
                        if let Some(progress) = &options.progress {
                            progress.on_entry_complete(&entry_path, true).await;
                        }
                    }
                    Err(e) => {
                        if matches!(options.overwrite, OverwritePolicy::Skip) {
                            result.entries_skipped += 1;
                        } else {
                            result.entries_failed += 1;
                            result.failures.push((entry_path.clone(), e.to_string()));
                        }
                        if let Some(progress) = &options.progress {
                            progress.on_entry_complete(&entry_path, false).await;
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Extracts entries with a cancellation token.
    ///
    /// This is a convenience method that wraps extraction with explicit
    /// cancellation support.
    pub async fn extract_with_cancellation(
        &mut self,
        dest: impl AsRef<Path>,
        selector: impl EntrySelector,
        options: &AsyncExtractOptions,
        cancel_token: CancellationToken,
    ) -> Result<ExtractResult> {
        tokio::select! {
            result = self.extract(dest, selector, options) => result,
            _ = cancel_token.cancelled() => {
                Err(Error::Cancelled)
            }
        }
    }

    async fn extract_entry_async(
        &mut self,
        entry_idx: usize,
        dest: &Path,
        options: &AsyncExtractOptions,
    ) -> Result<u64> {
        let entry_path_str = self.entries[entry_idx].path.as_str().to_string();
        let entry_size = self.entries[entry_idx].size;
        let entry_crc = self.entries[entry_idx].crc32;
        let folder_index = self.entries[entry_idx].folder_index;
        let stream_index = self.entries[entry_idx].stream_index;

        // Validate path safety
        let safe_path =
            Self::validate_path_async(entry_idx, &entry_path_str, dest, &options.path_safety)?;

        // Create parent directories asynchronously
        if let Some(parent) = safe_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(Error::Io)?;
        }

        // Check overwrite policy
        if safe_path.exists() {
            match options.overwrite {
                OverwritePolicy::Error => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("file already exists: {}", safe_path.display()),
                    )));
                }
                OverwritePolicy::Skip => {
                    return Ok(0);
                }
                OverwritePolicy::Overwrite => {}
            }
        }

        // Handle empty files (no folder assignment)
        let folder_idx = match folder_index {
            Some(idx) => idx,
            None => {
                // Empty file - just create it
                tokio::fs::File::create(&safe_path)
                    .await
                    .map_err(Error::Io)?;
                return Ok(0);
            }
        };

        // Extract the file content using blocking task for CPU-bound decompression
        let archive_data = self.archive_data.clone();
        let header = self.header.clone();
        let safe_path_clone = safe_path.clone();

        let bytes_written = tokio::task::spawn_blocking(move || {
            Self::extract_entry_sync(
                &archive_data,
                &header,
                folder_idx,
                stream_index,
                entry_size,
                &safe_path_clone,
            )
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))??;

        // Verify CRC if available
        if let Some(expected_crc) = entry_crc {
            let safe_path_for_crc = safe_path.clone();
            let actual_crc =
                tokio::task::spawn_blocking(move || Self::calculate_file_crc(&safe_path_for_crc))
                    .await
                    .map_err(|e| Error::Io(std::io::Error::other(e)))??;

            if actual_crc != expected_crc {
                // Delete corrupted file
                if let Err(e) = tokio::fs::remove_file(&safe_path).await {
                    log::warn!(
                        "Failed to clean up corrupted file '{}': {}",
                        safe_path.display(),
                        e
                    );
                }
                return Err(Error::CrcMismatch {
                    entry_index: entry_idx,
                    entry_name: Some(entry_path_str.clone()),
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
        }

        Ok(bytes_written)
    }

    /// Synchronous extraction helper for use in spawn_blocking.
    fn extract_entry_sync(
        archive_data: &[u8],
        header: &ArchiveHeader,
        folder_idx: usize,
        stream_index: Option<usize>,
        entry_size: u64,
        output_path: &Path,
    ) -> Result<u64> {
        // Get folder and pack info
        let unpack_info = header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        let folder = unpack_info.folders.get(folder_idx).ok_or_else(|| {
            Error::InvalidFormat(format!("folder index {} out of range", folder_idx))
        })?;

        let pack_info = header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        // Calculate pack position (32 bytes for signature header + pack_pos + sum of previous pack sizes)
        let mut pack_pos = SIGNATURE_HEADER_SIZE + pack_info.pack_pos;
        for i in 0..folder_idx {
            if i < pack_info.pack_sizes.len() {
                pack_pos += pack_info.pack_sizes[i];
            }
        }

        // Get pack size for this folder
        let pack_size = pack_info
            .pack_sizes
            .get(folder_idx)
            .copied()
            .ok_or_else(|| Error::InvalidFormat("missing pack size".into()))?;

        // Read packed data from archive buffer
        let pack_end = pack_pos as usize + pack_size as usize;
        if pack_end > archive_data.len() {
            return Err(Error::InvalidFormat(
                "pack data extends beyond archive".into(),
            ));
        }
        let packed_data = archive_data[pack_pos as usize..pack_end].to_vec();

        // Create output file
        let mut file = std::fs::File::create(output_path).map_err(Error::Io)?;

        // Check if this is a solid block
        let is_solid_block = header
            .substreams_info
            .as_ref()
            .and_then(|ss| ss.num_unpack_streams_in_folders.get(folder_idx))
            .map(|&count| count > 1)
            .unwrap_or(false);

        // Decompress and write
        let bytes_written = if is_solid_block {
            Self::extract_from_solid_block_sync(
                packed_data,
                folder,
                header,
                folder_idx,
                stream_index.unwrap_or(0),
                &mut file,
            )?
        } else {
            Self::extract_non_solid_sync(packed_data, folder, entry_size, &mut file)?
        };

        Ok(bytes_written)
    }

    /// Extracts a non-solid entry directly.
    fn extract_non_solid_sync(
        packed_data: Vec<u8>,
        folder: &crate::format::streams::Folder,
        expected_size: u64,
        output: &mut impl Write,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let coder = &folder.coders[0];
        let uncompressed_size = folder.final_unpack_size().unwrap_or(expected_size);

        let cursor = Cursor::new(packed_data);
        let mut decoder = codec::build_decoder(cursor, coder, uncompressed_size)?;

        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = decoder.read(&mut buf).map_err(Error::Io)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
        }

        Ok(total_written)
    }

    /// Extracts an entry from a solid block.
    fn extract_from_solid_block_sync(
        packed_data: Vec<u8>,
        folder: &crate::format::streams::Folder,
        header: &ArchiveHeader,
        folder_idx: usize,
        stream_index: usize,
        output: &mut impl Write,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        // Get entry sizes within this solid block
        let entry_sizes = Self::get_solid_block_entry_sizes_sync(header, folder_idx)?;

        if stream_index >= entry_sizes.len() {
            return Err(Error::InvalidFormat(format!(
                "stream index {} out of range for solid block",
                stream_index
            )));
        }

        let coder = &folder.coders[0];
        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);

        let cursor = Cursor::new(packed_data);
        let mut decoder = codec::build_decoder(cursor, coder, uncompressed_size)?;

        // Skip entries before the target
        for &skip_size in entry_sizes.iter().take(stream_index) {
            let mut remaining = skip_size;
            let mut buf = [0u8; READ_BUFFER_SIZE];
            while remaining > 0 {
                let to_read = buf.len().min(remaining as usize);
                let n = decoder.read(&mut buf[..to_read]).map_err(Error::Io)?;
                if n == 0 {
                    break;
                }
                remaining -= n as u64;
            }
        }

        // Read the target entry
        let target_size = entry_sizes[stream_index];
        let mut remaining = target_size;
        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        while remaining > 0 {
            let to_read = buf.len().min(remaining as usize);
            let n = decoder.read(&mut buf[..to_read]).map_err(Error::Io)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
            remaining -= n as u64;
        }

        Ok(total_written)
    }

    /// Gets entry sizes for a solid block.
    fn get_solid_block_entry_sizes_sync(
        header: &ArchiveHeader,
        folder_idx: usize,
    ) -> Result<Vec<u64>> {
        let substreams = header
            .substreams_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing substreams info".into()))?;

        let num_streams = *substreams
            .num_unpack_streams_in_folders
            .get(folder_idx)
            .ok_or_else(|| Error::InvalidFormat("folder index out of range".into()))?
            as usize;

        // Calculate the starting stream index for this folder
        let stream_offset: usize = substreams
            .num_unpack_streams_in_folders
            .iter()
            .take(folder_idx)
            .map(|&n| n as usize)
            .sum();

        // Get sizes from substreams info
        let sizes: Vec<u64> = (0..num_streams)
            .map(|i| {
                substreams
                    .unpack_sizes
                    .get(stream_offset + i)
                    .copied()
                    .unwrap_or(0)
            })
            .collect();

        Ok(sizes)
    }

    /// Calculate CRC32 of a file.
    fn calculate_file_crc(path: &Path) -> Result<u32> {
        let mut file = std::fs::File::open(path).map_err(Error::Io)?;
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

    fn validate_path_async(
        entry_idx: usize,
        entry_path: &str,
        dest: &Path,
        policy: &PathSafety,
    ) -> Result<std::path::PathBuf> {
        match policy {
            PathSafety::Disabled => Ok(dest.join(entry_path)),
            PathSafety::Relaxed | PathSafety::Strict => {
                let full_path = dest.join(entry_path);
                let canonical_dest =
                    std::fs::canonicalize(dest).unwrap_or_else(|_| dest.to_path_buf());

                if *policy == PathSafety::Strict {
                    for component in std::path::Path::new(entry_path).components() {
                        if let std::path::Component::Normal(name) = component {
                            if let Some(name_str) = name.to_str() {
                                if name_str.starts_with('/') {
                                    return Err(Error::PathTraversal {
                                        entry_index: entry_idx,
                                        path: entry_path.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }

                if !full_path.starts_with(&canonical_dest) && !full_path.starts_with(dest) {
                    return Err(Error::PathTraversal {
                        entry_index: entry_idx,
                        path: entry_path.to_string(),
                    });
                }

                Ok(full_path)
            }
        }
    }
}

// Unit tests for async_read module are consolidated in tests/async_tests.rs
// to avoid duplication between unit and integration test coverage.
