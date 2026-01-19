//! Random access reader for non-solid archives.
//!
//! This module provides [`RandomAccessReader`] for archives where each file
//! is stored in its own block, enabling direct access to specific entries.

use std::io::{self, Read, Seek, SeekFrom};

use crate::format::SIGNATURE_HEADER_SIZE;
use crate::format::header::StartHeader;
use crate::format::parser::{ArchiveHeader, read_archive_header};
use crate::format::streams::ResourceLimits;
use crate::read::Entry;
use crate::{Error, Result};

#[cfg(feature = "aes")]
use crate::Password;

use super::config::StreamingConfig;

/// Random access reader for non-solid archives.
///
/// Non-solid archives store each file in its own block, enabling direct
/// access to specific entries without decompressing others. This is more
/// flexible but typically achieves lower compression ratios.
///
/// # Note
///
/// Random access is only available for non-solid archives. For solid
/// archives, use [`crate::streaming::EntryIterator`] for sequential access.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::RandomAccessReader;
///
/// let mut reader = RandomAccessReader::new(file, password, config)?;
///
/// if reader.supports_random_access() {
///     // Access entries out of order
///     let entry3 = reader.entry_reader(3)?;
///     let entry1 = reader.entry_reader(1)?;
/// }
/// ```
pub struct RandomAccessReader<R: Read + Seek> {
    /// Source reader
    source: R,
    /// Start header
    #[allow(dead_code)] // Header metadata preserved for archive info
    start_header: StartHeader,
    /// Archive header
    header: ArchiveHeader,
    /// Entry list
    entries: Vec<Entry>,
    /// Entries that were skipped during parsing (e.g., invalid paths)
    skipped_entries: Vec<super::SkippedEntry>,
    /// Password for encrypted archives
    #[cfg(feature = "aes")]
    #[allow(dead_code)] // Reserved for encrypted random access
    password: Password,
    /// Configuration
    #[allow(dead_code)] // Reserved for configuration usage
    config: StreamingConfig,
    /// Pack data start position
    pack_start: u64,
    /// Whether the archive is solid
    is_solid: bool,
}

impl<R: Read + Seek> RandomAccessReader<R> {
    /// Creates a new random access reader.
    #[cfg(feature = "aes")]
    pub fn new(mut source: R, password: Password, config: StreamingConfig) -> Result<Self> {
        config.validate()?;

        let limits = ResourceLimits::default()
            .max_entries(config.max_entries)
            .ratio_limit(Some(crate::format::streams::RatioLimit::new(
                config.max_compression_ratio,
            )));

        let (start_header, header) = read_archive_header(&mut source, Some(limits))?;
        let (entries, skipped_entries) = Self::build_entries(&header);
        let is_solid = super::check_is_solid(&header);
        let pack_start = Self::calculate_pack_start(&start_header);

        Ok(Self {
            source,
            start_header,
            header,
            entries,
            skipped_entries,
            password,
            config,
            pack_start,
            is_solid,
        })
    }

    /// Creates a new random access reader (without AES support).
    #[cfg(not(feature = "aes"))]
    pub fn new(mut source: R, config: StreamingConfig) -> Result<Self> {
        config.validate()?;

        let limits = ResourceLimits::default()
            .max_entries(config.max_entries)
            .ratio_limit(Some(crate::format::streams::RatioLimit::new(
                config.max_compression_ratio,
            )));

        let (start_header, header) = read_archive_header(&mut source, Some(limits))?;
        let (entries, skipped_entries) = Self::build_entries(&header);
        let is_solid = super::check_is_solid(&header);
        let pack_start = Self::calculate_pack_start(&start_header);

        Ok(Self {
            source,
            start_header,
            header,
            entries,
            skipped_entries,
            config,
            pack_start,
            is_solid,
        })
    }

