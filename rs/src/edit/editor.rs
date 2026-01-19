//! Archive editor for modifying existing archives.

use std::collections::HashSet;
use std::io::{Read, Seek, Write};

use crate::read::Archive;
use crate::write::{WriteOptions, Writer};
use crate::{ArchivePath, Error, Result};

use super::operation::Operation;

/// Result of an edit operation.
#[must_use = "edit result should be checked to verify operation completed as expected"]
#[derive(Debug, Clone, Default)]
pub struct EditResult {
    /// Number of entries that were kept unchanged (copied raw).
    pub entries_kept: usize,
    /// Number of entries that were renamed.
    pub entries_renamed: usize,
    /// Number of entries that were deleted.
    pub entries_deleted: usize,
    /// Number of entries that were updated.
    pub entries_updated: usize,
    /// Number of new entries added.
    pub entries_added: usize,
    /// Total bytes in the new archive.
    pub total_bytes: u64,
    /// Packed (compressed) bytes in the new archive.
    pub packed_bytes: u64,
}

impl EditResult {
    /// Returns the total number of entries in the resulting archive.
    pub fn total_entries(&self) -> usize {
        self.entries_kept + self.entries_renamed + self.entries_updated + self.entries_added
    }

    /// Returns the compression ratio (packed / total).
    pub fn compression_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            1.0
        } else {
            self.packed_bytes as f64 / self.total_bytes as f64
        }
    }
}

/// An editor for modifying archive contents.
///
/// Operations are queued and only applied when `apply` is called.
/// This allows for efficient batch modifications.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::{Archive, ArchivePath};
/// use zesven::edit::ArchiveEditor;
///
/// let archive = Archive::open_path("original.7z")?;
/// let mut editor = ArchiveEditor::new(archive);
///
/// // Queue modifications
/// editor.rename("old_name.txt", "new_name.txt")?;
/// editor.delete("unwanted.txt")?;
/// editor.add(ArchivePath::new("new_file.txt")?, b"Hello!")?;
///
/// // Apply all changes to a new file
/// let mut output = File::create("modified.7z")?;
/// let result = editor.apply(&mut output)?;
/// println!("Kept {} entries, added {}", result.entries_kept, result.entries_added);
/// ```
pub struct ArchiveEditor<R: Read + Seek> {
    archive: Archive<R>,
    operations: Vec<Operation>,
    options: WriteOptions,
}

impl<R: Read + Seek> ArchiveEditor<R> {
    /// Creates a new editor for the given archive.
    pub fn new(archive: Archive<R>) -> Self {
        Self {
            archive,
            operations: Vec::new(),
            options: WriteOptions::default(),
        }
    }

    /// Sets the write options for the output archive.
    pub fn with_options(mut self, options: WriteOptions) -> Self {
        self.options = options;
        self
    }

    /// Returns the number of pending operations.
    pub fn pending_operations(&self) -> usize {
        self.operations.len()
    }

    /// Returns whether there are any pending operations.
    pub fn has_pending_operations(&self) -> bool {
        !self.operations.is_empty()
    }

    /// Clears all pending operations.
    pub fn clear_operations(&mut self) {
        self.operations.clear();
    }

    /// Queues a rename operation.
    ///
    /// The entry will be renamed in the output archive without recompression.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let from_path = ArchivePath::new(from)?;
        let to_path = ArchivePath::new(to)?;

        // Verify the source exists
        if !self.entry_exists(&from_path) {
            return Err(Error::EntryNotFound {
                path: from.to_string(),
            });
        }

        // Check for duplicate target
        if self.entry_exists(&to_path) {
            return Err(Error::EntryExists {
                path: to.to_string(),
            });
        }

