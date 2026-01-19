//! File extraction from archives.
//!
//! This module provides methods for extracting entries from archives
//! to various destinations (files, memory, custom destinations).

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::format::streams::Folder;
use crate::{Error, READ_BUFFER_SIZE, Result};

use super::metadata::{apply_metadata, calculate_file_crc};
use super::path_safety::{create_symlink, validate_path, validate_symlink_target};
use super::{
    Archive, EntrySelector, ExtractDestination, ExtractOptions, ExtractResult, ExtractionLimits,
    LinkPolicy, OverwritePolicy,
};

impl<R: Read + Seek> Archive<R> {
    /// Extracts entries to a destination directory.
    ///
    /// # Arguments
    ///
    /// * `dest` - Destination directory
    /// * `selector` - Selects which entries to extract. Pass `()` to extract all
    ///   entries, or use [`SelectAll`], a closure, or other [`EntrySelector`]
    ///   implementations for filtering.
    /// * `options` - Extraction options
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use zesven::{Archive, ExtractOptions};
    ///
    /// let mut archive = Archive::open_path("archive.7z")?;
    ///
    /// // Extract all entries (most common usage)
    /// archive.extract("./output", (), &ExtractOptions::default())?;
    ///
    /// // Extract only files smaller than 1 MiB
    /// archive.extract("./output", |e| e.size < 1024 * 1024, &ExtractOptions::default())?;
    /// ```
    ///
    /// # Returns
    ///
    /// An ExtractResult containing extraction statistics.
    ///
    /// [`SelectAll`]: crate::read::SelectAll
    /// [`EntrySelector`]: crate::read::EntrySelector
    pub fn extract(
        &mut self,
        dest: impl AsRef<Path>,
        selector: impl EntrySelector,
        options: &ExtractOptions,
    ) -> Result<ExtractResult> {
        let dest = dest.as_ref();
        let mut result = ExtractResult::default();

        // Create extraction limits context with shared tracker for total bytes
        let limits = ExtractionLimits::from_resource_limits(&options.limits);

        // Validate destination
        if !dest.exists() {
            std::fs::create_dir_all(dest).map_err(Error::Io)?;
        }

        // Collect entries to extract (to avoid borrow conflict)
        let entries_to_extract: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selector.select(e))
            .map(|(idx, _)| idx)
            .collect();

        for idx in entries_to_extract {
            // Check for cancellation before each entry
            if let Some(ref progress) = options.progress {
                if progress.should_cancel() {
                    return Err(Error::Cancelled);
                }
            }

            let entry = &self.entries[idx];

            if entry.is_directory {
                // Create directory
                let dir_path = dest.join(entry.path.as_str());
                if let Err(e) = std::fs::create_dir_all(&dir_path) {
                    result.entries_failed += 1;
                    result
                        .failures
                        .push((entry.path.as_str().to_string(), e.to_string()));
                } else {
                    result.entries_extracted += 1;
                }
            } else {
                // Extract file
                let entry_path = entry.path.as_str().to_string();
                match self.extract_entry_by_index(idx, dest, options, &limits) {
                    Ok(bytes) => {
                        result.entries_extracted += 1;
                        result.bytes_extracted += bytes;
                    }
                    Err(Error::Cancelled) => {
                        // Cancellation requested - clean up any partial file and return
                        let safe_path = dest.join(&entry_path);
                        if safe_path.exists() {
                            if let Err(e) = std::fs::remove_file(&safe_path) {
                                log::warn!(
                                    "Failed to clean up partial file '{}': {}",
                                    safe_path.display(),
                                    e
                                );
                            }
                        }
                        return Err(Error::Cancelled);
                    }
                    Err(e) => {
                        if matches!(options.overwrite, OverwritePolicy::Skip) {
                            result.entries_skipped += 1;
                        } else {
                            result.entries_failed += 1;
                            result.failures.push((entry_path, e.to_string()));
                        }
                    }
                }
            }

            // Check for cancellation after each entry
            if let Some(ref progress) = options.progress {
                if progress.should_cancel() {
                    return Err(Error::Cancelled);
                }
            }
        }

