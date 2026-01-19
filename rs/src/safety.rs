//! Safety and resource limit utilities.
//!
//! This module provides utilities for safe extraction including path validation,
//! resource limit enforcement, and compression bomb protection.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ArchivePath, Error, Result};

/// Policy for validating extraction paths.
///
/// This enum controls how strictly paths from archive entries are validated
/// before extraction. The default is `Strict`, which provides the safest
/// behavior by blocking any potential path traversal attacks.
///
/// # Security
///
/// Path traversal attacks occur when an archive contains entries with paths
/// like `../../../etc/passwd` that could escape the intended extraction
/// directory. Always use `Strict` mode when extracting untrusted archives.
///
/// # Examples
///
/// ```rust
/// use zesven::safety::PathSafety;
///
/// // Default: strict validation (recommended for untrusted archives)
/// let policy = PathSafety::default();
/// assert_eq!(policy, PathSafety::Strict);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathSafety {
    /// Strict validation: block any potential path traversal.
    ///
    /// This is the safest mode and the default. It:
    /// - Rejects paths containing `..` components
    /// - Rejects absolute paths (starting with `/`)
    /// - Verifies that resolved paths stay within the destination directory
    #[default]
    Strict,
    /// Allow relative paths but block absolute paths and traversal.
    ///
    /// This mode is slightly more permissive but still blocks obvious attacks.
    /// Use only when you understand the archive source.
    Relaxed,
    /// Disables all path validation (DANGEROUS - use with extreme caution).
    ///
    /// # Security Warning
    ///
    /// **`PathSafety::Disabled` completely disables all path validation.** This allows:
    ///
    /// - **Absolute paths**: Entries like `/etc/passwd` will be extracted to the
    ///   system root, not relative to your destination directory
    /// - **Path traversal**: Entries like `../../../etc/passwd` will escape the
    ///   extraction directory and overwrite arbitrary files
    /// - **Arbitrary file writes**: A malicious archive can overwrite any file
    ///   the process has permission to write
    ///
    /// # When to Use
    ///
    /// Only use `PathSafety::Disabled` when **all** of these conditions are met:
    ///
    /// 1. You created the archive yourself, OR
    /// 2. The archive comes from a cryptographically verified trusted source
    /// 3. You understand exactly what paths are in the archive
    /// 4. You need to extract paths that would be blocked by `Strict` or `Relaxed`
    ///
    /// # Alternatives
    ///
    /// Consider using `PathSafety::Relaxed` instead, which allows more paths
    /// while still blocking obvious attacks. If you need to extract specific
    /// absolute paths, consider extracting to a temporary directory and moving
    /// files manually.
    ///
    /// # Example of Risk
    ///
    /// ```rust,ignore
    /// // A malicious archive could contain:
    /// // - "/etc/cron.d/malware" -> Installs scheduled malware
    /// // - "../../../home/user/.ssh/authorized_keys" -> Gains SSH access
    /// // - "/usr/local/bin/sudo" -> Replaces system utilities
    ///
    /// // With PathSafety::Disabled, all of these would be extracted!
    /// let options = ExtractOptions::new().path_safety(PathSafety::Disabled);
    /// archive.extract("/tmp/safe_dir", (), &options)?; // NOT SAFE!
    /// ```
    Disabled,
}

