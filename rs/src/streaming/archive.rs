//! High-level streaming archive reader.
//!
//! This module provides [`StreamingArchive`], the main entry point for
//! streaming decompression of 7z archives.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use crate::format::header::StartHeader;
use crate::format::parser::{ArchiveHeader, read_archive_header};
use crate::format::streams::ResourceLimits;
use crate::read::{Entry, ExtractOptions};
use crate::{ArchivePath, Error, Result};

#[cfg(feature = "aes")]
use crate::Password;

use super::config::StreamingConfig;
use super::iterator::EntryIterator;
use super::memory::MemoryTracker;
use super::parallel::{
    ParallelExtractionOptions, ParallelExtractionResult, ParallelFolderExtractor,
};
use super::pool::{DecoderPool, PoolStats};

/// High-level streaming archive reader.
///
/// This is the main entry point for streaming decompression of 7z archives.
/// It provides memory-efficient access to archive contents through iterator
/// patterns and configurable memory bounds.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::{StreamingArchive, StreamingConfig};
///
/// // Open with default configuration
/// let mut archive = StreamingArchive::open(file, password)?;
///
/// // Check archive properties
/// if archive.is_solid() {
///     println!("Solid archive - sequential access only");
/// }
///
/// // Stream entries one at a time
/// for entry_result in archive.entries() {
///     let mut entry = entry_result?;
///     if should_extract(entry.entry()) {
///         entry.extract_to(&mut output)?;
///     } else {
///         entry.skip()?;
///     }
/// }
/// ```
///
/// # Memory Efficiency
///
/// The streaming API processes entries one at a time without loading
/// the entire archive into memory. Use [`StreamingConfig`] to control
/// buffer sizes and memory limits.
///
/// # Solid vs Non-Solid Archives
///
/// - **Non-solid archives**: Each file is independently compressed.
///   Random access is available via [`super::RandomAccessReader`].
///
/// - **Solid archives**: Multiple files compressed together for better
///   compression. Must use sequential access via [`entries()`](Self::entries).
pub struct StreamingArchive<R> {
    /// Source reader
    reader: R,
    /// Start header
    #[allow(dead_code)] // Header metadata preserved for archive info
    start_header: StartHeader,
    /// Archive header
    header: ArchiveHeader,
    /// Entry list
    entries: Vec<Entry>,
    /// Entries that were skipped during parsing (e.g., invalid paths)
    skipped_entries: Vec<super::SkippedEntry>,
    /// Password for encrypted archives
    #[cfg(feature = "aes")]
    password: Password,
    /// Configuration
    config: StreamingConfig,
    /// Memory tracker
    memory_tracker: MemoryTracker,
    /// Decoder pool for solid archive optimization
    decoder_pool: Option<DecoderPool>,
    /// Whether the archive is solid
    is_solid: bool,
}

impl StreamingArchive<BufReader<File>> {
    /// Opens an archive from a file path.
    #[cfg(feature = "aes")]
    pub fn open_path(path: impl AsRef<Path>, password: impl Into<Password>) -> Result<Self> {
        Self::open_path_with_config(path, password, StreamingConfig::default())
    }

    /// Opens an archive from a file path (without password).
    #[cfg(not(feature = "aes"))]
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_path_with_config(path, StreamingConfig::default())
    }

    /// Opens an archive from a file path with custom configuration.
    #[cfg(feature = "aes")]
    pub fn open_path_with_config(
        path: impl AsRef<Path>,
        password: impl Into<Password>,
        config: StreamingConfig,
    ) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open_with_config(reader, password, config)
    }

    /// Opens an archive from a file path with custom configuration (without password).
    #[cfg(not(feature = "aes"))]
    pub fn open_path_with_config(path: impl AsRef<Path>, config: StreamingConfig) -> Result<Self> {
        let file = File::open(path.as_ref()).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Self::open_with_config(reader, config)
    }
}

impl<R: Read + Seek + Send> StreamingArchive<R> {
    /// Opens an archive from a reader.
    #[cfg(feature = "aes")]
    pub fn open(reader: R, password: impl Into<Password>) -> Result<Self> {
        Self::open_with_config(reader, password, StreamingConfig::default())
    }

    /// Opens an archive from a reader (without password).
    #[cfg(not(feature = "aes"))]
    pub fn open(reader: R) -> Result<Self> {
        Self::open_with_config(reader, StreamingConfig::default())
    }

