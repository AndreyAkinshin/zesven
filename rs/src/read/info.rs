//! Archive information types.

use crate::codec::CodecMethod;

/// Encryption parameters for diagnostic purposes.
///
/// This struct exposes the AES encryption parameters used in the archive,
/// which can be useful for debugging, security auditing, or forensic analysis.
#[derive(Debug, Clone, Default)]
pub struct EncryptionInfo {
    /// Number of SHA-256 key derivation iterations (2^num_cycles_power).
    ///
    /// Higher values mean stronger key derivation but slower decryption.
    /// Typical values are 19 (524,288 iterations) for standard encryption.
    pub key_derivation_iterations: u64,
    /// The power value used for key derivation (iterations = 2^power).
    pub num_cycles_power: u8,
    /// Salt size in bytes (0-16).
    pub salt_size: usize,
    /// Initialization vector size in bytes (0-16, padded to 16).
    pub iv_size: usize,
}

impl EncryptionInfo {
    /// Creates a new EncryptionInfo from AES parameters.
    pub fn new(num_cycles_power: u8, salt_size: usize, iv_size: usize) -> Self {
        Self {
            key_derivation_iterations: 1u64 << num_cycles_power,
            num_cycles_power,
            salt_size,
            iv_size,
        }
    }
}

/// Information about an opened archive.
#[derive(Debug, Clone, Default)]
pub struct ArchiveInfo {
    /// Total number of entries (files and directories).
    pub entry_count: usize,
    /// Total uncompressed size of all files.
    pub total_size: u64,
    /// Total compressed size of packed data.
    pub packed_size: u64,
    /// Whether the archive uses solid compression.
    pub is_solid: bool,
    /// Whether any entries are encrypted.
    pub has_encrypted_entries: bool,
    /// Whether the header itself is encrypted.
    pub has_encrypted_header: bool,
    /// Compression methods used in the archive.
    pub compression_methods: Vec<CodecMethod>,
    /// Number of folders (compression blocks).
    pub folder_count: usize,
    /// Archive comment (if any).
    pub comment: Option<String>,
    /// Encryption parameters (if archive uses encryption).
    ///
    /// This is populated when the archive contains encrypted entries
    /// and provides details about the encryption configuration.
    pub encryption_info: Option<EncryptionInfo>,
}

impl ArchiveInfo {
    /// Returns the compression ratio (packed / unpacked).
    pub fn compression_ratio(&self) -> f64 {
        if self.total_size == 0 {
            1.0
        } else {
            self.packed_size as f64 / self.total_size as f64
        }
    }

    /// Returns the space savings percentage.
    pub fn space_savings(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            1.0 - self.compression_ratio()
        }
    }

    /// Returns the archive comment, if any.
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }
}

/// Result of testing an archive for integrity.
///
/// This struct contains information about how many entries were tested,
/// how many passed or failed, and details about any failures.
#[must_use = "test results should be checked to verify archive integrity"]
#[derive(Debug, Clone, Default)]
pub struct TestResult {
    /// Number of entries tested.
    pub entries_tested: usize,
    /// Number of entries that passed.
    pub entries_passed: usize,
    /// Number of entries that failed.
    pub entries_failed: usize,
    /// Detailed failures (entry path and error message).
    pub failures: Vec<(String, String)>,
}

impl TestResult {
    /// Returns true if all entries passed.
    pub fn is_ok(&self) -> bool {
        self.entries_failed == 0
    }

    /// Returns true if any entries failed.
    pub fn is_err(&self) -> bool {
        self.entries_failed > 0
    }
}

/// Result of extracting entries from an archive.
///
/// This struct contains information about how many entries were extracted,
/// skipped, or failed, along with details about any failures.
#[must_use = "extraction results should be checked for warnings or partial failures"]
#[derive(Debug, Clone, Default)]
pub struct ExtractResult {
    /// Number of entries extracted.
    pub entries_extracted: usize,
    /// Number of entries skipped.
    pub entries_skipped: usize,
    /// Number of entries that failed.
    pub entries_failed: usize,
    /// Total bytes extracted.
    pub bytes_extracted: u64,
    /// Detailed failures (entry path and error message).
    pub failures: Vec<(String, String)>,
}

impl ExtractResult {
    /// Returns true if all selected entries were extracted successfully.
    pub fn is_ok(&self) -> bool {
        self.entries_failed == 0
    }

    /// Returns true if any entries failed.
    pub fn is_err(&self) -> bool {
        self.entries_failed > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_info_compression_ratio() {
        let info = ArchiveInfo {
            total_size: 1000,
            packed_size: 500,
            ..Default::default()
        };
        assert!((info.compression_ratio() - 0.5).abs() < 0.001);
        assert!((info.space_savings() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_archive_info_empty() {
        let info = ArchiveInfo::default();
        assert!((info.compression_ratio() - 1.0).abs() < 0.001);
        assert!((info.space_savings() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_test_result() {
        let mut result = TestResult::default();
        assert!(result.is_ok());
        assert!(!result.is_err());

        result.entries_failed = 1;
        assert!(!result.is_ok());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_result() {
        let mut result = ExtractResult::default();
        assert!(result.is_ok());

        result.entries_failed = 1;
        assert!(!result.is_ok());
        assert!(result.is_err());
    }

    #[test]
    fn test_archive_info_comment() {
        // Test with no comment
        let info = ArchiveInfo::default();
        assert!(info.comment().is_none());

        // Test with comment
        let info = ArchiveInfo {
            comment: Some("Test comment".to_string()),
            ..Default::default()
        };
        assert_eq!(info.comment(), Some("Test comment"));
    }

    #[test]
    fn test_encryption_info() {
        // Test EncryptionInfo construction
        let info = EncryptionInfo::new(19, 8, 16);
        assert_eq!(info.num_cycles_power, 19);
        assert_eq!(info.key_derivation_iterations, 1u64 << 19); // 524288
        assert_eq!(info.salt_size, 8);
        assert_eq!(info.iv_size, 16);

        // Test with default (power of 0 = 1 iteration)
        let info = EncryptionInfo::new(0, 0, 0);
        assert_eq!(info.key_derivation_iterations, 1);
        assert_eq!(info.salt_size, 0);
        assert_eq!(info.iv_size, 0);
    }

    #[test]
    fn test_archive_info_with_encryption() {
        let encryption_info = EncryptionInfo::new(19, 8, 16);
        let info = ArchiveInfo {
            has_encrypted_entries: true,
            encryption_info: Some(encryption_info),
            ..Default::default()
        };

        assert!(info.has_encrypted_entries);
        assert!(info.encryption_info.is_some());
        let enc = info.encryption_info.unwrap();
        assert_eq!(enc.key_derivation_iterations, 524288);
    }
}