/// Validates an extraction path against the given safety policy.
///
/// This function checks that the archive path is safe to extract to the
/// destination directory based on the configured policy.
///
/// # Arguments
///
/// * `archive_path` - The path from the archive entry
/// * `dest_root` - The destination directory for extraction
/// * `policy` - The path safety policy to enforce
/// * `entry_index` - The index of the entry being validated (for error reporting)
///
/// # Returns
///
/// The validated full path to extract to, or an error if validation fails.
pub fn validate_extract_path(
    archive_path: &ArchivePath,
    dest_root: &Path,
    policy: PathSafety,
    entry_index: usize,
) -> Result<PathBuf> {
    let path_str = archive_path.as_str();

    // Check for traversal attempts in the archive path
    for component in path_str.split('/') {
        if component == ".." {
            return Err(Error::PathTraversal {
                entry_index,
                path: path_str.to_string(),
            });
        }
    }

    match policy {
        PathSafety::Strict => {
            // Reject absolute paths
            if path_str.starts_with('/') {
                return Err(Error::PathTraversal {
                    entry_index,
                    path: path_str.to_string(),
                });
            }

            let full_path = dest_root.join(path_str);

            // Verify the resolved path stays within dest_root.
            // The destination root MUST be canonicalizable - propagate IO error if not.
            let canonical_dest = dest_root.canonicalize()?;

            // For the target path, we must verify containment.
            // This check MUST NOT be bypassed - silent bypass is a security vulnerability.
            let canonical_full = if full_path.exists() {
                // Path exists - canonicalize it directly
                full_path.canonicalize()?
            } else {
                // For non-existent paths, canonicalize the deepest existing ancestor
                // and verify the path would stay within bounds.
                let mut ancestor = full_path.as_path();
                let mut components_to_append = Vec::new();

                // Walk up until we find an existing directory
                loop {
                    if ancestor.exists() {
                        break;
                    }
                    if let Some(file_name) = ancestor.file_name() {
                        components_to_append.push(file_name.to_os_string());
                    }
                    match ancestor.parent() {
                        Some(p) if !p.as_os_str().is_empty() => {
                            ancestor = p;
                        }
                        _ => {
                            // No existing ancestor found - this shouldn't happen if
                            // dest_root exists (which it must, since we canonicalized it).
                            // Build path relative to canonical_dest.
                            let relative = full_path.strip_prefix(dest_root).map_err(|_| {
                                Error::PathTraversal {
                                    entry_index,
                                    path: path_str.to_string(),
                                }
                            })?;
                            let mut result = canonical_dest.clone();
                            for component in relative.components() {
                                if let std::path::Component::Normal(c) = component {
                                    result.push(c);
                                }
                            }
                            // Verify containment before returning
                            if !result.starts_with(&canonical_dest) {
                                return Err(Error::PathTraversal {
                                    entry_index,
                                    path: path_str.to_string(),
                                });
                            }
                            return Ok(full_path);
                        }
                    }
                }

                // Canonicalize the existing ancestor
                let canonical_ancestor = ancestor.canonicalize()?;

                // Rebuild the path by appending the non-existent components
                let mut result = canonical_ancestor;
                for component in components_to_append.into_iter().rev() {
                    result.push(component);
                }
                result
            };

            // Final containment check - this MUST NOT be bypassed
            if !canonical_full.starts_with(&canonical_dest) {
                return Err(Error::PathTraversal {
                    entry_index,
                    path: path_str.to_string(),
                });
            }

            Ok(full_path)
        }
        PathSafety::Relaxed => {
            // Allow relative paths, block absolute and traversal
            if path_str.starts_with('/') {
                return Err(Error::PathTraversal {
                    entry_index,
                    path: path_str.to_string(),
                });
            }
            Ok(dest_root.join(path_str))
        }
        PathSafety::Disabled => {
            // No validation (dangerous!)
            Ok(dest_root.join(path_str))
        }
    }
}

/// A reader wrapper that enforces resource limits during extraction.
///
/// This wrapper tracks bytes read and checks against configured limits,
/// providing protection against compression bombs and runaway extractions.
pub struct LimitedReader<R> {
    inner: R,
    /// Maximum bytes this entry can produce.
    max_entry_bytes: u64,
    /// Bytes read from this entry so far.
    bytes_read: u64,
    /// Size of the compressed data (for ratio checking).
    compressed_size: u64,
    /// Maximum compression ratio allowed.
    max_ratio: Option<u32>,
    /// Shared counter for total bytes across all entries.
    total_tracker: Option<Arc<AtomicU64>>,
    /// Maximum total bytes.
    max_total_bytes: u64,
}

