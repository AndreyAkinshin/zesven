//! Streaming decompression API for memory-efficient extraction.
//!
//! This module provides a streaming API for extracting 7z archives with
//! bounded memory usage. It's designed for processing large archives that
//! shouldn't be fully loaded into memory.
//!
//! # Overview
//!
//! The streaming API provides:
//!
//! - **[`StreamingArchive`]**: High-level streaming archive reader
//! - **[`StreamingConfig`]**: Configuration for memory bounds and behavior
//! - **[`StreamingEntry`]**: Individual entry with streaming data access
//! - **[`EntryIterator`]**: Iterator over archive entries
//! - **[`MemoryTracker`]**: Memory usage monitoring and limits
//! - **[`RandomAccessReader`]**: Random access for non-solid archives
//! - **[`DecoderPool`]**: Stream pooling for efficient solid archive access
//! - **Various sinks**: For extracting to different Write implementations
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::streaming::{StreamingArchive, StreamingConfig};
//!
//! // Open with custom configuration
//! let config = StreamingConfig::new()
//!     .max_memory_buffer(32 * 1024 * 1024)  // 32 MiB
//!     .verify_crc(true);
//!
//! let mut archive = StreamingArchive::open_with_config(file, password, config)?;
//!
//! // Process entries one at a time
//! for entry_result in archive.entries()? {
//!     let mut entry = entry_result?;
//!
//!     if entry.is_directory() {
//!         std::fs::create_dir_all(entry.name())?;
//!         continue;
//!     }
//!
//!     let mut file = std::fs::File::create(entry.name())?;
//!     entry.extract_to(&mut file)?;
//! }
//! ```
//!
//! # Solid Archives
//!
//! 7z archives can be "solid", meaning multiple files are compressed
//! together as a single stream. This achieves better compression but
//! requires sequential decompression.
//!
//! For solid archives:
//! - Use [`StreamingArchive::entries()`] for sequential access
//! - Entries must be processed in order
//! - Skipping an entry still decompresses it (but discards the data)
//!
//! For non-solid archives:
//! - [`RandomAccessReader`] enables direct access to specific entries
//! - Entries can be accessed in any order
//!
//! # Memory Management
//!
//! Use [`StreamingConfig`] to control memory usage:
//!
//! ```rust,ignore
//! // Low memory configuration
//! let config = StreamingConfig::low_memory();
//!
//! // High performance configuration (more memory, better throughput)
//! let config = StreamingConfig::high_performance();
//!
//! // Custom configuration
//! let config = StreamingConfig::new()
//!     .max_memory_buffer(16 * 1024 * 1024)
//!     .read_buffer_size(32 * 1024);
//! ```
//!
//! Use [`MemoryTracker`] for fine-grained allocation control:
//!
//! ```rust,ignore
//! let tracker = MemoryTracker::new(64 * 1024 * 1024);
//!
//! // Allocate with RAII guard
//! let guard = tracker.allocate(1024)?;
//! // Memory automatically released when guard drops
//! ```
//!
//! # Write Sinks
//!
//! The sink module provides various Write implementations:
//!
//! - [`BoundedVecSink`]: In-memory buffer with size limit
//! - [`Crc32Sink`]: Computes CRC while discarding data
//! - [`NullSink`]: Discards data (for skipping)
//! - [`CountingSink`]: Wraps another writer and counts bytes
//! - [`ProgressSink`]: Wraps another writer with progress callbacks
//! - [`TeeSink`]: Writes to two destinations simultaneously

mod archive;
mod config;
mod iterator;
mod memory;
mod parallel;
mod pool;
mod progressive;
mod random;
mod sink;
mod solid;

use crate::format::SIGNATURE_HEADER_SIZE;
use crate::format::parser::ArchiveHeader;

/// Calculates the starting offset of packed data in the archive.
///
/// Pack data starts after the signature header plus the pack_pos
/// offset from the PackInfo structure.
pub(crate) fn calculate_pack_start(header: &ArchiveHeader) -> u64 {
    let pack_pos = header.pack_info.as_ref().map(|pi| pi.pack_pos).unwrap_or(0);
    SIGNATURE_HEADER_SIZE + pack_pos
}

/// Checks if an archive uses solid compression (multiple streams per folder).
pub(crate) fn check_is_solid(header: &ArchiveHeader) -> bool {
    header
        .substreams_info
        .as_ref()
        .map(|ss| {
            ss.num_unpack_streams_in_folders
                .iter()
                .any(|&count| count > 1)
        })
        .unwrap_or(false)
}

// Re-export main types
pub use archive::{ExtractAllResult, StreamingArchive};
pub use config::{CompressionMethod, MemoryEstimate, StreamingConfig, SystemMemoryInfo};
pub use iterator::{EntryIterator, StreamingEntry};
pub use memory::{MemoryGuard, MemoryTracker, TrackedBuffer};
pub use parallel::{
    CancellationToken, ParallelExtractionOptions, ParallelExtractionResult, ParallelFolderExtractor,
};
pub use pool::{DecoderPool, EntryLocation, PoolStats, PooledDecoder, SolidEntryLocator};
pub use progressive::{
    BoundedReader, ChainedReader, ProgressiveReader, ProgressiveReaderWithCallback,
};
pub use random::{EntryLocator, RandomAccessReader, RandomEntryReader};
pub use sink::{
    BoundedVecSink, CountingSink, Crc32Sink, ExtractToSink, NullSink, ProgressSink, TeeSink,
};
pub use solid::{SolidBlockInfo, SolidBlockStreamReader};

/// Information about an entry that was skipped during archive parsing.
///
/// When parsing an archive, entries with invalid paths (e.g., invalid UTF-8,
/// null bytes, or other path validation failures) are skipped rather than
/// causing the entire archive open operation to fail. This struct provides
/// information about what was skipped and why.
#[derive(Debug, Clone)]
pub struct SkippedEntry {
    /// The original index of this entry in the archive's file list.
    pub index: usize,
    /// The raw path bytes if available (useful for debugging encoding issues).
    pub raw_path: Option<Vec<u8>>,
    /// The reason why this entry was skipped.
    pub reason: SkipReason,
}

/// The reason why an archive entry was skipped during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The path failed validation (e.g., contained null bytes or invalid characters).
    InvalidPath(String),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::InvalidPath(msg) => write!(f, "invalid path: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify main types are accessible
        let _ = StreamingConfig::default();
        let tracker = MemoryTracker::new(1024);
        let _ = tracker.current_usage();
    }

    #[test]
    fn test_config_presets() {
        let low = StreamingConfig::low_memory();
        let high = StreamingConfig::high_performance();

        assert!(low.max_memory_buffer < high.max_memory_buffer);
    }

    #[test]
    fn test_sink_types() {
        let mut null_sink = NullSink::new();
        std::io::Write::write_all(&mut null_sink, &[1, 2, 3]).unwrap();
        assert_eq!(null_sink.bytes_discarded(), 3);
    }
}
