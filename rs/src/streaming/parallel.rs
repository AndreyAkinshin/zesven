//! Parallel folder extraction for non-solid archives.
//!
//! This module provides parallel extraction capabilities for non-solid 7z archives.
//! In non-solid archives, each file is compressed independently in its own folder,
//! allowing multiple files to be extracted concurrently.
//!
//! # Important
//!
//! Solid archives (where multiple files share a compression block) **cannot** be
//! extracted in parallel due to compression dependencies. Use sequential extraction
//! via [`super::EntryIterator`] for solid archives.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::streaming::{StreamingArchive, ParallelExtractionOptions};
//!
//! let mut archive = StreamingArchive::open_path("archive.7z", "")?;
//!
//! // Check if parallel extraction is possible
//! if !archive.is_solid() {
//!     let options = ParallelExtractionOptions::default();
//!     let result = archive.extract_all_parallel("/output/dir", &options)?;
//!     println!("Extracted {} files in parallel", result.entries_extracted);
//! }
//! ```

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[allow(unused_imports)]
use std::sync::{Arc, Mutex};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::format::parser::ArchiveHeader;
use crate::read::{Entry, Threads};
use crate::{Error, READ_BUFFER_SIZE, Result};

/// Options for parallel extraction.
#[derive(Debug, Clone)]
pub struct ParallelExtractionOptions {
    /// Thread configuration.
    pub threads: Threads,
    /// Whether to verify CRC checksums.
    pub verify_crc: bool,
    /// Whether to skip files that already exist.
    pub skip_existing: bool,
    /// Maximum number of folders to process per batch.
    /// Larger batches have better throughput but use more memory.
    pub batch_size: usize,
}

impl Default for ParallelExtractionOptions {
    fn default() -> Self {
        Self {
            threads: Threads::Auto,
            verify_crc: true,
            skip_existing: false,
            batch_size: 64,
        }
    }
}

impl ParallelExtractionOptions {
    /// Creates new parallel extraction options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the thread configuration.
    pub fn threads(mut self, threads: Threads) -> Self {
        self.threads = threads;
        self
    }

    /// Sets whether to verify CRC checksums.
    pub fn verify_crc(mut self, verify: bool) -> Self {
        self.verify_crc = verify;
        self
    }

    /// Sets whether to skip existing files.
    pub fn skip_existing(mut self, skip: bool) -> Self {
        self.skip_existing = skip;
        self
    }

    /// Sets the batch size for parallel processing.
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.max(1);
        self
    }
}

/// Result of parallel extraction.
#[derive(Debug, Default)]
pub struct ParallelExtractionResult {
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
    /// Number of threads used.
    pub threads_used: usize,
    /// Whether parallel extraction was actually used.
    pub used_parallel: bool,
}

impl ParallelExtractionResult {
    /// Returns true if all entries were successfully extracted.
    pub fn is_success(&self) -> bool {
        self.entries_failed == 0
    }

    /// Returns the total number of entries processed.
    pub fn total_processed(&self) -> usize {
        self.entries_extracted + self.entries_skipped + self.entries_failed
    }
}

/// Internal structure representing a folder's work item.
#[derive(Debug)]
struct FolderWorkItem {
    /// Folder index
    folder_index: usize,
    /// Entries in this folder
    entries: Vec<FolderEntry>,
    /// Pre-loaded packed data (loaded during preparation phase)
    packed_data: Vec<u8>,
}

/// An entry within a folder work item.
#[derive(Debug, Clone)]
struct FolderEntry {
    /// Entry index in the archive
    entry_index: usize,
    /// Entry path
    path: String,
    /// Uncompressed size
    size: u64,
    /// Stream index within folder
    stream_index: usize,
    /// Expected CRC (if available)
    expected_crc: Option<u32>,
}

/// Atomic counters for tracking parallel extraction progress.
struct ProgressCounters {
    entries_extracted: AtomicUsize,
    entries_skipped: AtomicUsize,
    entries_failed: AtomicUsize,
    bytes_extracted: AtomicU64,
    #[allow(dead_code)] // Reserved for cancellation support
    cancelled: AtomicBool,
}

impl Default for ProgressCounters {
    fn default() -> Self {
        Self {
            entries_extracted: AtomicUsize::new(0),
            entries_skipped: AtomicUsize::new(0),
            entries_failed: AtomicUsize::new(0),
            bytes_extracted: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
        }
    }
}

