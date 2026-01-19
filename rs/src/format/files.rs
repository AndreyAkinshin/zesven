//! Files info structures for 7z archives.
//!
//! These structures describe the file entries within an archive.

use crate::{Error, Result};
use std::io::Read;

use super::property_id;
use super::reader::{
    read_all_or_bits, read_bool_vector, read_bytes, read_u8, read_u32_le, read_u64_le,
    read_variable_u64,
};
use super::streams::ResourceLimits;

/// A single file entry in the archive.
#[derive(Debug, Clone, Default)]
pub struct ArchiveEntry {
    /// File name (path within the archive).
    pub name: String,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Whether this is an anti-item (for incremental backups).
    pub is_anti: bool,
    /// Whether this entry has an associated data stream.
    pub has_stream: bool,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// CRC32 checksum of uncompressed data.
    pub crc: Option<u32>,
    /// Creation time (Windows FILETIME, 100ns intervals since 1601-01-01).
    pub ctime: Option<u64>,
    /// Last access time (Windows FILETIME).
    pub atime: Option<u64>,
    /// Last modification time (Windows FILETIME).
    pub mtime: Option<u64>,
    /// Windows file attributes.
    pub attributes: Option<u32>,
}

impl ArchiveEntry {
    /// Returns true if this entry represents a file (not a directory).
    pub fn is_file(&self) -> bool {
        !self.is_directory
    }
}

/// Files info from the archive header.
#[derive(Debug, Clone, Default)]
pub struct FilesInfo {
    /// List of file entries.
    pub entries: Vec<ArchiveEntry>,
    /// Archive comment (if any).
    pub comment: Option<String>,
}

impl FilesInfo {
    /// Parses FilesInfo from a reader.
    ///
    /// The reader should be positioned after the K_FILES_INFO property ID.
    ///
    /// # Arguments
    ///
    /// * `r` - The reader
    /// * `unpack_sizes` - Sizes for each file with data
    /// * `digests` - CRC values for each file with data
    /// * `limits` - Resource limits
    pub fn parse<R: Read>(
        r: &mut R,
        unpack_sizes: &[u64],
        digests: &[Option<u32>],
        limits: &ResourceLimits,
    ) -> Result<Self> {
        let num_files = read_variable_u64(r)?;

        if num_files > limits.max_entries as u64 {
            return Err(Error::ResourceLimitExceeded(format!(
                "too many files: {}",
                num_files
            )));
        }

        let num_files = num_files as usize;
        let mut entries: Vec<ArchiveEntry> =
            (0..num_files).map(|_| ArchiveEntry::default()).collect();

        // Track which files have streams
        let mut empty_streams = vec![false; num_files];
        let mut empty_files = Vec::new();
        let mut anti_items = Vec::new();
        let mut comment: Option<String> = None;

        loop {
            let prop_id = read_u8(r)?;

            if prop_id == property_id::END {
                break;
            }

            let prop_size = read_variable_u64(r)?;

            match prop_id {
                property_id::NAME => {
                    // External flag
                    let external = read_u8(r)?;
                    if external != 0 {
                        return Err(Error::UnsupportedFeature {
                            feature: "external file names",
                        });
                    }

                    // Read names as UTF-16LE null-terminated strings
                    for entry in &mut entries {
                        entry.name = read_utf16le_string(r)?;
                    }
                }

                property_id::EMPTY_STREAM => {
                    empty_streams = read_bool_vector(r, num_files)?;
                }

                property_id::EMPTY_FILE => {
                    let num_empty = empty_streams.iter().filter(|&&x| x).count();
                    empty_files = read_bool_vector(r, num_empty)?;
                }

                property_id::ANTI => {
                    let num_empty = empty_streams.iter().filter(|&&x| x).count();
                    anti_items = read_bool_vector(r, num_empty)?;
                }

                property_id::CTIME => {
                    parse_timestamps(r, &mut entries, |e, t| e.ctime = Some(t))?;
                }

                property_id::ATIME => {
                    parse_timestamps(r, &mut entries, |e, t| e.atime = Some(t))?;
                }

                property_id::MTIME => {
                    parse_timestamps(r, &mut entries, |e, t| e.mtime = Some(t))?;
                }

                property_id::WIN_ATTRIBUTES => {
                    parse_attributes(r, &mut entries)?;
                }

                property_id::COMMENT => {
                    // Read comment as UTF-16LE string
                    // The format is: external flag (1 byte) + UTF-16LE null-terminated string
                    let external = read_u8(r)?;
                    if external != 0 {
                        return Err(Error::UnsupportedFeature {
                            feature: "external comments",
                        });
                    }
                    comment = Some(read_utf16le_string(r)?);
                }

                _ => {
                    // Skip unknown property
                    let _ = read_bytes(r, prop_size as usize)?;
                }
            }
        }

        // Process empty stream info
        let mut empty_idx = 0;
        for (i, &is_empty_stream) in empty_streams.iter().enumerate() {
            if is_empty_stream {
                entries[i].has_stream = false;

                // Check if it's a directory (not in empty_files) or anti-item
                if !empty_files.is_empty() && empty_idx < empty_files.len() {
                    entries[i].is_directory = !empty_files[empty_idx];
                } else {
                    entries[i].is_directory = true;
                }

                if !anti_items.is_empty() && empty_idx < anti_items.len() {
                    entries[i].is_anti = anti_items[empty_idx];
                }

                empty_idx += 1;
            } else {
                entries[i].has_stream = true;
            }
        }

        // Assign sizes and CRCs from substreams info
        let mut stream_idx = 0;
        for entry in &mut entries {
            if entry.has_stream {
                if stream_idx < unpack_sizes.len() {
                    entry.size = unpack_sizes[stream_idx];
                }
                if stream_idx < digests.len() {
                    entry.crc = digests[stream_idx];
                }
                stream_idx += 1;
            }
        }

        Ok(Self { entries, comment })
    }

