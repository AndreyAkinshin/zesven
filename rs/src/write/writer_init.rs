//! Writer initialization and finalization.
//!
//! This module provides methods for creating writers and finishing archive writing,
//! including signature header writing.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::format::{SIGNATURE, SIGNATURE_HEADER_SIZE};
use crate::volume::{MultiVolumeWriter, VolumeConfig};
use crate::{Error, Result};

use super::options::{WriteOptions, WriteResult};
use super::{StreamInfo, Writer, WriterState};

impl Writer<BufWriter<File>> {
    /// Creates a new archive file at the given path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file to create
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created.
    pub fn create_path(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::create(path.as_ref()).map_err(Error::Io)?;
        let writer = BufWriter::new(file);
        Self::create(writer)
    }

    /// Finishes writing the archive.
    ///
    /// # Returns
    ///
    /// A WriteResult with statistics about the written archive.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing fails.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<std::io::Cursor<Vec<u8>>> {
    /// Finishes writing the archive to an owned cursor.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<std::io::Cursor<&mut Vec<u8>>> {
    /// Finishes writing the archive to a borrowed cursor.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<MultiVolumeWriter> {
    /// Creates a new multi-volume archive writer.
    ///
    /// The archive will be split across multiple files when each volume
    /// reaches the configured size limit.
    ///
    /// # Arguments
    ///
    /// * `config` - Volume configuration specifying size and base path
    ///
    /// # Errors
    ///
    /// Returns an error if the first volume file cannot be created.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::{Writer, VolumeConfig, ArchivePath};
    ///
    /// let config = VolumeConfig::new("archive.7z", 50 * 1024 * 1024); // 50 MB volumes
    /// let mut writer = Writer::create_multivolume(config)?;
    /// writer.add_bytes(ArchivePath::new("data.bin")?, &large_data)?;
    /// let result = writer.finish()?;
    /// println!("Created {} volumes", result.volume_count);
    /// ```
    pub fn create_multivolume(config: VolumeConfig) -> Result<Self> {
        let writer = MultiVolumeWriter::create(config)?;
        Self::create(writer)
    }

    /// Finishes writing the multi-volume archive.
    ///
    /// This finalizes all volumes and returns a WriteResult with volume information.
    ///
    /// # Returns
    ///
    /// A WriteResult with statistics including volume_count and volume_sizes.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing or volume finalization fails.
    pub fn finish(self) -> Result<WriteResult> {
        let (mut result, mv_writer) = self.finish_into_inner()?;

        // Finalize the multi-volume writer and get volume sizes
        let volume_sizes = mv_writer.finish()?;
        result.volume_count = volume_sizes.len() as u32;
        result.volume_sizes = volume_sizes;

        Ok(result)
    }
}

impl<W: Write + Seek> Writer<W> {
    /// Creates a new archive writer.
    ///
    /// # Arguments
    ///
    /// * `sink` - The writer to output archive data to
    ///
    /// # Errors
    ///
    /// Returns an error if the initial seek fails.
    pub fn create(mut sink: W) -> Result<Self> {
        // Reserve space for signature header (32 bytes) by writing zeros
        let placeholder = [0u8; SIGNATURE_HEADER_SIZE as usize];
        sink.write_all(&placeholder).map_err(Error::Io)?;

        Ok(Self {
            sink,
            options: WriteOptions::default(),
            state: WriterState::AcceptingEntries,
            entries: Vec::new(),
            stream_info: StreamInfo::default(),
            compressed_bytes: 0,
            solid_buffer: Vec::new(),
            solid_buffer_size: 0,
        })
    }

    /// Sets the write options.
    pub fn options(mut self, options: WriteOptions) -> Self {
        self.options = options;
        self
    }

    /// Finishes writing the archive and returns the underlying sink.
    ///
    /// This is useful when you need access to the written data, such as
    /// when writing to a `Cursor<Vec<u8>>` and need to retrieve the buffer.
    ///
    /// # Returns
    ///
    /// A tuple of (WriteResult, W) where W is the underlying sink.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::write::Writer;
    /// use std::io::Cursor;
    ///
    /// let mut writer = Writer::create(Cursor::new(Vec::new()))?;
    /// writer.add_bytes("test.txt".try_into()?, b"Hello")?;
    /// let (result, cursor) = writer.finish_into_inner()?;
    /// let archive_bytes = cursor.into_inner();
    /// ```
    pub fn finish_into_inner(mut self) -> Result<(WriteResult, W)> {
        self.ensure_accepting_entries()?;
        self.state = WriterState::Building;

        // Flush any remaining solid buffer
        if !self.solid_buffer.is_empty() {
            self.flush_solid_buffer()?;
        }

        // Sort entries if deterministic mode
        if self.options.deterministic {
            self.entries
                .sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        }

        // Record header position
        let header_pos = self.sink.stream_position().map_err(Error::Io)?;

        // Write header (optionally encrypted)
        let header_data = self.encode_header()?;

        #[cfg(feature = "aes")]
        let header_data = if self.options.is_header_encrypted() {
            self.encode_encrypted_header(&header_data)?
        } else {
            header_data
        };

        self.sink.write_all(&header_data).map_err(Error::Io)?;

        // Write signature header at start
        self.write_signature_header(header_pos, &header_data)?;

        self.state = WriterState::Finished;

        // Get final position for single-file archive size
        let final_pos = self.sink.stream_position().map_err(Error::Io)?;

        // Build result
        let result = WriteResult {
            entries_written: self
                .entries
                .iter()
                .filter(|e| !e.meta.is_directory && !e.meta.is_anti)
                .count(),
            directories_written: self.entries.iter().filter(|e| e.meta.is_directory).count(),
            total_size: self.entries.iter().map(|e| e.uncompressed_size).sum(),
            compressed_size: self.compressed_bytes,
            volume_count: 1,
            volume_sizes: vec![final_pos],
        };

        Ok((result, self.sink))
    }

    /// Writes the signature header at the start of the file.
    pub(crate) fn write_signature_header(
        &mut self,
        header_pos: u64,
        header_data: &[u8],
    ) -> Result<()> {
        // Calculate values
        let next_header_offset = header_pos - SIGNATURE_HEADER_SIZE;
        let next_header_size = header_data.len() as u64;
        let next_header_crc = crc32fast::hash(header_data);

        // Build start header (20 bytes)
        let mut start_header = Vec::with_capacity(20);
        start_header.extend_from_slice(&next_header_offset.to_le_bytes());
        start_header.extend_from_slice(&next_header_size.to_le_bytes());
        start_header.extend_from_slice(&next_header_crc.to_le_bytes());

        let start_header_crc = crc32fast::hash(&start_header);

        // Seek to start and write signature header
        self.sink.seek(SeekFrom::Start(0)).map_err(Error::Io)?;

        // Write signature (6 bytes)
        self.sink.write_all(SIGNATURE).map_err(Error::Io)?;

        // Write version (2 bytes)
        self.sink.write_all(&[0x00, 0x04]).map_err(Error::Io)?;

        // Write start header CRC (4 bytes)
        self.sink
            .write_all(&start_header_crc.to_le_bytes())
            .map_err(Error::Io)?;

        // Write start header (20 bytes)
        self.sink.write_all(&start_header).map_err(Error::Io)?;

        Ok(())
    }

    /// Ensures the writer is in the AcceptingEntries state.
    pub(crate) fn ensure_accepting_entries(&self) -> Result<()> {
        if self.state != WriterState::AcceptingEntries {
            return Err(Error::InvalidFormat(
                "Writer is not accepting entries".into(),
            ));
        }
        Ok(())
    }
}
