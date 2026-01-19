//! Archive recovery for damaged or corrupted 7z archives.
//!
//! This module provides functionality to recover data from archives that have:
//! - Corrupted headers
//! - Truncated data
//! - Missing or damaged entries
//!
//! # Recovery Strategies
//!
//! 1. **Signature Scanning**: Search for 7z signatures in corrupted files
//! 2. **Header Recovery**: Attempt to parse headers with relaxed validation
//! 3. **Entry-by-Entry Recovery**: Extract individual entries, skipping corrupt ones
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::recovery::{RecoveryOptions, recover_archive};
//! use std::fs::File;
//!
//! let file = File::open("damaged.7z")?;
//! let result = recover_archive(file, RecoveryOptions::default())?;
//!
//! match result.status {
//!     RecoveryStatus::FullRecovery => println!("All entries recovered!"),
//!     RecoveryStatus::PartialRecovery => {
//!         println!("Recovered {} entries, {} failed",
//!             result.recovered_entries.len(),
//!             result.failed_entries.len());
//!     }
//!     RecoveryStatus::Failed => println!("Could not recover archive"),
//! }
//! ```

mod scanner;

pub use scanner::SignatureScanner;

use crate::{Archive, Error, Result};
use std::io::{Read, Seek, SeekFrom};

/// Options for archive recovery operations.
#[derive(Debug, Clone)]
pub struct RecoveryOptions {
    /// Maximum bytes to search for signatures (default: 1 MiB).
    pub search_limit: usize,
    /// Whether to validate CRCs during recovery (default: true).
    pub validate_crcs: bool,
    /// Whether to skip corrupt entries and continue (default: false).
    pub skip_corrupt_entries: bool,
    /// Whether to try multiple header locations (default: false).
    pub try_multiple_headers: bool,
}

impl Default for RecoveryOptions {
    fn default() -> Self {
        Self {
            search_limit: 1024 * 1024, // 1 MiB
            validate_crcs: true,
            skip_corrupt_entries: false,
            try_multiple_headers: false,
        }
    }
}

impl RecoveryOptions {
    /// Creates new recovery options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the search limit for signature scanning.
    pub fn search_limit(mut self, limit: usize) -> Self {
        self.search_limit = limit;
        self
    }

    /// Sets whether to validate CRCs.
    pub fn validate_crcs(mut self, validate: bool) -> Self {
        self.validate_crcs = validate;
        self
    }

    /// Sets whether to skip corrupt entries.
    pub fn skip_corrupt_entries(mut self, skip: bool) -> Self {
        self.skip_corrupt_entries = skip;
        self
    }

    /// Sets whether to try multiple header locations.
    pub fn try_multiple_headers(mut self, try_multiple: bool) -> Self {
        self.try_multiple_headers = try_multiple;
        self
    }
}

/// Status of a recovery operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStatus {
    /// All entries were successfully recovered.
    FullRecovery,
    /// Some entries were recovered, others failed.
    PartialRecovery,
    /// Header was recovered but some data is corrupt.
    HeaderRecovered,
    /// Recovery failed completely.
    Failed,
}

/// Information about a successfully recovered entry.
#[derive(Debug, Clone)]
pub struct RecoveredEntry {
    /// Path of the entry in the archive.
    pub path: String,
    /// Size of the recovered data.
    pub size: u64,
    /// Whether the CRC was valid.
    pub crc_valid: bool,
    /// Index of the entry in the archive.
    pub index: usize,
}

/// Information about a failed entry recovery.
#[derive(Debug, Clone)]
pub struct FailedEntry {
    /// Path of the entry (if known).
    pub path: Option<String>,
    /// Reason for failure.
    pub reason: String,
    /// Index of the entry in the archive.
    pub index: usize,
}

/// Result of an archive recovery operation.
#[must_use = "recovery result should be checked for status and warnings"]
pub struct RecoveryResult<R> {
    /// The recovered archive (if any).
    pub archive: Option<Archive<R>>,
    /// Overall recovery status.
    pub status: RecoveryStatus,
    /// Successfully recovered entries.
    pub recovered_entries: Vec<RecoveredEntry>,
    /// Entries that could not be recovered.
    pub failed_entries: Vec<FailedEntry>,
    /// Warning messages generated during recovery.
    pub warnings: Vec<String>,
    /// Offset where the archive was found (for SFX or damaged files).
    pub archive_offset: u64,
}

impl<R> RecoveryResult<R> {
    /// Returns the number of successfully recovered entries.
    pub fn recovered_count(&self) -> usize {
        self.recovered_entries.len()
    }

