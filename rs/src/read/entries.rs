//! Shared functions for building archive entries and info.
//!
//! This module provides internal functions used by both synchronous and async
//! archive readers to construct entry lists and archive metadata from parsed headers.

use crate::ArchivePath;
use crate::codec;
use crate::format::parser::ArchiveHeader;
use crate::format::streams::{Folder, UnpackInfo};

use super::Entry;
use super::info::ArchiveInfo;

/// Unix symlink file type mask (S_IFLNK = 0o120000).
const UNIX_SYMLINK_MODE: u32 = 0o120000;
/// Unix file type mask to extract file type from mode.
const UNIX_FILE_TYPE_MASK: u32 = 0o170000;
/// Windows REPARSE_POINT attribute flag.
const WINDOWS_REPARSE_POINT: u32 = 0x400;

/// Detects if an entry is a symbolic link from its attributes.
///
/// Checks both Unix mode bits (in high 16 bits of attributes) and
/// Windows REPARSE_POINT flag (in low 16 bits) for symlink detection.
fn is_symlink_from_attributes(attributes: Option<u32>) -> bool {
    let attrs = match attributes {
        Some(a) => a,
        None => return false,
    };

    // Check Unix mode in high 16 bits
    // 7z stores Unix mode as: (mode << 16) | windows_attrs
    let unix_mode = (attrs >> 16) & 0xFFFF;
    if unix_mode != 0 && (unix_mode & UNIX_FILE_TYPE_MASK) == UNIX_SYMLINK_MODE {
        return true;
    }

    // Check Windows REPARSE_POINT flag in low 16 bits
    if (attrs & WINDOWS_REPARSE_POINT) != 0 {
        return true;
    }

    false
}

#[cfg(feature = "aes")]
use super::info::EncryptionInfo;

#[cfg(feature = "aes")]
use crate::crypto::AesProperties;

/// Builds the list of entries from an archive header.
pub(crate) fn build_entries(header: &ArchiveHeader) -> Vec<Entry> {
    let files_info = match &header.files_info {
        Some(info) => info,
        None => return Vec::new(),
    };

    let substreams = header.substreams_info.as_ref();
    let unpack_info = header.unpack_info.as_ref();

    // Build mapping from entries to folders/streams
    let mut entries = Vec::with_capacity(files_info.entries.len());
    let mut stream_idx: usize = 0;
    let mut folder_idx: usize = 0;

    for (idx, archive_entry) in files_info.entries.iter().enumerate() {
        let path = match ArchivePath::new(&archive_entry.name) {
            Ok(p) => p,
            Err(_) => continue, // Skip entries with invalid paths
        };

        // Only entries with data streams get folder/stream indices.
        // Empty files (has_stream=false, is_directory=false) and directories
        // don't have associated streams and shouldn't advance the stream counter.
        let (folder_index, stream_index) = if !archive_entry.has_stream {
            (None, None)
        } else {
            // Map to folder and stream
            let fi = folder_idx;
            let si = stream_idx;

            // Advance stream index
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

        // Detect symlinks from attributes (not directories)
        let is_symlink =
            !archive_entry.is_directory && is_symlink_from_attributes(archive_entry.attributes);

        entries.push(Entry {
            path,
            is_directory: archive_entry.is_directory,
            size: archive_entry.size,
            crc32: archive_entry.crc,
            crc64: None, // Standard 7z archives only use CRC-32
            modification_time: archive_entry.mtime,
            creation_time: archive_entry.ctime,
            access_time: archive_entry.atime,
            attributes: archive_entry.attributes,
            is_encrypted: is_entry_encrypted(unpack_info, folder_index),
            is_symlink,
            is_anti: archive_entry.is_anti,
            ownership: None,
            index: idx,
            folder_index,
            stream_index,
        });
    }

    entries
}

/// Checks if an entry is encrypted based on its folder's coders.
pub(crate) fn is_entry_encrypted(
    unpack_info: Option<&UnpackInfo>,
    folder_index: Option<usize>,
) -> bool {
    let unpack_info = match unpack_info {
        Some(ui) => ui,
        None => return false,
    };

    // Check if a specific folder uses AES
    if let Some(idx) = folder_index {
        if let Some(folder) = unpack_info.folders.get(idx) {
            return folder_uses_encryption(folder);
        }
    }

    // Check if any folder uses AES
    unpack_info.folders.iter().any(folder_uses_encryption)
}

/// Checks if a folder uses AES encryption.
pub(crate) fn folder_uses_encryption(folder: &Folder) -> bool {
    folder
        .coders
        .iter()
        .any(|coder| coder.method_id.as_slice() == codec::method::AES)
}

/// Builds archive info from header and entries.
pub(crate) fn build_info(header: &ArchiveHeader, entries: &[Entry]) -> ArchiveInfo {
    let packed_size = header
        .pack_info
        .as_ref()
        .map(|pi| pi.pack_sizes.iter().sum())
        .unwrap_or(0);

    let folder_count = header
        .unpack_info
        .as_ref()
        .map(|ui| ui.folders.len())
        .unwrap_or(0);

    let is_solid = header
        .substreams_info
        .as_ref()
        .map(|ss| {
            ss.num_unpack_streams_in_folders
                .iter()
                .any(|&count| count > 1)
        })
        .unwrap_or(false);

    let compression_methods = header
        .unpack_info
        .as_ref()
        .map(|ui| {
            let mut methods = Vec::new();
            for folder in &ui.folders {
                for coder in &folder.coders {
                    if let Ok(method) = codec::CodecMethod::from_coder(coder) {
                        if !methods.contains(&method) {
                            methods.push(method);
                        }
                    }
                }
            }
            methods
        })
        .unwrap_or_default();

    let comment = header.files_info.as_ref().and_then(|fi| fi.comment.clone());

    // Extract encryption info from AES coders if present
    #[cfg(feature = "aes")]
    let encryption_info = extract_encryption_info(header);
    #[cfg(not(feature = "aes"))]
    let encryption_info = None;

    ArchiveInfo {
        entry_count: entries.len(),
        total_size: entries.iter().map(|e| e.size).sum(),
        packed_size,
        is_solid,
        has_encrypted_entries: entries.iter().any(|e| e.is_encrypted),
        has_encrypted_header: header.header_encrypted,
        compression_methods,
        folder_count,
        comment,
        encryption_info,
    }
}

/// Extracts encryption parameters from the archive header.
#[cfg(feature = "aes")]
pub(crate) fn extract_encryption_info(header: &ArchiveHeader) -> Option<EncryptionInfo> {
    let unpack_info = header.unpack_info.as_ref()?;

    for folder in &unpack_info.folders {
        for coder in &folder.coders {
            if coder.method_id.as_slice() == codec::method::AES {
                if let Some(ref props) = coder.properties {
                    if let Ok(aes_props) = AesProperties::parse(props) {
                        return Some(EncryptionInfo::new(
                            aes_props.num_cycles_power,
                            aes_props.salt.len(),
                            aes_props
                                .iv
                                .iter()
                                .rev()
                                .position(|&b| b != 0)
                                .map(|p| 16 - p)
                                .unwrap_or(0),
                        ));
                    }
                }
            }
        }
    }

    None
}
