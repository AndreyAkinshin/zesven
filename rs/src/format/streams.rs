//! Streams info structures for 7z archives.
//!
//! These structures describe the compressed data streams within an archive.

use crate::{Error, Result};
use std::io::Read;

use super::property_id;
use super::reader::{read_all_or_bits, read_bytes, read_u8, read_u32_le, read_variable_u64};

/// Mode for handling resource limit violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LimitMode {
    /// Return an error when the limit is exceeded.
    #[default]
    HardError,
    /// Log a warning but continue.
    Warn,
    /// Ignore the limit.
    Ignore,
}

/// Compression ratio limit configuration.
#[derive(Debug, Clone)]
pub struct RatioLimit {
    /// Maximum allowed compression ratio (uncompressed / compressed).
    pub max_ratio: u32,
    /// How to handle limit violations.
    pub mode: LimitMode,
}

impl Default for RatioLimit {
    fn default() -> Self {
        Self {
            max_ratio: 1000, // 1000:1 ratio (1 byte compressed -> 1000 bytes uncompressed)
            mode: LimitMode::HardError,
        }
    }
}

impl RatioLimit {
    /// Creates a new ratio limit.
    pub fn new(max_ratio: u32) -> Self {
        Self {
            max_ratio,
            mode: LimitMode::HardError,
        }
    }

    /// Sets the mode for handling violations.
    pub fn mode(mut self, mode: LimitMode) -> Self {
        self.mode = mode;
        self
    }

    /// Checks if the compression ratio is within acceptable bounds.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The ratio exceeds the configured limit
    /// - The ratio is infinite (compressed_size=0 with uncompressed_size>0)
    ///
    /// # Edge Cases
    ///
    /// - Both sizes 0: Returns `Ok(())` (empty file, valid)
    /// - Compressed 0, uncompressed > 0: Returns error (infinite ratio)
    /// - Compressed > 0, uncompressed 0: Returns `Ok(())` (ratio is 0)
    pub fn check(&self, compressed_size: u64, uncompressed_size: u64) -> Result<()> {
        // Handle infinite ratio edge case: compressed is 0 but uncompressed is > 0
        if compressed_size == 0 && uncompressed_size > 0 {
            return match self.mode {
                LimitMode::HardError => Err(Error::ResourceLimitExceeded(
                    "Infinite compression ratio: compressed size is 0 but uncompressed size is non-zero".into(),
                )),
                LimitMode::Warn | LimitMode::Ignore => Ok(()),
            };
        }

        // If compressed size is 0 and uncompressed is also 0, that's fine (empty file)
        if compressed_size == 0 {
            return Ok(());
        }

        // Use multiplication instead of division to avoid truncation errors.
        // Instead of: ratio = uncompressed / compressed; if ratio > max_ratio
        // We check:   uncompressed > max_ratio * compressed
        // This correctly catches fractional ratios that exceed the limit.
        let max_uncompressed = (self.max_ratio as u64).saturating_mul(compressed_size);
        if uncompressed_size > max_uncompressed {
            let ratio = uncompressed_size / compressed_size; // For error message only
            match self.mode {
                LimitMode::HardError => Err(Error::ResourceLimitExceeded(format!(
                    "Compression ratio {} exceeds limit {}",
                    ratio, self.max_ratio
                ))),
                LimitMode::Warn => {
                    // In a real implementation, this would log a warning
                    Ok(())
                }
                LimitMode::Ignore => Ok(()),
            }
        } else {
            Ok(())
        }
    }
}

/// Resource limits for parsing and extraction operations.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum number of entries (files, streams, etc.) allowed.
    pub max_entries: usize,
    /// Maximum bytes to read for header data.
    pub max_header_bytes: u64,
    /// Maximum total unpacked size for all entries.
    pub max_total_unpacked: u64,
    /// Maximum unpacked size for a single entry.
    pub max_entry_unpacked: u64,
    /// Compression ratio limit (for bomb protection).
    pub ratio_limit: Option<RatioLimit>,
}

impl Default for ResourceLimits {
    /// Creates resource limits with the following default values:
    ///
    /// | Limit | Default Value | Description |
    /// |-------|---------------|-------------|
    /// | `max_entries` | 1,000,000 | Maximum entries in archive |
    /// | `max_header_bytes` | 64 MiB | Maximum header size |
    /// | `max_total_unpacked` | 1 TiB | Maximum total extracted size |
    /// | `max_entry_unpacked` | 64 GiB | Maximum single entry size |
    /// | `ratio_limit` | 1000:1 (HardError) | Compression bomb protection |
    ///
    /// These defaults are designed to protect against malicious archives
    /// while allowing most legitimate archives to be processed. Use
    /// [`ResourceLimits::unlimited()`] to disable all limits.
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,       // 1 million entries
            max_header_bytes: 64 << 20,   // 64 MiB
            max_total_unpacked: 1 << 40,  // 1 TiB
            max_entry_unpacked: 64 << 30, // 64 GiB
            ratio_limit: Some(RatioLimit::default()),
        }
    }
}

impl ResourceLimits {
    /// Creates new resource limits with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates resource limits with no restrictions.
    pub fn unlimited() -> Self {
        Self {
            max_entries: usize::MAX,
            max_header_bytes: u64::MAX,
            max_total_unpacked: u64::MAX,
            max_entry_unpacked: u64::MAX,
            ratio_limit: None,
        }
    }

    /// Sets the maximum number of entries.
    pub fn max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// Sets the maximum header bytes.
    pub fn max_header_bytes(mut self, max: u64) -> Self {
        self.max_header_bytes = max;
        self
    }

    /// Sets the maximum total unpacked size.
    pub fn max_total_unpacked(mut self, max: u64) -> Self {
        self.max_total_unpacked = max;
        self
    }

    /// Sets the maximum entry unpacked size.
    pub fn max_entry_unpacked(mut self, max: u64) -> Self {
        self.max_entry_unpacked = max;
        self
    }

    /// Sets the compression ratio limit.
    pub fn ratio_limit(mut self, limit: Option<RatioLimit>) -> Self {
        self.ratio_limit = limit;
        self
    }
}