    fn build_entries(header: &ArchiveHeader) -> (Vec<Entry>, Vec<super::SkippedEntry>) {
        use crate::ArchivePath;

        let files_info = match &header.files_info {
            Some(info) => info,
            None => return (Vec::new(), Vec::new()),
        };

        let substreams = header.substreams_info.as_ref();

        let mut entries = Vec::with_capacity(files_info.entries.len());
        let mut skipped_entries = Vec::new();
        let mut stream_idx: usize = 0;
        let mut folder_idx: usize = 0;

        for (idx, archive_entry) in files_info.entries.iter().enumerate() {
            let path = match ArchivePath::new(&archive_entry.name) {
                Ok(p) => p,
                Err(e) => {
                    skipped_entries.push(super::SkippedEntry {
                        index: idx,
                        raw_path: Some(archive_entry.name.as_bytes().to_vec()),
                        reason: super::SkipReason::InvalidPath(e.to_string()),
                    });
                    continue;
                }
            };

            let (folder_index, stream_index) = if archive_entry.is_directory {
                (None, None)
            } else {
                let fi = folder_idx;
                let si = stream_idx;

                if let Some(ss) = substreams {
                    if folder_idx < ss.num_unpack_streams_in_folders.len() {
                        stream_idx += 1;
                        let num_streams = ss.num_unpack_streams_in_folders[folder_idx] as usize;
                        if stream_idx >= num_streams {
                            stream_idx = 0;
                            folder_idx += 1;
                        }
                    }
                } else {
                    folder_idx += 1;
                }

                (Some(fi), Some(si))
            };

            let is_encrypted = folder_index
                .and_then(|fi| Self::check_folder_encryption(header, fi))
                .unwrap_or(false);

            // Detect symlinks from attributes
            let is_symlink = !archive_entry.is_directory
                && archive_entry.attributes.is_some_and(|attrs| {
                    // Unix symlink mode (S_IFLNK = 0o120000) in high 16 bits
                    let unix_mode = (attrs >> 16) & 0xFFFF;
                    let is_unix_symlink = unix_mode != 0 && (unix_mode & 0o170000) == 0o120000;
                    // Windows REPARSE_POINT (0x400) in low 16 bits
                    let is_windows_symlink = (attrs & 0x400) != 0;
                    is_unix_symlink || is_windows_symlink
                });

            entries.push(Entry {
                path,
                is_directory: archive_entry.is_directory,
                size: archive_entry.size,
                crc32: archive_entry.crc,
                crc64: None,
                modification_time: archive_entry.mtime,
                creation_time: archive_entry.ctime,
                access_time: archive_entry.atime,
                attributes: archive_entry.attributes,
                is_encrypted,
                is_symlink,
                is_anti: archive_entry.is_anti,
                ownership: None,
                index: idx,
                folder_index,
                stream_index,
            });
        }

        (entries, skipped_entries)
    }

    fn check_folder_encryption(header: &ArchiveHeader, folder_index: usize) -> Option<bool> {
        let unpack_info = header.unpack_info.as_ref()?;
        let folder = unpack_info.folders.get(folder_index)?;
        Some(
            folder
                .coders
                .iter()
                .any(|coder| coder.method_id.as_slice() == crate::codec::method::AES),
        )
    }

    fn calculate_pack_start(start_header: &StartHeader) -> u64 {
        // Pack data starts after signature header + next header offset
        SIGNATURE_HEADER_SIZE + start_header.next_header_offset
    }

    /// Returns true if random access is supported (non-solid archive).
    pub fn supports_random_access(&self) -> bool {
        !self.is_solid
    }

    /// Returns true if this is a solid archive.
    pub fn is_solid(&self) -> bool {
        self.is_solid
    }

    /// Returns the archive entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Returns the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns entries that were skipped during archive parsing.
    ///
    /// Entries may be skipped if they have invalid paths.
    pub fn skipped_entries(&self) -> &[super::SkippedEntry] {
        &self.skipped_entries
    }

    /// Returns true if any entries were skipped during archive parsing.
    pub fn has_skipped_entries(&self) -> bool {
        !self.skipped_entries.is_empty()
    }