    /// Opens an archive with custom configuration.
    #[cfg(feature = "aes")]
    pub fn open_with_config(
        mut reader: R,
        password: impl Into<Password>,
        config: StreamingConfig,
    ) -> Result<Self> {
        config.validate()?;

        let limits = ResourceLimits::default()
            .max_entries(config.max_entries)
            .ratio_limit(Some(crate::format::streams::RatioLimit::new(
                config.max_compression_ratio,
            )));

        let (start_header, header) = read_archive_header(&mut reader, Some(limits))?;
        let (entries, skipped_entries) = Self::build_entries(&header);
        let is_solid = super::check_is_solid(&header);
        let memory_tracker = MemoryTracker::new(config.max_memory_buffer);

        // Initialize decoder pool based on configuration
        let decoder_pool = Self::create_decoder_pool(&config, is_solid);

        Ok(Self {
            reader,
            start_header,
            header,
            entries,
            skipped_entries,
            password: password.into(),
            config,
            memory_tracker,
            decoder_pool,
            is_solid,
        })
    }

    /// Opens an archive with custom configuration (without password).
    #[cfg(not(feature = "aes"))]
    pub fn open_with_config(mut reader: R, config: StreamingConfig) -> Result<Self> {
        config.validate()?;

        let limits = ResourceLimits::default()
            .max_entries(config.max_entries)
            .ratio_limit(Some(crate::format::streams::RatioLimit::new(
                config.max_compression_ratio,
            )));

        let (start_header, header) = read_archive_header(&mut reader, Some(limits))?;
        let (entries, skipped_entries) = Self::build_entries(&header);
        let is_solid = super::check_is_solid(&header);
        let memory_tracker = MemoryTracker::new(config.max_memory_buffer);

        // Initialize decoder pool based on configuration
        let decoder_pool = Self::create_decoder_pool(&config, is_solid);

        Ok(Self {
            reader,
            start_header,
            header,
            entries,
            skipped_entries,
            config,
            memory_tracker,
            decoder_pool,
            is_solid,
        })
    }

    /// Creates a decoder pool based on configuration and archive type.
    fn create_decoder_pool(config: &StreamingConfig, is_solid: bool) -> Option<DecoderPool> {
        // Only create pool for solid archives where it provides benefit
        if !is_solid {
            return None;
        }

        let capacity = config.resolved_decoder_pool_capacity();
        if capacity == 0 {
            return None;
        }

        Some(DecoderPool::new(capacity))
    }

    fn build_entries(header: &ArchiveHeader) -> (Vec<Entry>, Vec<super::SkippedEntry>) {
        let files_info = match &header.files_info {
            Some(info) => info,
            None => return (Vec::new(), Vec::new()),
        };

        let substreams = header.substreams_info.as_ref();

        let mut entries = Vec::with_capacity(files_info.entries.len());
        let mut skipped_entries = Vec::new();
        let mut stream_idx: usize = 0;
        let mut folder_idx: usize = 0;

        for (idx, archive_entry) in files_info.entries.iter().enumerate() {
            let path = match ArchivePath::new(&archive_entry.name) {
                Ok(p) => p,
                Err(e) => {
                    skipped_entries.push(super::SkippedEntry {
                        index: idx,
                        raw_path: Some(archive_entry.name.as_bytes().to_vec()),
                        reason: super::SkipReason::InvalidPath(e.to_string()),
                    });
                    continue;
                }
            };

            let (folder_index, stream_index) = if archive_entry.is_directory {
                (None, None)
            } else {
                let fi = folder_idx;
                let si = stream_idx;

                if let Some(ss) = substreams {
                    if folder_idx < ss.num_unpack_streams_in_folders.len() {
                        stream_idx += 1;
                        let num_streams = ss.num_unpack_streams_in_folders[folder_idx] as usize;
                        if stream_idx >= num_streams {
                            stream_idx = 0;
                            folder_idx += 1;
                        }
                    }
                } else {
                    folder_idx += 1;
                }

                (Some(fi), Some(si))
            };

            let is_encrypted = folder_index
                .and_then(|fi| Self::check_folder_encryption(header, fi))
                .unwrap_or(false);

            // Detect symlinks from attributes
            let is_symlink = !archive_entry.is_directory
                && archive_entry.attributes.is_some_and(|attrs| {
                    // Unix symlink mode (S_IFLNK = 0o120000) in high 16 bits
                    let unix_mode = (attrs >> 16) & 0xFFFF;
                    let is_unix_symlink = unix_mode != 0 && (unix_mode & 0o170000) == 0o120000;
                    // Windows REPARSE_POINT (0x400) in low 16 bits
                    let is_windows_symlink = (attrs & 0x400) != 0;
                    is_unix_symlink || is_windows_symlink
                });

            entries.push(Entry {
                path,
                is_directory: archive_entry.is_directory,
                size: archive_entry.size,
                crc32: archive_entry.crc,
                crc64: None,
                modification_time: archive_entry.mtime,
                creation_time: archive_entry.ctime,
                access_time: archive_entry.atime,
                attributes: archive_entry.attributes,
                is_encrypted,
                is_symlink,
                is_anti: archive_entry.is_anti,
                ownership: None,
                index: idx,
                folder_index,
                stream_index,
            });
        }

        (entries, skipped_entries)
    }