/// Information about packed (compressed) streams.
#[derive(Debug, Clone, Default)]
pub struct PackInfo {
    /// Position of the first pack stream (relative to archive data start).
    pub pack_pos: u64,
    /// Sizes of each packed stream.
    pub pack_sizes: Vec<u64>,
    /// Optional CRC values for each packed stream.
    pub pack_crcs: Vec<Option<u32>>,
}

impl PackInfo {
    /// Parses PackInfo from a reader.
    ///
    /// The reader should be positioned after the K_PACK_INFO property ID.
    pub fn parse<R: Read>(r: &mut R, limits: &ResourceLimits) -> Result<Self> {
        let pack_pos = read_variable_u64(r)?;
        let num_pack_streams = read_variable_u64(r)?;

        // Check resource limits
        if num_pack_streams > limits.max_entries as u64 {
            return Err(Error::ResourceLimitExceeded(format!(
                "too many pack streams: {}",
                num_pack_streams
            )));
        }

        let num_streams = num_pack_streams as usize;
        let mut pack_sizes = Vec::with_capacity(num_streams);
        let mut pack_crcs = vec![None; num_streams];

        loop {
            let prop_id = read_u8(r)?;

            match prop_id {
                property_id::END => break,

                property_id::SIZE => {
                    for _ in 0..num_streams {
                        pack_sizes.push(read_variable_u64(r)?);
                    }
                }

                property_id::CRC => {
                    let defined = read_all_or_bits(r, num_streams)?;
                    for (i, &has_crc) in defined.iter().enumerate() {
                        if has_crc {
                            pack_crcs[i] = Some(read_u32_le(r)?);
                        }
                    }
                }

                _ => {
                    return Err(Error::CorruptHeader {
                        offset: 0,
                        reason: format!("unexpected property ID in PackInfo: {:#x}", prop_id),
                    });
                }
            }
        }

        // If sizes weren't provided, default to empty (shouldn't happen in valid archives)
        if pack_sizes.is_empty() && num_streams > 0 {
            pack_sizes = vec![0; num_streams];
        }

        Ok(Self {
            pack_pos,
            pack_sizes,
            pack_crcs,
        })
    }

    /// Returns the number of pack streams.
    pub fn num_streams(&self) -> usize {
        self.pack_sizes.len()
    }

    /// Returns the total packed size.
    pub fn total_packed_size(&self) -> u64 {
        self.pack_sizes.iter().sum()
    }
}

/// A compression or encryption coder.
#[derive(Debug, Clone)]
pub struct Coder {
    /// Method ID bytes (variable length, typically 1-4 bytes).
    pub method_id: Vec<u8>,
    /// Number of input streams.
    pub num_in_streams: u64,
    /// Number of output streams.
    pub num_out_streams: u64,
    /// Optional coder properties (e.g., LZMA dictionary size).
    pub properties: Option<Vec<u8>>,
}

impl Coder {
    /// Returns the method ID as a u64 for comparison with known method constants.
    pub fn method_id_u64(&self) -> u64 {
        let mut result = 0u64;
        for (i, &byte) in self.method_id.iter().enumerate() {
            if i >= 8 {
                break;
            }
            result |= (byte as u64) << (8 * i);
        }
        result
    }
}

/// A binding pair connecting coder streams.
#[derive(Debug, Clone, Copy)]
pub struct BindPair {
    /// Index of the input stream.
    pub in_index: u64,
    /// Index of the output stream.
    pub out_index: u64,
}

/// A folder (block) containing one or more coders.
///
/// Folders describe how compressed data is processed through a chain
/// of coders (compression, encryption, filters).
#[derive(Debug, Clone)]
pub struct Folder {
    /// List of coders in this folder.
    pub coders: Vec<Coder>,
    /// Binding pairs connecting coder streams.
    pub bind_pairs: Vec<BindPair>,
    /// Indices of packed streams used as input.
    pub packed_streams: Vec<u64>,
    /// Unpacked sizes for each coder's output.
    pub unpack_sizes: Vec<u64>,
    /// Optional CRC of the unpacked data.
    pub unpack_crc: Option<u32>,
}

impl Folder {
    /// Parses a single folder from a reader.
    fn parse<R: Read>(r: &mut R, limits: &ResourceLimits) -> Result<Self> {
        let num_coders = read_variable_u64(r)?;

        if num_coders > 16 {
            return Err(Error::ResourceLimitExceeded(format!(
                "too many coders in folder: {}",
                num_coders
            )));
        }

        let mut coders = Vec::with_capacity(num_coders as usize);
        let mut total_in_streams = 0u64;
        let mut total_out_streams = 0u64;

        for _ in 0..num_coders {
            let flags = read_u8(r)?;

            // Method ID size is in lower 4 bits
            let method_id_size = (flags & 0x0F) as usize;
            let is_complex = (flags & 0x10) != 0;
            let has_attributes = (flags & 0x20) != 0;

            let method_id = read_bytes(r, method_id_size)?;

            let (num_in_streams, num_out_streams) = if is_complex {
                (read_variable_u64(r)?, read_variable_u64(r)?)
            } else {
                (1, 1)
            };

            let properties = if has_attributes {
                let props_size = read_variable_u64(r)? as usize;
                if props_size > limits.max_header_bytes as usize {
                    return Err(Error::ResourceLimitExceeded(
                        "coder properties too large".into(),
                    ));
                }
                Some(read_bytes(r, props_size)?)
            } else {
                None
            };

            total_in_streams += num_in_streams;
            total_out_streams += num_out_streams;

            coders.push(Coder {
                method_id,
                num_in_streams,
                num_out_streams,
                properties,
            });
        }

        // Read bind pairs
        let num_bind_pairs = total_out_streams.saturating_sub(1);
        let mut bind_pairs = Vec::with_capacity(num_bind_pairs as usize);

        for i in 0..num_bind_pairs {
            let in_index = read_variable_u64(r)?;
            let out_index = read_variable_u64(r)?;

            // Validate bind pair indices
            if in_index >= total_in_streams {
                return Err(Error::InvalidFormat(format!(
                    "bind_pair[{}].in_index {} exceeds total_in_streams {}",
                    i, in_index, total_in_streams
                )));
            }
            if out_index >= total_out_streams {
                return Err(Error::InvalidFormat(format!(
                    "bind_pair[{}].out_index {} exceeds total_out_streams {}",
                    i, out_index, total_out_streams
                )));
            }

            bind_pairs.push(BindPair {
                in_index,
                out_index,
            });
        }

        // Read packed streams indices
        let num_packed = total_in_streams.saturating_sub(num_bind_pairs);
        let mut packed_streams = Vec::with_capacity(num_packed as usize);

        if num_packed == 1 {
            // Find the unpaired input stream
            let mut bound_in_streams: Vec<bool> = vec![false; total_in_streams as usize];
            for bp in &bind_pairs {
                if (bp.in_index as usize) < bound_in_streams.len() {
                    bound_in_streams[bp.in_index as usize] = true;
                }
            }
            for (i, &bound) in bound_in_streams.iter().enumerate() {
                if !bound {
                    packed_streams.push(i as u64);
                    break;
                }
            }
        } else {
            for _ in 0..num_packed {
                packed_streams.push(read_variable_u64(r)?);
            }
        }

        Ok(Self {
            coders,
            bind_pairs,
            packed_streams,
            unpack_sizes: Vec::new(),
            unpack_crc: None,
        })
    }

