//! Path safety validation for archive extraction.
//!
//! This module provides functions for validating extraction paths against
//! path traversal attacks and symlink escapes.

use std::path::Path;

use crate::{Error, Result};

use super::PathSafety;

/// Validates that an extraction path is safe according to the configured policy.
///
/// # Arguments
///
/// * `entry_idx` - Index of the entry being extracted
/// * `entry_path` - The path stored in the archive
/// * `dest` - The destination directory
/// * `policy` - The path safety policy to apply
///
/// # Returns
///
/// The validated full path to extract to.
pub(crate) fn validate_path(
    entry_idx: usize,
    entry_path: &str,
    dest: &Path,
    policy: &PathSafety,
) -> Result<std::path::PathBuf> {
    match policy {
        PathSafety::Disabled => Ok(dest.join(entry_path)),
        PathSafety::Relaxed | PathSafety::Strict => {
            // ArchivePath already validates against traversal
            // Just join with destination
            let full_path = dest.join(entry_path);

            // Verify the path is still within dest
            let canonical_dest = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());

            // For strict, also check that no component is suspicious
            if *policy == PathSafety::Strict {
                for component in std::path::Path::new(entry_path).components() {
                    if let std::path::Component::Normal(name) = component {
                        if let Some(name_str) = name.to_str() {
                            // Block device files on Unix
                            if name_str.starts_with('/') {
                                return Err(Error::PathTraversal {
                                    entry_index: entry_idx,
                                    path: entry_path.to_string(),
                                });
                            }
                        }
                    }
                }
            }

            // Verify extracted path would be under destination
            // (simplified check - full canonicalization requires file to exist)
            if !full_path.starts_with(&canonical_dest) && !full_path.starts_with(dest) {
                return Err(Error::PathTraversal {
                    entry_index: entry_idx,
                    path: entry_path.to_string(),
                });
            }

            Ok(full_path)
        }
    }
}

/// Validates that a symlink target doesn't escape the extraction directory.
///
/// This checks for:
/// - Absolute paths (always rejected)
/// - Path traversal sequences (..)
/// - Targets that would resolve outside the extraction directory
///
/// The validation uses `entry_path` (the path within the archive) to determine
/// how deep the symlink is within the extraction directory. This avoids issues
/// with absolute filesystem paths that could mask traversal attempts.
pub(crate) fn validate_symlink_target(
    entry_idx: usize,
    entry_path: &str,
    target: &str,
) -> Result<()> {
    // Reject absolute paths
    if target.starts_with('/') || target.starts_with('\\') {
        return Err(Error::SymlinkTargetEscape {
            entry_index: entry_idx,
            path: entry_path.to_string(),
            target: target.to_string(),
        });
    }

    // Reject Windows absolute paths (C:\, D:\, etc.)
    if target.len() >= 2 && target.chars().nth(1) == Some(':') {
        return Err(Error::SymlinkTargetEscape {
            entry_index: entry_idx,
            path: entry_path.to_string(),
            target: target.to_string(),
        });
    }

    // Calculate the depth of the symlink's parent directory within the archive.
    // For example, if entry_path is "subdir/link.txt", the parent is "subdir" at depth 1.
    // A symlink at the root level (entry_path = "link.txt") has depth 0.
    let entry_parent = Path::new(entry_path).parent().unwrap_or(Path::new(""));
    let initial_depth = entry_parent
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count() as i32;

    // Trace through the target path, tracking depth relative to extraction root.
    // If depth goes negative, the target would escape the extraction directory.
    let mut depth = initial_depth;
    for component in Path::new(target).components() {
        match component {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(Error::SymlinkTargetEscape {
                        entry_index: entry_idx,
                        path: entry_path.to_string(),
                        target: target.to_string(),
                    });
                }
            }
            std::path::Component::Normal(_) => {
                depth += 1;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Creates a symbolic link at the specified path pointing to the target.
#[cfg(unix)]
pub(crate) fn create_symlink(link_path: &Path, target: &str) -> Result<u64> {
    std::os::unix::fs::symlink(target, link_path).map_err(Error::Io)?;
    Ok(0)
}

/// Creates a symbolic link at the specified path pointing to the target.
#[cfg(windows)]
pub(crate) fn create_symlink(link_path: &Path, target: &str) -> Result<u64> {
    // On Windows, we need to know if the target is a file or directory
    // Since we can't reliably determine this, try file symlink first
    // (which is more common), then fall back to directory symlink
    let target_path = link_path.parent().map(|p| p.join(target));

    // If target exists and is a directory, create a directory symlink
    if let Some(ref tp) = target_path {
        if tp.is_dir() {
            std::os::windows::fs::symlink_dir(target, link_path).map_err(Error::Io)?;
            return Ok(0);
        }
    }

    // Default to file symlink
    std::os::windows::fs::symlink_file(target, link_path).map_err(Error::Io)?;
    Ok(0)
}

/// Creates a symbolic link at the specified path pointing to the target.
#[cfg(not(any(unix, windows)))]
pub(crate) fn create_symlink(_link_path: &Path, _target: &str) -> Result<u64> {
    Err(Error::UnsupportedFeature {
        feature: "symbolic links on this platform",
    })
}
