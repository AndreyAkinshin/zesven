//! Entry compression methods.
//!
//! This module provides functions for compressing individual entries,
//! including solid and non-solid compression modes, and BCJ2 filter handling.

use std::io::{Read, Seek, Write};

use crate::{ArchivePath, Error, Result};

use super::options::EntryMeta;
use super::{Bcj2FolderInfo, PendingEntry, SolidBufferEntry, Writer};

impl<W: Write + Seek> Writer<W> {
    /// Compresses an entry in non-solid mode.
    pub(crate) fn compress_entry_non_solid(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        // Read all data and compute CRC
        let mut data = Vec::new();
        source.read_to_end(&mut data).map_err(Error::Io)?;

        // Check if BCJ2 filter is active - route to dedicated method
        if self.options.filter.is_bcj2() {
            return self.compress_entry_bcj2(archive_path, &data, meta);
        }

        let crc = crc32fast::hash(&data);
        let uncompressed_size = data.len() as u64;

        // Add entry (always, even for empty files)
        let entry = PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        };
        self.entries.push(entry);

        // Empty files don't get a folder/stream - they're marked as EmptyStream/EmptyFile
        // in the header. Only non-empty files need compression and stream tracking.
        if data.is_empty() {
            return Ok(());
        }

        // Process data through filter -> compress -> encrypt pipeline
        // 4 cases:
        // 1. No filter, no encryption -> 1-coder folder
        // 2. Filter only -> 2-coder folder (filter + codec)
        // 3. Encryption only -> 2-coder folder (AES + codec)
        // 4. Filter + encryption -> 3-coder folder (AES + codec + filter)
        #[cfg(feature = "aes")]
        let (output_data, filter_info, encryption_info) = if self.options.is_data_encrypted() {
            let (encrypted, filter_info, enc_info) =
                self.filter_compress_and_encrypt_data(&data)?;
            (encrypted, filter_info, Some(enc_info))
        } else {
            let (compressed, filter_info) = self.filter_and_compress_data(&data)?;
            (compressed, filter_info, None)
        };

        #[cfg(not(feature = "aes"))]
        let (output_data, filter_info, encryption_info) = {
            let (compressed, filter_info) = self.filter_and_compress_data(&data)?;
            (compressed, filter_info, Option::<()>::None)
        };

        let packed_size = output_data.len() as u64;

        // Write compressed (and possibly encrypted) data
        self.sink.write_all(&output_data).map_err(Error::Io)?;
        self.compressed_bytes += packed_size;

        // Track stream info (only for non-empty files)
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.crcs.push(crc);

        // Track encryption info for header writing
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(encryption_info);

        // Track filter info for header writing
        self.stream_info.filter_info.push(filter_info);

        // Track that this is not a BCJ2 folder
        self.stream_info.bcj2_folder_info.push(None);

        // Suppress unused variable warning when aes feature is disabled
        #[cfg(not(feature = "aes"))]
        let _ = encryption_info;

        // Track that this is a non-solid folder (1 stream per folder)
        self.stream_info.num_unpack_streams_per_folder.push(1);