    /// Returns the total number of output streams.
    pub fn total_out_streams(&self) -> u64 {
        self.coders.iter().map(|c| c.num_out_streams).sum()
    }

    /// Returns the final unpack size (size of the last output stream).
    pub fn final_unpack_size(&self) -> Option<u64> {
        // The last unpack size is for the final output stream
        self.unpack_sizes.last().copied()
    }

    /// BCJ2 method ID constant.
    const BCJ2_METHOD_ID: &'static [u8] = &[0x03, 0x03, 0x01, 0x1B];

    /// Checks if this folder uses BCJ2 compression.
    ///
    /// BCJ2 is a 4-stream x86 filter that requires special handling
    /// as it has 4 input streams instead of 1.
    pub fn uses_bcj2(&self) -> bool {
        self.coders
            .iter()
            .any(|c| c.method_id.as_slice() == Self::BCJ2_METHOD_ID)
    }

    /// Validates BCJ2 stream requirements.
    ///
    /// BCJ2 requires exactly 4 input streams. This method returns an error
    /// if a BCJ2 coder is present but doesn't have the correct stream count.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if no BCJ2 coder is present, or if BCJ2 has valid stream count
    /// - `Err` with details if BCJ2 has invalid stream count
    pub fn validate_bcj2_streams(&self) -> crate::Result<()> {
        for coder in &self.coders {
            if coder.method_id.as_slice() == Self::BCJ2_METHOD_ID {
                if coder.num_in_streams != 4 {
                    return Err(crate::Error::InvalidFormat(format!(
                        "BCJ2 coder requires exactly 4 input streams, found {}",
                        coder.num_in_streams
                    )));
                }
                if coder.num_out_streams != 1 {
                    return Err(crate::Error::InvalidFormat(format!(
                        "BCJ2 coder requires exactly 1 output stream, found {}",
                        coder.num_out_streams
                    )));
                }
            }
        }
        Ok(())
    }

    /// Validates packed_streams against pack stream count and input stream count.
    ///
    /// The `packed_streams` field contains input stream indices - it maps each pack
    /// stream to the input stream that receives its data. This validation ensures:
    /// 1. The number of entries matches `num_pack_streams`
    /// 2. Each entry is a valid input stream index (< `total_in_streams`)
    ///
    /// This should be called after both folder and pack_info are parsed.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if all indices are valid
    /// - `Err` with details if validation fails
    pub fn validate_packed_streams(&self, num_pack_streams: usize) -> crate::Result<()> {
        // Verify count matches: each pack stream should map to exactly one input stream
        if self.packed_streams.len() != num_pack_streams {
            return Err(crate::Error::InvalidFormat(format!(
                "packed_streams count {} does not match num_pack_streams {}",
                self.packed_streams.len(),
                num_pack_streams
            )));
        }

        // Verify each entry is a valid input stream index
        let total_in = self.total_in_streams();
        for (i, &in_stream_idx) in self.packed_streams.iter().enumerate() {
            if in_stream_idx >= total_in {
                return Err(crate::Error::InvalidFormat(format!(
                    "packed_streams[{}] input stream index {} exceeds total_in_streams {}",
                    i, in_stream_idx, total_in
                )));
            }
        }
        Ok(())
    }

    /// Validates bind_pairs indices against the number of input and output streams.
    ///
    /// Each `in_index` must be less than `total_in_streams()` and each `out_index`
    /// must be less than `total_out_streams()`. This prevents out-of-bounds access
    /// when traversing the coder graph.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if all indices are valid
    /// - `Err` with details if any index is out of bounds
    ///
    /// # Security
    ///
    /// This validation is critical for preventing malicious archives from causing
    /// index-out-of-bounds errors during decompression chain construction.
    pub fn validate_bind_pairs(&self) -> crate::Result<()> {
        let total_in = self.total_in_streams();
        let total_out = self.total_out_streams();

        for (i, bp) in self.bind_pairs.iter().enumerate() {
            if bp.in_index >= total_in {
                return Err(crate::Error::InvalidFormat(format!(
                    "bind_pair[{}].in_index {} >= total_in_streams {}",
                    i, bp.in_index, total_in
                )));
            }
            if bp.out_index >= total_out {
                return Err(crate::Error::InvalidFormat(format!(
                    "bind_pair[{}].out_index {} >= total_out_streams {}",
                    i, bp.out_index, total_out
                )));
            }
        }
        Ok(())
    }

    /// Finds the main coder index (the one whose output is the final output).
    ///
    /// The main coder is the one whose output is NOT consumed by any bind_pair.
    /// For single-coder folders, this is always 0.
    /// For multi-coder folders (like BCJ2), this identifies the final filter.
    pub fn main_coder_index(&self) -> Option<usize> {
        if self.coders.is_empty() {
            return None;
        }

        if self.coders.len() == 1 {
            return Some(0);
        }

        // Track which output streams are consumed by bind_pairs
        let total_out = self.total_out_streams() as usize;
        let mut consumed_outputs = vec![false; total_out];

        for bp in &self.bind_pairs {
            if (bp.out_index as usize) < consumed_outputs.len() {
                consumed_outputs[bp.out_index as usize] = true;
            }
        }

        // Find the coder with an unconsumed output
        let mut stream_idx = 0;
        for (coder_idx, coder) in self.coders.iter().enumerate() {
            for _ in 0..coder.num_out_streams {
                if stream_idx < consumed_outputs.len() && !consumed_outputs[stream_idx] {
                    return Some(coder_idx);
                }
                stream_idx += 1;
            }
        }

        // Fallback to first coder if no unconsumed output found
        Some(0)
    }

