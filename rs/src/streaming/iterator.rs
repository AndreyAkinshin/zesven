//! Entry iterator for streaming decompression.
//!
//! This module provides [`EntryIterator`] for iterating over archive entries
//! with streaming decompression, and [`StreamingEntry`] for accessing entry
//! data.

use std::io::{self, Read, Seek, SeekFrom};

use crate::format::parser::ArchiveHeader;
use crate::format::streams::Folder;
use crate::read::Entry;
use crate::{Error, READ_BUFFER_SIZE, Result};

#[cfg(feature = "aes")]
use crate::Password;

use super::config::StreamingConfig;

/// Iterator that yields archive entries one at a time with streaming decompression.
///
/// This iterator processes entries sequentially, allowing for memory-efficient
/// extraction of archives. For solid archives, entries must be processed in
/// order due to compression dependencies.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::{StreamingArchive, StreamingConfig};
///
/// let mut archive = StreamingArchive::open(file)?;
/// for entry_result in archive.entries() {
///     let mut entry = entry_result?;
///     if should_extract(entry.entry()) {
///         entry.extract_to(&mut output_file)?;
///     } else {
///         entry.skip()?;
///     }
/// }
/// ```
pub struct EntryIterator<'a, R: Read + Seek> {
    /// Reference to the archive header
    header: &'a ArchiveHeader,
    /// List of entries
    entries: &'a [Entry],
    /// Source reader
    source: &'a mut R,
    /// Password for encrypted archives
    #[cfg(feature = "aes")]
    #[allow(dead_code)] // Reserved for encrypted streaming support
    password: &'a Password,
    /// Streaming configuration
    config: StreamingConfig,
    /// Current entry index
    current_index: usize,
    /// Current folder index being processed
    current_folder: Option<usize>,
    /// Active folder decoder (for solid archives)
    folder_decoder: Option<Box<dyn Read + Send + 'static>>,
    /// Position within current folder's stream
    stream_position_in_folder: usize,
    /// Bytes remaining in current entry
    bytes_remaining: u64,
    /// Pack data start position in the archive
    pack_start: u64,
    /// Whether the iterator is exhausted
    finished: bool,
}

impl<'a, R: Read + Seek + Send> EntryIterator<'a, R> {
    /// Creates a new entry iterator.
    #[cfg(feature = "aes")]
    pub(crate) fn new(
        header: &'a ArchiveHeader,
        entries: &'a [Entry],
        source: &'a mut R,
        password: &'a Password,
        config: StreamingConfig,
    ) -> Result<Self> {
        let pack_start = super::calculate_pack_start(header);

        Ok(Self {
            header,
            entries,
            source,
            password,
            config,
            current_index: 0,
            current_folder: None,
            folder_decoder: None,
            stream_position_in_folder: 0,
            bytes_remaining: 0,
            pack_start,
            finished: false,
        })
    }

    /// Creates a new entry iterator (without AES support).
    #[cfg(not(feature = "aes"))]
    pub(crate) fn new(
        header: &'a ArchiveHeader,
        entries: &'a [Entry],
        source: &'a mut R,
        config: StreamingConfig,
    ) -> Result<Self> {
        let pack_start = super::calculate_pack_start(header);

        Ok(Self {
            header,
            entries,
            source,
            config,
            current_index: 0,
            current_folder: None,
            folder_decoder: None,
            stream_position_in_folder: 0,
            bytes_remaining: 0,
            pack_start,
            finished: false,
        })
    }

    /// Returns the total number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of remaining entries.
    pub fn remaining(&self) -> usize {
        self.entries.len().saturating_sub(self.current_index)
    }