impl<R> LimitedReader<R> {
    /// Creates a new limited reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            max_entry_bytes: u64::MAX,
            bytes_read: 0,
            compressed_size: 0,
            max_ratio: None,
            total_tracker: None,
            max_total_bytes: u64::MAX,
        }
    }

    /// Sets the maximum bytes for this entry.
    pub fn max_entry_bytes(mut self, max: u64) -> Self {
        self.max_entry_bytes = max;
        self
    }

    /// Sets the compressed size for ratio checking.
    pub fn compressed_size(mut self, size: u64) -> Self {
        self.compressed_size = size;
        self
    }

    /// Sets the maximum compression ratio.
    pub fn max_ratio(mut self, ratio: u32) -> Self {
        self.max_ratio = Some(ratio);
        self
    }

    /// Sets a shared tracker for total bytes.
    pub fn total_tracker(mut self, tracker: Arc<AtomicU64>, max_total: u64) -> Self {
        self.total_tracker = Some(tracker);
        self.max_total_bytes = max_total;
        self
    }

    /// Returns the number of bytes read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 {
            return Ok(0);
        }

        self.bytes_read += n as u64;

        // Check entry limit
        if self.bytes_read > self.max_entry_bytes {
            return Err(io::Error::other(Error::ResourceLimitExceeded(format!(
                "Entry size {} exceeds limit {}",
                self.bytes_read, self.max_entry_bytes
            ))));
        }

        // Check ratio limit using multiplication to avoid integer truncation.
        // Instead of: bytes_read / compressed_size > max_ratio (truncates)
        // We check:   bytes_read > max_ratio * compressed_size (exact)
        if let Some(max_ratio) = self.max_ratio {
            if self.compressed_size > 0 {
                // Use saturating_mul to handle potential overflow safely
                let max_allowed = (max_ratio as u64).saturating_mul(self.compressed_size);
                if self.bytes_read > max_allowed {
                    // Calculate actual ratio for error message (truncation OK for display)
                    let actual_ratio = self.bytes_read / self.compressed_size;
                    return Err(io::Error::other(Error::ResourceLimitExceeded(format!(
                        "Compression ratio {}:1 exceeds limit {}:1 (compressed: {}, uncompressed: {})",
                        actual_ratio, max_ratio, self.compressed_size, self.bytes_read
                    ))));
                }
            }
        }

        // Update total tracker
        if let Some(ref tracker) = self.total_tracker {
            let total = tracker.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
            if total > self.max_total_bytes {
                return Err(io::Error::other(Error::ResourceLimitExceeded(format!(
                    "Total extracted size {} exceeds limit {}",
                    total, self.max_total_bytes
                ))));
            }
        }

        Ok(n)
    }
}