    /// Returns the stream offsets for each coder.
    ///
    /// Each tuple contains (first_in_stream_idx, first_out_stream_idx).
    /// This is useful for navigating bind_pairs and packed_streams.
    pub fn coder_stream_offsets(&self) -> Vec<(usize, usize)> {
        let mut result = Vec::with_capacity(self.coders.len());
        let mut in_offset = 0;
        let mut out_offset = 0;

        for coder in &self.coders {
            result.push((in_offset, out_offset));
            in_offset += coder.num_in_streams as usize;
            out_offset += coder.num_out_streams as usize;
        }

        result
    }

    /// Returns the total number of input streams across all coders.
    pub fn total_in_streams(&self) -> u64 {
        self.coders.iter().map(|c| c.num_in_streams).sum()
    }

    /// Finds the bind_pair that provides input to the given stream index.
    ///
    /// Returns the bind_pair where `in_index` matches the given stream index.
    pub fn find_bind_pair_for_in_stream(&self, in_stream_idx: u64) -> Option<&BindPair> {
        self.bind_pairs
            .iter()
            .find(|bp| bp.in_index == in_stream_idx)
    }

    /// Checks if the given input stream index comes from a packed stream.
    ///
    /// Returns the packed stream index if found.
    pub fn find_packed_stream_index(&self, in_stream_idx: u64) -> Option<usize> {
        self.packed_streams
            .iter()
            .position(|&ps| ps == in_stream_idx)
    }
}

/// Unpack info containing folder definitions.
#[derive(Debug, Clone, Default)]
pub struct UnpackInfo {
    /// List of folders (blocks).
    pub folders: Vec<Folder>,
}

impl UnpackInfo {
    /// Parses UnpackInfo from a reader.
    ///
    /// The reader should be positioned after the K_UNPACK_INFO property ID.
    pub fn parse<R: Read>(r: &mut R, limits: &ResourceLimits) -> Result<Self> {
        let mut folders = Vec::new();

        loop {
            let prop_id = read_u8(r)?;

            match prop_id {
                property_id::END => break,

                property_id::FOLDER => {
                    let num_folders = read_variable_u64(r)?;

                    if num_folders > limits.max_entries as u64 {
                        return Err(Error::ResourceLimitExceeded(format!(
                            "too many folders: {}",
                            num_folders
                        )));
                    }

                    // External flag (0 = folders defined inline)
                    let external = read_u8(r)?;
                    if external != 0 {
                        return Err(Error::UnsupportedFeature {
                            feature: "external folder definitions",
                        });
                    }

                    for _ in 0..num_folders {
                        folders.push(Folder::parse(r, limits)?);
                    }
                }

                property_id::CODERS_UNPACK_SIZE => {
                    // Read unpack sizes for all folders
                    for folder in &mut folders {
                        let num_sizes = folder.total_out_streams() as usize;
                        folder.unpack_sizes = Vec::with_capacity(num_sizes);
                        for _ in 0..num_sizes {
                            folder.unpack_sizes.push(read_variable_u64(r)?);
                        }
                    }
                }

                property_id::CRC => {
                    let defined = read_all_or_bits(r, folders.len())?;
                    for (folder, &has_crc) in folders.iter_mut().zip(defined.iter()) {
                        if has_crc {
                            folder.unpack_crc = Some(read_u32_le(r)?);
                        }
                    }
                }

                _ => {
                    return Err(Error::CorruptHeader {
                        offset: 0,
                        reason: format!("unexpected property ID in UnpackInfo: {:#x}", prop_id),
                    });
                }
            }
        }

        // Validate folder structure for all folders
        for (folder_idx, folder) in folders.iter().enumerate() {
            folder.validate_bcj2_streams()?;
            folder
                .validate_bind_pairs()
                .map_err(|e| Error::InvalidFormat(format!("folder[{}]: {}", folder_idx, e)))?;
        }

        Ok(Self { folders })
    }

    /// Returns the number of folders.
    pub fn num_folders(&self) -> usize {
        self.folders.len()
    }
}

/// Information about substreams within folders.
///
/// In solid archives, multiple files can be packed into a single folder.
/// SubStreamsInfo describes how many files are in each folder and their sizes.
#[derive(Debug, Clone, Default)]
pub struct SubStreamsInfo {
    /// Number of unpack streams (files) in each folder.
    pub num_unpack_streams_in_folders: Vec<u64>,
    /// Unpacked sizes of each substream.
    pub unpack_sizes: Vec<u64>,
    /// Optional CRC values for each substream.
    pub digests: Vec<Option<u32>>,
}

