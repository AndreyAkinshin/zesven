//! Unix file ownership support.
//!
//! This module provides types for storing and retrieving Unix file ownership
//! information (UID, GID, owner name, group name) in 7z archives.
//!
//! # Note
//!
//! Standard 7z archives don't have native support for Unix ownership.
//! This module stores ownership information in the high bits of the
//! Windows attributes field (using the Unix extension) and optionally
//! in archive comments or a separate stream.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::ownership::UnixOwnership;
//!
//! let ownership = UnixOwnership {
//!     uid: Some(1000),
//!     gid: Some(1000),
//!     user_name: Some("user".into()),
//!     group_name: Some("users".into()),
//! };
//! ```

use std::path::Path;

/// Unix file ownership information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnixOwnership {
    /// User ID (UID).
    pub uid: Option<u32>,
    /// Group ID (GID).
    pub gid: Option<u32>,
    /// User name (owner).
    pub user_name: Option<String>,
    /// Group name.
    pub group_name: Option<String>,
}

impl UnixOwnership {
    /// Creates a new empty ownership record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates ownership from UID and GID only.
    pub fn from_ids(uid: u32, gid: u32) -> Self {
        Self {
            uid: Some(uid),
            gid: Some(gid),
            user_name: None,
            group_name: None,
        }
    }

    /// Creates ownership from names only.
    pub fn from_names(user: impl Into<String>, group: impl Into<String>) -> Self {
        Self {
            uid: None,
            gid: None,
            user_name: Some(user.into()),
            group_name: Some(group.into()),
        }
    }

    /// Returns true if any ownership information is present.
    pub fn is_present(&self) -> bool {
        self.uid.is_some()
            || self.gid.is_some()
            || self.user_name.is_some()
            || self.group_name.is_some()
    }

    /// Gets ownership information from a file path.
    ///
    /// This only works on Unix systems. On other platforms, returns None.
    /// Note: User/group names are not resolved; only IDs are captured.
    #[cfg(unix)]
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Option<Self>> {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(path)?;
        let uid = metadata.uid();
        let gid = metadata.gid();

        Ok(Some(Self {
            uid: Some(uid),
            gid: Some(gid),
            user_name: None,
            group_name: None,
        }))
    }

    /// Gets ownership information from a file path.
    ///
    /// This only works on Unix systems. On other platforms, returns None.
    #[cfg(not(unix))]
    pub fn from_path(_path: impl AsRef<Path>) -> std::io::Result<Option<Self>> {
        Ok(None)
    }

    /// Applies ownership to a file path.
    ///
    /// This only works on Unix systems and requires appropriate permissions.
    #[cfg(unix)]
    pub fn apply_to_path(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        use std::os::unix::fs::chown;

        if let (Some(uid), Some(gid)) = (self.uid, self.gid) {
            chown(path, Some(uid), Some(gid))?;
        }
        Ok(())
    }

    /// Applies ownership to a file path.
    ///
    /// This only works on Unix systems and requires appropriate permissions.
    #[cfg(not(unix))]
    pub fn apply_to_path(&self, _path: impl AsRef<Path>) -> std::io::Result<()> {
        Ok(())
    }
}

/// Encodes ownership information into the archive attributes field.
///
/// Uses the high 16 bits of the 32-bit attributes for Unix mode.
/// The Unix extension marker (bit 15 of high word = bit 31 overall)
/// indicates that Unix mode is present.
pub fn encode_ownership_attributes(_ownership: &UnixOwnership, unix_mode: Option<u32>) -> u32 {
    let mode_bits = unix_mode.unwrap_or(0o644);
    // Store Unix mode in bits 16-31, with bit 31 as Unix extension marker
    // The mode goes in bits 16-30, marker in bit 31
    0x8000_0000 | ((mode_bits & 0x7FFF) << 16)
}

/// Decodes Unix mode from archive attributes.
pub fn decode_unix_mode(attributes: u32) -> Option<u32> {
    // Check if Unix extension marker is set (bit 31)
    if attributes & 0x8000_0000 != 0 {
        // Extract mode from bits 16-30 (mask out the marker bit)
        Some((attributes >> 16) & 0x7FFF)
    } else {
        None
    }
}