    /// Returns the number of files.
    pub fn num_files(&self) -> usize {
        self.entries.len()
    }

    /// Returns the archive comment, if any.
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// Returns entries that have data streams (non-empty files).
    pub fn files_with_streams(&self) -> impl Iterator<Item = &ArchiveEntry> {
        self.entries.iter().filter(|e| e.has_stream)
    }

    /// Returns directory entries.
    pub fn directories(&self) -> impl Iterator<Item = &ArchiveEntry> {
        self.entries.iter().filter(|e| e.is_directory)
    }

    /// Returns anti-item entries (marked for deletion in incremental backups).
    ///
    /// Anti-items indicate files that should be removed when applying
    /// this archive as an incremental update.
    pub fn anti_items(&self) -> impl Iterator<Item = &ArchiveEntry> {
        self.entries.iter().filter(|e| e.is_anti)
    }

    /// Returns the number of anti-items.
    pub fn num_anti_items(&self) -> usize {
        self.entries.iter().filter(|e| e.is_anti).count()
    }
}

/// Maximum length for UTF-16LE strings read from archives (in code units).
///
/// This limit prevents denial-of-service attacks where a malicious archive
/// specifies an extremely long file name. 32,768 UTF-16 code units allows
/// for paths up to 65KB which far exceeds any reasonable file system path.
const MAX_UTF16_STRING_LENGTH: usize = 32768;

/// Reads a UTF-16LE null-terminated string.
fn read_utf16le_string<R: Read>(r: &mut R) -> Result<String> {
    let mut chars = Vec::new();

    loop {
        let mut buf = [0u8; 2];
        r.read_exact(&mut buf)?;
        let code_unit = u16::from_le_bytes(buf);

        if code_unit == 0 {
            break;
        }

        if chars.len() >= MAX_UTF16_STRING_LENGTH {
            return Err(Error::ResourceLimitExceeded(format!(
                "UTF-16 string exceeds maximum length of {} code units",
                MAX_UTF16_STRING_LENGTH
            )));
        }

        chars.push(code_unit);
    }

    String::from_utf16(&chars).map_err(|_| Error::InvalidFormat("invalid UTF-16 file name".into()))
}

/// Parses timestamps for entries.
fn parse_timestamps<R: Read, F>(
    r: &mut R,
    entries: &mut [ArchiveEntry],
    mut setter: F,
) -> Result<()>
where
    F: FnMut(&mut ArchiveEntry, u64),
{
    let defined = read_all_or_bits(r, entries.len())?;

    // External flag
    let external = read_u8(r)?;
    if external != 0 {
        return Err(Error::UnsupportedFeature {
            feature: "external timestamps",
        });
    }

    for (entry, &has_time) in entries.iter_mut().zip(defined.iter()) {
        if has_time {
            let timestamp = read_u64_le(r)?;
            setter(entry, timestamp);
        }
    }

    Ok(())
}

