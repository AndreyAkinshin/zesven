//! Solid block handling.
//!
//! This module provides functions for working with solid archives where
//! multiple files are compressed together in a single block.

use std::io::{Read, Seek};
#[cfg(feature = "lzma")]
use std::io::{SeekFrom, Write};

use crate::format::SIGNATURE_HEADER_SIZE;
#[cfg(feature = "lzma")]
use crate::format::streams::Folder;
use crate::{Error, Result};
#[cfg(feature = "lzma")]
use crate::{READ_BUFFER_SIZE, codec};

use super::Archive;
#[cfg(feature = "lzma")]
use super::{ExtractionLimits, map_io_error};

impl<R: Read + Seek> Archive<R> {
    /// Calculates the pack position for a folder.
    pub(crate) fn calculate_pack_position(&self, folder_idx: usize) -> Result<u64> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        // Start after SFX stub (if any) + signature header (32 bytes) + pack_pos
        let mut offset = self.sfx_offset + SIGNATURE_HEADER_SIZE + pack_info.pack_pos;

        // Sum up pack sizes for previous folders
        for i in 0..folder_idx {
            if i < pack_info.pack_sizes.len() {
                offset += pack_info.pack_sizes[i];
            }
        }

        Ok(offset)
    }

    /// Checks if a folder is a solid block (contains multiple files).
    pub(crate) fn is_solid_block(&self, folder_idx: usize) -> bool {
        self.header
            .substreams_info
            .as_ref()
            .and_then(|ss| ss.num_unpack_streams_in_folders.get(folder_idx))
            .map(|&count| count > 1)
            .unwrap_or(false)
    }

    /// Gets entry sizes for a solid block.
    pub(crate) fn get_solid_block_entry_sizes(&self, folder_idx: usize) -> Result<Vec<u64>> {
        let substreams = self
            .header
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

    /// Calculates the pack stream base index for a folder.
    ///
    /// For multi-stream folders (like BCJ2), we need to know where this folder's
    /// pack streams start in the global PackInfo.pack_sizes array.
    #[cfg(feature = "lzma")]
    pub(crate) fn calculate_folder_pack_base(&self, folder_idx: usize) -> Result<usize> {
        let unpack_info = self
            .header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        let mut base = 0;
        for i in 0..folder_idx {
            if let Some(folder) = unpack_info.folders.get(i) {
                base += folder.packed_streams.len();
            }
        }
        Ok(base)
    }

    /// Reads all pack streams for a folder.
    ///
    /// Returns a Vec of Vec<u8>, one for each pack stream in the folder.
    #[cfg(feature = "lzma")]
    pub(crate) fn read_folder_pack_streams(
        &mut self,
        folder: &Folder,
        folder_idx: usize,
    ) -> Result<Vec<Vec<u8>>> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        let pack_base = self.calculate_folder_pack_base(folder_idx)?;
        let num_pack_streams = folder.packed_streams.len();

        // Calculate the starting offset for this folder's pack streams
        // Include SFX offset for self-extracting archives
        let mut pack_offset = self.sfx_offset + SIGNATURE_HEADER_SIZE + pack_info.pack_pos;
        for i in 0..pack_base {
            if i < pack_info.pack_sizes.len() {
                pack_offset += pack_info.pack_sizes[i];
            }
        }

        let mut pack_data = Vec::with_capacity(num_pack_streams);

        for i in 0..num_pack_streams {
            let pack_idx = pack_base + i;
            let pack_size = pack_info.pack_sizes.get(pack_idx).copied().ok_or_else(|| {
                Error::InvalidFormat(format!(
                    "missing pack size for stream {} (pack_idx {})",
                    i, pack_idx
                ))
            })?;

            self.reader
                .seek(SeekFrom::Start(pack_offset))
                .map_err(Error::Io)?;

            let mut data = vec![0u8; pack_size as usize];
            self.reader.read_exact(&mut data).map_err(Error::Io)?;

            pack_data.push(data);
            pack_offset += pack_size;
        }

        Ok(pack_data)
    }

    /// Extracts a BCJ2-compressed entry.
    ///
    /// For solid archives, extracts only the specified stream (file) from the block.
    #[cfg(feature = "lzma")]
    pub(crate) fn extract_bcj2(
        &mut self,
        folder: &Folder,
        folder_idx: usize,
        stream_index: Option<usize>,
        output: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        // Read all pack streams for this folder
        let pack_data = self.read_folder_pack_streams(folder, folder_idx)?;

        // Calculate total compressed size for ratio limiting
        let compressed_size: u64 = pack_data.iter().map(|p| p.len() as u64).sum();

        // Build BCJ2 decoder
        let mut decoder = codec::build_bcj2_folder_decoder(folder, &pack_data)?;

        // Check if this is a solid block (multiple files in one folder)
        // Use is_solid_block() first to avoid requiring SubStreamsInfo for non-solid BCJ2
        let is_solid = self.is_solid_block(folder_idx);

        if is_solid {
            let entry_sizes = self.get_solid_block_entry_sizes(folder_idx)?;
            let stream_idx = stream_index.unwrap_or(0);

            if stream_idx >= entry_sizes.len() {
                return Err(Error::InvalidFormat(format!(
                    "stream index {} out of range for solid BCJ2 block",
                    stream_idx
                )));
            }

            // Skip entries before the target (no limit enforcement on skipped data)
            let mut buf = [0u8; READ_BUFFER_SIZE];
            for &skip_size in entry_sizes.iter().take(stream_idx) {
                let mut remaining = skip_size;
                while remaining > 0 {
                    let to_read = buf.len().min(remaining as usize);
                    let n = decoder.read(&mut buf[..to_read]).map_err(Error::Io)?;
                    if n == 0 {
                        return Err(Error::InvalidFormat(
                            "unexpected end of BCJ2 stream while skipping".into(),
                        ));
                    }
                    remaining -= n as u64;
                }
            }

            // Read only the target entry with limit enforcement
            let target_size = entry_sizes[stream_idx];
            let mut limited_decoder = limits.wrap_reader(&mut decoder, compressed_size);

            let mut remaining = target_size;
            let mut total_written = 0u64;

            while remaining > 0 {
                let to_read = buf.len().min(remaining as usize);
                let n = limited_decoder
                    .read(&mut buf[..to_read])
                    .map_err(map_io_error)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).map_err(Error::Io)?;
                total_written += n as u64;
                remaining -= n as u64;
            }

            Ok(total_written)
        } else {
            // Non-solid: decompress and write everything with limit enforcement
            let mut limited_decoder = limits.wrap_reader(&mut decoder, compressed_size);

            let mut total_written = 0u64;
            let mut buf = [0u8; READ_BUFFER_SIZE];

            loop {
                let n = limited_decoder.read(&mut buf).map_err(map_io_error)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).map_err(Error::Io)?;
                total_written += n as u64;
            }

            Ok(total_written)
        }
    }
}
