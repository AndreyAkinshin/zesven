//! Header encoding for 7z archives.
//!
//! This module provides functions for encoding the main archive header,
//! including folder definitions, coder chains, and file metadata.

use crate::Result;
use crate::format::property_id;
use crate::format::reader::write_variable_u64;

use super::encoding_utils::encode_bool_vector;
use super::options::WriteOptions;
use super::{FilteredFolderInfo, PendingEntry, StreamInfo};

/// A coder in a folder's chain, in the order coders are written.
enum PlannedCoder<'a> {
    /// AES-256 decryption.
    #[cfg(feature = "aes")]
    Aes(&'a [u8]),
    /// A branch-conversion filter.
    Filter(&'a FilteredFolderInfo),
    /// The compression codec configured for this archive.
    Compression,
}

/// The coder chain of a single folder.
///
/// Both the coder list and the `kCodersUnpackSize` list are generated from this,
/// which is what keeps the two in agreement.
struct FolderPlan<'a> {
    /// Coders in header order, each paired with the size of its decoded output.
    coders: Vec<(PlannedCoder<'a>, u64)>,
    /// `(in_index, out_index)` pairs connecting one coder's output to another's input.
    bind_pairs: Vec<(u64, u64)>,
}

/// Everything an archive header is built from.
///
/// The header depends on what was written, never on where it went, so this
/// holds the three pieces that describe an archive and nothing about the sink.
/// Both writers build one: they used to carry an encoder each, and every defect
/// in either had to be found twice.
pub(crate) struct HeaderModel<'a> {
    pub(crate) stream_info: &'a StreamInfo,
    pub(crate) entries: &'a [PendingEntry],
    pub(crate) options: &'a WriteOptions,
}

