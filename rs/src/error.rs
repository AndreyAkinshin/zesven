//! Error types for 7z archive operations.
//!
//! This module provides the [`Error`] enum which represents all possible
//! failure modes when working with 7z archives, along with a convenient
//! [`Result<T>`] type alias.
//!
//! # Error Handling
//!
//! All fallible operations in this crate return `Result<T, Error>`. You can
//! handle errors using pattern matching or the `?` operator:
//!
//! ## Using the `?` Operator
//!
//! ```rust,no_run
//! use zesven::{Archive, ExtractOptions, Result};
//!
//! fn extract_archive(path: &str, dest: &str) -> Result<()> {
//!     let mut archive = Archive::open_path(path)?;
//!     archive.extract(dest, (), &ExtractOptions::default())?;
//!     Ok(())
//! }
//! ```
//!
//! ## Exhaustive Error Matching
//!
//! For fine-grained error handling, match on specific error variants:
//!
//! ```rust,no_run
//! use zesven::{Archive, Error};
//!
//! fn open_with_recovery(path: &str) -> zesven::Result<Archive<std::io::BufReader<std::fs::File>>> {
//!     match Archive::open_path(path) {
//!         Ok(archive) => Ok(archive),
//!
//!         // File system errors
//!         Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
//!             eprintln!("Archive not found: {}", path);
//!             Err(Error::Io(e))
//!         }
//!         Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
//!             eprintln!("Permission denied: {}", path);
//!             Err(Error::Io(e))
//!         }
//!
//!         // Format errors
//!         Err(Error::InvalidFormat(msg)) => {
//!             eprintln!("Not a valid 7z archive: {}", msg);
//!             Err(Error::InvalidFormat(msg))
//!         }
//!         Err(Error::CorruptHeader { offset, reason }) => {
//!             eprintln!("Archive corrupted at byte {:#x}: {}", offset, reason);
//!             Err(Error::CorruptHeader { offset, reason })
//!         }
//!
//!         // Encryption errors (requires password)
//!         Err(e @ Error::WrongPassword { .. }) => {
//!             eprintln!("This archive is encrypted. Please provide the correct password.");
//!             Err(e)
//!         }
//!
//!         // Unsupported features
//!         Err(Error::UnsupportedMethod { method_id }) => {
//!             eprintln!("Archive uses unsupported compression method: {:#x}", method_id);
//!             Err(Error::UnsupportedMethod { method_id })
//!         }
//!
//!         // Other errors
//!         Err(e) => Err(e),
//!     }
//! }
//! ```
//!
//! ## User-Friendly Error Messages
//!
//! The [`Error`] type implements [`std::fmt::Display`] with clear messages:
//!
//! ```rust
//! use zesven::Error;
//!
//! fn print_user_message(error: &Error) {
//!     match error {
//!         Error::Io(e) => println!("File error: {}", e),
//!         Error::InvalidFormat(_) => println!("The file is not a valid 7z archive."),
//!         Error::WrongPassword { .. } => println!("Incorrect password. Please try again."),
//!         Error::UnsupportedMethod { .. } => {
//!             println!("This archive uses a compression method not supported by this version.");
//!         }
//!         Error::CrcMismatch { .. } => {
//!             println!("Archive integrity check failed. The file may be corrupted.");
//!         }
//!         Error::PathTraversal { .. } => {
//!             println!("Security: Archive contains unsafe file paths.");
//!         }
//!         Error::ResourceLimitExceeded(_) => {
//!             println!("Archive exceeds configured resource limits.");
//!         }
//!         _ => println!("Error: {}", error),
//!     }
//! }
//! ```

use std::io;

/// How a wrong password was detected.
///
/// This enum indicates the method used to detect that an incorrect password
/// was provided for an encrypted archive. Use [`Error::PasswordRequired`] instead
/// when no password was provided at all.
///
/// [`Error::PasswordRequired`]: Error::PasswordRequired
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PasswordDetectionMethod {
    /// Detected early by checking if decrypted data looks like valid compression headers.
    ///
    /// This is the fastest detection method as it only requires decrypting and
    /// examining the first block of data. It works by checking if the decrypted
    /// bytes form valid LZMA/LZMA2/Deflate/etc. header patterns.
    EarlyHeaderValidation,

    /// Detected after decompression via CRC-32 mismatch.
    ///
    /// This method requires full decompression of the entry before the wrong
    /// password can be detected. It's slower but catches cases where early
    /// detection isn't possible (e.g., encrypted-only data without compression).
    CrcMismatch,

    /// Detected due to decompression failure.
    ///
    /// The decrypted data caused the decompressor to fail, which typically
    /// indicates garbage data from a wrong password.
    DecompressionFailure,
}

impl std::fmt::Display for PasswordDetectionMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EarlyHeaderValidation => write!(f, "early header validation"),
            Self::CrcMismatch => write!(f, "CRC mismatch after decompression"),
            Self::DecompressionFailure => write!(f, "decompression failure"),
        }
    }
}

/// Helper struct for formatting WrongPassword error messages.
struct WrongPasswordDisplay<'a> {
    entry_index: Option<usize>,
    entry_name: Option<&'a str>,
}

impl std::fmt::Display for WrongPasswordDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Wrong password")?;
        match (self.entry_index, self.entry_name) {
            (Some(idx), Some(name)) => write!(f, " for entry {} ({})", idx, name),
            (Some(idx), None) => write!(f, " for entry {}", idx),
            (None, Some(name)) => write!(f, " for entry '{}'", name),
            (None, None) => Ok(()),
        }
    }
}