    /// Finds an entry by name.
    pub fn entry_by_name(&self, name: &str) -> Option<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.path.as_str() == name)
    }

    /// Returns a reader for a specific entry by index.
    ///
    /// This is only available for non-solid archives.
    pub fn entry_reader(&mut self, index: usize) -> Result<RandomEntryReader<'_>> {
        if self.is_solid {
            return Err(Error::UnsupportedFeature {
                feature: "random access for solid archives - use sequential iteration",
            });
        }

        // First pass: validate and extract info without holding borrow
        let (is_directory, folder_index) = {
            let entry = self.entries.get(index).ok_or_else(|| {
                Error::InvalidFormat(format!("entry index {} out of range", index))
            })?;
            (entry.is_directory, entry.folder_index)
        };

        // Handle directory or empty file (no decoder needed)
        if is_directory || folder_index.is_none() {
            let entry = &self.entries[index];
            return Ok(RandomEntryReader::empty(entry));
        }

        let folder_index = folder_index.unwrap();

        // Get folder info (need to clone coders to avoid borrow issues)
        let (coder_id, coder_properties, uncompressed_size) = {
            let folders = self
                .header
                .unpack_info
                .as_ref()
                .map(|ui| &ui.folders)
                .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

            let folder = folders.get(folder_index).ok_or_else(|| {
                Error::InvalidFormat(format!("folder index {} out of range", folder_index))
            })?;

            if folder.coders.is_empty() {
                return Err(Error::InvalidFormat("folder has no coders".into()));
            }

            let coder = &folder.coders[0];
            (
                coder.method_id.clone(),
                coder.properties.clone(),
                folder.final_unpack_size().unwrap_or(0),
            )
        };

        // Calculate folder offset
        let folder_offset = self.calculate_folder_offset(folder_index)?;

        // Get pack size
        let pack_size = self
            .header
            .pack_info
            .as_ref()
            .and_then(|pi| pi.pack_sizes.get(folder_index).copied())
            .unwrap_or(0);

        // Seek and read packed data
        self.source
            .seek(SeekFrom::Start(folder_offset))
            .map_err(Error::Io)?;

        let mut packed_data = vec![0u8; pack_size as usize];
        self.source
            .read_exact(&mut packed_data)
            .map_err(Error::Io)?;

        // Build decoder with copied coder info
        let cursor = std::io::Cursor::new(packed_data);
        let temp_coder = crate::format::streams::Coder {
            method_id: coder_id,
            properties: coder_properties,
            num_in_streams: 1,
            num_out_streams: 1,
        };
        let decoder = crate::codec::build_decoder(cursor, &temp_coder, uncompressed_size)?;
        let boxed_decoder: Box<dyn Read + Send + 'static> = Box::new(decoder);

        // Now get entry reference for return
        let entry = &self.entries[index];
        Ok(RandomEntryReader::with_decoder(entry, boxed_decoder))
    }

    /// Returns a reader for a specific entry by name.
    pub fn entry_reader_by_name(&mut self, name: &str) -> Result<RandomEntryReader<'_>> {
        let index = self
            .entries
            .iter()
            .position(|e| e.path.as_str() == name)
            .ok_or_else(|| Error::InvalidFormat(format!("entry not found: {}", name)))?;

        self.entry_reader(index)
    }

    /// Extracts a specific entry to a Write sink.
    pub fn extract_entry_to<W: io::Write>(&mut self, index: usize, sink: &mut W) -> Result<u64> {
        let mut reader = self.entry_reader(index)?;
        let written = io::copy(&mut reader, sink).map_err(Error::Io)?;
        Ok(written)
    }

    /// Extracts a specific entry by name to a Write sink.
    pub fn extract_entry_by_name_to<W: io::Write>(
        &mut self,
        name: &str,
        sink: &mut W,
    ) -> Result<u64> {
        let mut reader = self.entry_reader_by_name(name)?;
        let written = io::copy(&mut reader, sink).map_err(Error::Io)?;
        Ok(written)
    }

    fn calculate_folder_offset(&self, folder_index: usize) -> Result<u64> {
        let pack_info = self
            .header
            .pack_info
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

        let mut offset = self.pack_start + pack_info.pack_pos;

        // Sum up pack sizes for previous folders
        for i in 0..folder_index {
            if i < pack_info.pack_sizes.len() {
                offset += pack_info.pack_sizes[i];
            }
        }

        Ok(offset)
    }
}

/// Reader for a single entry accessed randomly.
pub struct RandomEntryReader<'a> {
    entry: &'a Entry,
    decoder: Option<Box<dyn Read + Send + 'static>>,
    bytes_read: u64,
}

impl<'a> RandomEntryReader<'a> {
    fn empty(entry: &'a Entry) -> Self {
        Self {
            entry,
            decoder: None,
            bytes_read: 0,
        }
    }

    fn with_decoder(entry: &'a Entry, decoder: Box<dyn Read + Send + 'static>) -> Self {
        Self {
            entry,
            decoder: Some(decoder),
            bytes_read: 0,
        }
    }

    /// Returns the entry metadata.
    pub fn entry(&self) -> &Entry {
        self.entry
    }

    /// Returns the expected size of the entry.
    pub fn size(&self) -> u64 {
        self.entry.size
    }

    /// Returns the number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the remaining bytes to read.
    pub fn remaining(&self) -> u64 {
        self.entry.size.saturating_sub(self.bytes_read)
    }

    /// Reads the entire entry into a Vec.
    pub fn read_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::with_capacity(self.entry.size as usize);
        io::copy(self, &mut data).map_err(Error::Io)?;
        Ok(data)
    }
}

impl Read for RandomEntryReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let decoder = match &mut self.decoder {
            Some(d) => d,
            None => return Ok(0),
        };

        let n = decoder.read(buf)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

/// Entry locator for finding entries by various criteria.
pub struct EntryLocator<'a> {
    entries: &'a [Entry],
}

impl<'a> EntryLocator<'a> {
    /// Creates a new entry locator.
    pub fn new(entries: &'a [Entry]) -> Self {
        Self { entries }
    }