impl HeaderModel<'_> {
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
            for (i, &unpack_size) in self.stream_info.unpack_sizes.iter().enumerate() {
                self.encode_folder(&mut header, i, unpack_size)?;
            }

            // Unpack sizes
            header.push(property_id::CODERS_UNPACK_SIZE);
            for (i, &unpack_size) in self.stream_info.unpack_sizes.iter().enumerate() {
                self.encode_unpack_sizes(&mut header, i, unpack_size)?;
            }

            // Folder CRCs, declared defined only where one is actually known.
            // Claiming a zero as a real checksum states something about the data
            // that is not true.
            let defined: Vec<bool> = self.stream_info.crcs.iter().map(Option::is_some).collect();
            if defined.iter().any(|d| *d) {
                header.push(property_id::CRC);
                if defined.iter().all(|d| *d) {
                    header.push(1); // all defined
                } else {
                    header.push(0);
                    header.extend_from_slice(&encode_bool_vector(&defined));
                }
                for crc in self.stream_info.crcs.iter().flatten() {
                    header.extend_from_slice(&crc.to_le_bytes());
                }
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

    /// Plans a folder's coder chain.
    ///
    /// A folder is described by two lists that are written to different parts of
    /// the header: the coder definitions, and `kCodersUnpackSize`, whose i-th
    /// entry is the output size of coder i. Both are derived from this single
    /// plan so they cannot drift apart. They previously were built independently
    /// and disagreed for every encrypted folder, which made archives unreadable
    /// by other 7z implementations while our own round-trips stayed green.
    ///
    /// Returns `None` for BCJ2 folders, which have their own encoding.
    fn plan_folder(&self, folder_idx: usize, unpack_size: u64) -> Option<FolderPlan<'_>> {
        if self
            .stream_info
            .bcj2_folder_info
            .get(folder_idx)
            .and_then(|f| f.as_ref())
            .is_some()
        {
            return None;
        }

        let filter_info = self
            .stream_info
            .filter_info
            .get(folder_idx)
            .and_then(|f| f.as_ref());

        // Decoding runs from the packed bytes towards the original data, and the
        // coders are listed in that order. Each coder's recorded size is what it
        // produces while decoding: AES yields the compressed bytes, the codec
        // yields the filtered bytes, the filter yields the original data.
        #[cfg(feature = "aes")]
        if let Some(enc_info) = self
            .stream_info
            .encryption_info
            .get(folder_idx)
            .and_then(|e| e.as_ref())
        {
            let aes = (
                PlannedCoder::Aes(&enc_info.aes_properties),
                enc_info.compressed_size,
            );

            return Some(match filter_info {
                Some(flt_info) => FolderPlan {
                    coders: vec![
                        aes,
                        (PlannedCoder::Filter(flt_info), unpack_size),
                        (PlannedCoder::Compression, flt_info.filtered_size),
                    ],
                    // AES output feeds the codec; the codec's output feeds the filter.
                    bind_pairs: vec![(2, 0), (1, 2)],
                },
                None => FolderPlan {
                    coders: vec![aes, (PlannedCoder::Compression, unpack_size)],
                    // AES output feeds the codec.
                    bind_pairs: vec![(1, 0)],
                },
            });
        }

        Some(match filter_info {
            Some(flt_info) => FolderPlan {
                coders: vec![
                    (PlannedCoder::Filter(flt_info), unpack_size),
                    (PlannedCoder::Compression, flt_info.filtered_size),
                ],
                // Codec output feeds the filter.
                bind_pairs: vec![(0, 1)],
            },
            None => FolderPlan {
                coders: vec![(PlannedCoder::Compression, unpack_size)],
                bind_pairs: Vec::new(),
            },
        })
    }

    /// Encodes a single folder's coder chain.
    fn encode_folder(
        &self,
        header: &mut Vec<u8>,
        folder_idx: usize,
        unpack_size: u64,
    ) -> Result<()> {
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

        let plan = self
            .plan_folder(folder_idx, unpack_size)
            .expect("BCJ2 folders are handled above");

        write_variable_u64(header, plan.coders.len() as u64)?;

        for (coder, _) in &plan.coders {
            match coder {
                #[cfg(feature = "aes")]
                PlannedCoder::Aes(properties) => self.write_aes_coder(header, properties)?,
                PlannedCoder::Filter(info) => self.write_filter_coder(header, info)?,
                PlannedCoder::Compression => self.write_compression_coder(header, folder_idx)?,
            }
        }

        for &(in_index, out_index) in &plan.bind_pairs {
            write_variable_u64(header, in_index)?;
            write_variable_u64(header, out_index)?;
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

        let plan = self
            .plan_folder(folder_idx, unpack_size)
            .expect("BCJ2 folders are handled above");

        // One size per coder output, in coder order - the same order the coders
        // were written in.
        for (_, output_size) in &plan.coders {
            write_variable_u64(header, *output_size)?;
        }

        Ok(())
    }

    /// Encodes SubStreamsInfo section.
    fn encode_substreams_info(&self, header: &mut Vec<u8>) -> Result<()> {
        if self.stream_info.substream_crcs.is_empty() {
            return Ok(());
        }

        header.push(property_id::SUBSTREAMS_INFO);

        // The counts are only written when some folder holds more than one
        // entry; one per folder is the default a reader assumes.
        let has_multi = self
            .stream_info
            .num_unpack_streams_per_folder
            .iter()
            .any(|&n| n > 1);
        if has_multi {
            header.push(property_id::NUM_UNPACK_STREAM);
            for &count in &self.stream_info.num_unpack_streams_per_folder {
                write_variable_u64(header, count)?;
            }
        }

        // One size per entry except the last of each folder, which is what is
        // left of the folder's total. A folder holding a single entry therefore
        // contributes nothing here.
        if has_multi {
            header.push(property_id::SIZE);

            let mut stream_idx = 0;
            for &count in &self.stream_info.num_unpack_streams_per_folder {
                for i in 0..(count as usize).saturating_sub(1) {
                    if let Some(&size) = self.stream_info.substream_sizes.get(stream_idx + i) {
                        write_variable_u64(header, size)?;
                    }
                }
                stream_idx += count as usize;
            }
        }

        // Per-entry checksums. Without these an archive carries nothing another
        // implementation can verify: 7-Zip reports a corrupted archive as fine
        // because it has no digest to compare against.
        header.push(property_id::CRC);
        header.push(1); // all defined
        for &crc in &self.stream_info.substream_crcs {
            header.extend_from_slice(&crc.to_le_bytes());
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

    /// Returns the compression coder's properties for a folder.
    ///
    /// These were recorded by the encoder that produced the folder, so they
    /// describe the settings the bytes were actually written with.
    pub(crate) fn coder_properties(&self, folder_idx: usize) -> &[u8] {
        self.stream_info
            .coder_properties
            .get(folder_idx)
            .map_or(&[], Vec::as_slice)
    }

    /// Writes a compression coder to the header.
    pub(crate) fn write_compression_coder(
        &self,
        header: &mut Vec<u8>,
        folder_idx: usize,
    ) -> Result<()> {
        use super::encoding_utils::encode_method_id;

        // The method this folder was written with, not whatever the options
        // say now: they can have changed since.
        let method = self
            .stream_info
            .coder_methods
            .get(folder_idx)
            .copied()
            .unwrap_or(self.options.method);
        let method_bytes = encode_method_id(method.method_id());

        let props = self.coder_properties(folder_idx);
        let flags = (method_bytes.len() as u8) | if props.is_empty() { 0 } else { 0x20 };

        header.push(flags);
        header.extend_from_slice(&method_bytes);

        if !props.is_empty() {
            write_variable_u64(header, props.len() as u64)?;
            header.extend_from_slice(props);
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
