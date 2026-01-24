//! Stream pooling for solid archive decompression.
//!
//! This module provides [`DecoderPool`] for caching decompression streams,
//! enabling efficient random access to solid archives by reusing decoders.
//!
//! # Performance Impact
//!
//! For solid archives with many files, stream pooling can provide significant
//! speedups (up to 190x reported in sevenzip Go implementation) by avoiding
//! repeated decompression from the start of solid blocks.
//!
//! # How It Works
//!
//! When accessing entries in a solid archive:
//! 1. Check if a cached decoder exists at or before the target position
//! 2. If found, skip forward from cached position to target
//! 3. If not found, create new decoder from start
//! 4. Cache the decoder after use for potential reuse

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::num::NonZeroUsize;

use crate::format::parser::ArchiveHeader;
use crate::s3fifo::S3FifoCache;
use crate::{Error, READ_BUFFER_SIZE, Result};

/// A cached decoder with its current position in the decompressed stream.
struct CachedDecoder {
    /// The decoder itself
    decoder: Box<dyn Read + Send>,
    /// Current byte offset in the uncompressed stream
    byte_offset: u64,
    /// Total uncompressed size of the folder
    total_size: u64,
}

/// Stream pool for caching decompression streams.
///
/// The pool maintains an LRU cache of active decoders, allowing reuse
/// when accessing files within the same solid block.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::DecoderPool;
///
/// let mut pool = DecoderPool::new(4); // Cache up to 4 decoders
///
/// // Get or create a decoder for a specific position
/// let mut decoder = pool.get_decoder(
///     &header,
///     &mut source,
///     folder_index,
///     target_offset,
/// )?;
///
/// // Read data...
/// let mut buf = vec![0u8; 1024];
/// decoder.read_exact(&mut buf)?;
///
/// // Return decoder to pool for potential reuse
/// pool.return_decoder(decoder);
/// ```
pub struct DecoderPool {
    /// S3Fifo cache of decoders keyed by folder index
    cache: S3FifoCache<usize, CachedDecoder>,
    /// Statistics
    stats: PoolStats,
}

/// Statistics for pool usage.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Number of cache hits (decoder reused)
    pub hits: u64,
    /// Number of cache misses (new decoder created)
    pub misses: u64,
    /// Total bytes skipped forward (reusing cached decoder)
    pub bytes_skipped: u64,
    /// Total bytes re-decompressed (starting fresh)
    pub bytes_redecompressed: u64,
}

impl PoolStats {
    /// Returns the cache hit ratio.
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Returns the estimated bytes saved by caching.
    pub fn bytes_saved(&self) -> u64 {
        // Bytes we would have re-decompressed but didn't
        self.bytes_skipped
    }
}

impl DecoderPool {
    /// Creates a new decoder pool with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of decoders to cache.
    ///   A reasonable default is 4-8 for typical workloads.
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            cache: S3FifoCache::new(cap),
            stats: PoolStats::default(),
        }
    }

    /// Creates a new decoder pool with capacity automatically sized to the number of CPUs.
    ///
    /// This is the recommended constructor for most use cases, as it adapts to the
    /// available parallelism on the system.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::streaming::DecoderPool;
    ///
    /// let pool = DecoderPool::auto_sized();
    /// // On an 8-core system, capacity will be 8
    /// ```
    pub fn auto_sized() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(cpus)
    }

    /// Creates a new decoder pool with capacity based on CPU count times a multiplier.
    ///
    /// Useful when you expect high cache contention or want to cache more decoders
    /// than the default CPU-based sizing.
    ///
    /// # Arguments
    ///
    /// * `multiplier` - Multiply the CPU count by this factor. A value of 0 will
    ///   result in a capacity of 1.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::streaming::DecoderPool;
    ///
    /// // Double the default capacity
    /// let pool = DecoderPool::with_multiplier(2);
    /// // On an 8-core system, capacity will be 16
    /// ```
    pub fn with_multiplier(multiplier: usize) -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(cpus.saturating_mul(multiplier).max(1))
    }

    /// Returns the number of available CPUs detected on this system.
    ///
    /// This is used by [`auto_sized`](Self::auto_sized) and
    /// [`with_multiplier`](Self::with_multiplier) for capacity calculation.
    pub fn detected_cpu_count() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
}

impl Default for DecoderPool {
    /// Creates a decoder pool with capacity automatically sized to the number of CPUs.
    ///
    /// This is equivalent to [`DecoderPool::auto_sized()`].
    fn default() -> Self {
        Self::auto_sized()
    }
}

impl DecoderPool {
    /// Returns the pool's capacity.
    pub fn capacity(&self) -> usize {
        self.cache.capacity()
    }

    /// Returns the current number of cached decoders.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns true if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Returns the pool statistics.
    pub fn stats(&self) -> &PoolStats {
        &self.stats
    }

    /// Resets the pool statistics.
    pub fn reset_stats(&mut self) {
        self.stats = PoolStats::default();
    }