impl SubStreamsInfo {
    /// Parses SubStreamsInfo from a reader.
    ///
    /// The reader should be positioned after the K_SUB_STREAMS_INFO property ID.
    pub fn parse<R: Read>(r: &mut R, folders: &[Folder], limits: &ResourceLimits) -> Result<Self> {
        let num_folders = folders.len();

        // Default: 1 stream per folder
        let mut num_unpack_streams_in_folders = vec![1u64; num_folders];
        let mut unpack_sizes = Vec::new();
        let mut digests = Vec::new();

        loop {
            let prop_id = read_u8(r)?;

            match prop_id {
                property_id::END => break,

                property_id::NUM_UNPACK_STREAM => {
                    for streams in num_unpack_streams_in_folders.iter_mut() {
                        *streams = read_variable_u64(r)?;
                    }
                }

                property_id::SIZE => {
                    // Read sizes for each substream
                    // The last size in each folder is implicit
                    for (folder_idx, &num_streams) in
                        num_unpack_streams_in_folders.iter().enumerate()
                    {
                        if num_streams == 0 {
                            continue;
                        }

                        let folder_size = folders[folder_idx].final_unpack_size().unwrap_or(0);
                        let mut remaining = folder_size;

                        // Read n-1 sizes, last one is implicit
                        for _ in 0..num_streams.saturating_sub(1) {
                            let size = read_variable_u64(r)?;
                            unpack_sizes.push(size);
                            remaining = remaining.saturating_sub(size);
                        }

                        // Add implicit last size
                        if num_streams > 0 {
                            unpack_sizes.push(remaining);
                        }
                    }
                }

                property_id::CRC => {
                    // Calculate total number of substreams needing CRC
                    let total_streams: u64 = num_unpack_streams_in_folders.iter().sum();

                    if total_streams > limits.max_entries as u64 {
                        return Err(Error::ResourceLimitExceeded(format!(
                            "too many substreams: {}",
                            total_streams
                        )));
                    }

                    // Count streams that need CRCs (those without folder CRC)
                    let mut streams_needing_crc = 0usize;
                    for (folder_idx, &num_streams) in
                        num_unpack_streams_in_folders.iter().enumerate()
                    {
                        if folders[folder_idx].unpack_crc.is_none() || num_streams != 1 {
                            streams_needing_crc += num_streams as usize;
                        }
                    }

                    let defined = read_all_or_bits(r, streams_needing_crc)?;
                    let mut defined_iter = defined.iter();

                    for (folder_idx, &num_streams) in
                        num_unpack_streams_in_folders.iter().enumerate()
                    {
                        let folder = &folders[folder_idx];

                        if folder.unpack_crc.is_some() && num_streams == 1 {
                            // Use folder CRC for single-stream folder
                            digests.push(folder.unpack_crc);
                        } else {
                            for _ in 0..num_streams {
                                if let Some(&has_crc) = defined_iter.next() {
                                    if has_crc {
                                        digests.push(Some(read_u32_le(r)?));
                                    } else {
                                        digests.push(None);
                                    }
                                } else {
                                    digests.push(None);
                                }
                            }
                        }
                    }
                }

                _ => {
                    return Err(Error::CorruptHeader {
                        offset: 0,
                        reason: format!("unexpected property ID in SubStreamsInfo: {:#x}", prop_id),
                    });
                }
            }
        }

        // If sizes weren't read, use folder sizes directly
        if unpack_sizes.is_empty() {
            for (folder_idx, &num_streams) in num_unpack_streams_in_folders.iter().enumerate() {
                if num_streams == 1 {
                    if let Some(size) = folders[folder_idx].final_unpack_size() {
                        unpack_sizes.push(size);
                    }
                }
            }
        }

        // If digests weren't read, inherit from folders where applicable
        if digests.is_empty() {
            for (folder_idx, &num_streams) in num_unpack_streams_in_folders.iter().enumerate() {
                if num_streams == 1 {
                    digests.push(folders[folder_idx].unpack_crc);
                } else {
                    for _ in 0..num_streams {
                        digests.push(None);
                    }
                }
            }
        }

        Ok(Self {
            num_unpack_streams_in_folders,
            unpack_sizes,
            digests,
        })
    }

    /// Returns the total number of substreams.
    pub fn total_streams(&self) -> u64 {
        self.num_unpack_streams_in_folders.iter().sum()
    }
}