/// Parallel folder extractor for non-solid archives.
///
/// This extractor distributes folder decompression across multiple threads,
/// providing significant speedup for archives with many independently-compressed files.
///
/// # Design
///
/// The extractor uses a two-phase approach:
/// 1. **Preparation phase**: Sequentially read all compressed data from the source
/// 2. **Extraction phase**: Decompress and write files in parallel
///
/// This design separates I/O from computation for better performance and
/// avoids the complexity of sharing a reader across threads.
pub struct ParallelFolderExtractor<'a> {
    /// Archive header
    header: &'a ArchiveHeader,
    /// Archive entries
    entries: &'a [Entry],
    /// Pack data start position
    pack_start: u64,
    /// Extraction options
    options: ParallelExtractionOptions,
}

impl<'a> ParallelFolderExtractor<'a> {
    /// Creates a new parallel folder extractor.
    pub fn new(
        header: &'a ArchiveHeader,
        entries: &'a [Entry],
        options: ParallelExtractionOptions,
    ) -> Self {
        let pack_start = super::calculate_pack_start(header);
        Self {
            header,
            entries,
            pack_start,
            options,
        }
    }

    /// Checks if the archive is suitable for parallel extraction.
    ///
    /// Returns `false` for solid archives where files share compression blocks.
    pub fn can_extract_parallel(&self) -> bool {
        !super::check_is_solid(self.header)
    }

    /// Builds work items grouped by folder, pre-loading packed data.
    fn build_work_items<R: Read + Seek>(&self, source: &mut R) -> Result<Vec<FolderWorkItem>> {
        let pack_info = match &self.header.pack_info {
            Some(pi) => pi,
            None => return Ok(Vec::new()),
        };

        let folders = match &self.header.unpack_info {
            Some(ui) => &ui.folders,
            None => return Ok(Vec::new()),
        };

        // Group entries by folder index
        let mut folder_entries: std::collections::HashMap<usize, Vec<FolderEntry>> =
            std::collections::HashMap::new();

        for entry in self.entries {
            if entry.is_directory {
                continue;
            }

            if let Some(folder_idx) = entry.folder_index {
                let folder_entry = FolderEntry {
                    entry_index: entry.index,
                    path: entry.path.as_str().to_string(),
                    size: entry.size,
                    stream_index: entry.stream_index.unwrap_or(0),
                    expected_crc: entry.crc32,
                };

                folder_entries
                    .entry(folder_idx)
                    .or_default()
                    .push(folder_entry);
            }
        }

        // Build work items with pre-loaded data
        let mut work_items = Vec::new();
        let mut pack_offset = self.pack_start;

        for (folder_idx, _folder) in folders.iter().enumerate() {
            let pack_size = pack_info.pack_sizes.get(folder_idx).copied().unwrap_or(0);

            if let Some(entries) = folder_entries.remove(&folder_idx) {
                // Sort entries by stream index for correct ordering
                let mut sorted_entries = entries;
                sorted_entries.sort_by_key(|e| e.stream_index);

                // Read packed data for this folder
                source
                    .seek(SeekFrom::Start(pack_offset))
                    .map_err(Error::Io)?;
                let mut packed_data = vec![0u8; pack_size as usize];
                source.read_exact(&mut packed_data).map_err(Error::Io)?;

                work_items.push(FolderWorkItem {
                    folder_index: folder_idx,
                    entries: sorted_entries,
                    packed_data,
                });
            }

            pack_offset += pack_size;
        }

        Ok(work_items)
    }

    /// Extracts all entries to a directory using parallel decompression.
    ///
    /// # Arguments
    ///
    /// * `source` - The archive reader (used for sequential I/O in preparation phase)
    /// * `dest` - Destination directory
    #[cfg(feature = "parallel")]
    pub fn extract_to_directory<R: Read + Seek>(
        &self,
        source: &mut R,
        dest: impl AsRef<Path>,
    ) -> Result<ParallelExtractionResult> {
        let dest = dest.as_ref();

        if !self.can_extract_parallel() {
            return Err(Error::UnsupportedFeature {
                feature: "parallel extraction for solid archives",
            });
        }

        // Create destination directory
        if !dest.exists() {
            std::fs::create_dir_all(dest).map_err(Error::Io)?;
        }

        // Extract directories first (single-threaded, fast)
        for entry in self.entries {
            if entry.is_directory {
                let dir_path = dest.join(entry.path.as_str());
                std::fs::create_dir_all(&dir_path).map_err(Error::Io)?;
            }
        }

        // Phase 1: Sequential I/O - read all pack data
        let work_items = self.build_work_items(source)?;

        if work_items.is_empty() {
            return Ok(ParallelExtractionResult {
                entries_extracted: self.entries.iter().filter(|e| e.is_directory).count(),
                used_parallel: false,
                threads_used: 1,
                ..Default::default()
            });
        }

        let counters = Arc::new(ProgressCounters::default());
        let failures: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

        // Configure thread pool
        let thread_count = self.options.threads.count();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build()
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;

        // Phase 2: Parallel decompression
        let dest_arc = Arc::new(dest.to_path_buf());
        let options = self.options.clone();
        let header = self.header;
        let counters_ref = Arc::clone(&counters);
        let failures_ref = Arc::clone(&failures);

        pool.install(|| {
            work_items.par_iter().for_each(|work_item| {
                if counters_ref.cancelled.load(Ordering::Relaxed) {
                    return;
                }

                match Self::process_folder(work_item, header, &dest_arc, &options, &counters_ref) {
                    Ok(()) => {}
                    Err(e) => {
                        counters_ref
                            .entries_failed
                            .fetch_add(work_item.entries.len(), Ordering::Relaxed);
                        let mut failures = failures_ref.lock().unwrap();
                        for entry in &work_item.entries {
                            failures.push((entry.path.clone(), e.to_string()));
                        }
                    }
                }
            });
        });

        let mut result = ParallelExtractionResult {
            entries_extracted: counters.entries_extracted.load(Ordering::Relaxed),
            entries_skipped: counters.entries_skipped.load(Ordering::Relaxed),
            entries_failed: counters.entries_failed.load(Ordering::Relaxed),
            bytes_extracted: counters.bytes_extracted.load(Ordering::Relaxed),
            failures: Arc::try_unwrap(failures).unwrap().into_inner().unwrap(),
            threads_used: thread_count,
            used_parallel: true,
        };

        // Add directory count to extracted
        result.entries_extracted += self.entries.iter().filter(|e| e.is_directory).count();

        Ok(result)
    }