        self.operations.push(Operation::Rename {
            from: from_path,
            to: to_path,
        });
        Ok(())
    }

    /// Queues a delete operation.
    ///
    /// The entry will be excluded from the output archive.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let archive_path = ArchivePath::new(path)?;

        // Verify the entry exists
        if !self.entry_exists(&archive_path) {
            return Err(Error::EntryNotFound {
                path: path.to_string(),
            });
        }

        self.operations
            .push(Operation::Delete { path: archive_path });
        Ok(())
    }

    /// Queues an update operation.
    ///
    /// The entry will be replaced with new data in the output archive.
    pub fn update(&mut self, path: &str, data: impl Into<Vec<u8>>) -> Result<()> {
        let archive_path = ArchivePath::new(path)?;

        // Verify the entry exists
        if !self.entry_exists(&archive_path) {
            return Err(Error::EntryNotFound {
                path: path.to_string(),
            });
        }

        self.operations.push(Operation::Update {
            path: archive_path,
            data: data.into(),
        });
        Ok(())
    }

    /// Queues an add operation.
    ///
    /// A new entry will be added to the output archive.
    pub fn add(&mut self, path: ArchivePath, data: impl Into<Vec<u8>>) -> Result<()> {
        // Check for duplicate
        if self.entry_exists(&path) {
            return Err(Error::EntryExists {
                path: path.as_str().to_string(),
            });
        }

        self.operations.push(Operation::Add {
            path,
            data: data.into(),
        });
        Ok(())
    }

    /// Applies all pending operations and writes to the output.
    ///
    /// This creates a new archive with all modifications applied.
    /// Unchanged entries are copied efficiently (without recompression when possible).
    pub fn apply<W: Write + Seek>(mut self, output: W) -> Result<EditResult> {
        let mut result = EditResult::default();

        // Build sets for quick lookup
        let deleted_paths = self.collect_deleted_paths();
        let renamed_paths = self.collect_renamed_paths();
        let updated_paths = self.collect_updated_paths();

        // Collect entry information first to avoid borrow issues
        let entry_infos: Vec<_> = self
            .archive
            .entries()
            .iter()
            .enumerate()
            .map(|(idx, e)| (idx, e.path.clone(), e.is_directory))
            .collect();

        // Create writer for output
        let mut writer = Writer::create(output)?.options(self.options.clone());

        // Process existing entries
        for (entry_idx, entry_path, is_directory) in entry_infos {
            let path_str = entry_path.as_str();

            // Skip deleted entries
            if deleted_paths.contains(path_str) {
                result.entries_deleted += 1;
                continue;
            }

            // Skip directories (they are recreated automatically)
            if is_directory {
                continue;
            }

            // Handle renamed entries
            if let Some(new_path) = renamed_paths.get(path_str) {
                // Read and re-add with new name
                let data = self.archive.extract_entry_to_vec_by_index(entry_idx)?;
                writer.add_bytes(new_path.clone(), &data)?;
                result.entries_renamed += 1;
                result.total_bytes += data.len() as u64;
                continue;
            }

            // Handle updated entries
            if let Some(new_data) = updated_paths.get(path_str) {
                writer.add_bytes(entry_path.clone(), new_data)?;
                result.entries_updated += 1;
                result.total_bytes += new_data.len() as u64;
                continue;
            }

            // Copy unchanged entry
            let data = self.archive.extract_entry_to_vec_by_index(entry_idx)?;
            writer.add_bytes(entry_path.clone(), &data)?;
            result.entries_kept += 1;
            result.total_bytes += data.len() as u64;
        }

        // Add new entries
        for op in &self.operations {
            if let Operation::Add { path, data } = op {
                writer.add_bytes(path.clone(), data)?;
                result.entries_added += 1;
                result.total_bytes += data.len() as u64;
            }
        }

        // Finish writing
        let (write_result, _) = writer.finish_into_inner()?;
        result.packed_bytes = write_result.compressed_size;

        Ok(result)
    }

    /// Checks if an entry exists in the archive or is being added.
    fn entry_exists(&self, path: &ArchivePath) -> bool {
        // Check original archive
        let path_str = path.as_str();
        for entry in self.archive.entries() {
            if entry.path.as_str() == path_str {
                return true;
            }
        }

        // Check pending additions
        for op in &self.operations {
            if let Operation::Add { path: add_path, .. } = op {
                if add_path.as_str() == path_str {
                    return true;
                }
            }
        }

        false
    }

    /// Collects paths that are being deleted.
    fn collect_deleted_paths(&self) -> HashSet<String> {
        self.operations
            .iter()
            .filter_map(|op| {
                if let Operation::Delete { path } = op {
                    Some(path.as_str().to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collects paths that are being renamed (old_path -> new_path).
    fn collect_renamed_paths(&self) -> std::collections::HashMap<String, ArchivePath> {
        self.operations
            .iter()
            .filter_map(|op| {
                if let Operation::Rename { from, to } = op {
                    Some((from.as_str().to_string(), to.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collects paths that are being updated (path -> new_data).
    fn collect_updated_paths(&self) -> std::collections::HashMap<String, Vec<u8>> {
        self.operations
            .iter()
            .filter_map(|op| {
                if let Operation::Update { path, data } = op {
                    Some((path.as_str().to_string(), data.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Trait extension for Archive to enable editing.
pub trait EditableArchive<R: Read + Seek>: Sized {
    /// Creates an editor for this archive.
    fn edit(self) -> ArchiveEditor<R>;
}

impl<R: Read + Seek> EditableArchive<R> for Archive<R> {
    fn edit(self) -> ArchiveEditor<R> {
        ArchiveEditor::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_result_defaults() {
        let result = EditResult::default();
        assert_eq!(result.total_entries(), 0);
        assert_eq!(result.compression_ratio(), 1.0);
    }

    #[test]
    fn test_edit_result_total_entries() {
        let result = EditResult {
            entries_kept: 5,
            entries_renamed: 2,
            entries_updated: 1,
            entries_added: 3,
            ..Default::default()
        };
        assert_eq!(result.total_entries(), 11);
    }

    #[test]
    fn test_edit_result_compression_ratio() {
        let result = EditResult {
            total_bytes: 1000,
            packed_bytes: 500,
            ..Default::default()
        };
        assert!((result.compression_ratio() - 0.5).abs() < 0.001);
    }
}
