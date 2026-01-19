//! Solid block stream reader for sequential archive access.
//!
//! This module provides [`SolidBlockStreamReader`] for reading solid archives
//! where multiple files are compressed together in a single block.

use std::io::{self, Read, Seek, SeekFrom};

use crate::format::SIGNATURE_HEADER_SIZE;
use crate::format::parser::ArchiveHeader;
use crate::format::streams::Folder;
use crate::{Error, READ_BUFFER_SIZE, Result};

#[cfg(feature = "aes")]
use crate::Password;

use super::config::StreamingConfig;

/// Sequential reader for solid archive blocks.
///
/// Solid archives compress multiple files as a single continuous stream,
/// achieving better compression but requiring sequential decompression.
/// This reader handles the sequential constraint while minimizing memory usage.
///
/// # Key Constraints
///
/// 1. **Sequential Access Only**: Entries must be processed in order.
///    You cannot skip ahead without decompressing intermediate data.
///
/// 2. **Memory Efficiency**: While sequential decompression is required,
///    this reader minimizes buffering by streaming data through.
///
/// 3. **Skip Operations**: Calling `skip()` on an entry still decompresses
///    the data but discards it.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::{SolidBlockStreamReader, StreamingConfig};
///
/// let mut reader = SolidBlockStreamReader::new(
///     &archive,
///     &mut source,
///     block_index,
///     &password,
///     StreamingConfig::default(),
/// )?;
///
/// while let Some(entry_result) = reader.next_entry() {
///     let (entry_idx, size) = entry_result?;
///     // Read the entry data from the reader
///     // ...
/// }
/// ```
pub struct SolidBlockStreamReader<'a, R: Read + Seek> {
    /// Archive header reference
    header: &'a ArchiveHeader,
    /// Source reader
    source: &'a mut R,
    /// Folder (block) index
    block_index: usize,
    /// Password for encrypted archives
    #[cfg(feature = "aes")]
    #[allow(dead_code)] // Reserved for encrypted solid block support
    password: &'a Password,
    /// Configuration
    #[allow(dead_code)] // Reserved for configuration usage
    config: StreamingConfig,
    /// The decoder for the solid block
    decoder: Option<Box<dyn Read + Send + 'a>>,
    /// Current entry index within the block
    current_entry_index: usize,
    /// Sizes of entries in this block
    entry_sizes: Vec<u64>,
    /// Bytes remaining in current entry
    bytes_remaining_in_entry: u64,
    /// Total bytes decompressed so far
    total_decompressed: u64,
    /// Whether the reader has been initialized
    initialized: bool,
}

impl<'a, R: Read + Seek + Send> SolidBlockStreamReader<'a, R> {
    /// Creates a new solid block reader.
    #[cfg(feature = "aes")]
    pub fn new(
        header: &'a ArchiveHeader,
        source: &'a mut R,
        block_index: usize,
        password: &'a Password,
        config: StreamingConfig,
    ) -> Result<Self> {
        let entry_sizes = Self::collect_entry_sizes(header, block_index)?;

        Ok(Self {
            header,
            source,
            block_index,
            password,
            config,
            decoder: None,
            current_entry_index: 0,
            entry_sizes,
            bytes_remaining_in_entry: 0,
            total_decompressed: 0,
            initialized: false,
        })
    }

    /// Creates a new solid block reader (without AES support).
    #[cfg(not(feature = "aes"))]
    pub fn new(
        header: &'a ArchiveHeader,
        source: &'a mut R,
        block_index: usize,
        config: StreamingConfig,
    ) -> Result<Self> {
        let entry_sizes = Self::collect_entry_sizes(header, block_index)?;

        Ok(Self {
            header,
            source,
            block_index,
            config,
            decoder: None,
            current_entry_index: 0,
            entry_sizes,
            bytes_remaining_in_entry: 0,
            total_decompressed: 0,
            initialized: false,
        })
    }