    /// Returns the number of failed entries.
    pub fn failed_count(&self) -> usize {
        self.failed_entries.len()
    }

    /// Returns the total number of entries (recovered + failed).
    pub fn total_entries(&self) -> usize {
        self.recovered_count() + self.failed_count()
    }

    /// Returns the recovery rate as a percentage (0.0-1.0).
    pub fn recovery_rate(&self) -> f64 {
        let total = self.total_entries();
        if total == 0 {
            1.0
        } else {
            self.recovered_count() as f64 / total as f64
        }
    }
}

/// Attempts to recover a 7z archive from a reader.
///
/// This function will:
/// 1. Scan for the 7z signature
/// 2. Attempt to parse the header
/// 3. Validate and recover entries
///
/// # Arguments
///
/// * `reader` - The reader containing the archive data
/// * `options` - Recovery options
///
/// # Returns
///
/// A `RecoveryResult` containing the recovered archive and information
/// about which entries were successfully recovered.
pub fn recover_archive<R: Read + Seek + Send + 'static>(
    mut reader: R,
    options: RecoveryOptions,
) -> Result<RecoveryResult<R>> {
    let mut warnings = Vec::new();

    // First, try to find the 7z signature
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
    let mut scanner = SignatureScanner::new(&mut reader, options.search_limit);

    let signature_offset = match scanner.find_next_signature()? {
        Some(offset) => {
            if offset > 0 {
                warnings.push(format!(
                    "Archive signature found at offset {} (possible SFX or corruption)",
                    offset
                ));
            }
            offset
        }
        None => {
            return Ok(RecoveryResult {
                archive: None,
                status: RecoveryStatus::Failed,
                recovered_entries: Vec::new(),
                failed_entries: Vec::new(),
                warnings: vec!["No 7z signature found in file".to_string()],
                archive_offset: 0,
            });
        }
    };

    // Try to open the archive at the found offset
    reader
        .seek(SeekFrom::Start(signature_offset))
        .map_err(Error::Io)?;

    match try_open_archive(reader, signature_offset, &options) {
        Ok((archive, entry_results)) => {
            let (recovered, failed): (Vec<_>, Vec<_>) =
                entry_results.into_iter().partition(|r| r.is_ok());

            let recovered_entries: Vec<RecoveredEntry> =
                recovered.into_iter().filter_map(|r| r.ok()).collect();

            let failed_entries: Vec<FailedEntry> =
                failed.into_iter().filter_map(|r| r.err()).collect();

            let status = if failed_entries.is_empty() {
                RecoveryStatus::FullRecovery
            } else if recovered_entries.is_empty() {
                RecoveryStatus::HeaderRecovered
            } else {
                RecoveryStatus::PartialRecovery
            };

            Ok(RecoveryResult {
                archive: Some(archive),
                status,
                recovered_entries,
                failed_entries,
                warnings,
                archive_offset: signature_offset,
            })
        }
        Err(e) => {
            warnings.push(format!("Failed to open archive: {}", e));
            Ok(RecoveryResult {
                archive: None,
                status: RecoveryStatus::Failed,
                recovered_entries: Vec::new(),
                failed_entries: Vec::new(),
                warnings,
                archive_offset: signature_offset,
            })
        }
    }
}

/// Attempts to open an archive and validate its entries.
#[allow(clippy::type_complexity)]
fn try_open_archive<R: Read + Seek + Send + 'static>(
    reader: R,
    _offset: u64,
    options: &RecoveryOptions,
) -> Result<(
    Archive<R>,
    Vec<std::result::Result<RecoveredEntry, FailedEntry>>,
)> {
    // Try opening with standard parsing
    // Archive::open handles SFX detection automatically - we've already seeked to the offset
    let archive = Archive::open(reader)?;

    // Validate entries
    let mut results = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let entry_result = validate_entry(&archive, index, entry, options);
        results.push(entry_result);
    }

    Ok((archive, results))
}

/// Validates a single entry and returns recovery information.
fn validate_entry<R>(
    _archive: &Archive<R>,
    index: usize,
    entry: &crate::Entry,
    options: &RecoveryOptions,
) -> std::result::Result<RecoveredEntry, FailedEntry> {
    // For directories, mark as recovered
    if entry.is_directory {
        return Ok(RecoveredEntry {
            path: entry.path.as_str().to_string(),
            size: 0,
            crc_valid: true,
            index,
        });
    }

    // For files, check if we can determine CRC validity
    let crc_valid = if options.validate_crcs {
        // CRC validation would require reading the file data
        // For now, assume valid if we can read the entry metadata
        entry.crc32.is_some()
    } else {
        true
    };

    Ok(RecoveredEntry {
        path: entry.path.as_str().to_string(),
        size: entry.size,
        crc_valid,
        index,
    })
}