    /// Extracts all entries (single-threaded fallback).
    #[cfg(not(feature = "parallel"))]
    pub fn extract_to_directory<R: Read + Seek>(
        &self,
        source: &mut R,
        dest: impl AsRef<Path>,
    ) -> Result<ParallelExtractionResult> {
        let dest = dest.as_ref();

        if !dest.exists() {
            std::fs::create_dir_all(dest).map_err(Error::Io)?;
        }

        // Extract directories first
        for entry in self.entries {
            if entry.is_directory {
                let dir_path = dest.join(entry.path.as_str());
                std::fs::create_dir_all(&dir_path).map_err(Error::Io)?;
            }
        }

        let work_items = self.build_work_items(source)?;
        let counters = Arc::new(ProgressCounters::default());
        let mut failures = Vec::new();

        let dest_arc = Arc::new(dest.to_path_buf());

        // Process folders sequentially
        for work_item in &work_items {
            match Self::process_folder(work_item, self.header, &dest_arc, &self.options, &counters)
            {
                Ok(()) => {}
                Err(e) => {
                    counters
                        .entries_failed
                        .fetch_add(work_item.entries.len(), Ordering::Relaxed);
                    for entry in &work_item.entries {
                        failures.push((entry.path.clone(), e.to_string()));
                    }
                }
            }
        }

        let mut result = ParallelExtractionResult {
            entries_extracted: counters.entries_extracted.load(Ordering::Relaxed),
            entries_skipped: counters.entries_skipped.load(Ordering::Relaxed),
            entries_failed: counters.entries_failed.load(Ordering::Relaxed),
            bytes_extracted: counters.bytes_extracted.load(Ordering::Relaxed),
            failures,
            threads_used: 1,
            used_parallel: false,
        };

        result.entries_extracted += self.entries.iter().filter(|e| e.is_directory).count();

        Ok(result)
    }

    /// Processes a single folder's entries using pre-loaded packed data.
    fn process_folder(
        work_item: &FolderWorkItem,
        header: &ArchiveHeader,
        dest: &Arc<std::path::PathBuf>,
        options: &ParallelExtractionOptions,
        counters: &ProgressCounters,
    ) -> Result<()> {
        // Get folder info
        let folder = header
            .unpack_info
            .as_ref()
            .and_then(|ui| ui.folders.get(work_item.folder_index))
            .ok_or_else(|| Error::InvalidFormat("missing folder info".into()))?;

        // Build decoder from pre-loaded packed data
        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);
        let cursor = std::io::Cursor::new(work_item.packed_data.clone());

        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let coder = &folder.coders[0];
        let mut decoder = crate::codec::build_decoder(cursor, coder, uncompressed_size)?;

        // Get stream sizes for this folder
        let stream_sizes = Self::get_folder_stream_sizes(header, work_item.folder_index);

        // Extract each entry in the folder
        let mut current_stream = 0usize;

