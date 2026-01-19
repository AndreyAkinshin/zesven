//! Archive editing and modification.
//!
//! This module provides the ability to modify existing 7z archives by:
//! - Renaming entries
//! - Deleting entries
//! - Updating entry contents
//! - Adding new entries
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::{Archive, ArchivePath};
//! use zesven::edit::{ArchiveEditor, EditableArchive};
//! use std::fs::File;
//!
//! // Open an existing archive
//! let archive = Archive::open_path("original.7z")?;
//!
//! // Create an editor
//! let mut editor = archive.edit();
//!
//! // Queue modifications
//! editor.rename("old_name.txt", "new_name.txt")?;
//! editor.delete("unwanted.txt")?;
//! editor.add(ArchivePath::new("new_file.txt")?, b"Hello, World!")?;
//!
//! // Apply changes to a new file
//! let output = File::create("modified.7z")?;
//! let result = editor.apply(output)?;
//!
//! println!("Kept {} entries, deleted {}, added {}",
//!          result.entries_kept,
//!          result.entries_deleted,
//!          result.entries_added);
//! ```
//!
//! # Implementation Notes
//!
//! The editor works by:
//! 1. Queueing all requested operations
//! 2. When `apply()` is called, iterating through the original archive
//! 3. Copying unchanged entries (currently via decompress/recompress)
//! 4. Applying renames by copying with new path
//! 5. Skipping deleted entries
//! 6. Adding updated and new entries
//!
//! Future optimizations may include raw stream copying for unchanged solid blocks.

mod editor;
mod operation;

pub use editor::{ArchiveEditor, EditResult, EditableArchive};
pub use operation::{Operation, OperationBuilder};