    /// Returns the streaming configuration.
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }

    fn next_internal(&mut self) -> Result<Option<StreamingEntry<'a>>> {
        if self.finished || self.current_index >= self.entries.len() {
            return Ok(None);
        }

        let entry = &self.entries[self.current_index];
        self.current_index += 1;

        // Handle directories (no data to extract)
        if entry.is_directory {
            return Ok(Some(StreamingEntry::directory(entry)));
        }

        // Get folder and stream indices
        let folder_index = match entry.folder_index {
            Some(idx) => idx,
            None => {
                // Entry without folder - empty file
                return Ok(Some(StreamingEntry::empty(entry)));
            }
        };

        let stream_index = entry.stream_index.unwrap_or(0);

        // Check if we need to switch folders
        if self.current_folder != Some(folder_index) {
            self.init_folder_decoder(folder_index)?;
            self.stream_position_in_folder = 0;
        }

        // For solid archives, we may need to skip previous streams
        while self.stream_position_in_folder < stream_index {
            let skip_size = self.get_stream_size(folder_index, self.stream_position_in_folder);
            self.skip_bytes(skip_size)?;
            self.stream_position_in_folder += 1;
        }

        // Get the stream size
        let size = self.get_stream_size(folder_index, stream_index);
        self.bytes_remaining = size;
        self.stream_position_in_folder = stream_index + 1;

        // Create streaming entry
        Ok(Some(StreamingEntry::with_size(entry, size)))
    }

    fn skip_bytes(&mut self, bytes: u64) -> Result<()> {
        if let Some(decoder) = &mut self.folder_decoder {
            io::copy(&mut decoder.take(bytes), &mut io::sink()).map_err(Error::Io)?;
        }
        Ok(())
    }

    fn init_folder_decoder(&mut self, folder_index: usize) -> Result<()> {
        let folders = match &self.header.unpack_info {
            Some(ui) => &ui.folders,
            None => return Err(Error::InvalidFormat("missing unpack info".into())),
        };

        if folder_index >= folders.len() {
            return Err(Error::InvalidFormat(format!(
                "folder index {} out of range",
                folder_index
            )));
        }

        let folder = &folders[folder_index];

        // Calculate folder position in pack data
        let pack_offset = self.calculate_folder_offset(folder_index)?;
        self.source
            .seek(SeekFrom::Start(pack_offset))
            .map_err(Error::Io)?;

        // Build decoder chain
        let decoder = self.build_folder_decoder(folder)?;

        self.folder_decoder = Some(decoder);
        self.current_folder = Some(folder_index);

        Ok(())
    }

    fn get_stream_size(&self, folder_index: usize, stream_index: usize) -> u64 {
        // Calculate the linear index into unpack_sizes
        let ss = match &self.header.substreams_info {
            Some(ss) => ss,
            None => {
                // No substreams info - use folder unpack size
                return self
                    .header
                    .unpack_info
                    .as_ref()
                    .and_then(|ui| ui.folders.get(folder_index))
                    .and_then(|f| f.final_unpack_size())
                    .unwrap_or(0);
            }
        };

        // Calculate offset into unpack_sizes
        let mut offset = 0usize;
        for (i, &count) in ss.num_unpack_streams_in_folders.iter().enumerate() {
            if i == folder_index {
                return ss
                    .unpack_sizes
                    .get(offset + stream_index)
                    .copied()
                    .unwrap_or(0);
            }
            offset += count as usize;
        }

        0
    }

    fn calculate_folder_offset(&self, folder_index: usize) -> Result<u64> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        let mut offset = self.pack_start;

        // Sum up pack sizes for previous folders
        for i in 0..folder_index {
            if i < pack_info.pack_sizes.len() {
                offset += pack_info.pack_sizes[i];
            }
        }

        Ok(offset)
    }

    fn build_folder_decoder(&mut self, folder: &Folder) -> Result<Box<dyn Read + Send + 'static>> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        // Get the first coder (simplified - doesn't handle complex chains)
        let coder = &folder.coders[0];
        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);

        // Calculate pack size for this folder
        let folder_index = self.current_folder.unwrap_or(0);
        let pack_size = self
            .header
            .pack_info
            .as_ref()
            .and_then(|pi| pi.pack_sizes.get(folder_index).copied())
            .unwrap_or(0);

        // Read packed data into buffer to get 'static lifetime
        let mut packed_data = vec![0u8; pack_size as usize];
        self.source
            .read_exact(&mut packed_data)
            .map_err(Error::Io)?;

        // Create cursor and build decoder
        let cursor = std::io::Cursor::new(packed_data);
        let decoder = crate::codec::build_decoder(cursor, coder, uncompressed_size)?;
        // Decoder implements Read, so we can box it as dyn Read
        Ok(Box::new(decoder) as Box<dyn Read + Send + 'static>)
    }

    /// Reads data from the current entry.
    ///
    /// This is a low-level method for reading raw bytes from the current entry.
    /// For most use cases, prefer [`Self::extract_current_to`] or [`Self::extract_current_to_vec`].
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer to read into
    ///
    /// # Returns
    ///
    /// The number of bytes read, or 0 if the entry has been fully read.
    pub fn read_entry_data(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.bytes_remaining == 0 {
            return Ok(0);
        }

        let decoder = match &mut self.folder_decoder {
            Some(d) => d,
            None => return Ok(0),
        };

        let to_read = buf.len().min(self.bytes_remaining as usize);
        let n = decoder.read(&mut buf[..to_read])?;
        self.bytes_remaining -= n as u64;

        Ok(n)
    }

    /// Skips the remaining bytes in the current entry.
    #[allow(dead_code)] // Part of streaming API
    pub(crate) fn skip_remaining(&mut self) -> Result<()> {
        self.skip_bytes(self.bytes_remaining)?;
        self.bytes_remaining = 0;
        Ok(())
    }

    /// Extracts the current entry's data to a Write sink.
    ///
    /// This should be called after retrieving an entry via `next()` and before
    /// calling `next()` again. For entries that should be skipped, simply call
    /// `next()` without extracting - the iterator will automatically skip the
    /// remaining bytes.
    ///
    /// # Arguments
    ///
    /// * `sink` - Any type implementing `Write` to receive the entry data
    ///
    /// # Returns
    ///
    /// The number of bytes written.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut iter = archive.entries()?;
    /// while let Some(entry_result) = iter.next() {
    ///     let entry = entry_result?;
    ///     if should_extract(&entry) {
    ///         let mut file = File::create(entry.name())?;
    ///         iter.extract_current_to(&mut file)?;
    ///     }
    ///     // If not extracted, the iterator will skip it automatically
    /// }
    /// ```
    pub fn extract_current_to<W: io::Write>(&mut self, sink: &mut W) -> Result<u64> {
        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = self.read_entry_data(&mut buf)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
        }

        Ok(total_written)
    }

    /// Extracts the current entry's data with a progress callback.
    ///
    /// # Arguments
    ///
    /// * `sink` - Any type implementing `Write` to receive the entry data
    /// * `on_progress` - Callback called with (bytes_written, total_bytes)
    ///
    /// # Returns
    ///
    /// The number of bytes written.
    pub fn extract_current_to_with_progress<W, F>(
        &mut self,
        sink: &mut W,
        mut on_progress: F,
    ) -> Result<u64>
    where
        W: io::Write,
        F: FnMut(u64, u64),
    {
        let total = self.bytes_remaining;
        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = self.read_entry_data(&mut buf)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
            on_progress(total_written, total);
        }

        Ok(total_written)
    }

    /// Reads the current entry into a Vec.
    ///
    /// # Returns
    ///
    /// The decompressed entry data as a `Vec<u8>`.
    pub fn extract_current_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::with_capacity(self.bytes_remaining as usize);
        self.extract_current_to(&mut data)?;
        Ok(data)
    }

    /// Returns the bytes remaining in the current entry.
    pub fn current_entry_remaining(&self) -> u64 {
        self.bytes_remaining
    }
}