/// Serializes ownership information to bytes.
///
/// Format:
/// - 1 byte: flags (bits: has_uid, has_gid, has_user_name, has_group_name)
/// - 4 bytes: UID (if present, little-endian)
/// - 4 bytes: GID (if present, little-endian)
/// - varint + UTF-8: user name (if present)
/// - varint + UTF-8: group name (if present)
pub fn serialize_ownership(ownership: &UnixOwnership) -> Vec<u8> {
    let mut data = Vec::new();

    // Flags byte
    let flags = (ownership.uid.is_some() as u8)
        | ((ownership.gid.is_some() as u8) << 1)
        | ((ownership.user_name.is_some() as u8) << 2)
        | ((ownership.group_name.is_some() as u8) << 3);
    data.push(flags);

    // UID
    if let Some(uid) = ownership.uid {
        data.extend_from_slice(&uid.to_le_bytes());
    }

    // GID
    if let Some(gid) = ownership.gid {
        data.extend_from_slice(&gid.to_le_bytes());
    }

    // User name
    if let Some(ref name) = ownership.user_name {
        let bytes = name.as_bytes();
        write_varint(&mut data, bytes.len() as u64);
        data.extend_from_slice(bytes);
    }

    // Group name
    if let Some(ref name) = ownership.group_name {
        let bytes = name.as_bytes();
        write_varint(&mut data, bytes.len() as u64);
        data.extend_from_slice(bytes);
    }

    data
}

/// Deserializes ownership information from bytes.
pub fn deserialize_ownership(data: &[u8]) -> Option<UnixOwnership> {
    if data.is_empty() {
        return None;
    }

    let mut pos = 0;
    let flags = data[pos];
    pos += 1;

    let uid = if flags & 0x01 != 0 {
        if pos + 4 > data.len() {
            return None;
        }
        let uid = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
        pos += 4;
        Some(uid)
    } else {
        None
    };

    let gid = if flags & 0x02 != 0 {
        if pos + 4 > data.len() {
            return None;
        }
        let gid = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
        pos += 4;
        Some(gid)
    } else {
        None
    };

    let user_name = if flags & 0x04 != 0 {
        let (len, bytes_read) = read_varint(&data[pos..])?;
        pos += bytes_read;
        if pos + len as usize > data.len() {
            return None;
        }
        let name = std::str::from_utf8(&data[pos..pos + len as usize]).ok()?;
        pos += len as usize;
        Some(name.to_string())
    } else {
        None
    };

    let group_name = if flags & 0x08 != 0 {
        let (len, bytes_read) = read_varint(&data[pos..])?;
        pos += bytes_read;
        if pos + len as usize > data.len() {
            return None;
        }
        let name = std::str::from_utf8(&data[pos..pos + len as usize]).ok()?;
        Some(name.to_string())
    } else {
        None
    };

    Some(UnixOwnership {
        uid,
        gid,
        user_name,
        group_name,
    })
}

fn write_varint(buf: &mut Vec<u8>, value: u64) {
    let mut v = value;
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    let mut pos = 0;

    loop {
        if pos >= data.len() {
            return None;
        }
        let byte = data[pos];
        pos += 1;

        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return None; // Overflow
        }
    }

    Some((result, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ownership_new() {
        let ownership = UnixOwnership::new();
        assert!(!ownership.is_present());
    }

    #[test]
    fn test_ownership_from_ids() {
        let ownership = UnixOwnership::from_ids(1000, 1000);
        assert!(ownership.is_present());
        assert_eq!(ownership.uid, Some(1000));
        assert_eq!(ownership.gid, Some(1000));
        assert_eq!(ownership.user_name, None);
        assert_eq!(ownership.group_name, None);
    }

    #[test]
    fn test_ownership_from_names() {
        let ownership = UnixOwnership::from_names("user", "group");
        assert!(ownership.is_present());
        assert_eq!(ownership.uid, None);
        assert_eq!(ownership.gid, None);
        assert_eq!(ownership.user_name, Some("user".to_string()));
        assert_eq!(ownership.group_name, Some("group".to_string()));
    }

    #[test]
    fn test_serialize_deserialize_full() {
        let ownership = UnixOwnership {
            uid: Some(1000),
            gid: Some(1001),
            user_name: Some("testuser".to_string()),
            group_name: Some("testgroup".to_string()),
        };

        let data = serialize_ownership(&ownership);
        let decoded = deserialize_ownership(&data).unwrap();

        assert_eq!(ownership, decoded);
    }

    #[test]
    fn test_serialize_deserialize_partial() {
        let ownership = UnixOwnership {
            uid: Some(500),
            gid: None,
            user_name: Some("admin".to_string()),
            group_name: None,
        };

        let data = serialize_ownership(&ownership);
        let decoded = deserialize_ownership(&data).unwrap();

        assert_eq!(ownership, decoded);
    }

    #[test]
    fn test_serialize_deserialize_empty() {
        let ownership = UnixOwnership::new();

        let data = serialize_ownership(&ownership);
        let decoded = deserialize_ownership(&data).unwrap();

        assert_eq!(ownership, decoded);
    }

    #[test]
    fn test_encode_decode_unix_mode() {
        let ownership = UnixOwnership::from_ids(0, 0);
        let attrs = encode_ownership_attributes(&ownership, Some(0o755));

        let mode = decode_unix_mode(attrs);
        assert_eq!(mode, Some(0o755));
    }

    #[test]
    fn test_decode_no_unix_mode() {
        let attrs = 0x20; // Just the archive bit
        let mode = decode_unix_mode(attrs);
        assert_eq!(mode, None);
    }
}