#[cfg(test)]
#[allow(clippy::vec_init_then_push)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_variable_u64(buf: &mut Vec<u8>, value: u64) {
        use super::super::reader::write_variable_u64;
        write_variable_u64(buf, value).unwrap();
    }

    #[test]
    fn test_pack_info_basic() {
        let mut data = Vec::new();

        // pack_pos = 100
        write_variable_u64(&mut data, 100);
        // num_pack_streams = 2
        write_variable_u64(&mut data, 2);
        // K_SIZE
        data.push(property_id::SIZE);
        // sizes: 50, 75
        write_variable_u64(&mut data, 50);
        write_variable_u64(&mut data, 75);
        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let pack_info = PackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(pack_info.pack_pos, 100);
        assert_eq!(pack_info.pack_sizes, vec![50, 75]);
        assert_eq!(pack_info.num_streams(), 2);
        assert_eq!(pack_info.total_packed_size(), 125);
    }

    #[test]
    fn test_pack_info_with_crcs() {
        let mut data = Vec::new();

        write_variable_u64(&mut data, 0); // pack_pos
        write_variable_u64(&mut data, 2); // num_streams

        // K_SIZE
        data.push(property_id::SIZE);
        write_variable_u64(&mut data, 100);
        write_variable_u64(&mut data, 200);

        // K_CRC - all defined
        data.push(property_id::CRC);
        data.push(0x01); // all defined flag
        data.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());
        data.extend_from_slice(&0xCAFEBABEu32.to_le_bytes());

        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let pack_info = PackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(pack_info.pack_crcs[0], Some(0xDEADBEEF));
        assert_eq!(pack_info.pack_crcs[1], Some(0xCAFEBABE));
    }

    #[test]
    fn test_pack_info_partial_crcs() {
        let mut data = Vec::new();

        write_variable_u64(&mut data, 0);
        write_variable_u64(&mut data, 3);

        data.push(property_id::SIZE);
        write_variable_u64(&mut data, 100);
        write_variable_u64(&mut data, 200);
        write_variable_u64(&mut data, 300);

        // K_CRC - not all defined
        data.push(property_id::CRC);
        data.push(0x00); // not all defined
        data.push(0b10100000); // bits: true, false, true
        data.extend_from_slice(&0x11111111u32.to_le_bytes()); // CRC for stream 0
        data.extend_from_slice(&0x33333333u32.to_le_bytes()); // CRC for stream 2

        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let pack_info = PackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(pack_info.pack_crcs[0], Some(0x11111111));
        assert_eq!(pack_info.pack_crcs[1], None);
        assert_eq!(pack_info.pack_crcs[2], Some(0x33333333));
    }

    #[test]
    fn test_pack_info_resource_limit() {
        let mut data = Vec::new();

        write_variable_u64(&mut data, 0);
        write_variable_u64(&mut data, 1_000_001); // exceeds default limit

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let err = PackInfo::parse(&mut cursor, &limits).unwrap_err();

        assert!(matches!(err, Error::ResourceLimitExceeded(_)));
    }

    #[test]
    fn test_pack_info_empty() {
        let mut data = Vec::new();

        write_variable_u64(&mut data, 50); // pack_pos
        write_variable_u64(&mut data, 0); // zero streams
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let pack_info = PackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(pack_info.pack_pos, 50);
        assert!(pack_info.pack_sizes.is_empty());
        assert_eq!(pack_info.num_streams(), 0);
    }

    // UnpackInfo tests

    #[test]
    fn test_coder_method_id_u64() {
        let coder = Coder {
            method_id: vec![0x21], // LZMA2
            num_in_streams: 1,
            num_out_streams: 1,
            properties: None,
        };
        assert_eq!(coder.method_id_u64(), 0x21);

        let coder2 = Coder {
            method_id: vec![0x01, 0x01, 0x03], // LZMA
            num_in_streams: 1,
            num_out_streams: 1,
            properties: None,
        };
        assert_eq!(coder2.method_id_u64(), 0x030101);
    }

    #[test]
    fn test_unpack_info_simple() {
        let mut data = Vec::new();

        // K_FOLDER
        data.push(property_id::FOLDER);
        write_variable_u64(&mut data, 1); // 1 folder
        data.push(0x00); // not external

        // Folder: 1 coder (LZMA2)
        write_variable_u64(&mut data, 1); // 1 coder
        // Coder flags: method_id_size=1, not complex, has attributes
        data.push(0x21); // flags: 1 byte method ID, has properties
        data.push(0x21); // method ID (LZMA2)
        write_variable_u64(&mut data, 5); // 5 bytes of properties
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00]); // dummy properties

        // K_CODERS_UNPACK_SIZE
        data.push(property_id::CODERS_UNPACK_SIZE);
        write_variable_u64(&mut data, 1000); // unpack size

        // K_CRC - all defined
        data.push(property_id::CRC);
        data.push(0x01); // all defined
        data.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let unpack_info = UnpackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(unpack_info.num_folders(), 1);
        let folder = &unpack_info.folders[0];
        assert_eq!(folder.coders.len(), 1);
        assert_eq!(folder.coders[0].method_id_u64(), 0x21);
        assert_eq!(folder.unpack_sizes, vec![1000]);
        assert_eq!(folder.unpack_crc, Some(0xDEADBEEF));
    }

    #[test]
    fn test_unpack_info_empty() {
        let mut data = Vec::new();
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let unpack_info = UnpackInfo::parse(&mut cursor, &limits).unwrap();

        assert_eq!(unpack_info.num_folders(), 0);
    }

    // SubStreamsInfo tests

    fn create_test_folder(unpack_size: u64, has_crc: bool) -> Folder {
        Folder {
            coders: vec![Coder {
                method_id: vec![0x21],
                num_in_streams: 1,
                num_out_streams: 1,
                properties: None,
            }],
            bind_pairs: vec![],
            packed_streams: vec![0],
            unpack_sizes: vec![unpack_size],
            unpack_crc: if has_crc { Some(0x12345678) } else { None },
        }
    }

    #[test]
    fn test_substreams_info_single_file_per_folder() {
        // Default case: 1 file per folder, inherits folder info
        let folders = vec![
            create_test_folder(1000, true),
            create_test_folder(2000, true),
        ];

        let mut data = Vec::new();
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let substreams = SubStreamsInfo::parse(&mut cursor, &folders, &limits).unwrap();

        assert_eq!(substreams.num_unpack_streams_in_folders, vec![1, 1]);
        assert_eq!(substreams.unpack_sizes, vec![1000, 2000]);
        assert_eq!(substreams.total_streams(), 2);
    }

    #[test]
    fn test_substreams_info_multiple_files() {
        // 2 files in first folder (solid), 1 in second
        let folders = vec![
            create_test_folder(1500, true), // total size 1500
            create_test_folder(500, true),
        ];

        let mut data = Vec::new();

        // K_NUM_UNPACK_STREAM
        data.push(property_id::NUM_UNPACK_STREAM);
        write_variable_u64(&mut data, 2); // 2 files in folder 0
        write_variable_u64(&mut data, 1); // 1 file in folder 1

        // K_SIZE - sizes for files in solid blocks
        // For folder 0: 2 files, read 1 size (last is implicit)
        // For folder 1: 1 file, no sizes to read
        data.push(property_id::SIZE);
        write_variable_u64(&mut data, 1000); // first file: 1000, second: 500 (implicit)

        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let substreams = SubStreamsInfo::parse(&mut cursor, &folders, &limits).unwrap();

        assert_eq!(substreams.num_unpack_streams_in_folders, vec![2, 1]);
        // First folder: 1000 + 500 (implicit) = 1500
        // Second folder: 500
        assert_eq!(substreams.unpack_sizes, vec![1000, 500, 500]);
        assert_eq!(substreams.total_streams(), 3);
    }

    #[test]
    fn test_substreams_info_empty() {
        let folders: Vec<Folder> = vec![];

        let mut data = Vec::new();
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let substreams = SubStreamsInfo::parse(&mut cursor, &folders, &limits).unwrap();

        assert!(substreams.num_unpack_streams_in_folders.is_empty());
        assert_eq!(substreams.total_streams(), 0);
    }

    // Folder BCJ2 helper method tests

    fn create_bcj2_folder() -> Folder {
        // BCJ2 folder with 5 coders:
        // [0] BCJ2: 4 inputs, 1 output (main filter)
        // [1-4] LZMA2: 1 input, 1 output each (feed BCJ2 inputs)
        //
        // bind_pairs connect LZMA2 outputs to BCJ2 inputs:
        // LZMA2[1] out 1 -> BCJ2 in 0
        // LZMA2[2] out 2 -> BCJ2 in 1
        // LZMA2[3] out 3 -> BCJ2 in 2
        // LZMA2[4] out 4 -> BCJ2 in 3
        //
        // packed_streams: inputs 4,5,6,7 (LZMA2 inputs from archive)
        Folder {
            coders: vec![
                Coder {
                    method_id: vec![0x03, 0x03, 0x01, 0x1B], // BCJ2
                    num_in_streams: 4,
                    num_out_streams: 1,
                    properties: None,
                },
                Coder {
                    method_id: vec![0x21], // LZMA2
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
                Coder {
                    method_id: vec![0x21], // LZMA2
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
                Coder {
                    method_id: vec![0x21], // LZMA2
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
                Coder {
                    method_id: vec![0x21], // LZMA2
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
            ],
            bind_pairs: vec![
                BindPair {
                    in_index: 0,
                    out_index: 1,
                }, // BCJ2 in 0 <- LZMA2[1] out
                BindPair {
                    in_index: 1,
                    out_index: 2,
                }, // BCJ2 in 1 <- LZMA2[2] out
                BindPair {
                    in_index: 2,
                    out_index: 3,
                }, // BCJ2 in 2 <- LZMA2[3] out
                BindPair {
                    in_index: 3,
                    out_index: 4,
                }, // BCJ2 in 3 <- LZMA2[4] out
            ],
            packed_streams: vec![4, 5, 6, 7], // LZMA2 inputs from archive
            unpack_sizes: vec![1000, 500, 100, 50, 25],
            unpack_crc: Some(0x12345678),
        }
    }

    #[test]
    fn test_folder_uses_bcj2() {
        let bcj2_folder = create_bcj2_folder();
        assert!(bcj2_folder.uses_bcj2());

        let simple_folder = create_test_folder(1000, true);
        assert!(!simple_folder.uses_bcj2());
    }

    #[test]
    fn test_folder_main_coder_index_bcj2() {
        let folder = create_bcj2_folder();
        // BCJ2 coder (index 0) has output 0, which is not consumed by any bind_pair
        // (bind_pairs consume outputs 1,2,3,4 from LZMA2 coders)
        assert_eq!(folder.main_coder_index(), Some(0));
    }

    #[test]
    fn test_folder_main_coder_index_simple() {
        let folder = create_test_folder(1000, true);
        // Single coder folder - main coder is index 0
        assert_eq!(folder.main_coder_index(), Some(0));
    }

    #[test]
    fn test_folder_main_coder_index_empty() {
        let folder = Folder {
            coders: vec![],
            bind_pairs: vec![],
            packed_streams: vec![],
            unpack_sizes: vec![],
            unpack_crc: None,
        };
        assert_eq!(folder.main_coder_index(), None);
    }

    #[test]
    fn test_folder_coder_stream_offsets_bcj2() {
        let folder = create_bcj2_folder();
        let offsets = folder.coder_stream_offsets();

        // BCJ2: 4 in, 1 out -> starts at (0, 0)
        // LZMA2[1]: 1 in, 1 out -> starts at (4, 1)
        // LZMA2[2]: 1 in, 1 out -> starts at (5, 2)
        // LZMA2[3]: 1 in, 1 out -> starts at (6, 3)
        // LZMA2[4]: 1 in, 1 out -> starts at (7, 4)
        assert_eq!(offsets.len(), 5);
        assert_eq!(offsets[0], (0, 0)); // BCJ2
        assert_eq!(offsets[1], (4, 1)); // LZMA2[1]
        assert_eq!(offsets[2], (5, 2)); // LZMA2[2]
        assert_eq!(offsets[3], (6, 3)); // LZMA2[3]
        assert_eq!(offsets[4], (7, 4)); // LZMA2[4]
    }

    #[test]
    fn test_folder_coder_stream_offsets_simple() {
        let folder = create_test_folder(1000, true);
        let offsets = folder.coder_stream_offsets();

        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0], (0, 0));
    }

    #[test]
    fn test_folder_total_in_streams_bcj2() {
        let folder = create_bcj2_folder();
        // BCJ2 (4) + 4 LZMA2 (1 each) = 8
        assert_eq!(folder.total_in_streams(), 8);
    }

    #[test]
    fn test_folder_total_in_streams_simple() {
        let folder = create_test_folder(1000, true);
        assert_eq!(folder.total_in_streams(), 1);
    }

    #[test]
    fn test_folder_find_bind_pair_for_in_stream() {
        let folder = create_bcj2_folder();

        // BCJ2 inputs 0-3 have bind_pairs
        assert!(folder.find_bind_pair_for_in_stream(0).is_some());
        assert_eq!(folder.find_bind_pair_for_in_stream(0).unwrap().out_index, 1);

        assert!(folder.find_bind_pair_for_in_stream(1).is_some());
        assert_eq!(folder.find_bind_pair_for_in_stream(1).unwrap().out_index, 2);

        assert!(folder.find_bind_pair_for_in_stream(2).is_some());
        assert!(folder.find_bind_pair_for_in_stream(3).is_some());

        // LZMA2 inputs (4-7) come from packed_streams, no bind_pairs
        assert!(folder.find_bind_pair_for_in_stream(4).is_none());
        assert!(folder.find_bind_pair_for_in_stream(5).is_none());
    }

    #[test]
    fn test_folder_find_packed_stream_index() {
        let folder = create_bcj2_folder();

        // BCJ2 inputs 0-3 are NOT in packed_streams (they come from bind_pairs)
        assert!(folder.find_packed_stream_index(0).is_none());
        assert!(folder.find_packed_stream_index(1).is_none());
        assert!(folder.find_packed_stream_index(2).is_none());
        assert!(folder.find_packed_stream_index(3).is_none());

        // LZMA2 inputs 4-7 ARE in packed_streams
        assert_eq!(folder.find_packed_stream_index(4), Some(0));
        assert_eq!(folder.find_packed_stream_index(5), Some(1));
        assert_eq!(folder.find_packed_stream_index(6), Some(2));
        assert_eq!(folder.find_packed_stream_index(7), Some(3));
    }

    // RatioLimit tests

    #[test]
    fn test_ratio_limit_normal_ratio() {
        let limit = RatioLimit::new(100);
        // 10:1 ratio (10 compressed -> 100 uncompressed)
        assert!(limit.check(10, 100).is_ok());
    }

    #[test]
    fn test_ratio_limit_exceeds_limit() {
        let limit = RatioLimit::new(10);
        // 20:1 ratio exceeds 10:1 limit
        assert!(limit.check(10, 200).is_err());
    }

    #[test]
    fn test_ratio_limit_infinite_ratio() {
        let limit = RatioLimit::new(1000);
        // Infinite ratio: compressed=0, uncompressed>0
        let result = limit.check(0, 1000);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Infinite compression ratio")
        );
    }

    #[test]
    fn test_ratio_limit_zero_both() {
        let limit = RatioLimit::new(100);
        // Both zero is valid (empty file)
        assert!(limit.check(0, 0).is_ok());
    }

    #[test]
    fn test_ratio_limit_compressed_only() {
        let limit = RatioLimit::new(100);
        // Compressed > 0, uncompressed = 0 is valid (ratio is 0)
        assert!(limit.check(100, 0).is_ok());
    }

    #[test]
    fn test_ratio_limit_ignore_mode() {
        let limit = RatioLimit::new(10).mode(LimitMode::Ignore);
        // Even with exceeding ratio, ignore mode passes
        assert!(limit.check(10, 200).is_ok());
        // Infinite ratio with ignore mode also passes
        assert!(limit.check(0, 1000).is_ok());
    }

    #[test]
    fn test_ratio_limit_warn_mode() {
        let limit = RatioLimit::new(10).mode(LimitMode::Warn);
        // Warn mode passes even with exceeding ratio
        assert!(limit.check(10, 200).is_ok());
        // Infinite ratio with warn mode also passes
        assert!(limit.check(0, 1000).is_ok());
    }

    #[test]
    fn test_ratio_limit_no_truncation() {
        // Test that fractional ratios above the limit are correctly rejected.
        // With integer division, 15/10 = 1, which would incorrectly pass a limit of 1.
        // But the actual ratio 1.5:1 exceeds a 1:1 limit and should be rejected.
        let limit = RatioLimit::new(1);
        // 15 uncompressed from 10 compressed = 1.5:1 ratio, should exceed 1:1 limit
        assert!(
            limit.check(10, 15).is_err(),
            "1.5:1 ratio should exceed 1:1 limit"
        );

        // Exact boundary: 10 uncompressed from 10 compressed = 1:1 ratio, should pass
        assert!(
            limit.check(10, 10).is_ok(),
            "1:1 ratio should pass 1:1 limit"
        );

        // Just over boundary: 11 uncompressed from 10 compressed = 1.1:1 ratio
        assert!(
            limit.check(10, 11).is_err(),
            "1.1:1 ratio should exceed 1:1 limit"
        );
    }

    // BindPair validation tests

    #[test]
    fn test_validate_bind_pairs_valid() {
        // Create a 2-coder folder where the second coder feeds into the first
        let folder = Folder {
            coders: vec![
                Coder {
                    method_id: vec![0x03, 0x01, 0x01], // BCJ filter
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
                Coder {
                    method_id: vec![0x21], // LZMA2
                    num_in_streams: 1,
                    num_out_streams: 1,
                    properties: None,
                },
            ],
            // Coder 1 output (index 1) feeds into coder 0 input (index 0)
            bind_pairs: vec![BindPair {
                in_index: 0,  // First coder's input stream
                out_index: 1, // Second coder's output stream
            }],
            packed_streams: vec![1], // LZMA2 reads from packed stream
            unpack_sizes: vec![100, 100],
            unpack_crc: None,
        };

        // Valid bind_pairs should pass
        assert!(folder.validate_bind_pairs().is_ok());
    }

    #[test]
    fn test_validate_bind_pairs_in_index_out_of_bounds() {
        // Create a folder where bind_pair.in_index exceeds total_in_streams
        let folder = Folder {
            coders: vec![Coder {
                method_id: vec![0x21],
                num_in_streams: 1, // Only 1 input stream (index 0)
                num_out_streams: 1,
                properties: None,
            }],
            bind_pairs: vec![BindPair {
                in_index: 5, // Invalid: exceeds total_in_streams (1)
                out_index: 0,
            }],
            packed_streams: vec![0],
            unpack_sizes: vec![100],
            unpack_crc: None,
        };

        let result = folder.validate_bind_pairs();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("in_index"),
            "Error should mention in_index: {}",
            err_msg
        );
    }

    #[test]
    fn test_validate_bind_pairs_out_index_out_of_bounds() {
        // Create a folder where bind_pair.out_index exceeds total_out_streams
        let folder = Folder {
            coders: vec![Coder {
                method_id: vec![0x21],
                num_in_streams: 1,
                num_out_streams: 1, // Only 1 output stream (index 0)
                properties: None,
            }],
            bind_pairs: vec![BindPair {
                in_index: 0,
                out_index: 10, // Invalid: exceeds total_out_streams (1)
            }],
            packed_streams: vec![0],
            unpack_sizes: vec![100],
            unpack_crc: None,
        };

        let result = folder.validate_bind_pairs();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("out_index"),
            "Error should mention out_index: {}",
            err_msg
        );
    }

    #[test]
    fn test_validate_bind_pairs_empty_is_valid() {
        // Single-coder folder with no bind_pairs is valid
        let folder = Folder {
            coders: vec![Coder {
                method_id: vec![0x21],
                num_in_streams: 1,
                num_out_streams: 1,
                properties: None,
            }],
            bind_pairs: vec![], // No bind pairs for single coder
            packed_streams: vec![0],
            unpack_sizes: vec![100],
            unpack_crc: None,
        };

        assert!(folder.validate_bind_pairs().is_ok());
    }

    // =========================================================================
    // ResourceLimits Builder Tests
    // =========================================================================

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();

        // Verify default limits are reasonable
        assert!(limits.max_entries > 0);
        assert!(limits.max_header_bytes > 0);
        assert!(limits.max_total_unpacked > 0);
        assert!(limits.max_entry_unpacked > 0);
        assert!(limits.ratio_limit.is_some());
    }

    #[test]
    fn test_resource_limits_unlimited() {
        let limits = ResourceLimits::unlimited();

        assert_eq!(limits.max_entries, usize::MAX);
        assert_eq!(limits.max_header_bytes, u64::MAX);
        assert_eq!(limits.max_total_unpacked, u64::MAX);
        assert_eq!(limits.max_entry_unpacked, u64::MAX);
        assert!(limits.ratio_limit.is_none());
    }

    #[test]
    fn test_resource_limits_builder_methods() {
        let limits = ResourceLimits::new()
            .max_entries(100)
            .max_header_bytes(1024)
            .max_total_unpacked(1_000_000)
            .max_entry_unpacked(100_000);

        assert_eq!(limits.max_entries, 100);
        assert_eq!(limits.max_header_bytes, 1024);
        assert_eq!(limits.max_total_unpacked, 1_000_000);
        assert_eq!(limits.max_entry_unpacked, 100_000);
    }
}