        Ok(result)
    }

    /// Extracts all entries to a custom destination.
    ///
    /// This method provides flexible extraction using any type implementing
    /// the [`ExtractDestination`] trait.
    ///
    /// # Arguments
    ///
    /// * `dest` - The destination to extract entries to
    ///
    /// # Returns
    ///
    /// The extraction result containing statistics.
    pub fn extract_to_destination<D: ExtractDestination>(
        &mut self,
        dest: &mut D,
    ) -> Result<ExtractResult> {
        let mut result = ExtractResult::default();

        dest.on_start(self.entries.len())?;

        for entry_idx in 0..self.entries.len() {
            // Clone entry to avoid borrow issues
            let entry = self.entries[entry_idx].clone();

            if entry.is_directory {
                dest.on_directory(&entry)?;
                result.entries_extracted += 1;
            } else {
                // Extract the entry to a writer
                let mut writer = dest.create_writer(&entry)?;

                match self.extract_entry_to_writer_by_index(entry_idx, &mut *writer) {
                    Ok(bytes) => {
                        drop(writer); // Ensure writer is dropped before on_complete
                        dest.on_complete(&entry, true)?;
                        result.entries_extracted += 1;
                        result.bytes_extracted += bytes;
                    }
                    Err(e) => {
                        drop(writer);
                        dest.on_complete(&entry, false)?;
                        result.entries_failed += 1;
                        result
                            .failures
                            .push((entry.path.as_str().to_string(), e.to_string()));
                    }
                }
            }
        }

        dest.on_finish(result.entries_failed == 0)?;

        Ok(result)
    }

    /// Extracts an entry to a writer by index.
    fn extract_entry_to_writer_by_index<W: std::io::Write + ?Sized>(
        &mut self,
        entry_idx: usize,
        writer: &mut W,
    ) -> Result<u64> {
        // Extract to a vec first, then write to the writer
        let data = self.extract_entry_to_vec_by_index(entry_idx)?;
        writer.write_all(&data).map_err(Error::Io)?;
        Ok(data.len() as u64)
    }

    pub(crate) fn extract_entry_by_index(
        &mut self,
        entry_idx: usize,
        dest: &Path,
        options: &ExtractOptions,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        // Copy needed data from entry to avoid borrow issues
        let entry_path_str = self.entries[entry_idx].path.as_str().to_string();
        let entry_size = self.entries[entry_idx].size;
        let entry_crc = self.entries[entry_idx].crc32;
        let folder_index = self.entries[entry_idx].folder_index;
        let stream_index = self.entries[entry_idx].stream_index;
        let is_symlink = self.entries[entry_idx].is_symlink;

        // Copy metadata for preservation
        let modification_time = self.entries[entry_idx].modification_time;
        let creation_time = self.entries[entry_idx].creation_time;
        let attributes = self.entries[entry_idx].attributes;

        // Check symlink policy BEFORE doing any extraction work
        if is_symlink {
            match options.link_policy {
                LinkPolicy::Forbid => {
                    return Err(Error::SymlinkRejected {
                        entry_index: entry_idx,
                        path: entry_path_str,
                    });
                }
                LinkPolicy::ValidateTargets | LinkPolicy::Allow => {
                    // Will handle symlink creation below after extracting target
                }
            }
        }

        // Validate path safety
        let safe_path = validate_path(entry_idx, &entry_path_str, dest, &options.path_safety)?;

        // Create parent directories
        if let Some(parent) = safe_path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }

        // Check overwrite policy
        if safe_path.exists() {
            match options.overwrite {
                OverwritePolicy::Error => {
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("file already exists: {}", safe_path.display()),
                    )));
                }
                OverwritePolicy::Skip => {
                    return Ok(0);
                }
                OverwritePolicy::Overwrite => {}
            }
        }

        // Handle empty files (no folder assignment)
        let folder_idx = match folder_index {
            Some(idx) => idx,
            None => {
                // Empty file or empty symlink - just create it
                if is_symlink {
                    // Empty symlink target - this is invalid
                    return Err(Error::InvalidFormat(format!(
                        "symlink '{}' has no target content",
                        entry_path_str
                    )));
                }
                File::create(&safe_path).map_err(Error::Io)?;
                return Ok(0);
            }
        };

        // Get folder and pack info
        let unpack_info = self
            .header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        // Clone folder to release the borrow before mutable extraction calls
        let folder = unpack_info
            .folders
            .get(folder_idx)
            .ok_or_else(|| {
                Error::InvalidFormat(format!("folder index {} out of range", folder_idx))
            })?
            .clone();

        // Handle symlinks specially - extract target to memory, then create symlink
        if is_symlink {
            // Extract symlink target content to memory
            let mut target_bytes = Vec::with_capacity(entry_size as usize);

            #[cfg(feature = "lzma")]
            if folder.uses_bcj2() {
                self.extract_bcj2(&folder, folder_idx, stream_index, &mut target_bytes, limits)?;
            } else {
                self.extract_single_stream(
                    &folder,
                    folder_idx,
                    stream_index,
                    entry_size,
                    &mut target_bytes,
                    limits,
                )?;
            }

            #[cfg(not(feature = "lzma"))]
            self.extract_single_stream(
                &folder,
                folder_idx,
                stream_index,
                entry_size,
                &mut target_bytes,
                limits,
            )?;

            // Convert target bytes to string
            let target = String::from_utf8(target_bytes).map_err(|_| {
                Error::InvalidFormat(format!("symlink '{}' has non-UTF-8 target", entry_path_str))
            })?;

            // Validate target if policy requires it
            if matches!(options.link_policy, LinkPolicy::ValidateTargets) {
                validate_symlink_target(entry_idx, &entry_path_str, &target)?;
            }

            // Create the symlink
            return create_symlink(&safe_path, &target);
        }

        // Create output file (regular file path)
        let mut file = File::create(&safe_path).map_err(Error::Io)?;

        // Check for BCJ2 (multi-stream extraction)
        #[cfg(feature = "lzma")]
        let bytes_written = if folder.uses_bcj2() {
            self.extract_bcj2(&folder, folder_idx, stream_index, &mut file, limits)?
        } else {
            self.extract_single_stream(
                &folder,
                folder_idx,
                stream_index,
                entry_size,
                &mut file,
                limits,
            )?
        };

        #[cfg(not(feature = "lzma"))]
        let bytes_written = self.extract_single_stream(
            &folder,
            folder_idx,
            stream_index,
            entry_size,
            &mut file,
            limits,
        )?;

        // Verify CRC if available
        if let Some(expected_crc) = entry_crc {
            // Re-read file and calculate CRC
            file.flush().map_err(Error::Io)?;
            drop(file);

            let actual_crc = calculate_file_crc(&safe_path)?;
            if actual_crc != expected_crc {
                // Delete corrupted file
                if let Err(e) = std::fs::remove_file(&safe_path) {
                    log::warn!(
                        "Failed to clean up corrupted file '{}': {}",
                        safe_path.display(),
                        e
                    );
                }
                return Err(Error::CrcMismatch {
                    entry_index: entry_idx,
                    entry_name: Some(entry_path_str.clone()),
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
        }

        // Preserve metadata based on options
        apply_metadata(
            &safe_path,
            &options.preserve_metadata,
            modification_time,
            creation_time,
            attributes,
        );

        Ok(bytes_written)
    }

    /// Extracts an entry by name to a Vec.
    ///
    /// This is useful for in-memory extraction, such as in WASM environments.
    ///
    /// # Arguments
    ///
    /// * `name` - Path of the entry to extract
    ///
    /// # Returns
    ///
    /// The decompressed entry data as a `Vec<u8>`.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry is not found, is a directory, or extraction fails.
    pub fn extract_to_vec(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry_idx = self
            .entries
            .iter()
            .position(|e| e.path.as_str() == name)
            .ok_or_else(|| Error::InvalidFormat(format!("entry not found: {}", name)))?;

        self.extract_entry_to_vec_by_index(entry_idx)
    }

    /// Extracts an entry by index to a Vec.
    ///
    /// # Arguments
    ///
    /// * `entry_idx` - Index of the entry to extract
    ///
    /// # Returns
    ///
    /// The decompressed entry data as a `Vec<u8>`.
    pub fn extract_entry_to_vec_by_index(&mut self, entry_idx: usize) -> Result<Vec<u8>> {
        let entry = self.entries.get(entry_idx).ok_or_else(|| {
            Error::InvalidFormat(format!("entry index {} out of range", entry_idx))
        })?;

        if entry.is_directory {
            return Err(Error::InvalidFormat(
                "cannot extract directory to vec".into(),
            ));
        }

        // Empty files (size=0, no stream) can be extracted as empty Vec
        // These have folder_index=None because they don't have data streams
        if entry.size == 0 && entry.folder_index.is_none() {
            return Ok(Vec::new());
        }

        let entry_size = entry.size;
        let entry_crc = entry.crc32;
        let folder_idx = entry
            .folder_index
            .ok_or_else(|| Error::InvalidFormat("entry has no folder index".into()))?;
        let stream_index = entry.stream_index;

        // Get folder and pack info
        let unpack_info = self
            .header
            .unpack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

        // Clone folder to release the borrow before mutable extraction calls
        let folder = unpack_info
            .folders
            .get(folder_idx)
            .ok_or_else(|| {
                Error::InvalidFormat(format!("folder index {} out of range", folder_idx))
            })?
            .clone();

        // Extract to Vec (use unlimited limits for public API backward compatibility)
        let limits = ExtractionLimits::unlimited();
        let mut output = Vec::with_capacity(entry_size as usize);

        // Check for BCJ2 (multi-stream extraction)
        #[cfg(feature = "lzma")]
        if folder.uses_bcj2() {
            self.extract_bcj2(&folder, folder_idx, stream_index, &mut output, &limits)?;
        } else {
            self.extract_single_stream(
                &folder,
                folder_idx,
                stream_index,
                entry_size,
                &mut output,
                &limits,
            )?;
        }

        #[cfg(not(feature = "lzma"))]
        self.extract_single_stream(
            &folder,
            folder_idx,
            stream_index,
            entry_size,
            &mut output,
            &limits,
        )?;

        // Verify CRC if available
        if let Some(expected_crc) = entry_crc {
            let actual_crc = crc32fast::hash(&output);
            if actual_crc != expected_crc {
                return Err(Error::CrcMismatch {
                    entry_index: entry_idx,
                    entry_name: Some(self.entries[entry_idx].path.as_str().to_string()),
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
        }

        Ok(output)
    }

    /// Extracts a non-solid entry directly.
    pub(crate) fn extract_non_solid(
        &self,
        packed_data: Vec<u8>,
        folder: &Folder,
        expected_size: u64,
        output: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        let uncompressed_size = folder.final_unpack_size().unwrap_or(expected_size);
        let compressed_size = packed_data.len() as u64;

        let cursor = Cursor::new(packed_data);
        let decoder = self.build_decoder_chain(cursor, folder, uncompressed_size)?;

        // Wrap decoder with LimitedReader for resource limit enforcement
        let mut limited_decoder = limits.wrap_reader(decoder, compressed_size);

        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        loop {
            let n = limited_decoder
                .read(&mut buf)
                .map_err(super::map_io_error)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
        }

        Ok(total_written)
    }

    /// Extracts an entry from a solid block.
    pub(crate) fn extract_from_solid_block(
        &self,
        packed_data: Vec<u8>,
        folder: &Folder,
        folder_idx: usize,
        stream_index: usize,
        output: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        if folder.coders.is_empty() {
            return Err(Error::InvalidFormat("folder has no coders".into()));
        }

        // Get entry sizes within this solid block
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

        // Read the target entry with limit enforcement
        // For ratio checking in solid blocks, we use the target entry's size ratio
        // against the full compressed block (conservative estimate)
        let target_size = entry_sizes[stream_index];
        let mut limited_decoder = limits.wrap_reader(&mut decoder, compressed_size);

        let mut remaining = target_size;
        let mut total_written = 0u64;
        let mut buf = [0u8; READ_BUFFER_SIZE];

        while remaining > 0 {
            let to_read = buf.len().min(remaining as usize);
            let n = limited_decoder
                .read(&mut buf[..to_read])
                .map_err(super::map_io_error)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).map_err(Error::Io)?;
            total_written += n as u64;
            remaining -= n as u64;
        }

        Ok(total_written)
    }

    /// Extracts a single-stream entry (non-BCJ2).
    pub(crate) fn extract_single_stream(
        &mut self,
        folder: &Folder,
        folder_idx: usize,
        stream_index: Option<usize>,
        expected_size: u64,
        output: &mut impl Write,
        limits: &ExtractionLimits,
    ) -> Result<u64> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        // Calculate pack position (offset in the archive file)
        let pack_pos = self.calculate_pack_position(folder_idx)?;

        // Get pack size for this folder
        let pack_size = pack_info
            .pack_sizes
            .get(folder_idx)
            .copied()
            .ok_or_else(|| Error::InvalidFormat("missing pack size".into()))?;

        // Seek to pack position
        self.reader
            .seek(SeekFrom::Start(pack_pos))
            .map_err(Error::Io)?;

        // Read packed data
        let mut packed_data = vec![0u8; pack_size as usize];
        self.reader
            .read_exact(&mut packed_data)
            .map_err(Error::Io)?;

        // Check if this is a solid block (multiple entries in one folder)
        let is_solid_block = self.is_solid_block(folder_idx);

        // Decompress and write
        if is_solid_block {
            // Solid block: need to decompress sequentially and extract the right entry
            self.extract_from_solid_block(
                packed_data,
                folder,
                folder_idx,
                stream_index.unwrap_or(0),
                output,
                limits,
            )
        } else {
            // Non-solid: direct decompression
            self.extract_non_solid(packed_data, folder, expected_size, output, limits)
        }
    }
}