/// Parses Windows attributes for entries.
fn parse_attributes<R: Read>(r: &mut R, entries: &mut [ArchiveEntry]) -> Result<()> {
    let defined = read_all_or_bits(r, entries.len())?;

    // External flag
    let external = read_u8(r)?;
    if external != 0 {
        return Err(Error::UnsupportedFeature {
            feature: "external attributes",
        });
    }

    for (entry, &has_attr) in entries.iter_mut().zip(defined.iter()) {
        if has_attr {
            entry.attributes = Some(read_u32_le(r)?);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_variable_u64(buf: &mut Vec<u8>, value: u64) {
        use super::super::reader::write_variable_u64;
        write_variable_u64(buf, value).unwrap();
    }

    fn write_utf16le_string(buf: &mut Vec<u8>, s: &str) {
        for c in s.encode_utf16() {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        buf.extend_from_slice(&[0x00, 0x00]); // null terminator
    }

    #[test]
    fn test_utf16le_string() {
        let mut data = Vec::new();
        write_utf16le_string(&mut data, "test.txt");

        let mut cursor = Cursor::new(&data);
        let name = read_utf16le_string(&mut cursor).unwrap();
        assert_eq!(name, "test.txt");
    }

    #[test]
    fn test_utf16le_unicode() {
        let mut data = Vec::new();
        write_utf16le_string(&mut data, "日本語.txt");

        let mut cursor = Cursor::new(&data);
        let name = read_utf16le_string(&mut cursor).unwrap();
        assert_eq!(name, "日本語.txt");
    }

    #[test]
    fn test_files_info_basic() {
        let mut data = Vec::new();

        // num_files = 2
        write_variable_u64(&mut data, 2);

        // K_NAME property
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00); // not external
        write_utf16le_string(&mut names_data, "file1.txt");
        write_utf16le_string(&mut names_data, "dir/file2.txt");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![100, 200];
        let crcs = vec![Some(0x11111111), Some(0x22222222)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.num_files(), 2);
        assert_eq!(files_info.entries[0].name, "file1.txt");
        assert_eq!(files_info.entries[1].name, "dir/file2.txt");
        assert_eq!(files_info.entries[0].size, 100);
        assert_eq!(files_info.entries[1].size, 200);
        assert_eq!(files_info.entries[0].crc, Some(0x11111111));
    }

    #[test]
    fn test_files_info_with_directory() {
        let mut data = Vec::new();

        // num_files = 2 (1 dir, 1 file)
        write_variable_u64(&mut data, 2);

        // K_NAME
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00);
        write_utf16le_string(&mut names_data, "mydir");
        write_utf16le_string(&mut names_data, "mydir/file.txt");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_EMPTY_STREAM - first entry is empty (directory)
        data.push(property_id::EMPTY_STREAM);
        write_variable_u64(&mut data, 1); // 1 byte for 2 bits
        data.push(0b10000000); // first is empty, second is not

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![500]; // only 1 file has data
        let crcs = vec![Some(0xABCDEF01)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.num_files(), 2);
        assert!(files_info.entries[0].is_directory);
        assert!(!files_info.entries[0].has_stream);
        assert!(!files_info.entries[1].is_directory);
        assert!(files_info.entries[1].has_stream);
        assert_eq!(files_info.entries[1].size, 500);
    }

    #[test]
    fn test_files_info_empty() {
        let mut data = Vec::new();
        write_variable_u64(&mut data, 0);
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let files_info = FilesInfo::parse(&mut cursor, &[], &[], &limits).unwrap();

        assert_eq!(files_info.num_files(), 0);
    }

    #[test]
    fn test_archive_entry_is_file() {
        let file_entry = ArchiveEntry {
            name: "test.txt".into(),
            is_directory: false,
            has_stream: true,
            ..Default::default()
        };
        assert!(file_entry.is_file());

        let dir_entry = ArchiveEntry {
            name: "mydir".into(),
            is_directory: true,
            has_stream: false,
            ..Default::default()
        };
        assert!(!dir_entry.is_file());
    }

    #[test]
    fn test_files_info_with_anti_item() {
        let mut data = Vec::new();

        // num_files = 3 (1 normal file, 1 anti-item, 1 directory)
        write_variable_u64(&mut data, 3);

        // K_NAME
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00);
        write_utf16le_string(&mut names_data, "keep.txt");
        write_utf16le_string(&mut names_data, "delete.txt");
        write_utf16le_string(&mut names_data, "mydir");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_EMPTY_STREAM - second and third entries are empty
        data.push(property_id::EMPTY_STREAM);
        write_variable_u64(&mut data, 1); // 1 byte for 3 bits
        data.push(0b01100000); // first has stream, second and third are empty

        // K_EMPTY_FILE - of the 2 empty ones, first is empty file (not dir)
        data.push(property_id::EMPTY_FILE);
        write_variable_u64(&mut data, 1); // 1 byte for 2 bits
        data.push(0b10000000); // first empty entry is empty file, second is dir

        // K_ANTI - of the 2 empty ones, first is anti-item
        data.push(property_id::ANTI);
        write_variable_u64(&mut data, 1); // 1 byte for 2 bits
        data.push(0b10000000); // first empty entry is anti-item

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![100]; // only 1 file has data
        let crcs = vec![Some(0x12345678)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.num_files(), 3);

        // First entry: normal file with data
        assert!(!files_info.entries[0].is_directory);
        assert!(files_info.entries[0].has_stream);
        assert!(!files_info.entries[0].is_anti);
        assert_eq!(files_info.entries[0].size, 100);

        // Second entry: anti-item (marked for deletion)
        assert!(!files_info.entries[1].is_directory);
        assert!(!files_info.entries[1].has_stream);
        assert!(files_info.entries[1].is_anti);

        // Third entry: directory
        assert!(files_info.entries[2].is_directory);
        assert!(!files_info.entries[2].has_stream);
        assert!(!files_info.entries[2].is_anti);

        // Test helper methods
        assert_eq!(files_info.num_anti_items(), 1);
        let anti_items: Vec<_> = files_info.anti_items().collect();
        assert_eq!(anti_items.len(), 1);
        assert_eq!(anti_items[0].name, "delete.txt");
    }

    #[test]
    fn test_files_info_with_comment() {
        let mut data = Vec::new();

        // num_files = 1
        write_variable_u64(&mut data, 1);

        // K_NAME property
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00); // not external
        write_utf16le_string(&mut names_data, "file.txt");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_COMMENT property
        data.push(property_id::COMMENT);
        let mut comment_data = Vec::new();
        comment_data.push(0x00); // not external
        write_utf16le_string(&mut comment_data, "This is a test archive");
        write_variable_u64(&mut data, comment_data.len() as u64);
        data.extend_from_slice(&comment_data);

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![100];
        let crcs = vec![Some(0x12345678)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.num_files(), 1);
        assert_eq!(files_info.entries[0].name, "file.txt");
        assert_eq!(files_info.comment(), Some("This is a test archive"));
    }

    #[test]
    fn test_files_info_no_comment() {
        let mut data = Vec::new();

        // num_files = 1
        write_variable_u64(&mut data, 1);

        // K_NAME property
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00); // not external
        write_utf16le_string(&mut names_data, "file.txt");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![100];
        let crcs = vec![Some(0x12345678)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.num_files(), 1);
        assert!(files_info.comment().is_none());
    }

    #[test]
    fn test_files_info_unicode_comment() {
        let mut data = Vec::new();

        // num_files = 1
        write_variable_u64(&mut data, 1);

        // K_NAME property
        data.push(property_id::NAME);
        let mut names_data = Vec::new();
        names_data.push(0x00);
        write_utf16le_string(&mut names_data, "file.txt");
        write_variable_u64(&mut data, names_data.len() as u64);
        data.extend_from_slice(&names_data);

        // K_COMMENT property with Unicode
        data.push(property_id::COMMENT);
        let mut comment_data = Vec::new();
        comment_data.push(0x00);
        write_utf16le_string(&mut comment_data, "日本語コメント 🎉");
        write_variable_u64(&mut data, comment_data.len() as u64);
        data.extend_from_slice(&comment_data);

        // K_END
        data.push(property_id::END);

        let mut cursor = Cursor::new(&data);
        let limits = ResourceLimits::default();
        let sizes = vec![100];
        let crcs = vec![Some(0x12345678)];
        let files_info = FilesInfo::parse(&mut cursor, &sizes, &crcs, &limits).unwrap();

        assert_eq!(files_info.comment(), Some("日本語コメント 🎉"));
    }
}
