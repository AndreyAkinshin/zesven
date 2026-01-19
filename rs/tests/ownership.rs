//! Integration tests for Unix ownership metadata preservation.
//!
//! These tests verify that Unix ownership information (UID, GID, user/group names)
//! can be captured, serialized, and restored correctly.

#[cfg(unix)]
use tempfile::TempDir;
use zesven::ownership::{
    UnixOwnership, decode_unix_mode, deserialize_ownership, encode_ownership_attributes,
    serialize_ownership,
};

/// Tests ownership creation from IDs.
#[test]
fn test_ownership_from_ids() {
    let ownership = UnixOwnership::from_ids(1000, 1000);

    assert!(ownership.is_present());
    assert_eq!(ownership.uid, Some(1000));
    assert_eq!(ownership.gid, Some(1000));
    assert_eq!(ownership.user_name, None);
    assert_eq!(ownership.group_name, None);
}

/// Tests ownership creation from names.
#[test]
fn test_ownership_from_names() {
    let ownership = UnixOwnership::from_names("testuser", "testgroup");

    assert!(ownership.is_present());
    assert_eq!(ownership.uid, None);
    assert_eq!(ownership.gid, None);
    assert_eq!(ownership.user_name, Some("testuser".to_string()));
    assert_eq!(ownership.group_name, Some("testgroup".to_string()));
}

/// Tests full serialization and deserialization roundtrip.
#[test]
fn test_ownership_serialization_deserialization() {
    let ownership = UnixOwnership {
        uid: Some(1000),
        gid: Some(1001),
        user_name: Some("alice".to_string()),
        group_name: Some("developers".to_string()),
    };

    let serialized = serialize_ownership(&ownership);
    let deserialized = deserialize_ownership(&serialized).expect("Failed to deserialize");

    assert_eq!(ownership, deserialized);
}

/// Tests serialization of partial ownership data.
#[test]
fn test_ownership_serialization_partial() {
    // Only UID
    let ownership1 = UnixOwnership {
        uid: Some(500),
        gid: None,
        user_name: None,
        group_name: None,
    };
    let data1 = serialize_ownership(&ownership1);
    let restored1 = deserialize_ownership(&data1).unwrap();
    assert_eq!(ownership1, restored1);

    // Only names
    let ownership2 = UnixOwnership {
        uid: None,
        gid: None,
        user_name: Some("root".to_string()),
        group_name: Some("wheel".to_string()),
    };
    let data2 = serialize_ownership(&ownership2);
    let restored2 = deserialize_ownership(&data2).unwrap();
    assert_eq!(ownership2, restored2);

    // UID and user_name only
    let ownership3 = UnixOwnership {
        uid: Some(0),
        gid: None,
        user_name: Some("root".to_string()),
        group_name: None,
    };
    let data3 = serialize_ownership(&ownership3);
    let restored3 = deserialize_ownership(&data3).unwrap();
    assert_eq!(ownership3, restored3);
}

/// Tests empty ownership serialization.
#[test]
fn test_ownership_serialization_empty() {
    let empty = UnixOwnership::new();
    assert!(!empty.is_present());

    let data = serialize_ownership(&empty);
    let restored = deserialize_ownership(&data).unwrap();

    assert_eq!(empty, restored);
    assert!(!restored.is_present());
}

/// Tests Unix mode encoding and decoding.
#[test]
fn test_unix_mode_encoding_decoding() {
    let ownership = UnixOwnership::from_ids(0, 0);

    // Test various Unix modes
    let test_modes = [0o644, 0o755, 0o700, 0o600, 0o777, 0o000, 0o4755, 0o2755];

    for &mode in &test_modes {
        let attrs = encode_ownership_attributes(&ownership, Some(mode));
        let decoded = decode_unix_mode(attrs);
        // Mode bits 16-30 only, so mask to 15 bits
        assert_eq!(
            decoded,
            Some(mode & 0x7FFF),
            "Mode {:#o} should roundtrip",
            mode
        );
    }
}