    /// Collects entry sizes for this block.
    fn collect_entry_sizes(header: &ArchiveHeader, block_index: usize) -> Result<Vec<u64>> {
        let substreams = header.substreams_info.as_ref().ok_or_else(|| {
            Error::InvalidFormat("missing substreams info for solid block".into())
        })?;

        let num_streams = *substreams
            .num_unpack_streams_in_folders
            .get(block_index)
            .ok_or_else(|| {
                Error::InvalidFormat(format!("block index {} out of range", block_index))
            })? as usize;

        // Calculate the starting stream index for this block
        let stream_offset: usize = substreams
            .num_unpack_streams_in_folders
            .iter()
            .take(block_index)
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

    /// Initializes the decoder for this block.
    fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        let folders = match &self.header.unpack_info {
            Some(ui) => &ui.folders,
            None => return Err(Error::InvalidFormat("missing unpack info".into())),
        };

        let folder = folders.get(self.block_index).ok_or_else(|| {
            Error::InvalidFormat(format!("block index {} out of range", self.block_index))
        })?;

        // Seek to the block's position
        let pack_offset = self.calculate_block_offset()?;
        self.source
            .seek(SeekFrom::Start(pack_offset))
            .map_err(Error::Io)?;

        // Build decoder
        self.decoder = Some(self.build_decoder(folder)?);
        self.initialized = true;

        // Set up first entry
        if !self.entry_sizes.is_empty() {
            self.bytes_remaining_in_entry = self.entry_sizes[0];
        }

        Ok(())
    }

    fn calculate_block_offset(&self) -> Result<u64> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        // Start after signature header
        let mut offset = SIGNATURE_HEADER_SIZE + pack_info.pack_pos;

        // Sum up pack sizes for previous blocks
        for i in 0..self.block_index {
            if i < pack_info.pack_sizes.len() {
                offset += pack_info.pack_sizes[i];
            }
        }

        Ok(offset)
    }

    fn build_decoder(&mut self, folder: &Folder) -> Result<Box<dyn Read + Send + 'a>> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);

        // Get pack size for this folder
        let pack_size = self
            .header
            .pack_info
            .as_ref()
            .and_then(|pi| pi.pack_sizes.get(self.block_index).copied())
            .unwrap_or(0);

        // Read packed data into buffer to avoid lifetime issues
        let mut packed_data = vec![0u8; pack_size as usize];
        self.source
            .read_exact(&mut packed_data)
            .map_err(Error::Io)?;

        let cursor = std::io::Cursor::new(packed_data);
        // Build decoder chain to handle filter+codec combinations (e.g., BCJ + LZMA2)
        crate::codec::build_decoder_chain(cursor, folder, uncompressed_size)
    }

    /// Returns the number of entries in this block.
    pub fn num_entries(&self) -> usize {
        self.entry_sizes.len()
    }

    /// Returns the current entry index within this block.
    pub fn current_index(&self) -> usize {
        self.current_entry_index
    }

    /// Returns true if all entries have been processed.
    pub fn is_exhausted(&self) -> bool {
        self.current_entry_index >= self.entry_sizes.len()
    }

    /// Returns the total bytes decompressed so far.
    pub fn total_decompressed(&self) -> u64 {
        self.total_decompressed
    }

    /// Advances to the next entry in the solid block.
    ///
    /// Returns the entry index within the block and its size.
    pub fn next_entry(&mut self) -> Option<Result<(usize, u64)>> {
        if self.is_exhausted() {
            return None;
        }

        // Initialize on first call
        if !self.initialized {
            if let Err(e) = self.initialize() {
                return Some(Err(e));
            }
        }

        let idx = self.current_entry_index;
        let size = self.entry_sizes.get(idx).copied()?;

        self.bytes_remaining_in_entry = size;
        Some(Ok((idx, size)))
    }

    /// Reads data from the current entry.
    ///
    /// Call this repeatedly until it returns 0 to read all entry data.
    pub fn read_entry_data(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.bytes_remaining_in_entry == 0 {
            return Ok(0);
        }

        let decoder = match &mut self.decoder {
            Some(d) => d,
            None => return Ok(0),
        };

        let to_read = buf.len().min(self.bytes_remaining_in_entry as usize);
        let n = decoder.read(&mut buf[..to_read])?;

        self.bytes_remaining_in_entry -= n as u64;
        self.total_decompressed += n as u64;

        Ok(n)
    }

    /// Finishes the current entry and advances to the next.
    ///
    /// Call this after reading all data from an entry (or to skip it).
    pub fn finish_entry(&mut self) -> Result<()> {
        // Skip remaining bytes in current entry
        if self.bytes_remaining_in_entry > 0 {
            let decoder = match &mut self.decoder {
                Some(d) => d,
                None => return Ok(()),
            };

            let remaining = self.bytes_remaining_in_entry;
            io::copy(&mut decoder.take(remaining), &mut io::sink()).map_err(Error::Io)?;
            self.total_decompressed += remaining;
            self.bytes_remaining_in_entry = 0;
        }

        self.current_entry_index += 1;
        if self.current_entry_index < self.entry_sizes.len() {
            self.bytes_remaining_in_entry = self.entry_sizes[self.current_entry_index];
        }

        Ok(())
    }

    /// Skips the current entry without reading its data.
    ///
    /// Note: For solid blocks, this still decompresses the data but discards it.
    pub fn skip_current_entry(&mut self) -> Result<()> {
        self.finish_entry()
    }

    /// Reads the current entry to a Vec.
    pub fn read_entry_to_vec(&mut self) -> Result<Vec<u8>> {
        let size = self.bytes_remaining_in_entry as usize;
        let mut data = Vec::with_capacity(size);

        loop {
            let mut buf = [0u8; READ_BUFFER_SIZE];
            let n = self.read_entry_data(&mut buf)?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }

        self.current_entry_index += 1;
        if self.current_entry_index < self.entry_sizes.len() {
            self.bytes_remaining_in_entry = self.entry_sizes[self.current_entry_index];
        }

        Ok(data)
    }

    /// Extracts the current entry to a writer.
    pub fn extract_entry_to<W: io::Write>(&mut self, writer: &mut W) -> Result<u64> {
        let mut total = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = self.read_entry_data(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).map_err(Error::Io)?;
            total += n as u64;
        }

        self.current_entry_index += 1;
        if self.current_entry_index < self.entry_sizes.len() {
            self.bytes_remaining_in_entry = self.entry_sizes[self.current_entry_index];
        }

        Ok(total)
    }
}