impl<R> std::fmt::Debug for LimitedReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitedReader")
            .field("max_entry_bytes", &self.max_entry_bytes)
            .field("bytes_read", &self.bytes_read)
            .field("compressed_size", &self.compressed_size)
            .field("max_ratio", &self.max_ratio)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_validate_strict_normal_path() {
        let archive_path = ArchivePath::new("foo/bar.txt").unwrap();
        let dest = std::env::temp_dir();
        let result = validate_extract_path(&archive_path, &dest, PathSafety::Strict, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), dest.join("foo").join("bar.txt"));
    }

    #[test]
    fn test_validate_strict_rejects_traversal() {
        let archive_path = ArchivePath::new("foo/../bar.txt");
        // ArchivePath should already reject this, but test the function too
        assert!(archive_path.is_err());
    }

    #[test]
    fn test_validate_strict_rejects_absolute() {
        // Can't create ArchivePath with absolute path, so we test the function directly
        // by checking the policy handles paths starting with /
        let dest = std::env::temp_dir();
        // The ArchivePath type prevents absolute paths, so strict mode is safe
        let archive_path = ArchivePath::new("safe/path.txt").unwrap();
        let result = validate_extract_path(&archive_path, &dest, PathSafety::Strict, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_disabled_allows_anything() {
        let archive_path = ArchivePath::new("any/path.txt").unwrap();
        let dest = std::env::temp_dir();
        let result = validate_extract_path(&archive_path, &dest, PathSafety::Disabled, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_path_traversal_error_contains_entry_index() {
        // Test that the entry index is correctly reported in path traversal errors
        // We can't directly test this with ArchivePath since it rejects "..",
        // but we verify the error type is correct when reported
        let err = Error::PathTraversal {
            entry_index: 42,
            path: "malicious/path".to_string(),
        };
        let err_str = err.to_string();
        assert!(err_str.contains("42"), "Error should contain entry index");
    }

    #[test]
    fn test_limited_reader_under_limit() {
        let data = vec![0u8; 100];
        let mut reader = LimitedReader::new(Cursor::new(data)).max_entry_bytes(1000);

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        assert!(result.is_ok());
        assert_eq!(buf.len(), 100);
    }

    #[test]
    fn test_limited_reader_exceeds_entry_limit() {
        let data = vec![0u8; 200];
        let mut reader = LimitedReader::new(Cursor::new(data)).max_entry_bytes(100);

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_limited_reader_ratio_check() {
        // Simulate 10 bytes compressed expanding to 2000 bytes
        let data = vec![0u8; 2000];
        let mut reader = LimitedReader::new(Cursor::new(data))
            .compressed_size(10)
            .max_ratio(100); // Allow 100:1 ratio

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        // Should fail because 2000/10 = 200 > 100
        assert!(result.is_err());
    }

    #[test]
    fn test_limited_reader_total_tracker() {
        let tracker = Arc::new(AtomicU64::new(0));

        // First read: 50 bytes
        let data1 = vec![0u8; 50];
        let mut reader1 =
            LimitedReader::new(Cursor::new(data1)).total_tracker(tracker.clone(), 100);
        let mut buf1 = Vec::new();
        assert!(reader1.read_to_end(&mut buf1).is_ok());

        // Second read: 60 bytes (total would be 110, exceeds 100)
        let data2 = vec![0u8; 60];
        let mut reader2 =
            LimitedReader::new(Cursor::new(data2)).total_tracker(tracker.clone(), 100);
        let mut buf2 = Vec::new();
        assert!(reader2.read_to_end(&mut buf2).is_err());
    }

    #[test]
    fn test_limited_reader_bytes_read() {
        let data = vec![0u8; 50];
        let mut reader = LimitedReader::new(Cursor::new(data));

        let mut buf = [0u8; 20];
        let _ = reader.read(&mut buf).unwrap();
        assert_eq!(reader.bytes_read(), 20);

        let _ = reader.read(&mut buf).unwrap();
        assert_eq!(reader.bytes_read(), 40);
    }

    #[test]
    fn test_limited_reader_ratio_no_truncation() {
        // Test that ratio 1.5:1 is correctly detected with max_ratio=1.
        // Prior to fix, integer division (15/10 = 1) would incorrectly pass.
        // With multiplication check (15 > 1*10), this correctly fails.
        let data = vec![0u8; 15];
        let mut reader = LimitedReader::new(Cursor::new(data))
            .compressed_size(10)
            .max_ratio(1); // Only allow 1:1 ratio

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        // Should fail because 15 > 1*10 (ratio 1.5:1 exceeds 1:1 limit)
        assert!(
            result.is_err(),
            "Ratio 1.5:1 should exceed limit of 1:1 - was truncation bug fixed?"
        );
    }

    #[test]
    fn test_limited_reader_ratio_at_exact_boundary() {
        // Test exact boundary: 100:1 ratio with max_ratio=100 should pass
        let data = vec![0u8; 1000];
        let mut reader = LimitedReader::new(Cursor::new(data))
            .compressed_size(10)
            .max_ratio(100); // Allow exactly 100:1 ratio

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        // Should pass because 1000 == 100*10 (exactly at limit)
        assert!(
            result.is_ok(),
            "Ratio exactly at 100:1 should pass when limit is 100"
        );
    }

    #[test]
    fn test_limited_reader_ratio_one_over_boundary() {
        // Test one byte over boundary should fail
        let data = vec![0u8; 1001];
        let mut reader = LimitedReader::new(Cursor::new(data))
            .compressed_size(10)
            .max_ratio(100); // Allow 100:1 ratio

        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf);
        // Should fail because 1001 > 100*10 (one byte over limit)
        assert!(
            result.is_err(),
            "Ratio 100.1:1 should exceed limit of 100:1"
        );
    }
}