/// Tests that attributes without Unix marker return None.
#[test]
fn test_decode_no_unix_mode() {
    // Regular Windows attribute (archive flag)
    let windows_attrs = 0x20;
    assert_eq!(decode_unix_mode(windows_attrs), None);

    // Zero attributes
    assert_eq!(decode_unix_mode(0), None);

    // Random Windows attributes without Unix marker
    assert_eq!(decode_unix_mode(0x123), None);
}

/// Tests ownership from real file (Unix only).
#[cfg(unix)]
#[test]
fn test_ownership_from_real_file() {
    use std::fs::File;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test_file.txt");

    // Create a file
    File::create(&file_path).expect("Failed to create file");

    // Get ownership
    let ownership = UnixOwnership::from_path(&file_path)
        .expect("Failed to get metadata")
        .expect("Should return Some on Unix");

    // On Unix, we should get UID and GID
    assert!(ownership.uid.is_some());
    assert!(ownership.gid.is_some());
    assert!(ownership.is_present());

    // The UID should be the current user's UID
    // (we can't easily check the exact value, but it should be set)
}

/// Tests ownership roundtrip in archive context.
#[test]
fn test_ownership_roundtrip_serialization() {
    // Simulate storing ownership in archive and retrieving it
    let original = UnixOwnership {
        uid: Some(1000),
        gid: Some(100),
        user_name: Some("developer".to_string()),
        group_name: Some("staff".to_string()),
    };

    // Serialize (as would happen when writing to archive)
    let archived_data = serialize_ownership(&original);

    // Deserialize (as would happen when reading from archive)
    let restored = deserialize_ownership(&archived_data).expect("Failed to deserialize");

    assert_eq!(original.uid, restored.uid);
    assert_eq!(original.gid, restored.gid);
    assert_eq!(original.user_name, restored.user_name);
    assert_eq!(original.group_name, restored.group_name);
}

/// Tests handling of unicode user/group names.
#[test]
fn test_ownership_unicode_names() {
    let ownership = UnixOwnership {
        uid: Some(1000),
        gid: Some(1000),
        user_name: Some("用户名".to_string()),    // Chinese
        group_name: Some("グループ".to_string()), // Japanese
    };

    let data = serialize_ownership(&ownership);
    let restored = deserialize_ownership(&data).unwrap();

    assert_eq!(ownership.user_name, restored.user_name);
    assert_eq!(ownership.group_name, restored.group_name);
}

/// Tests handling of long user/group names.
#[test]
fn test_ownership_long_names() {
    let long_name = "a".repeat(1000);
    let ownership = UnixOwnership {
        uid: None,
        gid: None,
        user_name: Some(long_name.clone()),
        group_name: Some(long_name.clone()),
    };

    let data = serialize_ownership(&ownership);
    let restored = deserialize_ownership(&data).unwrap();

    assert_eq!(ownership.user_name, restored.user_name);
    assert_eq!(ownership.group_name, restored.group_name);
}

/// Tests deserialization of truncated data.
#[test]
fn test_ownership_deserialize_invalid() {
    // Empty data
    assert!(deserialize_ownership(&[]).is_none());

    // Truncated (claims to have UID but doesn't)
    assert!(deserialize_ownership(&[0x01]).is_none()); // flags say has UID, but no data

    // Truncated UID
    assert!(deserialize_ownership(&[0x01, 0x00, 0x00]).is_none()); // Only 3 bytes of UID
}

/// Tests is_present with various combinations.
#[test]
fn test_ownership_is_present() {
    assert!(!UnixOwnership::new().is_present());
    assert!(
        !UnixOwnership {
            uid: None,
            gid: None,
            user_name: None,
            group_name: None
        }
        .is_present()
    );

    assert!(
        UnixOwnership {
            uid: Some(0),
            gid: None,
            user_name: None,
            group_name: None
        }
        .is_present()
    );
    assert!(
        UnixOwnership {
            uid: None,
            gid: Some(0),
            user_name: None,
            group_name: None
        }
        .is_present()
    );
    assert!(
        UnixOwnership {
            uid: None,
            gid: None,
            user_name: Some("".to_string()),
            group_name: None
        }
        .is_present()
    );
    assert!(
        UnixOwnership {
            uid: None,
            gid: None,
            user_name: None,
            group_name: Some("".to_string())
        }
        .is_present()
    );
}