        Ok(())
    }

    /// Compresses an entry using BCJ2 4-stream filter.
    ///
    /// BCJ2 splits x86 code into 4 streams for improved compression:
    /// - Stream 0: Main code
    /// - Stream 1: CALL destinations (big-endian)
    /// - Stream 2: JMP destinations (big-endian)
    /// - Stream 3: Range-coded selector bits
    pub(crate) fn compress_entry_bcj2(
        &mut self,
        archive_path: ArchivePath,
        data: &[u8],
        meta: EntryMeta,
    ) -> Result<()> {
        use crate::codec::bcj2::bcj2_encode;

        let crc = crc32fast::hash(data);
        let uncompressed_size = data.len() as u64;

        // Add entry
        let entry = PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        };
        self.entries.push(entry);

        // Empty files don't get a folder/stream
        if data.is_empty() {
            return Ok(());
        }

        // Encode with BCJ2 - produces 4 streams
        let streams = bcj2_encode(data);

        // Write all 4 streams sequentially to output
        self.sink.write_all(&streams.main).map_err(Error::Io)?;
        self.sink.write_all(&streams.call).map_err(Error::Io)?;
        self.sink.write_all(&streams.jump).map_err(Error::Io)?;
        self.sink.write_all(&streams.range).map_err(Error::Io)?;

        let total_packed = streams.total_size() as u64;
        self.compressed_bytes += total_packed;

        // Track BCJ2 folder info
        let bcj2_info = Bcj2FolderInfo {
            pack_sizes: [
                streams.main.len() as u64,
                streams.call.len() as u64,
                streams.jump.len() as u64,
                streams.range.len() as u64,
            ],
        };

        // For BCJ2, we don't use pack_sizes (handled separately)
        // Store unpack_size and CRC
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.crcs.push(crc);

        // Track filter info as None (BCJ2 handled separately)
        self.stream_info.filter_info.push(None);

        // Track BCJ2 folder info
        self.stream_info.bcj2_folder_info.push(Some(bcj2_info));

        // Track encryption info (BCJ2 + encryption not supported yet)
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(None);

        // Track that this is a non-solid folder (1 stream per folder)
        self.stream_info.num_unpack_streams_per_folder.push(1);

        Ok(())
    }

    /// Buffers an entry for solid compression.
    pub(crate) fn buffer_entry_solid(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        // Read all data and compute CRC
        let mut data = Vec::new();
        source.read_to_end(&mut data).map_err(Error::Io)?;
        let crc = crc32fast::hash(&data);
        let data_size = data.len() as u64;

        // Buffer the entry
        self.solid_buffer_size += data_size;
        self.solid_buffer.push(SolidBufferEntry {
            path: archive_path,
            data,
            meta,
            crc,
        });

        // Check if buffer should be flushed
        let size_exceeded = self
            .options
            .solid
            .block_size
            .is_some_and(|limit| self.solid_buffer_size >= limit);
        let count_exceeded = self
            .options
            .solid
            .files_per_block
            .is_some_and(|limit| self.solid_buffer.len() >= limit);

        if size_exceeded || count_exceeded {
            self.flush_solid_buffer()?;
        }

        Ok(())
    }

    /// Flushes the solid buffer, compressing all buffered entries as one block.
    pub(crate) fn flush_solid_buffer(&mut self) -> Result<()> {
        if self.solid_buffer.is_empty() {
            return Ok(());
        }

        // Concatenate all entry data (only non-empty entries have data streams)
        let total_uncompressed: u64 = self.solid_buffer.iter().map(|e| e.data.len() as u64).sum();
        let mut combined = Vec::with_capacity(total_uncompressed as usize);

        // Collect sizes and CRCs for substreams (only non-empty entries)
        // Empty entries (size=0) are marked as EmptyStream and don't have data streams
        let mut sizes = Vec::new();
        let mut crcs = Vec::new();
        let mut num_streams = 0u64;

        for entry in &self.solid_buffer {
            if !entry.data.is_empty() {
                combined.extend_from_slice(&entry.data);
                sizes.push(entry.data.len() as u64);
                crcs.push(entry.crc);
                num_streams += 1;
            }
            // Empty entries are handled via EmptyStream/EmptyFile in FilesInfo
        }

        // Process data through filter -> compress -> encrypt pipeline
        #[cfg(feature = "aes")]
        let (output_data, filter_info, encryption_info) = if self.options.is_data_encrypted() {
            let (encrypted, filter_info, enc_info) =
                self.filter_compress_and_encrypt_data(&combined)?;
            (encrypted, filter_info, Some(enc_info))
        } else {
            let (compressed, filter_info) = self.filter_and_compress_data(&combined)?;
            (compressed, filter_info, None)
        };

        #[cfg(not(feature = "aes"))]
        let (output_data, filter_info, encryption_info) = {
            let (compressed, filter_info) = self.filter_and_compress_data(&combined)?;
            (compressed, filter_info, Option::<()>::None)
        };

        let packed_size = output_data.len() as u64;

        // Write compressed (and possibly encrypted) data
        self.sink.write_all(&output_data).map_err(Error::Io)?;
        self.compressed_bytes += packed_size;

        // Record ONE folder with streams for non-empty entries only
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(total_uncompressed);

        // Track encryption info for header writing
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(encryption_info);

        // Track filter info for header writing
        self.stream_info.filter_info.push(filter_info);

        // Track that this is not a BCJ2 folder
        self.stream_info.bcj2_folder_info.push(None);

        // Suppress unused variable warning when aes feature is disabled
        #[cfg(not(feature = "aes"))]
        let _ = encryption_info;

        // Record number of streams in this folder (only non-empty entries)
        self.stream_info
            .num_unpack_streams_per_folder
            .push(num_streams);

        // For solid blocks with multiple streams, folder CRC is not used (substream CRCs are).
        // For solid blocks with exactly 1 stream, use folder CRC directly (no SubStreamsInfo needed).
        if num_streams == 1 {
            // Single non-empty file: use folder CRC, no substreams needed
            self.stream_info
                .crcs
                .push(crcs.first().copied().unwrap_or(0));
            // Don't add to substream_sizes/crcs - not needed for single stream
        } else {
            // Multiple non-empty files: use substream CRCs
            self.stream_info.crcs.push(0);
            self.stream_info.substream_sizes.extend_from_slice(&sizes);
            self.stream_info.substream_crcs.extend_from_slice(&crcs);
        }

        // Create entries for all buffered files
        for entry in self.solid_buffer.drain(..) {
            let uncompressed_size = entry.data.len() as u64;
            self.entries.push(PendingEntry {
                path: entry.path,
                meta: entry.meta,
                uncompressed_size,
            });
        }

        self.solid_buffer_size = 0;

        Ok(())
    }
}
