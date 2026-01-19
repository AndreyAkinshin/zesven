//! Header encoding for 7z archives.
//!
//! This module provides functions for encoding the main archive header,
//! including folder definitions, coder chains, and file metadata.

use std::io::{Seek, Write};

use crate::Result;
use crate::format::property_id;
use crate::format::reader::write_variable_u64;

use super::encoding_utils::encode_bool_vector;
use super::{FilteredFolderInfo, Writer};

impl<W: Write + Seek> Writer<W> {
    /// Encodes the archive header.
    pub(crate) fn encode_header(&self) -> Result<Vec<u8>> {
        let mut header = Vec::new();

        // Header marker
        header.push(property_id::HEADER);

        // Check if we have BCJ2 folders
        let has_bcj2 = self
            .stream_info
            .bcj2_folder_info
            .iter()
            .any(|f| f.is_some());

        // Count total pack streams:
        // - Regular folders: 1 pack stream each
        // - BCJ2 folders: 4 pack streams each
        let total_pack_streams: usize = self
            .stream_info
            .bcj2_folder_info
            .iter()
            .map(|f| if f.is_some() { 4 } else { 1 })
            .sum();

        // MainStreamsInfo (if we have data)
        let has_streams = !self.stream_info.pack_sizes.is_empty() || has_bcj2;
        if has_streams {
            header.push(property_id::MAIN_STREAMS_INFO);

            // PackInfo
            header.push(property_id::PACK_INFO);
            write_variable_u64(&mut header, 0)?; // pack_pos (relative to data start)
            write_variable_u64(&mut header, total_pack_streams as u64)?;

            // Pack sizes - write all pack sizes for all folders
            header.push(property_id::SIZE);
            let mut non_bcj2_pack_idx = 0;
            for bcj2_info in self.stream_info.bcj2_folder_info.iter() {
                if let Some(info) = bcj2_info {
                    // BCJ2 folder: 4 pack sizes
                    for &size in &info.pack_sizes {
                        write_variable_u64(&mut header, size)?;
                    }
                } else {
                    // Regular folder: 1 pack size
                    if let Some(&size) = self.stream_info.pack_sizes.get(non_bcj2_pack_idx) {
                        write_variable_u64(&mut header, size)?;
                    }
                    non_bcj2_pack_idx += 1;
                }
            }
            header.push(property_id::END);

            // UnpackInfo
            header.push(property_id::UNPACK_INFO);
            header.push(property_id::FOLDER);
            write_variable_u64(&mut header, self.stream_info.unpack_sizes.len() as u64)?;

            // External = 0 (coders inline)
            header.push(0);

            // For each folder (one per file in non-solid mode)
            for i in 0..self.stream_info.unpack_sizes.len() {
                self.encode_folder(&mut header, i)?;
            }

            // Unpack sizes
            header.push(property_id::CODERS_UNPACK_SIZE);
            for (i, &unpack_size) in self.stream_info.unpack_sizes.iter().enumerate() {
                self.encode_unpack_sizes(&mut header, i, unpack_size)?;
            }

            // CRCs for folders
            header.push(property_id::CRC);
            header.push(1); // all defined
            for &crc in &self.stream_info.crcs {
                header.extend_from_slice(&crc.to_le_bytes());
            }

            header.push(property_id::END); // End UnpackInfo

            // SubStreamsInfo - needed if any folder has more than 1 stream
            self.encode_substreams_info(&mut header)?;

            header.push(property_id::END); // End MainStreamsInfo
        }

        // FilesInfo
        self.encode_files_info(&mut header)?;

        header.push(property_id::END); // End Header

        Ok(header)
    }