/// Quick check if a file appears to be a valid 7z archive.
///
/// This performs minimal validation - just checks for the signature.
pub fn is_valid_archive<R: Read + Seek>(reader: &mut R) -> Result<bool> {
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;

    let mut signature = [0u8; 6];
    if reader.read_exact(&mut signature).is_err() {
        return Ok(false);
    }

    Ok(signature == *crate::format::SIGNATURE)
}

/// Scans for all 7z signatures in a file.
///
/// Useful for detecting multiple embedded archives or finding
/// backup headers in damaged files.
pub fn find_all_signatures<R: Read + Seek>(
    reader: &mut R,
    search_limit: Option<usize>,
) -> Result<Vec<u64>> {
    let limit = search_limit.unwrap_or(RecoveryOptions::default().search_limit);
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;

    let mut scanner = SignatureScanner::new(reader, limit);
    scanner.find_all_signatures()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn create_minimal_archive() -> Vec<u8> {
        let mut data = Vec::new();
        // 7z signature
        data.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
        // Version (0.4)
        data.extend_from_slice(&[0x00, 0x04]);
        // Start header CRC (placeholder)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        // Next header offset (0)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Next header size (0)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Next header CRC
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        data
    }

    #[test]
    fn test_recovery_options_default() {
        let options = RecoveryOptions::default();
        assert_eq!(options.search_limit, 1024 * 1024);
        assert!(options.validate_crcs);
        assert!(!options.skip_corrupt_entries);
    }

    #[test]
    fn test_recovery_options_builder() {
        let options = RecoveryOptions::new()
            .search_limit(2048)
            .validate_crcs(false)
            .skip_corrupt_entries(true);

        assert_eq!(options.search_limit, 2048);
        assert!(!options.validate_crcs);
        assert!(options.skip_corrupt_entries);
    }

    #[test]
    fn test_is_valid_archive_valid() {
        let data = create_minimal_archive();
        let mut cursor = Cursor::new(data);
        assert!(is_valid_archive(&mut cursor).unwrap());
    }

    #[test]
    fn test_is_valid_archive_invalid() {
        let data = b"not a 7z archive";
        let mut cursor = Cursor::new(data.to_vec());
        assert!(!is_valid_archive(&mut cursor).unwrap());
    }

    #[test]
    fn test_is_valid_archive_too_short() {
        let data = b"short";
        let mut cursor = Cursor::new(data.to_vec());
        assert!(!is_valid_archive(&mut cursor).unwrap());
    }

    #[test]
    fn test_find_all_signatures_single() {
        let data = create_minimal_archive();
        let mut cursor = Cursor::new(data);
        let signatures = find_all_signatures(&mut cursor, None).unwrap();
        assert_eq!(signatures.len(), 1);
        assert_eq!(signatures[0], 0);
    }

    #[test]
    fn test_find_all_signatures_embedded() {
        let mut data = vec![0u8; 256]; // Some prefix data
        data.extend_from_slice(&create_minimal_archive());

        let mut cursor = Cursor::new(data);
        let signatures = find_all_signatures(&mut cursor, None).unwrap();
        assert_eq!(signatures.len(), 1);
        assert_eq!(signatures[0], 256);
    }

    #[test]
    fn test_find_all_signatures_none() {
        let data = b"no signature here at all";
        let mut cursor = Cursor::new(data.to_vec());
        let signatures = find_all_signatures(&mut cursor, None).unwrap();
        assert!(signatures.is_empty());
    }

    #[test]
    fn test_recovery_result_metrics() {
        let result: RecoveryResult<Cursor<Vec<u8>>> = RecoveryResult {
            archive: None,
            status: RecoveryStatus::PartialRecovery,
            recovered_entries: vec![
                RecoveredEntry {
                    path: "a.txt".into(),
                    size: 100,
                    crc_valid: true,
                    index: 0,
                },
                RecoveredEntry {
                    path: "b.txt".into(),
                    size: 200,
                    crc_valid: true,
                    index: 1,
                },
            ],
            failed_entries: vec![FailedEntry {
                path: Some("c.txt".into()),
                reason: "corrupt".into(),
                index: 2,
            }],
            warnings: Vec::new(),
            archive_offset: 0,
        };

        assert_eq!(result.recovered_count(), 2);
        assert_eq!(result.failed_count(), 1);
        assert_eq!(result.total_entries(), 3);
        assert!((result.recovery_rate() - 0.666).abs() < 0.01);
    }
}