    /// Finds an entry by exact name match.
    pub fn by_name(&self, name: &str) -> Option<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.path.as_str() == name)
    }

    /// Finds entries matching a pattern (glob-like).
    pub fn by_pattern(&self, pattern: &str) -> Vec<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| Self::matches_pattern(e.path.as_str(), pattern))
            .collect()
    }

    /// Finds entries by extension.
    pub fn by_extension(&self, ext: &str) -> Vec<(usize, &Entry)> {
        let ext_lower = ext.to_lowercase();
        let ext_with_dot = if ext.starts_with('.') {
            ext_lower
        } else {
            format!(".{}", ext_lower)
        };

        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.path.as_str().to_lowercase().ends_with(&ext_with_dot))
            .collect()
    }

    /// Finds files only (not directories).
    pub fn files_only(&self) -> Vec<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_directory)
            .collect()
    }

    /// Finds directories only.
    pub fn directories_only(&self) -> Vec<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_directory)
            .collect()
    }

    /// Simple pattern matching (supports * and ?).
    fn matches_pattern(name: &str, pattern: &str) -> bool {
        let name_chars: Vec<char> = name.chars().collect();
        let pattern_chars: Vec<char> = pattern.chars().collect();

        Self::match_recursive(&name_chars, &pattern_chars, 0, 0)
    }

    fn match_recursive(name: &[char], pattern: &[char], ni: usize, pi: usize) -> bool {
        if pi >= pattern.len() {
            return ni >= name.len();
        }

        let p = pattern[pi];

        if p == '*' {
            // Match zero or more characters
            for i in ni..=name.len() {
                if Self::match_recursive(name, pattern, i, pi + 1) {
                    return true;
                }
            }
            return false;
        }

        if ni >= name.len() {
            return false;
        }

        if p == '?' || p == name[ni] {
            return Self::match_recursive(name, pattern, ni + 1, pi + 1);
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_entries() -> Vec<Entry> {
        use crate::ArchivePath;

        vec![
            Entry {
                path: ArchivePath::new("readme.txt").unwrap(),
                is_directory: false,
                size: 100,
                crc32: Some(0x12345678),
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
                folder_index: Some(0),
                stream_index: Some(0),
            },
            Entry {
                path: ArchivePath::new("src/main.rs").unwrap(),
                is_directory: false,
                size: 500,
                crc32: Some(0xDEADBEEF),
                crc64: None,
                modification_time: None,
                creation_time: None,
                access_time: None,
                attributes: None,
                is_encrypted: false,
                is_symlink: false,
                is_anti: false,
                ownership: None,
                index: 1,
                folder_index: Some(1),
                stream_index: Some(0),
            },
            Entry {
                path: ArchivePath::new("src").unwrap(),
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
                index: 2,
                folder_index: None,
                stream_index: None,
            },
        ]
    }

    #[test]
    fn test_entry_locator_by_name() {
        let entries = make_test_entries();
        let locator = EntryLocator::new(&entries);

        let found = locator.by_name("readme.txt");
        assert!(found.is_some());
        assert_eq!(found.unwrap().0, 0);

        let not_found = locator.by_name("nonexistent.txt");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_entry_locator_by_extension() {
        let entries = make_test_entries();
        let locator = EntryLocator::new(&entries);

        let txt_files = locator.by_extension("txt");
        assert_eq!(txt_files.len(), 1);

        let rs_files = locator.by_extension(".rs");
        assert_eq!(rs_files.len(), 1);
    }

    #[test]
    fn test_entry_locator_files_only() {
        let entries = make_test_entries();
        let locator = EntryLocator::new(&entries);

        let files = locator.files_only();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_entry_locator_directories_only() {
        let entries = make_test_entries();
        let locator = EntryLocator::new(&entries);

        let dirs = locator.directories_only();
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn test_pattern_matching() {
        assert!(EntryLocator::matches_pattern("readme.txt", "*.txt"));
        assert!(EntryLocator::matches_pattern("readme.txt", "readme.*"));
        assert!(EntryLocator::matches_pattern("readme.txt", "r?adme.txt"));
        assert!(EntryLocator::matches_pattern("readme.txt", "*"));
        assert!(!EntryLocator::matches_pattern("readme.txt", "*.rs"));
    }

    #[test]
    fn test_entry_locator_by_pattern() {
        let entries = make_test_entries();
        let locator = EntryLocator::new(&entries);

        let txt_files = locator.by_pattern("*.txt");
        assert_eq!(txt_files.len(), 1);

        let src_files = locator.by_pattern("src/*");
        assert_eq!(src_files.len(), 1);
    }
}