    fn check_folder_encryption(header: &ArchiveHeader, folder_index: usize) -> Option<bool> {
        let unpack_info = header.unpack_info.as_ref()?;
        let folder = unpack_info.folders.get(folder_index)?;
        Some(
            folder
                .coders
                .iter()
                .any(|coder| coder.method_id.as_slice() == crate::codec::method::AES),
        )
    }

    /// Returns the archive header.
    pub fn header(&self) -> &ArchiveHeader {
        &self.header
    }

    /// Returns the configuration.
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }

    /// Returns the memory tracker.
    pub fn memory_tracker(&self) -> &MemoryTracker {
        &self.memory_tracker
    }

    /// Returns true if this is a solid archive.
    ///
    /// Solid archives compress multiple files together for better compression
    /// but require sequential decompression.
    pub fn is_solid(&self) -> bool {
        self.is_solid
    }

    /// Returns the entries in the archive.
    pub fn entries_list(&self) -> &[Entry] {
        &self.entries
    }

    /// Returns the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns entries that were skipped during archive parsing.
    ///
    /// Entries may be skipped if they have invalid paths (e.g., containing
    /// null bytes or other invalid characters). Use this method to check
    /// if any entries were silently skipped during opening.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let archive = StreamingArchive::open_path("archive.7z", password)?;
    /// if archive.has_skipped_entries() {
    ///     eprintln!("Warning: {} entries were skipped", archive.skipped_entries().len());
    ///     for skipped in archive.skipped_entries() {
    ///         eprintln!("  Entry {}: {}", skipped.index, skipped.reason);
    ///     }
    /// }
    /// ```
    pub fn skipped_entries(&self) -> &[super::SkippedEntry] {
        &self.skipped_entries
    }

    /// Returns true if any entries were skipped during archive parsing.
    ///
    /// This is a convenience method equivalent to `!archive.skipped_entries().is_empty()`.
    pub fn has_skipped_entries(&self) -> bool {
        !self.skipped_entries.is_empty()
    }

    /// Finds an entry by path.
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path.as_str() == path)
    }

    /// Returns the total uncompressed size of all entries.
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// Returns the number of folders (compression blocks).
    pub fn num_folders(&self) -> usize {
        self.header
            .unpack_info
            .as_ref()
            .map(|ui| ui.folders.len())
            .unwrap_or(0)
    }

    /// Returns true if a decoder pool is active for this archive.
    ///
    /// A pool is only created for solid archives when pooling is enabled
    /// in the configuration.
    pub fn has_decoder_pool(&self) -> bool {
        self.decoder_pool.is_some()
    }

    /// Returns the decoder pool statistics, if a pool is active.
    ///
    /// Pool statistics can help understand the effectiveness of caching
    /// for solid archive access patterns.
    pub fn pool_stats(&self) -> Option<&PoolStats> {
        self.decoder_pool.as_ref().map(|p| p.stats())
    }

    /// Returns the decoder pool capacity, if a pool is active.
    pub fn pool_capacity(&self) -> Option<usize> {
        self.decoder_pool.as_ref().map(|p| p.capacity())
    }

    /// Resets the decoder pool statistics.
    pub fn reset_pool_stats(&mut self) {
        if let Some(pool) = &mut self.decoder_pool {
            pool.reset_stats();
        }
    }

    /// Clears all cached decoders in the pool.
    ///
    /// This frees memory but may reduce performance for subsequent
    /// out-of-order access to solid blocks.
    pub fn clear_decoder_pool(&mut self) {
        if let Some(pool) = &mut self.decoder_pool {
            pool.clear();
        }
    }

    /// Returns an iterator over entries with streaming readers.
    ///
    /// This is the primary API for extracting entries with bounded memory.
    /// Entries are yielded one at a time and must be processed or skipped
    /// before moving to the next.
    ///
    /// # Solid Archives
    ///
    /// For solid archives, entries must be processed in order. Skipping
    /// an entry still decompresses it but discards the data.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// for entry_result in archive.entries() {
    ///     let mut entry = entry_result?;
    ///     println!("Entry: {}", entry.name());
    ///
    ///     if entry.entry().is_directory {
    ///         continue;
    ///     }
    ///
    ///     // Either extract...
    ///     entry.extract_to(&mut output)?;
    ///     // ...or skip
    ///     // entry.skip()?;
    /// }
    /// ```
    #[cfg(feature = "aes")]
    pub fn entries(&mut self) -> Result<EntryIterator<'_, R>> {
        EntryIterator::new(
            &self.header,
            &self.entries,
            &mut self.reader,
            &self.password,
            self.config.clone(),
        )
    }

    /// Returns an iterator over entries with streaming readers (without AES).
    #[cfg(not(feature = "aes"))]
    pub fn entries(&mut self) -> Result<EntryIterator<'_, R>> {
        EntryIterator::new(
            &self.header,
            &self.entries,
            &mut self.reader,
            self.config.clone(),
        )
    }

    /// Extracts all entries to a directory with bounded memory.
    ///
    /// This method extracts all entries using the streaming API,
    /// suitable for large archives that shouldn't be fully loaded
    /// into memory.
    pub fn extract_all(
        &mut self,
        dest: impl AsRef<Path>,
        _options: &ExtractOptions,
    ) -> Result<ExtractAllResult> {
        let dest = dest.as_ref();

        if !dest.exists() {
            std::fs::create_dir_all(dest).map_err(Error::Io)?;
        }

        let mut result = ExtractAllResult::default();
        let mut iter = self.entries()?;

        while let Some(entry_result) = iter.next() {
            match entry_result {
                Ok(entry) => {
                    let entry_name = entry.name().to_string();
                    let entry_path = dest.join(&entry_name);

                    if entry.is_directory() {
                        if let Err(e) = std::fs::create_dir_all(&entry_path) {
                            result.entries_failed += 1;
                            result.failures.push((entry_name, e.to_string()));
                        } else {
                            result.entries_extracted += 1;
                        }
                        continue;
                    }

                    // Create parent directories
                    if let Some(parent) = entry_path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            result.entries_failed += 1;
                            result.failures.push((entry_name, e.to_string()));
                            continue;
                        }
                    }

                    // Extract to file using iterator's extraction method
                    match std::fs::File::create(&entry_path) {
                        Ok(mut file) => match iter.extract_current_to(&mut file) {
                            Ok(bytes) => {
                                result.entries_extracted += 1;
                                result.bytes_extracted += bytes;
                            }
                            Err(e) => {
                                result.entries_failed += 1;
                                result.failures.push((entry_name, e.to_string()));
                            }
                        },
                        Err(e) => {
                            result.entries_failed += 1;
                            result.failures.push((entry_name, e.to_string()));
                        }
                    }
                }
                Err(e) => {
                    result.entries_failed += 1;
                    result.failures.push(("unknown".to_string(), e.to_string()));
                }
            }
        }

        Ok(result)
    }

    /// Extracts entries to custom sinks via a factory function.
    ///
    /// This method allows extracting entries to arbitrary Write
    /// implementations, such as network streams or in-memory buffers.
    ///
    /// # Arguments
    ///
    /// * `sink_factory` - Function that returns a Write sink for each entry,
    ///   or None to skip the entry.
    pub fn extract_all_to_sinks<W, F>(&mut self, mut sink_factory: F) -> Result<ExtractAllResult>
    where
        W: std::io::Write,
        F: FnMut(&Entry) -> Option<W>,
    {
        let mut result = ExtractAllResult::default();
        let mut iter = self.entries()?;

        while let Some(entry_result) = iter.next() {
            match entry_result {
                Ok(entry) => {
                    if entry.is_directory() {
                        result.entries_extracted += 1;
                        continue;
                    }

                    let entry_name = entry.name().to_string();
                    match sink_factory(entry.entry()) {
                        Some(mut sink) => match iter.extract_current_to(&mut sink) {
                            Ok(bytes) => {
                                result.entries_extracted += 1;
                                result.bytes_extracted += bytes;
                            }
                            Err(e) => {
                                result.entries_failed += 1;
                                result.failures.push((entry_name, e.to_string()));
                            }
                        },
                        None => {
                            // Skip this entry (iterator will skip remaining bytes on next())
                            result.entries_skipped += 1;
                        }
                    }
                }
                Err(e) => {
                    result.entries_failed += 1;
                    result.failures.push(("unknown".to_string(), e.to_string()));
                }
            }
        }

        Ok(result)
    }

    /// Extracts all entries to a directory using parallel decompression.
    ///
    /// This method enables parallel extraction for non-solid archives, where
    /// each file is compressed independently. This can provide significant
    /// speedup on multi-core systems.
    ///
    /// # Important
    ///
    /// - **Non-solid archives**: Extracted in parallel (2-4x speedup on 4+ cores)
    /// - **Solid archives**: Returns an error; use [`extract_all`](Self::extract_all) instead
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::streaming::{StreamingArchive, ParallelExtractionOptions};
    /// use zesven::read::Threads;
    ///
    /// let mut archive = StreamingArchive::open_path("archive.7z", "")?;
    ///
    /// if !archive.is_solid() {
    ///     let options = ParallelExtractionOptions::new()
    ///         .threads(Threads::count_or_single(4))
    ///         .verify_crc(true);
    ///
    ///     let result = archive.extract_all_parallel("/output/dir", &options)?;
    ///     println!("Extracted {} files using {} threads",
    ///              result.entries_extracted, result.threads_used);
    /// } else {
    ///     // Fall back to sequential extraction for solid archives
    ///     archive.extract_all("/output/dir", &Default::default())?;
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedFeature`] if called on a solid archive.
    pub fn extract_all_parallel(
        &mut self,
        dest: impl AsRef<Path>,
        options: &ParallelExtractionOptions,
    ) -> Result<ParallelExtractionResult> {
        if self.is_solid {
            return Err(Error::UnsupportedFeature {
                feature: "parallel extraction for solid archives",
            });
        }

        let extractor = ParallelFolderExtractor::new(&self.header, &self.entries, options.clone());

        extractor.extract_to_directory(&mut self.reader, dest)
    }

    /// Checks if parallel extraction is available for this archive.
    ///
    /// Returns `true` for non-solid archives where files can be extracted
    /// in parallel, `false` for solid archives where sequential extraction
    /// is required.
    pub fn supports_parallel_extraction(&self) -> bool {
        !self.is_solid
    }
}

/// Result of extracting all entries.
#[derive(Debug, Default)]
pub struct ExtractAllResult {
    /// Number of entries successfully extracted.
    pub entries_extracted: usize,
    /// Number of entries skipped.
    pub entries_skipped: usize,
    /// Number of entries that failed.
    pub entries_failed: usize,
    /// Total bytes extracted.
    pub bytes_extracted: u64,
    /// List of failures (entry name, error message).
    pub failures: Vec<(String, String)>,
}

impl ExtractAllResult {
    /// Returns true if all entries were successfully extracted.
    pub fn is_success(&self) -> bool {
        self.entries_failed == 0
    }

    /// Returns the total number of entries processed.
    pub fn total_processed(&self) -> usize {
        self.entries_extracted + self.entries_skipped + self.entries_failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_config_default() {
        let config = StreamingConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_extract_all_result_default() {
        let result = ExtractAllResult::default();
        assert!(result.is_success());
        assert_eq!(result.total_processed(), 0);
    }

    #[test]
    fn test_extract_all_result_with_failures() {
        let result = ExtractAllResult {
            entries_extracted: 5,
            entries_skipped: 2,
            entries_failed: 1,
            bytes_extracted: 1000,
            failures: vec![("test.txt".to_string(), "error".to_string())],
        };

        assert!(!result.is_success());
        assert_eq!(result.total_processed(), 8);
    }
}