    /// Encodes a single folder's coder chain.
    fn encode_folder(&self, header: &mut Vec<u8>, folder_idx: usize) -> Result<()> {
        // Check if this is a BCJ2 folder
        let bcj2_info = self
            .stream_info
            .bcj2_folder_info
            .get(folder_idx)
            .and_then(|f| f.as_ref());

        // BCJ2 folders have special encoding
        if bcj2_info.is_some() {
            // BCJ2 folder: 1 coder with 4 inputs, 1 output
            header.push(0x01); // num_coders = 1

            // BCJ2 coder definition
            // flags: method_id_len | 0x10 (complex = has num_in/out streams)
            let method_id = crate::codec::method::BCJ2;
            let flags = (method_id.len() as u8) | 0x10;
            header.push(flags);
            header.extend_from_slice(method_id);

            // num_in_streams = 4, num_out_streams = 1
            write_variable_u64(header, 4)?;
            write_variable_u64(header, 1)?;

            // No bind pairs (num_bind_pairs = num_out - 1 = 0)

            // Write packed stream indices explicitly (since num_packed > 1)
            // num_packed = total_in - num_bind_pairs = 4 - 0 = 4
            // The 4 inputs map to input stream indices 0, 1, 2, 3
            write_variable_u64(header, 0)?; // main stream -> input 0
            write_variable_u64(header, 1)?; // call stream -> input 1
            write_variable_u64(header, 2)?; // jump stream -> input 2
            write_variable_u64(header, 3)?; // range stream -> input 3

            return Ok(());
        }

        // Check if this folder has a filter
        let filter_info = self
            .stream_info
            .filter_info
            .get(folder_idx)
            .and_then(|f| f.as_ref());

        // Check if this folder is encrypted
        #[cfg(feature = "aes")]
        let encryption_info = self
            .stream_info
            .encryption_info
            .get(folder_idx)
            .and_then(|e| e.as_ref());

        #[cfg(feature = "aes")]
        {
            match (filter_info, encryption_info) {
                // Case 4: Filter + Encryption -> 3-coder folder
                (Some(flt_info), Some(enc_info)) => {
                    header.push(0x03); // num_coders = 3

                    // Coder 0: AES (decryption)
                    self.write_aes_coder(header, &enc_info.aes_properties)?;

                    // Coder 1: Filter (unfiltering)
                    self.write_filter_coder(header, flt_info)?;

                    // Coder 2: Compression (decompression)
                    self.write_compression_coder(header)?;

                    // BindPairs: connect AES -> Compression -> Filter
                    write_variable_u64(header, 2)?; // in_index (compression input)
                    write_variable_u64(header, 0)?; // out_index (AES output)
                    write_variable_u64(header, 1)?; // in_index (filter input)
                    write_variable_u64(header, 2)?; // out_index (compression output)
                }

                // Case 3: Encryption only -> 2-coder folder
                (None, Some(enc_info)) => {
                    header.push(0x02); // num_coders = 2

                    self.write_aes_coder(header, &enc_info.aes_properties)?;
                    self.write_compression_coder(header)?;

                    // BindPair: AES output (0) -> Codec input (1)
                    write_variable_u64(header, 1)?; // in_index
                    write_variable_u64(header, 0)?; // out_index
                }

                // Case 2: Filter only -> 2-coder folder
                (Some(flt_info), None) => {
                    header.push(0x02); // num_coders = 2

                    self.write_filter_coder(header, flt_info)?;
                    self.write_compression_coder(header)?;

                    // BindPair: Codec output (1) -> Filter input (0)
                    write_variable_u64(header, 0)?; // in_index
                    write_variable_u64(header, 1)?; // out_index
                }

                // Case 1: No filter, no encryption -> 1-coder folder
                (None, None) => {
                    header.push(0x01);
                    self.write_compression_coder(header)?;
                }
            }
        }

        #[cfg(not(feature = "aes"))]
        {
            if let Some(flt_info) = filter_info {
                // Case 2: Filter only -> 2-coder folder
                header.push(0x02); // num_coders = 2

                self.write_filter_coder(header, flt_info)?;
                self.write_compression_coder(header)?;

                // BindPair: Codec output (1) -> Filter input (0)
                write_variable_u64(header, 0)?; // in_index
                write_variable_u64(header, 1)?; // out_index
            } else {
                // Case 1: No filter, no encryption -> 1-coder folder
                header.push(0x01);
                self.write_compression_coder(header)?;
            }
        }

        Ok(())
    }