        for entry in &work_item.entries {
            // Skip to the correct stream
            while current_stream < entry.stream_index {
                let skip_size = stream_sizes.get(current_stream).copied().unwrap_or(0);
                std::io::copy(&mut (&mut decoder).take(skip_size), &mut std::io::sink())
                    .map_err(Error::Io)?;
                current_stream += 1;
            }

            let entry_path = dest.join(&entry.path);

            // Check if file exists and skip if configured
            if options.skip_existing && entry_path.exists() {
                // Skip this entry's data
                let skip_size = stream_sizes
                    .get(entry.stream_index)
                    .copied()
                    .unwrap_or(entry.size);
                std::io::copy(&mut (&mut decoder).take(skip_size), &mut std::io::sink())
                    .map_err(Error::Io)?;
                current_stream += 1;
                counters.entries_skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            // Create parent directories
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }

            // Extract entry
            let stream_size = stream_sizes
                .get(entry.stream_index)
                .copied()
                .unwrap_or(entry.size);
            let mut file = std::fs::File::create(&entry_path).map_err(Error::Io)?;

            // Use CRC verification if enabled
            let bytes_written = if options.verify_crc && entry.expected_crc.is_some() {
                let mut hasher = crc32fast::Hasher::new();
                let mut buf = [0u8; READ_BUFFER_SIZE];
                let mut remaining = stream_size;
                let mut written = 0u64;

                while remaining > 0 {
                    let to_read = (remaining as usize).min(buf.len());
                    let n = decoder.read(&mut buf[..to_read]).map_err(Error::Io)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    std::io::Write::write_all(&mut file, &buf[..n]).map_err(Error::Io)?;
                    remaining -= n as u64;
                    written += n as u64;
                }

                // Verify CRC
                if let Some(expected) = entry.expected_crc {
                    let actual = hasher.finalize();
                    if actual != expected {
                        return Err(Error::CrcMismatch {
                            entry_index: entry.entry_index,
                            entry_name: Some(entry.path.clone()),
                            expected,
                            actual,
                        });
                    }
                }

                written
            } else {
                std::io::copy(&mut (&mut decoder).take(stream_size), &mut file)
                    .map_err(Error::Io)?
            };

            current_stream += 1;

            counters.entries_extracted.fetch_add(1, Ordering::Relaxed);
            counters
                .bytes_extracted
                .fetch_add(bytes_written, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Gets the stream sizes for a folder.
    fn get_folder_stream_sizes(header: &ArchiveHeader, folder_index: usize) -> Vec<u64> {
        let ss = match &header.substreams_info {
            Some(ss) => ss,
            None => {
                // No substreams - folder has single stream
                let size = header
                    .unpack_info
                    .as_ref()
                    .and_then(|ui| ui.folders.get(folder_index))
                    .and_then(|f| f.final_unpack_size())
                    .unwrap_or(0);
                return vec![size];
            }
        };

        // Calculate offset into unpack_sizes
        let mut offset = 0usize;
        for (i, &count) in ss.num_unpack_streams_in_folders.iter().enumerate() {
            if i == folder_index {
                let count = count as usize;
                return ss.unpack_sizes[offset..offset + count].to_vec();
            }
            offset += count as usize;
        }

        Vec::new()
    }
}

/// Request cancellation of ongoing parallel extraction.
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a new cancellation token.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Cancels the operation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Checks if cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_extraction_options_default() {
        let opts = ParallelExtractionOptions::default();
        assert!(matches!(opts.threads, Threads::Auto));
        assert!(opts.verify_crc);
        assert!(!opts.skip_existing);
        assert_eq!(opts.batch_size, 64);
    }

    #[test]
    fn test_parallel_extraction_options_builder() {
        let opts = ParallelExtractionOptions::new()
            .threads(Threads::count_or_single(4))
            .verify_crc(false)
            .skip_existing(true)
            .batch_size(32);

        assert_eq!(opts.threads.count(), 4);
        assert!(!opts.verify_crc);
        assert!(opts.skip_existing);
        assert_eq!(opts.batch_size, 32);
    }

    #[test]
    fn test_parallel_extraction_result_success() {
        let result = ParallelExtractionResult {
            entries_extracted: 10,
            entries_skipped: 2,
            entries_failed: 0,
            bytes_extracted: 10000,
            failures: Vec::new(),
            threads_used: 4,
            used_parallel: true,
        };

        assert!(result.is_success());
        assert_eq!(result.total_processed(), 12);
    }

    #[test]
    fn test_parallel_extraction_result_failure() {
        let result = ParallelExtractionResult {
            entries_extracted: 10,
            entries_skipped: 0,
            entries_failed: 1,
            bytes_extracted: 10000,
            failures: vec![("file.txt".to_string(), "error".to_string())],
            threads_used: 4,
            used_parallel: true,
        };

        assert!(!result.is_success());
        assert_eq!(result.total_processed(), 11);
    }

    #[test]
    fn test_cancellation_token() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_batch_size_minimum() {
        let opts = ParallelExtractionOptions::new().batch_size(0);
        assert_eq!(opts.batch_size, 1);
    }
}