/// Helper struct for formatting CrcMismatch error messages.
struct CrcMismatchDisplay<'a> {
    entry_index: usize,
    entry_name: Option<&'a str>,
    expected: u32,
    actual: u32,
}

impl std::fmt::Display for CrcMismatchDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CRC mismatch for entry {}", self.entry_index)?;
        if let Some(name) = self.entry_name {
            write!(f, " ({})", name)?;
        }
        write!(f, ": expected {:#x}, got {:#x}", self.expected, self.actual)
    }
}

/// The main error type for 7z archive operations.
///
/// This enum represents all possible errors that can occur when reading,
/// writing, or extracting 7z archives. Each variant includes relevant
/// context to help diagnose the issue.
///
/// # Error Categories
///
/// Errors fall into several categories:
///
/// | Category | Variants | Typical Cause |
/// |----------|----------|---------------|
/// | I/O | [`Io`][Self::Io] | File system operations |
/// | Format | [`InvalidFormat`][Self::InvalidFormat], [`CorruptHeader`][Self::CorruptHeader] | Invalid archive data |
/// | Security | [`WrongPassword`][Self::WrongPassword], [`PathTraversal`][Self::PathTraversal] | Security checks |
/// | Compatibility | [`UnsupportedMethod`][Self::UnsupportedMethod], [`UnsupportedFeature`][Self::UnsupportedFeature] | Missing features |
/// | Integrity | [`CrcMismatch`][Self::CrcMismatch] | Data corruption |
/// | Resources | [`ResourceLimitExceeded`][Self::ResourceLimitExceeded] | Safety limits |
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An I/O error occurred during file operations.
    ///
    /// This wraps [`std::io::Error`] and is returned when file operations fail.
    /// Common causes include:
    /// - File not found
    /// - Permission denied
    /// - Disk full
    /// - Network errors (for remote files)
    ///
    /// # Recovery
    ///
    /// Check the underlying [`std::io::ErrorKind`] for specific handling:
    ///
    /// ```rust
    /// use zesven::Error;
    /// use std::io::ErrorKind;
    ///
    /// fn handle_io_error(error: &Error) {
    ///     if let Error::Io(e) = error {
    ///         match e.kind() {
    ///             ErrorKind::NotFound => println!("File not found"),
    ///             ErrorKind::PermissionDenied => println!("Access denied"),
    ///             _ => println!("I/O error: {}", e),
    ///         }
    ///     }
    /// }
    /// ```
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The archive format is invalid or not recognized.
    ///
    /// This error occurs when:
    /// - The file doesn't have a valid 7z signature
    /// - Required header fields are missing
    /// - The file is not a 7z archive (e.g., ZIP, RAR)
    ///
    /// The string contains a description of what was expected vs. found.
    #[error("Invalid 7z format: {0}")]
    InvalidFormat(String),

    /// The archive header is corrupt or truncated.
    ///
    /// This indicates the archive was likely damaged during download or storage.
    /// The error includes the byte offset where corruption was detected.
    ///
    /// # Recovery
    ///
    /// - Re-download the archive if possible
    /// - Try recovery tools designed for 7z archives
    /// - The offset can help locate the corruption
    #[error("Corrupt header at offset {offset:#x}: {reason}")]
    CorruptHeader {
        /// The byte offset where corruption was detected.
        offset: u64,
        /// A description of the corruption.
        reason: String,
    },

    /// The archive uses a compression method not supported by this build.
    ///
    /// Common method IDs:
    /// - `0x00`: Copy (uncompressed)
    /// - `0x21`: LZMA2
    /// - `0x030101`: LZMA
    /// - `0x040108`: Deflate
    /// - `0x040202`: BZip2
    /// - `0x030401`: PPMd
    ///
    /// # Recovery
    ///
    /// Enable the corresponding feature flag when building:
    /// ```toml
    /// zesven = { version = "0.1", features = ["lzma2", "deflate", "bzip2"] }
    /// ```
    #[error("Unsupported method: {method_id:#x}")]
    UnsupportedMethod {
        /// The method ID that is not supported.
        method_id: u64,
    },

    /// A feature required by the archive is not supported.
    ///
    /// This occurs when the archive uses 7z features that haven't been
    /// implemented yet, such as certain filter combinations.
    #[error("Unsupported feature: {feature}")]
    UnsupportedFeature {
        /// The name of the unsupported feature.
        feature: &'static str,
    },

    /// The password is incorrect or encrypted data is corrupted.
    ///
    /// This error is returned when an incorrect password was provided for an
    /// encrypted archive, or when the encrypted data itself is corrupted.
    ///
    /// **Note:** If no password was provided at all, [`Error::PasswordRequired`]
    /// is returned instead.
    ///
    /// # Detection Methods
    ///
    /// Wrong passwords are detected through:
    /// - **Early detection**: Decrypted data doesn't match expected compression header patterns
    /// - **CRC verification**: After full decompression, CRC mismatch indicates wrong password
    ///
    /// [`Error::PasswordRequired`]: Error::PasswordRequired
    ///
    /// # Recovery
    ///
    /// Prompt the user for the correct password:
    ///
    /// ```rust,ignore
    /// use zesven::{Archive, Error, Password};
    ///
    /// fn open_with_retry(path: &str) -> zesven::Result<Archive<_>> {
    ///     match Archive::open_path(path) {
    ///         Ok(archive) => Ok(archive),
    ///         Err(Error::WrongPassword { .. }) => {
    ///             let password = prompt_user_for_password();
    ///             Archive::open_path_with_password(path, Password::new(&password))
    ///         }
    ///         Err(e) => Err(e),
    ///     }
    /// }
    /// ```
    #[error("{}", WrongPasswordDisplay { entry_index: *.entry_index, entry_name: entry_name.as_deref() })]
    WrongPassword {
        /// The entry index where the wrong password was detected (if known).
        entry_index: Option<usize>,
        /// The entry name where the wrong password was detected (if known).
        entry_name: Option<String>,
        /// How the wrong password was detected.
        detection_method: PasswordDetectionMethod,
    },

    /// The operation was cancelled by the user.
    ///
    /// This error is returned when a progress callback returns `false` to
    /// indicate the operation should be cancelled, or when cancellation
    /// is requested through an [`AtomicProgress`] handle.
    ///
    /// # Recovery
    ///
    /// This is typically a user-initiated action and doesn't require recovery.
    /// Partial files created during extraction are automatically cleaned up.
    ///
    /// [`AtomicProgress`]: crate::progress::AtomicProgress
    #[error("Operation cancelled")]
    Cancelled,

    /// A cryptographic operation failed.
    ///
    /// This indicates an internal error in the encryption/decryption process,
    /// which should not occur under normal circumstances.
    #[error("Cryptographic error: {0}")]
    CryptoError(String),

    /// The CRC checksum does not match the expected value.
    ///
    /// This indicates data corruption during extraction. The file at the
    /// given entry index has different content than what was originally
    /// archived.
    ///
    /// # Recovery
    ///
    /// - Re-download the archive if possible
    /// - The archive may be partially corrupted; other entries may be valid
    #[error("{}", CrcMismatchDisplay { entry_index: *entry_index, entry_name: entry_name.as_deref(), expected: *expected, actual: *actual })]
    CrcMismatch {
        /// The entry index with the CRC mismatch.
        entry_index: usize,
        /// The entry name/path with the CRC mismatch (if known).
        entry_name: Option<String>,
        /// The expected CRC value from the archive.
        expected: u32,
        /// The actual CRC value of the extracted data.
        actual: u32,
    },

    /// Path traversal attack detected in an archive entry.
    ///
    /// This is a **security error** indicating the archive contains paths
    /// designed to escape the extraction directory (e.g., `../../etc/passwd`).
    ///
    /// **Never extract archives with this error without understanding the risk.**
    ///
    /// # Security
    ///
    /// This check is enabled by default with [`PathSafety::Strict`]. If you
    /// trust the archive source, you can disable it (not recommended):
    ///
    /// ```rust
    /// use zesven::{ExtractOptions, read::PathSafety};
    ///
    /// // NOT RECOMMENDED - only for trusted archives
    /// let options = ExtractOptions::new()
    ///     .path_safety(PathSafety::Disabled);
    /// ```
    ///
    /// [`PathSafety::Strict`]: crate::read::PathSafety::Strict
    #[error("Path traversal detected in entry {entry_index}: {path}")]
    PathTraversal {
        /// The entry index with path traversal.
        entry_index: usize,
        /// The path that contains traversal.
        path: String,
    },

    /// A symbolic link was rejected by the link policy.
    ///
    /// This is a **security error** indicating the archive contains a symbolic
    /// link that was rejected due to the configured [`LinkPolicy`].
    ///
    /// Symbolic links in archives can be a security risk because:
    /// - They may point to files outside the extraction directory
    /// - They can be used to overwrite sensitive files
    /// - They may create unexpected file system structures
    ///
    /// # Security
    ///
    /// The default [`LinkPolicy::Forbid`] rejects all symlinks. If you need to
    /// extract symlinks, use [`LinkPolicy::ValidateTargets`] to validate that
    /// symlink targets stay within the extraction directory.
    ///
    /// [`LinkPolicy`]: crate::read::LinkPolicy
    /// [`LinkPolicy::Forbid`]: crate::read::LinkPolicy::Forbid
    /// [`LinkPolicy::ValidateTargets`]: crate::read::LinkPolicy::ValidateTargets
    #[error("Symbolic link rejected at entry {entry_index}: {path}")]
    SymlinkRejected {
        /// The entry index containing the symlink.
        entry_index: usize,
        /// The path of the symlink entry.
        path: String,
    },

    /// A symbolic link target escapes the extraction directory.
    ///
    /// This is a **security error** indicating the archive contains a symbolic
    /// link whose target would point outside the extraction directory.
    ///
    /// This error is returned when using [`LinkPolicy::ValidateTargets`] and
    /// the symlink target contains path traversal sequences or absolute paths.
    ///
    /// [`LinkPolicy::ValidateTargets`]: crate::read::LinkPolicy::ValidateTargets
    #[error(
        "Symbolic link target escapes extraction directory at entry {entry_index}: {path} -> {target}"
    )]
    SymlinkTargetEscape {
        /// The entry index containing the symlink.
        entry_index: usize,
        /// The path of the symlink entry.
        path: String,
        /// The target path that escapes the extraction directory.
        target: String,
    },

    /// A resource limit was exceeded.
    ///
    /// This error protects against malicious archives (e.g., "zip bombs")
    /// that decompress to extremely large sizes.
    ///
    /// Limits include:
    /// - Maximum decompression ratio
    /// - Maximum file size
    /// - Maximum memory usage
    ///
    /// # Configuration
    ///
    /// Adjust limits using [`ResourceLimits`]:
    ///
    /// ```rust
    /// use zesven::format::streams::ResourceLimits;
    ///
    /// let limits = ResourceLimits::default()
    ///     .max_entries(10000); // Max 10,000 entries
    /// ```
    ///
    /// [`ResourceLimits`]: crate::format::streams::ResourceLimits
    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    /// An archive path is invalid.
    ///
    /// Archive paths must:
    /// - Not contain null bytes
    /// - Not be empty
    /// - Not contain path traversal sequences
    /// - Use forward slashes as separators
    ///
    /// # Recovery
    ///
    /// Use [`ArchivePath::new`] to validate paths before use:
    ///
    /// ```rust
    /// use zesven::ArchivePath;
    ///
    /// match ArchivePath::new("path/to/file.txt") {
    ///     Ok(path) => println!("Valid path: {}", path.as_str()),
    ///     Err(e) => eprintln!("Invalid path: {}", e),
    /// }
    /// ```
    ///
    /// [`ArchivePath::new`]: crate::ArchivePath::new
    #[error("Invalid archive path: {0}")]
    InvalidArchivePath(String),

    /// A volume file is missing from a multi-volume archive.
    ///
    /// This error occurs when reading a multi-volume archive and one of
    /// the volume files (.7z.001, .7z.002, etc.) cannot be found.
    ///
    /// # Recovery
    ///
    /// - Ensure all volume files are present in the same directory
    /// - Check that volume files haven't been renamed
    /// - Re-download missing volumes if possible
    #[error(
        "Volume {volume} missing: expected at '{path}' (multi-volume archives require all parts in the same directory)"
    )]
    VolumeMissing {
        /// The volume number (1-indexed) that is missing.
        volume: u32,
        /// The expected path of the missing volume.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A volume file is corrupted or truncated.
    ///
    /// This indicates that a volume file in a multi-volume archive has
    /// been damaged or is incomplete.
    #[error("Volume {volume} corrupted: {details}")]
    VolumeCorrupted {
        /// The volume number (1-indexed) that is corrupted.
        volume: u32,
        /// Description of the corruption.
        details: String,
    },

    /// A multi-volume archive is incomplete.
    ///
    /// This error occurs when not all expected volumes are present.
    #[error("Incomplete archive: expected {expected} volumes, found {found}")]
    IncompleteArchive {
        /// The expected number of volumes.
        expected: u32,
        /// The number of volumes found.
        found: u32,
    },

    /// An entry was not found in the archive.
    ///
    /// This error occurs during archive editing operations when the specified
    /// entry path does not exist in the archive.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::{Archive, Error};
    ///
    /// let result = archive.extract_to_vec("nonexistent.txt");
    /// if let Err(Error::EntryNotFound { path }) = result {
    ///     eprintln!("Entry not found: {}", path);
    /// }
    /// ```
    #[error("Entry not found: {path}")]
    EntryNotFound {
        /// The path that was not found.
        path: String,
    },

    /// An entry already exists in the archive.
    ///
    /// This error occurs during archive editing operations when attempting
    /// to add or rename an entry to a path that already exists.
    #[error("Entry already exists: {path}")]
    EntryExists {
        /// The path that already exists.
        path: String,
    },

    /// An invalid regular expression pattern was provided.
    ///
    /// This error occurs when creating a [`SelectByRegex`] selector with
    /// an invalid regex pattern.
    ///
    /// [`SelectByRegex`]: crate::read::SelectByRegex
    #[cfg(feature = "regex")]
    #[error("Invalid regex pattern '{pattern}': {reason}")]
    InvalidRegex {
        /// The invalid regex pattern.
        pattern: String,
        /// Description of why the pattern is invalid.
        reason: String,
    },

    /// An invalid compression level was provided.
    ///
    /// Compression levels must be in the range 0-9:
    /// - 0: No compression (store only)
    /// - 1-3: Fast compression, lower ratio
    /// - 4-6: Balanced compression (default is 5)
    /// - 7-9: Maximum compression, slower
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::{Error, write::WriteOptions};
    ///
    /// // Valid level succeeds
    /// let opts = WriteOptions::new().level(5);
    /// assert!(opts.is_ok());
    ///
    /// // Invalid level fails
    /// let result = WriteOptions::new().level(15);
    /// assert!(matches!(result, Err(Error::InvalidCompressionLevel { level: 15 })));
    /// ```
    #[error("invalid compression level {level}: must be 0-9")]
    InvalidCompressionLevel {
        /// The invalid level that was provided.
        level: u32,
    },

    /// A password is required but none was provided.
    ///
    /// This error is returned when attempting to read or extract an encrypted
    /// archive without providing a password.
    ///
    /// # Recovery
    ///
    /// Provide a password when opening the archive:
    ///
    /// ```rust,ignore
    /// use zesven::{Archive, Error, Password};
    ///
    /// fn open_encrypted(path: &str) -> zesven::Result<Archive<_>> {
    ///     match Archive::open_path(path) {
    ///         Err(Error::PasswordRequired) => {
    ///             let password = prompt_user_for_password();
    ///             Archive::open_path_with_password(path, Password::new(&password))
    ///         }
    ///         other => other,
    ///     }
    /// }
    /// ```
    #[error("password required for encrypted archive")]
    PasswordRequired,
}