    /// Encodes unpack sizes for a folder.
    fn encode_unpack_sizes(
        &self,
        header: &mut Vec<u8>,
        folder_idx: usize,
        unpack_size: u64,
    ) -> Result<()> {
        // Check for BCJ2 folder
        let bcj2_info = self
            .stream_info
            .bcj2_folder_info
            .get(folder_idx)
            .and_then(|f| f.as_ref());

        // BCJ2 folder: single unpack size (final decoded output)
        if bcj2_info.is_some() {
            write_variable_u64(header, unpack_size)?;
            return Ok(());
        }

        let filter_info = self
            .stream_info
            .filter_info
            .get(folder_idx)
            .and_then(|f| f.as_ref());

        #[cfg(feature = "aes")]
        let encryption_info = self
            .stream_info
            .encryption_info
            .get(folder_idx)
            .and_then(|e| e.as_ref());

        #[cfg(feature = "aes")]
        {
            match (filter_info, encryption_info) {
                // Case 4: Filter + Encryption -> 3 coders, 3 sizes
                (Some(flt_info), Some(enc_info)) => {
                    write_variable_u64(header, unpack_size)?;
                    write_variable_u64(header, flt_info.filtered_size)?;
                    write_variable_u64(header, enc_info.compressed_size)?;
                }

                // Case 3: Encryption only -> 2 coders, 2 sizes
                (None, Some(enc_info)) => {
                    write_variable_u64(header, unpack_size)?;
                    write_variable_u64(header, enc_info.compressed_size)?;
                }

                // Case 2: Filter only -> 2 coders, 2 sizes
                (Some(flt_info), None) => {
                    write_variable_u64(header, unpack_size)?;
                    write_variable_u64(header, flt_info.filtered_size)?;
                }

                // Case 1: Single coder
                (None, None) => {
                    write_variable_u64(header, unpack_size)?;
                }
            }
        }

        #[cfg(not(feature = "aes"))]
        {
            if let Some(flt_info) = filter_info {
                write_variable_u64(header, unpack_size)?;
                write_variable_u64(header, flt_info.filtered_size)?;
            } else {
                write_variable_u64(header, unpack_size)?;
            }
        }

        Ok(())
    }

    /// Encodes SubStreamsInfo section.
    fn encode_substreams_info(&self, header: &mut Vec<u8>) -> Result<()> {
        let has_substreams = self
            .stream_info
            .num_unpack_streams_per_folder
            .iter()
            .any(|&n| n > 1);

        if !has_substreams {
            return Ok(());
        }

        header.push(property_id::SUBSTREAMS_INFO);

        // NumUnpackStream
        header.push(property_id::NUM_UNPACK_STREAM);
        for &count in &self.stream_info.num_unpack_streams_per_folder {
            write_variable_u64(header, count)?;
        }

        // Sizes for substreams (all except last in each folder)
        if !self.stream_info.substream_sizes.is_empty() {
            header.push(property_id::SIZE);

            let mut stream_idx = 0;
            for &count in &self.stream_info.num_unpack_streams_per_folder {
                // Write all sizes except the last one in each folder
                for i in 0..(count as usize).saturating_sub(1) {
                    if stream_idx + i < self.stream_info.substream_sizes.len() {
                        write_variable_u64(
                            header,
                            self.stream_info.substream_sizes[stream_idx + i],
                        )?;
                    }
                }
                stream_idx += count as usize;
            }
        }

        // CRCs for substreams
        if !self.stream_info.substream_crcs.is_empty() {
            header.push(property_id::CRC);
            header.push(1); // all defined
            for &crc in &self.stream_info.substream_crcs {
                header.extend_from_slice(&crc.to_le_bytes());
            }
        }

        header.push(property_id::END); // End SubStreamsInfo

        Ok(())
    }

    /// Encodes FilesInfo section.
    fn encode_files_info(&self, header: &mut Vec<u8>) -> Result<()> {
        if self.entries.is_empty() {
            return Ok(());
        }

        header.push(property_id::FILES_INFO);
        write_variable_u64(header, self.entries.len() as u64)?;

        // EmptyStream (directories and empty files)
        let empty_entries: Vec<_> = self
            .entries
            .iter()
            .map(|e| e.meta.is_directory || e.uncompressed_size == 0)
            .collect();

        if empty_entries.iter().any(|&x| x) {
            header.push(property_id::EMPTY_STREAM);
            let bool_vec = encode_bool_vector(&empty_entries);
            write_variable_u64(header, bool_vec.len() as u64)?;
            header.extend_from_slice(&bool_vec);

            // EmptyFile (empty files that are not directories)
            let empty_files: Vec<_> = self
                .entries
                .iter()
                .filter(|e| e.meta.is_directory || e.uncompressed_size == 0)
                .map(|e| !e.meta.is_directory)
                .collect();

            if empty_files.iter().any(|&x| x) {
                header.push(property_id::EMPTY_FILE);
                let bool_vec = encode_bool_vector(&empty_files);
                write_variable_u64(header, bool_vec.len() as u64)?;
                header.extend_from_slice(&bool_vec);
            }

            // Anti (anti-items are empty entries marked for deletion)
            let anti_items: Vec<_> = self
                .entries
                .iter()
                .filter(|e| e.meta.is_directory || e.uncompressed_size == 0)
                .map(|e| e.meta.is_anti)
                .collect();

            if anti_items.iter().any(|&x| x) {
                header.push(property_id::ANTI);
                let bool_vec = encode_bool_vector(&anti_items);
                write_variable_u64(header, bool_vec.len() as u64)?;
                header.extend_from_slice(&bool_vec);
            }
        }

        // Names
        header.push(property_id::NAME);
        let names_data = self.encode_names();
        write_variable_u64(header, names_data.len() as u64 + 1)?; // +1 for external byte
        header.push(0); // external = 0
        header.extend_from_slice(&names_data);

        // MTime (if any entries have it)
        let has_mtime: Vec<_> = self
            .entries
            .iter()
            .map(|e| e.meta.modification_time.is_some())
            .collect();
        if has_mtime.iter().any(|&x| x) {
            header.push(property_id::MTIME);
            let mtime_data = self.encode_times(&has_mtime, |e| e.meta.modification_time);
            write_variable_u64(header, mtime_data.len() as u64)?;
            header.extend_from_slice(&mtime_data);
        }

        // Comment (if set in options)
        if let Some(ref comment) = self.options.comment {
            header.push(property_id::COMMENT);
            let comment_data = self.encode_comment(comment);
            write_variable_u64(header, comment_data.len() as u64)?;
            header.extend_from_slice(&comment_data);
        }

        header.push(property_id::END); // End FilesInfo

        Ok(())
    }