    /// Clears all cached decoders.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Gets a decoder for the specified folder and target offset.
    ///
    /// If a cached decoder exists at or before the target offset,
    /// it will be reused and skipped forward. Otherwise, a new
    /// decoder is created.
    ///
    /// # Arguments
    ///
    /// * `header` - Archive header
    /// * `source` - Source reader
    /// * `folder_index` - Index of the folder to decode
    /// * `target_offset` - Target byte offset in the uncompressed stream
    /// * `pack_start` - Starting position of packed data
    pub fn get_decoder<R: Read + Seek + Send>(
        &mut self,
        header: &ArchiveHeader,
        source: &mut R,
        folder_index: usize,
        target_offset: u64,
        pack_start: u64,
    ) -> Result<PooledDecoder> {
        // Check if we have a cached decoder at a useful position
        if let Some(cached) = self.cache.pop(&folder_index) {
            if cached.byte_offset <= target_offset {
                // Can reuse this decoder by skipping forward
                let skip_bytes = target_offset - cached.byte_offset;
                self.stats.hits += 1;
                self.stats.bytes_skipped += cached.byte_offset; // Bytes we didn't re-decompress

                return Ok(PooledDecoder {
                    folder_index,
                    decoder: cached.decoder,
                    byte_offset: cached.byte_offset,
                    total_size: cached.total_size,
                    skip_remaining: skip_bytes,
                });
            }
            // Cached decoder is past target - can't use it
            // (Let it drop, it's not useful)
        }

        // Need to create a new decoder
        self.stats.misses += 1;
        self.stats.bytes_redecompressed += target_offset;

        let decoder = self.create_decoder(header, source, folder_index, pack_start)?;
        let total_size = self.get_folder_uncompressed_size(header, folder_index);

        Ok(PooledDecoder {
            folder_index,
            decoder,
            byte_offset: 0,
            total_size,
            skip_remaining: target_offset,
        })
    }

    /// Returns a decoder to the pool for potential reuse.
    pub fn return_decoder(&mut self, decoder: PooledDecoder) {
        // Only cache if not exhausted
        if decoder.byte_offset < decoder.total_size {
            let cached = CachedDecoder {
                decoder: decoder.decoder,
                byte_offset: decoder.byte_offset,
                total_size: decoder.total_size,
            };
            self.cache.insert(decoder.folder_index, cached);
        }
    }

    /// Creates a new decoder for the specified folder.
    fn create_decoder<R: Read + Seek + Send>(
        &self,
        header: &ArchiveHeader,
        source: &mut R,
        folder_index: usize,
        pack_start: u64,
    ) -> Result<Box<dyn Read + Send>> {
        let unpack_info = header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        let folder = unpack_info.folders.get(folder_index).ok_or_else(|| {
            Error::InvalidFormat(format!("folder index {} out of range", folder_index))
        })?;

        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        // Calculate pack offset
        let pack_offset = self.calculate_pack_offset(header, folder_index, pack_start)?;

        // Get pack size
        let pack_size = header
            .pack_info
            .as_ref()
            .and_then(|pi| pi.pack_sizes.get(folder_index).copied())
            .unwrap_or(0);

        // Seek and read packed data
        source
            .seek(SeekFrom::Start(pack_offset))
            .map_err(Error::Io)?;

        let mut packed_data = vec![0u8; pack_size as usize];
        source.read_exact(&mut packed_data).map_err(Error::Io)?;

        let coder = &folder.coders[0];
        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);

        let cursor = std::io::Cursor::new(packed_data);
        let decoder = crate::codec::build_decoder(cursor, coder, uncompressed_size)?;

        Ok(Box::new(decoder))
    }

    /// Calculates the pack offset for a folder.
    fn calculate_pack_offset(
        &self,
        header: &ArchiveHeader,
        folder_index: usize,
        pack_start: u64,
    ) -> Result<u64> {
        let pack_info = header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        let mut offset = pack_start + pack_info.pack_pos;

        // Sum pack sizes for previous folders
        for i in 0..folder_index {
            if let Some(&size) = pack_info.pack_sizes.get(i) {
                offset += size;
            }
        }

        Ok(offset)
    }

    /// Gets the uncompressed size of a folder.
    fn get_folder_uncompressed_size(&self, header: &ArchiveHeader, folder_index: usize) -> u64 {
        header
            .unpack_info
            .as_ref()
            .and_then(|ui| ui.folders.get(folder_index))
            .and_then(|f| f.final_unpack_size())
            .unwrap_or(0)
    }
}

/// A decoder borrowed from the pool.
///
/// This wraps a decoder with position tracking and automatic skip handling.
/// When done, return it to the pool via [`DecoderPool::return_decoder`].
pub struct PooledDecoder {
    folder_index: usize,
    decoder: Box<dyn Read + Send>,
    byte_offset: u64,
    total_size: u64,
    skip_remaining: u64,
}

impl PooledDecoder {
    /// Returns the folder index this decoder is for.
    pub fn folder_index(&self) -> usize {
        self.folder_index
    }

    /// Returns the current byte offset in the uncompressed stream.
    pub fn byte_offset(&self) -> u64 {
        self.byte_offset
    }