impl Error {
    /// Returns `true` if this error indicates a security issue.
    ///
    /// Security errors should generally cause extraction to abort unless
    /// the archive source is fully trusted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::Error;
    ///
    /// fn should_abort(error: &Error) -> bool {
    ///     error.is_security_error()
    /// }
    /// ```
    pub fn is_security_error(&self) -> bool {
        matches!(
            self,
            Error::PathTraversal { .. }
                | Error::SymlinkRejected { .. }
                | Error::SymlinkTargetEscape { .. }
        )
    }

    /// Returns `true` if this error might be recoverable.
    ///
    /// Recoverable errors are those where the operation could potentially succeed
    /// if tried again or with different parameters:
    ///
    /// - `WrongPassword`: Retry with a different password
    /// - `VolumeMissing`: User can provide the missing volume file
    /// - `Cancelled`: Operation can be restarted
    /// - `Io` (transient kinds only): Retry may succeed for `WouldBlock`, `Interrupted`, `TimedOut`
    ///
    /// Non-transient I/O errors like `InvalidData`, `UnexpectedEof`, or `PermissionDenied`
    /// are not recoverable as they indicate fundamental issues that won't resolve on retry.
    ///
    /// Note: `ResourceLimitExceeded` is intentionally not recoverable - if limits are
    /// exceeded, the archive is likely a compression bomb or exceeds user-defined limits.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::Error;
    ///
    /// fn handle_error(error: &Error) {
    ///     if error.is_recoverable() {
    ///         println!("This error might be recoverable");
    ///     } else {
    ///         println!("Cannot recover from this error");
    ///     }
    /// }
    /// ```
    pub fn is_recoverable(&self) -> bool {
        match self {
            Error::WrongPassword { .. } => true,
            Error::PasswordRequired => true,
            Error::Cancelled => true,
            // VolumeMissing is recoverable: user can provide the missing volume
            Error::VolumeMissing { .. } => true,
            // Only transient I/O errors are recoverable
            Error::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::TimedOut
            ),
            // ResourceLimitExceeded is NOT recoverable (likely compression bomb)
            _ => false,
        }
    }

    /// Returns `true` if this is a data corruption error.
    ///
    /// Corruption errors indicate the archive or extracted data is damaged.
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            Error::CrcMismatch { .. } | Error::CorruptHeader { .. }
        )
    }

    /// Returns `true` if this is an encryption-related error.
    ///
    /// Encryption errors include wrong passwords, missing passwords, and crypto failures.
    pub fn is_encryption_error(&self) -> bool {
        matches!(
            self,
            Error::WrongPassword { .. } | Error::CryptoError(_) | Error::PasswordRequired
        )
    }

    /// Returns `true` if this error is related to unsupported features or methods.
    ///
    /// These errors indicate the archive uses features not supported by this build.
    pub fn is_unsupported(&self) -> bool {
        matches!(
            self,
            Error::UnsupportedMethod { .. } | Error::UnsupportedFeature { .. }
        )
    }

    /// Returns the entry index associated with this error, if any.
    ///
    /// Many errors include context about which entry caused the error.
    /// This method provides a unified way to access that information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::Error;
    ///
    /// fn log_error(error: &Error) {
    ///     if let Some(idx) = error.entry_index() {
    ///         eprintln!("Error in entry {}: {}", idx, error);
    ///     } else {
    ///         eprintln!("Error: {}", error);
    ///     }
    /// }
    /// ```
    pub fn entry_index(&self) -> Option<usize> {
        match self {
            Error::WrongPassword { entry_index, .. } => *entry_index,
            Error::CrcMismatch { entry_index, .. } => Some(*entry_index),
            Error::PathTraversal { entry_index, .. } => Some(*entry_index),
            Error::SymlinkRejected { entry_index, .. } => Some(*entry_index),
            Error::SymlinkTargetEscape { entry_index, .. } => Some(*entry_index),
            _ => None,
        }
    }

    /// Returns the entry name/path associated with this error, if any.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::Error;
    ///
    /// fn log_error(error: &Error) {
    ///     if let Some(name) = error.entry_name() {
    ///         eprintln!("Error for '{}': {}", name, error);
    ///     }
    /// }
    /// ```
    pub fn entry_name(&self) -> Option<&str> {
        match self {
            Error::WrongPassword { entry_name, .. } => entry_name.as_deref(),
            Error::CrcMismatch { entry_name, .. } => entry_name.as_deref(),
            Error::PathTraversal { path, .. } => Some(path.as_str()),
            Error::SymlinkRejected { path, .. } => Some(path.as_str()),
            Error::SymlinkTargetEscape { path, .. } => Some(path.as_str()),
            Error::VolumeMissing { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// Returns the method ID if this is an unsupported method error.
    pub fn method_id(&self) -> Option<u64> {
        match self {
            Error::UnsupportedMethod { method_id } => Some(*method_id),
            _ => None,
        }
    }

    /// Creates a WrongPassword error with full context.
    ///
    /// This is a convenience constructor for creating wrong password errors
    /// with all available context information.
    pub fn wrong_password(
        entry_index: Option<usize>,
        entry_name: Option<String>,
        detection_method: PasswordDetectionMethod,
    ) -> Self {
        Error::WrongPassword {
            entry_index,
            entry_name,
            detection_method,
        }
    }

    /// Creates a CrcMismatch error.
    ///
    /// This is a convenience constructor for creating CRC mismatch errors.
    pub fn crc_mismatch(
        entry_index: usize,
        entry_name: Option<String>,
        expected: u32,
        actual: u32,
    ) -> Self {
        Error::CrcMismatch {
            entry_index,
            entry_name,
            expected,
            actual,
        }
    }

    /// Creates a CorruptHeader error.
    ///
    /// This is a convenience constructor for creating corrupt header errors.
    pub fn corrupt_header(offset: u64, reason: impl Into<String>) -> Self {
        Error::CorruptHeader {
            offset,
            reason: reason.into(),
        }
    }
}

/// A specialized Result type for 7z operations.
///
/// This is defined as `std::result::Result<T, Error>` for convenience.
///
/// # Example
///
/// ```rust
/// use zesven::Result;
///
/// fn my_function() -> Result<()> {
///     // Operations that may fail...
///     Ok(())
/// }
/// ```
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_from() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_invalid_format() {
        let err = Error::InvalidFormat("missing signature".into());
        assert_eq!(err.to_string(), "Invalid 7z format: missing signature");
    }

    #[test]
    fn test_corrupt_header() {
        let err = Error::CorruptHeader {
            offset: 0x1234,
            reason: "unexpected end of header".into(),
        };
        assert!(err.to_string().contains("0x1234"));
        assert!(err.to_string().contains("unexpected end of header"));
    }

    #[test]
    fn test_unsupported_method() {
        let err = Error::UnsupportedMethod {
            method_id: 0x030101,
        };
        assert!(err.to_string().contains("0x30101"));
    }

    #[test]
    fn test_unsupported_feature() {
        let err = Error::UnsupportedFeature {
            feature: "solid blocks",
        };
        assert!(err.to_string().contains("solid blocks"));
    }

    #[test]
    fn test_wrong_password() {
        // Basic wrong password without context
        let err = Error::WrongPassword {
            entry_index: None,
            entry_name: None,
            detection_method: PasswordDetectionMethod::CrcMismatch,
        };
        assert!(err.to_string().contains("Wrong password"));

        // Wrong password with entry index
        let err = Error::WrongPassword {
            entry_index: Some(5),
            entry_name: None,
            detection_method: PasswordDetectionMethod::EarlyHeaderValidation,
        };
        assert!(err.to_string().contains("entry 5"));

        // Wrong password with entry name
        let err = Error::WrongPassword {
            entry_index: Some(3),
            entry_name: Some("file.txt".into()),
            detection_method: PasswordDetectionMethod::DecompressionFailure,
        };
        assert!(err.to_string().contains("file.txt"));
        assert!(err.to_string().contains("entry 3"));
    }

    #[test]
    fn test_cancelled() {
        let err = Error::Cancelled;
        assert!(err.to_string().contains("cancelled"));
        assert!(err.is_recoverable());
    }

    #[test]
    fn test_crypto_error() {
        let err = Error::CryptoError("invalid key size".into());
        assert!(err.to_string().contains("invalid key size"));
    }

    #[test]
    fn test_crc_mismatch() {
        // Without entry name
        let err = Error::CrcMismatch {
            entry_index: 5,
            entry_name: None,
            expected: 0xDEADBEEF,
            actual: 0xCAFEBABE,
        };
        let msg = err.to_string();
        assert!(msg.contains("entry 5"));
        assert!(msg.contains("0xdeadbeef"));
        assert!(msg.contains("0xcafebabe"));

        // With entry name
        let err = Error::CrcMismatch {
            entry_index: 5,
            entry_name: Some("path/to/file.txt".into()),
            expected: 0xDEADBEEF,
            actual: 0xCAFEBABE,
        };
        let msg = err.to_string();
        assert!(msg.contains("entry 5"));
        assert!(msg.contains("path/to/file.txt"));
        assert!(msg.contains("0xdeadbeef"));
        assert!(msg.contains("0xcafebabe"));
    }

    #[test]
    fn test_path_traversal() {
        let err = Error::PathTraversal {
            entry_index: 3,
            path: "../etc/passwd".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("entry 3"));
        assert!(msg.contains("../etc/passwd"));
    }

    #[test]
    fn test_volume_missing_with_source() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let err = Error::VolumeMissing {
            volume: 2,
            path: "test.7z.002".into(),
            source: io_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("Volume 2"), "Should show volume number");
        assert!(msg.contains("test.7z.002"), "Should show path");
        // Verify source chain is preserved
        assert!(
            std::error::Error::source(&err).is_some(),
            "Source chain should be preserved"
        );
    }

    #[test]
    fn test_volume_corrupted() {
        let err = Error::VolumeCorrupted {
            volume: 3,
            details: "truncated at offset 0x1000".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("3"), "Should show volume number");
        assert!(msg.contains("corrupted"), "Should mention corruption");
        assert!(msg.contains("truncated"), "Should include details");
    }

    #[test]
    fn test_incomplete_archive() {
        let err = Error::IncompleteArchive {
            expected: 5,
            found: 3,
        };
        let msg = err.to_string();
        assert!(msg.contains("5"), "Should show expected count");
        assert!(msg.contains("3"), "Should show found count");
        assert!(msg.contains("Incomplete"), "Should indicate incomplete");
    }

    #[cfg(feature = "regex")]
    #[test]
    fn test_invalid_regex() {
        let err = Error::InvalidRegex {
            pattern: "[invalid".into(),
            reason: "unclosed bracket".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("[invalid"), "Should show pattern");
        assert!(msg.contains("unclosed bracket"), "Should show reason");
    }

    #[test]
    fn test_resource_limit_exceeded() {
        let err = Error::ResourceLimitExceeded("file too large".into());
        assert!(err.to_string().contains("file too large"));
    }

    #[test]
    fn test_invalid_archive_path() {
        let err = Error::InvalidArchivePath("contains NUL byte".into());
        assert!(err.to_string().contains("contains NUL byte"));
    }

    #[test]
    fn test_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }

    #[test]
    fn test_is_encryption_error() {
        let err = Error::WrongPassword {
            entry_index: None,
            entry_name: None,
            detection_method: PasswordDetectionMethod::CrcMismatch,
        };
        assert!(err.is_encryption_error());

        let err = Error::CryptoError("test".into());
        assert!(err.is_encryption_error());

        let err = Error::Io(io::Error::new(io::ErrorKind::NotFound, "test"));
        assert!(!err.is_encryption_error());
    }

    #[test]
    fn test_is_unsupported() {
        let err = Error::UnsupportedMethod { method_id: 0x1234 };
        assert!(err.is_unsupported());

        let err = Error::UnsupportedFeature { feature: "test" };
        assert!(err.is_unsupported());

        let err = Error::InvalidFormat("test".into());
        assert!(!err.is_unsupported());
    }

    #[test]
    fn test_entry_index() {
        let err = Error::CrcMismatch {
            entry_index: 5,
            entry_name: None,
            expected: 0,
            actual: 1,
        };
        assert_eq!(err.entry_index(), Some(5));

        let err = Error::PathTraversal {
            entry_index: 3,
            path: "test".into(),
        };
        assert_eq!(err.entry_index(), Some(3));

        let err = Error::WrongPassword {
            entry_index: Some(7),
            entry_name: None,
            detection_method: PasswordDetectionMethod::CrcMismatch,
        };
        assert_eq!(err.entry_index(), Some(7));

        let err = Error::InvalidFormat("test".into());
        assert_eq!(err.entry_index(), None);
    }

    #[test]
    fn test_entry_name() {
        let err = Error::WrongPassword {
            entry_index: None,
            entry_name: Some("file.txt".into()),
            detection_method: PasswordDetectionMethod::CrcMismatch,
        };
        assert_eq!(err.entry_name(), Some("file.txt"));

        let err = Error::CrcMismatch {
            entry_index: 0,
            entry_name: Some("data/file.bin".into()),
            expected: 0,
            actual: 1,
        };
        assert_eq!(err.entry_name(), Some("data/file.bin"));

        let err = Error::PathTraversal {
            entry_index: 0,
            path: "../etc/passwd".into(),
        };
        assert_eq!(err.entry_name(), Some("../etc/passwd"));

        let err = Error::InvalidFormat("test".into());
        assert_eq!(err.entry_name(), None);
    }

    #[test]
    fn test_method_id() {
        let err = Error::UnsupportedMethod {
            method_id: 0x030101,
        };
        assert_eq!(err.method_id(), Some(0x030101));

        let err = Error::InvalidFormat("test".into());
        assert_eq!(err.method_id(), None);
    }

    #[test]
    fn test_convenience_constructors() {
        let err = Error::wrong_password(
            Some(5),
            Some("file.txt".into()),
            PasswordDetectionMethod::EarlyHeaderValidation,
        );
        assert!(err.is_encryption_error());
        assert_eq!(err.entry_index(), Some(5));
        assert_eq!(err.entry_name(), Some("file.txt"));

        let err = Error::crc_mismatch(3, Some("test.txt".into()), 0xDEAD, 0xBEEF);
        assert!(err.is_corruption());
        assert_eq!(err.entry_index(), Some(3));
        assert_eq!(err.entry_name(), Some("test.txt"));

        let err = Error::corrupt_header(0x1000, "truncated");
        assert!(err.is_corruption());
        assert!(err.to_string().contains("0x1000"));
        assert!(err.to_string().contains("truncated"));
    }

    // Tests for is_recoverable() with improved I/O error classification

    #[test]
    fn test_is_recoverable_transient_io_errors() {
        // Transient I/O errors ARE recoverable
        let err = Error::Io(io::Error::new(io::ErrorKind::WouldBlock, "would block"));
        assert!(err.is_recoverable(), "WouldBlock should be recoverable");

        let err = Error::Io(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        assert!(err.is_recoverable(), "Interrupted should be recoverable");

        let err = Error::Io(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        assert!(err.is_recoverable(), "TimedOut should be recoverable");
    }

    #[test]
    fn test_is_recoverable_non_transient_io_errors() {
        // Non-transient I/O errors are NOT recoverable
        let err = Error::Io(io::Error::new(io::ErrorKind::NotFound, "not found"));
        assert!(!err.is_recoverable(), "NotFound should not be recoverable");

        let err = Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        assert!(
            !err.is_recoverable(),
            "PermissionDenied should not be recoverable"
        );

        let err = Error::Io(io::Error::new(io::ErrorKind::InvalidData, "invalid"));
        assert!(
            !err.is_recoverable(),
            "InvalidData should not be recoverable"
        );

        let err = Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
        assert!(
            !err.is_recoverable(),
            "UnexpectedEof should not be recoverable"
        );
    }

    #[test]
    fn test_is_recoverable_volume_missing() {
        let err = Error::VolumeMissing {
            volume: 2,
            path: "archive.7z.002".into(),
            source: io::Error::new(io::ErrorKind::NotFound, "file not found"),
        };
        assert!(
            err.is_recoverable(),
            "VolumeMissing should be recoverable (user can provide the file)"
        );
    }

    #[test]
    fn test_is_recoverable_resource_limit_not_recoverable() {
        // ResourceLimitExceeded is NOT recoverable (likely compression bomb)
        let err = Error::ResourceLimitExceeded("size limit".into());
        assert!(
            !err.is_recoverable(),
            "ResourceLimitExceeded should NOT be recoverable"
        );
    }

    #[test]
    fn test_is_recoverable_other_errors_not_recoverable() {
        // Format errors are not recoverable
        let err = Error::InvalidFormat("bad format".into());
        assert!(!err.is_recoverable());

        // CRC mismatches are not recoverable
        let err = Error::CrcMismatch {
            entry_index: 0,
            entry_name: None,
            expected: 0xDEAD,
            actual: 0xBEEF,
        };
        assert!(!err.is_recoverable());

        // Unsupported methods are not recoverable
        let err = Error::UnsupportedMethod { method_id: 0x999 };
        assert!(!err.is_recoverable());
    }

    #[test]
    fn test_entry_not_found() {
        let err = Error::EntryNotFound {
            path: "test/file.txt".into(),
        };
        assert_eq!(err.to_string(), "Entry not found: test/file.txt");
        assert!(!err.is_recoverable());
        assert!(!err.is_security_error());
    }

    #[test]
    fn test_entry_exists() {
        let err = Error::EntryExists {
            path: "existing/file.txt".into(),
        };
        assert_eq!(err.to_string(), "Entry already exists: existing/file.txt");
        assert!(!err.is_recoverable());
        assert!(!err.is_security_error());
    }

    #[test]
    fn test_invalid_compression_level() {
        let err = Error::InvalidCompressionLevel { level: 15 };
        assert_eq!(err.to_string(), "invalid compression level 15: must be 0-9");
        assert!(!err.is_recoverable());
        assert!(!err.is_security_error());

        // Test boundary values
        let err = Error::InvalidCompressionLevel { level: 10 };
        assert!(err.to_string().contains("10"));

        let err = Error::InvalidCompressionLevel { level: u32::MAX };
        assert!(err.to_string().contains(&u32::MAX.to_string()));
    }

    // =========================================================================
    // Symlink Error Tests (moved from tests/malformed_archives.rs)
    // =========================================================================

    #[test]
    fn test_symlink_error_types() {
        // SymlinkRejected error
        let rejected_err = Error::SymlinkRejected {
            entry_index: 5,
            path: "malicious_link".into(),
        };
        assert!(rejected_err.is_security_error());
        assert_eq!(rejected_err.entry_index(), Some(5));
        assert_eq!(rejected_err.entry_name(), Some("malicious_link"));

        let msg = rejected_err.to_string();
        assert!(msg.contains("Symbolic link rejected"));
        assert!(msg.contains("entry 5"));
        assert!(msg.contains("malicious_link"));

        // SymlinkTargetEscape error
        let escape_err = Error::SymlinkTargetEscape {
            entry_index: 7,
            path: "sneaky_link".into(),
            target: "../../../etc/passwd".into(),
        };
        assert!(escape_err.is_security_error());
        assert_eq!(escape_err.entry_index(), Some(7));
        assert_eq!(escape_err.entry_name(), Some("sneaky_link"));

        let msg = escape_err.to_string();
        assert!(msg.contains("Symbolic link target escapes"));
        assert!(msg.contains("entry 7"));
        assert!(msg.contains("sneaky_link"));
        assert!(msg.contains("../../../etc/passwd"));
    }

    #[test]
    fn test_symlink_absolute_path_error() {
        let err = Error::SymlinkTargetEscape {
            entry_index: 0,
            path: "link_to_passwd".into(),
            target: "/etc/passwd".into(),
        };

        assert!(err.is_security_error());
        let msg = err.to_string();
        assert!(msg.contains("/etc/passwd"));
    }

    #[test]
    fn test_symlink_windows_absolute_path_error() {
        let err = Error::SymlinkTargetEscape {
            entry_index: 0,
            path: "link_to_system".into(),
            target: "C:\\Windows\\System32".into(),
        };

        assert!(err.is_security_error());
        let msg = err.to_string();
        assert!(msg.contains("C:\\Windows\\System32"));
    }

    #[test]
    fn test_symlink_errors_not_recoverable() {
        let rejected = Error::SymlinkRejected {
            entry_index: 0,
            path: "link".into(),
        };
        // Symlink rejection is a security policy, not a recoverable error
        assert!(!rejected.is_recoverable());

        let escape = Error::SymlinkTargetEscape {
            entry_index: 0,
            path: "link".into(),
            target: "../escape".into(),
        };
        // Symlink escape is a security issue, not recoverable
        assert!(!escape.is_recoverable());
    }

    // =========================================================================
    // PasswordDetectionMethod Display Tests
    // =========================================================================

    #[test]
    fn test_password_detection_method_display() {
        assert!(
            PasswordDetectionMethod::EarlyHeaderValidation
                .to_string()
                .contains("header")
        );
        assert!(
            PasswordDetectionMethod::CrcMismatch
                .to_string()
                .contains("CRC")
        );
        assert!(
            PasswordDetectionMethod::DecompressionFailure
                .to_string()
                .contains("decompression")
        );
    }

    #[test]
    fn test_wrong_password_error_has_full_context() {
        let err = Error::WrongPassword {
            entry_index: Some(5),
            entry_name: Some("secret.txt".into()),
            detection_method: PasswordDetectionMethod::CrcMismatch,
        };

        // Check error classification methods
        assert!(err.is_encryption_error());
        assert!(err.is_recoverable());
        assert!(!err.is_security_error());

        // Check context accessors
        assert_eq!(err.entry_index(), Some(5));
        assert_eq!(err.entry_name(), Some("secret.txt"));

        // Check display formatting includes all context
        let msg = err.to_string();
        assert!(msg.contains("Wrong password"));
        assert!(msg.contains("entry 5"));
        assert!(msg.contains("secret.txt"));
    }

    #[test]
    fn test_password_required_error() {
        let err = Error::PasswordRequired;

        // Check error classification methods
        assert!(err.is_encryption_error());
        assert!(err.is_recoverable());
        assert!(!err.is_security_error());

        // Check display formatting
        let msg = err.to_string();
        assert!(msg.contains("password required"));
        assert!(msg.contains("encrypted"));
    }
}