    /// Returns whether the method has properties to encode.
    pub(crate) fn method_has_properties(&self) -> bool {
        use crate::codec::CodecMethod;
        matches!(
            self.options.method,
            CodecMethod::Lzma | CodecMethod::Lzma2 | CodecMethod::PPMd
        )
    }

    /// Encodes method-specific properties.
    pub(crate) fn encode_method_properties(&self) -> Vec<u8> {
        #[allow(unused_imports)]
        use crate::codec::CodecMethod;

        match self.options.method {
            #[cfg(feature = "lzma2")]
            CodecMethod::Lzma2 => {
                vec![crate::codec::lzma::encode_lzma2_dict_size(
                    1 << (16 + self.options.level),
                )]
            }
            #[cfg(feature = "lzma")]
            CodecMethod::Lzma => {
                let dict_size: u32 = 1 << (16 + self.options.level);
                let mut props = vec![0x5D]; // Default lc=3, lp=0, pb=2
                props.extend_from_slice(&dict_size.to_le_bytes());
                props
            }
            #[cfg(feature = "ppmd")]
            CodecMethod::PPMd => {
                let (order, mem_size): (u32, u32) = match self.options.level {
                    0..=2 => (4, 4 * 1024 * 1024),
                    3..=4 => (6, 8 * 1024 * 1024),
                    5..=6 => (6, 16 * 1024 * 1024),
                    7..=8 => (8, 32 * 1024 * 1024),
                    _ => (8, 64 * 1024 * 1024),
                };
                let mut props = vec![order as u8];
                props.extend_from_slice(&mem_size.to_le_bytes());
                props
            }
            _ => Vec::new(),
        }
    }

    /// Writes a compression coder to the header.
    pub(crate) fn write_compression_coder(&self, header: &mut Vec<u8>) -> Result<()> {
        use super::encoding_utils::encode_method_id;

        let method_id = self.options.method.method_id();
        let method_bytes = encode_method_id(method_id);

        let id_size = method_bytes.len() as u8;
        let has_props = self.method_has_properties();
        let flags = id_size | if has_props { 0x20 } else { 0 };

        header.push(flags);
        header.extend_from_slice(&method_bytes);

        if has_props {
            let props = self.encode_method_properties();
            write_variable_u64(header, props.len() as u64)?;
            header.extend_from_slice(&props);
        }

        Ok(())
    }

    /// Writes an AES coder to the header.
    #[cfg(feature = "aes")]
    pub(crate) fn write_aes_coder(&self, header: &mut Vec<u8>, properties: &[u8]) -> Result<()> {
        use crate::codec::method;

        let flags = (method::AES.len() as u8) | 0x20; // 4 bytes + has properties
        header.push(flags);
        header.extend_from_slice(method::AES);
        write_variable_u64(header, properties.len() as u64)?;
        header.extend_from_slice(properties);

        Ok(())
    }

    /// Writes a filter coder to the header.
    pub(crate) fn write_filter_coder(
        &self,
        header: &mut Vec<u8>,
        info: &FilteredFolderInfo,
    ) -> Result<()> {
        let method_id = &info.filter_method;
        let has_props = info.filter_properties.is_some();
        let flags = (method_id.len() as u8) | if has_props { 0x20 } else { 0 };

        header.push(flags);
        header.extend_from_slice(method_id);

        if let Some(props) = &info.filter_properties {
            write_variable_u64(header, props.len() as u64)?;
            header.extend_from_slice(props);
        }

        Ok(())
    }
}
