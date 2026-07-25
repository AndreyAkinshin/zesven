//! Decompression chain building.
//!
//! This module provides functions for building decoder chains for
//! various compression methods and filter combinations.

use std::io::{Cursor, Read, Write};

use crate::format::streams::Folder;
use crate::{Error, READ_BUFFER_SIZE, Result, codec};

#[cfg(feature = "aes")]
use super::entries;
use super::{Archive, ExtractionLimits, map_io_error};

impl<R: Read + std::io::Seek> Archive<R> {
    /// Decompresses a standard (non-BCJ2) entry to a sink.
    ///
    /// Handles both solid and non-solid entries by dispatching to the appropriate
    /// decompression method. This helper eliminates code duplication between
    /// cfg(feature = "lzma") and cfg(not(feature = "lzma")) paths.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decompress_standard_entry(
        &mut self,
        packed_data: Vec<u8>,
        folder: &Folder,
        folder_idx: usize,
        stream_index: Option<usize>,
        entry_size: u64,
        sink: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<()> {
        if self.is_solid_block(folder_idx) {
            self.decompress_to_sink_solid(
                packed_data,
                folder,
                folder_idx,
                stream_index.unwrap_or(0),
                sink,
                limits,
            )?;
        } else {
            self.decompress_to_sink_non_solid(packed_data, folder, entry_size, sink, limits)?;
        }
        Ok(())
    }

    /// Decompresses non-solid entry to a sink.
    pub(crate) fn decompress_to_sink_non_solid(
        &self,
        packed_data: Vec<u8>,
        folder: &Folder,
        expected_size: u64,
        sink: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let uncompressed_size = folder.final_unpack_size().unwrap_or(expected_size);
        let compressed_size = packed_data.len() as u64;
        let cursor = Cursor::new(packed_data);

        // Build decoder chain - handles both single coders and filter+codec chains
        let decoder = self.build_decoder_chain(cursor, folder, uncompressed_size)?;

        // Wrap decoder with LimitedReader for resource limit enforcement
        let mut limited_decoder = limits.wrap_reader(decoder, compressed_size);

        let mut total = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = limited_decoder.read(&mut buf).map_err(map_io_error)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(Error::Io)?;
            total += n as u64;
        }

        Ok(total)
    }

    /// Builds a decoder chain for a folder, handling filter+codec combinations
    /// and encrypted folders.
    ///
    /// For single-coder folders, returns a simple decoder.
    /// For two-coder folders (filter + codec), chains them in correct order:
    /// packed_data -> codec -> filter -> output
    ///
    /// For encrypted folders, uses the stored password to decrypt.
    pub(crate) fn build_decoder_chain<T: Read + Send + 'static>(
        &self,
        input: T,
        folder: &Folder,
        uncompressed_size: u64,
    ) -> Result<Box<dyn Read + Send>> {
        // Check if the folder uses AES encryption
        #[cfg(feature = "aes")]
        if entries::folder_uses_encryption(folder) {
            let password = self.password.as_ref().ok_or(Error::PasswordRequired)?;
            let decoder =
                codec::build_encrypted_folder_decoder(input, folder, uncompressed_size, password)?;
            return Ok(Box::new(decoder));
        }

        codec::build_decoder_chain(input, folder, uncompressed_size)
    }

    /// Decompresses solid block entry to a sink.
    ///
    /// Supports filter+codec combinations (e.g., BCJ + LZMA2) in solid blocks
    /// by building a full decoder chain.
    pub(crate) fn decompress_to_sink_solid(
        &self,
        packed_data: Vec<u8>,
        folder: &Folder,
        folder_idx: usize,
        stream_index: usize,
        sink: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let entry_sizes = self.get_solid_block_entry_sizes(folder_idx)?;

        if stream_index >= entry_sizes.len() {
            return Err(Error::InvalidFormat(format!(
                "stream index {} out of range for solid block",
                stream_index
            )));
        }

        let uncompressed_size = folder.final_unpack_size().unwrap_or(0);
        let compressed_size = packed_data.len() as u64;

        let cursor = Cursor::new(packed_data);
        // Go through the reader's own chain builder so that encrypted solid blocks
        // are decrypted; calling the codec helper directly skipped the password.
        let mut decoder = self.build_decoder_chain(cursor, folder, uncompressed_size)?;

        // Skip entries before the target (no limit enforcement on skipped data)
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

        // Read the target entry to sink with limit enforcement
        let target_size = entry_sizes[stream_index];
        let mut limited_decoder = limits.wrap_reader(&mut decoder, compressed_size);

        let mut remaining = target_size;
        let mut total = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        while remaining > 0 {
            let to_read = buf.len().min(remaining as usize);
            let n = limited_decoder
                .read(&mut buf[..to_read])
                .map_err(map_io_error)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(Error::Io)?;
            total += n as u64;
            remaining -= n as u64;
        }

        Ok(total)
    }
}