/// Information about a solid block.
#[derive(Debug, Clone)]
pub struct SolidBlockInfo {
    /// Index of the block/folder
    pub block_index: usize,
    /// Number of entries in this block
    pub num_entries: usize,
    /// Total uncompressed size of all entries
    pub total_size: u64,
    /// Compressed size of the block
    pub packed_size: u64,
    /// Individual entry sizes
    pub entry_sizes: Vec<u64>,
}

impl SolidBlockInfo {
    /// Creates block info from archive header.
    pub fn from_header(header: &ArchiveHeader, block_index: usize) -> Result<Self> {
        let substreams = header
            .substreams_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing substreams info".into()))?;

        let num_entries = substreams
            .num_unpack_streams_in_folders
            .get(block_index)
            .copied()
            .unwrap_or(0) as usize;

        let folders = header.unpack_info.as_ref().map(|ui| &ui.folders);
        let total_size = folders
            .and_then(|f| f.get(block_index))
            .and_then(|folder| folder.final_unpack_size())
            .unwrap_or(0);

        let packed_size = header
            .pack_info
            .as_ref()
            .and_then(|pi| pi.pack_sizes.get(block_index).copied())
            .unwrap_or(0);

        // Calculate entry sizes
        let stream_offset: usize = substreams
            .num_unpack_streams_in_folders
            .iter()
            .take(block_index)
            .map(|&n| n as usize)
            .sum();

        let entry_sizes: Vec<u64> = (0..num_entries)
            .map(|i| {
                substreams
                    .unpack_sizes
                    .get(stream_offset + i)
                    .copied()
                    .unwrap_or(0)
            })
            .collect();

        Ok(Self {
            block_index,
            num_entries,
            total_size,
            packed_size,
            entry_sizes,
        })
    }

    /// Returns the compression ratio.
    pub fn compression_ratio(&self) -> f64 {
        if self.packed_size == 0 {
            0.0
        } else {
            self.total_size as f64 / self.packed_size as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solid_block_info_compression_ratio() {
        let info = SolidBlockInfo {
            block_index: 0,
            num_entries: 5,
            total_size: 1000,
            packed_size: 100,
            entry_sizes: vec![200, 200, 200, 200, 200],
        };

        assert!((info.compression_ratio() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_solid_block_info_zero_packed() {
        let info = SolidBlockInfo {
            block_index: 0,
            num_entries: 0,
            total_size: 0,
            packed_size: 0,
            entry_sizes: vec![],
        };

        assert!((info.compression_ratio() - 0.0).abs() < f64::EPSILON);
    }
}