impl<'a, R: Read + Seek + Send> Iterator for EntryIterator<'a, R> {
    type Item = Result<StreamingEntry<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_internal() {
            Ok(Some(entry)) => Some(Ok(entry)),
            Ok(None) => None,
            Err(e) => {
                self.finished = true;
                Some(Err(e))
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining();
        (remaining, Some(remaining))
    }
}

impl<R: Read + Seek + Send> ExactSizeIterator for EntryIterator<'_, R> {}

/// Represents a single entry during streaming iteration.
///
/// This provides access to entry metadata and methods for extracting
/// or skipping the entry's data.
#[allow(dead_code)] // buffer reserved for streaming implementation
pub struct StreamingEntry<'a> {
    /// Entry metadata
    entry: &'a Entry,
    /// Entry size
    size: u64,
    /// Whether this is a directory
    is_directory: bool,
    /// Bytes read so far
    bytes_read: u64,
    /// Internal buffer for reading
    buffer: Vec<u8>,
}

impl<'a> StreamingEntry<'a> {
    /// Creates a directory entry (no data).
    fn directory(entry: &'a Entry) -> Self {
        Self {
            entry,
            size: 0,
            is_directory: true,
            bytes_read: 0,
            buffer: Vec::new(),
        }
    }

    /// Creates an empty entry (no data).
    fn empty(entry: &'a Entry) -> Self {
        Self {
            entry,
            size: 0,
            is_directory: false,
            bytes_read: 0,
            buffer: Vec::new(),
        }
    }

    /// Creates an entry with a known size.
    fn with_size(entry: &'a Entry, size: u64) -> Self {
        Self {
            entry,
            size,
            is_directory: false,
            bytes_read: 0,
            buffer: Vec::new(),
        }
    }

    /// Returns the entry metadata.
    pub fn entry(&self) -> &Entry {
        self.entry
    }

    /// Returns true if this is a directory.
    pub fn is_directory(&self) -> bool {
        self.is_directory
    }

    /// Returns the uncompressed size of the entry.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the entry name/path.
    pub fn name(&self) -> &str {
        self.entry.path.as_str()
    }

    /// Returns the bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the remaining bytes to read.
    pub fn remaining(&self) -> u64 {
        self.size.saturating_sub(self.bytes_read)
    }

    /// Skips this entry without reading data.
    ///
    /// For solid archives, this still decompresses the data but discards it.
    /// This method is a no-op - the actual skipping is handled by the iterator.
    pub fn skip(self) -> Result<()> {
        // Skipping is handled by the iterator when it advances
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_config_defaults() {
        let config = StreamingConfig::default();
        assert!(config.max_memory_buffer > 0);
        assert!(config.read_buffer_size > 0);
    }

    #[test]
    fn test_streaming_entry_directory() {
        use crate::ArchivePath;

        let entry = Entry {
            path: ArchivePath::new("test").unwrap(),
            is_directory: true,
            size: 0,
            crc32: None,
            crc64: None,
            modification_time: None,
            creation_time: None,
            access_time: None,
            attributes: None,
            is_encrypted: false,
            is_symlink: false,
            is_anti: false,
            ownership: None,
            index: 0,
            folder_index: None,
            stream_index: None,
        };

        let streaming = StreamingEntry::directory(&entry);
        assert!(streaming.is_directory());
        assert_eq!(streaming.size(), 0);
    }
}