    /// Returns the total uncompressed size of the folder.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns remaining bytes to read in the folder.
    pub fn remaining(&self) -> u64 {
        self.total_size.saturating_sub(self.byte_offset)
    }

    /// Skips forward in the stream (if needed).
    ///
    /// This should be called before reading if the decoder was
    /// reused from cache at an earlier position.
    pub fn skip_to_offset(&mut self) -> io::Result<()> {
        if self.skip_remaining > 0 {
            // Skip forward by reading and discarding
            let mut remaining = self.skip_remaining;
            let mut buf = [0u8; READ_BUFFER_SIZE];

            while remaining > 0 {
                let to_read = (remaining as usize).min(buf.len());
                let n = self.decoder.read(&mut buf[..to_read])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "unexpected EOF while skipping",
                    ));
                }
                remaining -= n as u64;
                self.byte_offset += n as u64;
            }

            self.skip_remaining = 0;
        }

        Ok(())
    }
}

impl Read for PooledDecoder {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Skip to target position if needed
        self.skip_to_offset()?;

        let n = self.decoder.read(buf)?;
        self.byte_offset += n as u64;
        Ok(n)
    }
}

/// Helper to determine entry positions within solid blocks.
#[derive(Debug, Clone)]
pub struct SolidEntryLocator {
    /// Map from entry index to (folder_index, offset_within_folder, size)
    entries: HashMap<usize, EntryLocation>,
}

/// Location of an entry within a solid block.
#[derive(Debug, Clone, Copy)]
pub struct EntryLocation {
    /// Folder (solid block) index
    pub folder_index: usize,
    /// Byte offset within the uncompressed folder data
    pub offset: u64,
    /// Size of this entry
    pub size: u64,
}

impl SolidEntryLocator {
    /// Creates a new locator from archive header.
    pub fn from_header(header: &ArchiveHeader) -> Self {
        let mut entries = HashMap::new();

        let files_info = match &header.files_info {
            Some(fi) => fi,
            None => return Self { entries },
        };

        let substreams = match &header.substreams_info {
            Some(ss) => ss,
            None => return Self { entries },
        };

        let mut entry_idx = 0;
        let mut stream_offset = 0usize;

        for (folder_idx, &num_streams) in
            substreams.num_unpack_streams_in_folders.iter().enumerate()
        {
            let mut folder_offset = 0u64;

            for stream_idx in 0..(num_streams as usize) {
                // Find corresponding file entry
                while entry_idx < files_info.entries.len() {
                    let file = &files_info.entries[entry_idx];
                    if file.is_directory {
                        entry_idx += 1;
                        continue;
                    }
                    break;
                }

                if entry_idx >= files_info.entries.len() {
                    break;
                }

                let size = substreams
                    .unpack_sizes
                    .get(stream_offset + stream_idx)
                    .copied()
                    .unwrap_or(0);

                entries.insert(
                    entry_idx,
                    EntryLocation {
                        folder_index: folder_idx,
                        offset: folder_offset,
                        size,
                    },
                );

                folder_offset += size;
                entry_idx += 1;
            }

            stream_offset += num_streams as usize;
        }

        Self { entries }
    }

    /// Gets the location of an entry by index.
    pub fn get(&self, entry_index: usize) -> Option<&EntryLocation> {
        self.entries.get(&entry_index)
    }

    /// Returns the number of located entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if no entries are located.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_stats_hit_ratio() {
        let stats = PoolStats {
            hits: 3,
            misses: 1,
            bytes_skipped: 1000,
            bytes_redecompressed: 500,
        };

        assert!((stats.hit_ratio() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pool_stats_zero_total() {
        let stats = PoolStats::default();
        assert!((stats.hit_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decoder_pool_capacity() {
        let pool = DecoderPool::new(4);
        assert_eq!(pool.capacity(), 4);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_decoder_pool_clear() {
        let mut pool = DecoderPool::new(4);
        pool.clear();
        assert!(pool.is_empty());
    }

    #[test]
    fn test_decoder_pool_auto_sized() {
        let pool = DecoderPool::auto_sized();
        let expected = DecoderPool::detected_cpu_count();
        assert_eq!(pool.capacity(), expected);
        assert!(pool.capacity() >= 1);
    }

    #[test]
    fn test_decoder_pool_with_multiplier() {
        let pool = DecoderPool::with_multiplier(2);
        let expected = DecoderPool::detected_cpu_count() * 2;
        assert_eq!(pool.capacity(), expected);
    }

    #[test]
    fn test_decoder_pool_with_multiplier_zero() {
        let pool = DecoderPool::with_multiplier(0);
        // Should be at least 1 even with 0 multiplier
        assert_eq!(pool.capacity(), 1);
    }

    #[test]
    fn test_decoder_pool_default() {
        let pool = DecoderPool::default();
        let auto_pool = DecoderPool::auto_sized();
        assert_eq!(pool.capacity(), auto_pool.capacity());
    }

    #[test]
    fn test_detected_cpu_count() {
        let count = DecoderPool::detected_cpu_count();
        assert!(count >= 1, "CPU count should be at least 1");
    }
}
