//! Extraction destination abstractions.
//!
//! This module provides the [`ExtractDestination`] trait for pluggable extraction
//! destinations, along with built-in implementations for common use cases.
//!
//! # Built-in Destinations
//!
//! - [`FilesystemDestination`] - Extracts to filesystem paths (default behavior)
//! - [`MemoryDestination`] - Extracts to in-memory buffers
//! - [`NullDestination`] - Discards extracted data (for testing/benchmarking)
//!
//! # Custom Destinations
//!
//! You can implement [`ExtractDestination`] for custom extraction targets:
//!
//! ```rust,ignore
//! use zesven::read::{Entry, ExtractDestination};
//! use std::io::Write;
//!
//! struct S3Destination {
//!     bucket: String,
//!     prefix: String,
//! }
//!
//! impl ExtractDestination for S3Destination {
//!     fn create_writer(&mut self, entry: &Entry) -> zesven::Result<Box<dyn Write + Send>> {
//!         // Create S3 upload writer
//!         // ...
//!     }
//!
//!     fn on_complete(&mut self, entry: &Entry, success: bool) -> zesven::Result<()> {
//!         // Finalize S3 upload
//!         // ...
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::read::Entry;
use crate::{Error, Result};

/// Trait for extraction destinations.
///
/// This trait abstracts where extracted data is written, enabling extraction
/// to various targets: filesystem, memory, network storage, etc.
pub trait ExtractDestination: Send {
    /// Creates a writer for the given entry.
    ///
    /// This is called before extracting each file entry. The returned writer
    /// receives the decompressed file data.
    ///
    /// For directory entries, this method is not called (use [`Self::on_directory`] instead).
    ///
    /// # Arguments
    ///
    /// * `entry` - The archive entry being extracted
    ///
    /// # Returns
    ///
    /// A boxed writer that will receive the entry's data.
    fn create_writer(&mut self, entry: &Entry) -> Result<Box<dyn Write + Send>>;

    /// Called when extraction of an entry completes.
    ///
    /// This is called after all data has been written to the writer returned
    /// by [`Self::create_writer`]. Use this to finalize the extraction (e.g., set
    /// file permissions, close network connections).
    ///
    /// # Arguments
    ///
    /// * `entry` - The archive entry that was extracted
    /// * `success` - Whether extraction succeeded without errors
    fn on_complete(&mut self, entry: &Entry, success: bool) -> Result<()>;

    /// Called when a directory entry is encountered.
    ///
    /// The default implementation does nothing. Override to create directories
    /// or perform other directory-specific handling.
    ///
    /// # Arguments
    ///
    /// * `entry` - The directory entry
    fn on_directory(&mut self, entry: &Entry) -> Result<()> {
        let _ = entry;
        Ok(())
    }

    /// Called before extraction begins.
    ///
    /// This is called once before any entries are extracted. Use this for
    /// setup or initialization.
    ///
    /// # Arguments
    ///
    /// * `total_entries` - Total number of entries to be extracted
    fn on_start(&mut self, total_entries: usize) -> Result<()> {
        let _ = total_entries;
        Ok(())
    }

    /// Called after all extraction completes.
    ///
    /// This is called once after all entries have been extracted. Use this
    /// for cleanup or finalization.
    ///
    /// # Arguments
    ///
    /// * `success` - Whether all extractions succeeded
    fn on_finish(&mut self, success: bool) -> Result<()> {
        let _ = success;
        Ok(())
    }
}

/// Filesystem extraction destination.
///
/// Extracts entries to files in a specified output directory. This is the
/// default extraction behavior.
///
/// # Features
///
/// - Creates directories as needed
/// - Optionally sets Unix permissions
/// - Handles path traversal protection
///
/// # Example
///
/// ```rust,ignore
/// use zesven::read::{Archive, FilesystemDestination};
///
/// let archive = Archive::open("archive.7z")?;
/// let mut dest = FilesystemDestination::new("./output");
/// archive.extract_to_destination(&mut dest)?;
/// ```
pub struct FilesystemDestination {
    /// Output directory root
    output_dir: PathBuf,
    /// Whether to preserve file permissions
    preserve_permissions: bool,
    /// Currently open file path (for cleanup on failure)
    current_path: Option<PathBuf>,
    /// Entry index for error reporting
    current_entry_index: usize,
}

impl FilesystemDestination {
    /// Creates a new filesystem destination.
    ///
    /// # Arguments
    ///
    /// * `output_dir` - Directory where files will be extracted
    pub fn new(output_dir: impl AsRef<Path>) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
            preserve_permissions: true,
            current_path: None,
            current_entry_index: 0,
        }
    }

    /// Sets whether to preserve file permissions (Unix only).
    pub fn preserve_permissions(mut self, preserve: bool) -> Self {
        self.preserve_permissions = preserve;
        self
    }

    fn resolve_path(&self, entry: &Entry) -> Result<PathBuf> {
        let entry_path = Path::new(entry.path.as_str());

        // Check for path traversal
        let resolved = self.output_dir.join(entry_path);
        let canonical_output = self
            .output_dir
            .canonicalize()
            .unwrap_or_else(|_| self.output_dir.clone());

        // Ensure the resolved path is within output_dir
        let mut check_path = resolved.clone();
        while !check_path.exists() {
            if let Some(parent) = check_path.parent() {
                check_path = parent.to_path_buf();
            } else {
                break;
            }
        }

        if check_path.exists() {
            let canonical_resolved = check_path.canonicalize().map_err(Error::Io)?;
            if !canonical_resolved.starts_with(&canonical_output) {
                return Err(Error::PathTraversal {
                    entry_index: self.current_entry_index,
                    path: entry.path.as_str().to_string(),
                });
            }
        }

        Ok(resolved)
    }
}

impl ExtractDestination for FilesystemDestination {
    fn create_writer(&mut self, entry: &Entry) -> Result<Box<dyn Write + Send>> {
        self.current_entry_index = entry.index;
        let path = self.resolve_path(entry)?;

        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(Error::Io)?;
        }

        // Create the file
        let file = File::create(&path).map_err(Error::Io)?;
        self.current_path = Some(path);

        Ok(Box::new(file))
    }

    fn on_complete(&mut self, entry: &Entry, success: bool) -> Result<()> {
        if !success {
            // Remove partially extracted file
            if let Some(path) = self.current_path.take() {
                if let Err(e) = fs::remove_file(&path) {
                    log::warn!(
                        "Failed to clean up partial file '{}': {}",
                        path.display(),
                        e
                    );
                }
            }
            return Ok(());
        }

        #[cfg(unix)]
        {
            let path = match self.current_path.take() {
                Some(p) => p,
                None => return Ok(()),
            };

            // Set Unix permissions
            if self.preserve_permissions {
                if let Some(mode) = entry.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(mode)) {
                        log::warn!("Failed to set permissions on '{}': {}", path.display(), e);
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            self.current_path.take();
            let _ = entry;
        }

        Ok(())
    }

    fn on_directory(&mut self, entry: &Entry) -> Result<()> {
        self.current_entry_index = entry.index;
        let path = self.resolve_path(entry)?;
        fs::create_dir_all(&path).map_err(Error::Io)?;
        Ok(())
    }
}

/// In-memory extraction destination.
///
/// Extracts entries to in-memory buffers. Useful for processing archive
/// contents without writing to disk.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::read::{Archive, MemoryDestination};
///
/// let archive = Archive::open("archive.7z")?;
/// let mut dest = MemoryDestination::new();
/// archive.extract_to_destination(&mut dest)?;
///
/// // Access extracted data
/// for (path, data) in dest.files() {
///     println!("{}: {} bytes", path, data.len());
/// }
/// ```
pub struct MemoryDestination {
    /// Extracted file contents by path
    files: HashMap<String, Vec<u8>>,
    /// Current entry path being written
    current_path: Option<String>,
    /// Current buffer being written to (shared with writer)
    current_buffer: Option<Arc<Mutex<Vec<u8>>>>,
}

impl MemoryDestination {
    /// Creates a new memory destination.
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            current_path: None,
            current_buffer: None,
        }
    }

    /// Returns the extracted files.
    ///
    /// Returns a map from entry paths to their extracted contents.
    pub fn files(&self) -> &HashMap<String, Vec<u8>> {
        &self.files
    }

    /// Takes ownership of the extracted files.
    ///
    /// Consumes the destination and returns the extracted file contents.
    pub fn into_files(self) -> HashMap<String, Vec<u8>> {
        self.files
    }

    /// Gets the extracted content for a specific path.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(|v| v.as_slice())
    }

    /// Returns the number of extracted files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns true if no files have been extracted.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl Default for MemoryDestination {
    fn default() -> Self {
        Self::new()
    }
}

/// Writer wrapper that captures data to a shared buffer.
///
/// Uses `Arc<Mutex<Vec<u8>>>` to allow the destination to retrieve
/// the written data after the writer is dropped.
struct SharedBufferWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl SharedBufferWriter {
    fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { buffer }
    }
}

impl Write for SharedBufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self
            .buffer
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl ExtractDestination for MemoryDestination {
    fn create_writer(&mut self, entry: &Entry) -> Result<Box<dyn Write + Send>> {
        self.current_path = Some(entry.path.as_str().to_string());

        // Create a shared buffer that will be accessible after the writer is dropped
        let buffer = Arc::new(Mutex::new(Vec::with_capacity(entry.size as usize)));
        self.current_buffer = Some(Arc::clone(&buffer));

        Ok(Box::new(SharedBufferWriter::new(buffer)))
    }

    fn on_complete(&mut self, _entry: &Entry, success: bool) -> Result<()> {
        let path = self.current_path.take();
        let buffer = self.current_buffer.take();

        if success {
            if let (Some(path), Some(buffer)) = (path, buffer) {
                // Extract data from the shared buffer
                let data = Arc::try_unwrap(buffer)
                    .map(|mutex| mutex.into_inner().unwrap_or_default())
                    .unwrap_or_else(|arc| {
                        // Buffer is still referenced elsewhere, clone the data
                        arc.lock().map(|guard| guard.clone()).unwrap_or_default()
                    });
                self.files.insert(path, data);
            }
        }

        Ok(())
    }
}

/// Null extraction destination.
///
/// Discards all extracted data. Useful for testing, benchmarking, or
/// validating archive integrity without writing anywhere.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::read::{Archive, NullDestination};
///
/// let archive = Archive::open("archive.7z")?;
/// let mut dest = NullDestination::new();
/// // Validate archive by extracting to null
/// archive.extract_to_destination(&mut dest)?;
/// println!("Validated {} entries", dest.entries_processed());
/// ```
pub struct NullDestination {
    /// Number of entries processed
    entries_processed: usize,
    /// Total bytes discarded
    bytes_discarded: u64,
}

impl NullDestination {
    /// Creates a new null destination.
    pub fn new() -> Self {
        Self {
            entries_processed: 0,
            bytes_discarded: 0,
        }
    }

    /// Returns the number of entries processed.
    pub fn entries_processed(&self) -> usize {
        self.entries_processed
    }

    /// Returns the total bytes discarded.
    pub fn bytes_discarded(&self) -> u64 {
        self.bytes_discarded
    }
}

impl Default for NullDestination {
    fn default() -> Self {
        Self::new()
    }
}

/// Writer that discards all data.
struct NullWriter {
    bytes_written: u64,
}

impl NullWriter {
    fn new() -> Self {
        Self { bytes_written: 0 }
    }
}

impl Write for NullWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes_written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl ExtractDestination for NullDestination {
    fn create_writer(&mut self, _entry: &Entry) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(NullWriter::new()))
    }

    fn on_complete(&mut self, entry: &Entry, success: bool) -> Result<()> {
        if success {
            self.entries_processed += 1;
            self.bytes_discarded += entry.size;
        }
        Ok(())
    }

    fn on_directory(&mut self, _entry: &Entry) -> Result<()> {
        self.entries_processed += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchivePath;

    fn make_entry(path: &str, is_dir: bool, size: u64) -> Entry {
        Entry {
            path: ArchivePath::new(path).unwrap(),
            is_directory: is_dir,
            size,
            crc32: None,
            crc64: None,
            modification_time: None,
            creation_time: None,
            access_time: None,
            attributes: None,
            is_encrypted: false,
            is_symlink: false,
            is_anti: false,
            ownership: None,
            index: 0,
            folder_index: None,
            stream_index: None,
        }
    }

    #[test]
    fn test_null_destination() {
        let mut dest = NullDestination::new();

        let entry = make_entry("test.txt", false, 100);
        let mut writer = dest.create_writer(&entry).unwrap();
        writer.write_all(b"test data").unwrap();
        dest.on_complete(&entry, true).unwrap();

        assert_eq!(dest.entries_processed(), 1);
        assert_eq!(dest.bytes_discarded(), 100);
    }

    #[test]
    fn test_memory_destination() {
        let dest = MemoryDestination::new();
        assert!(dest.is_empty());
        assert_eq!(dest.len(), 0);
    }

    #[test]
    fn test_filesystem_destination_creation() {
        let dest = FilesystemDestination::new("/tmp/output");
        assert_eq!(dest.output_dir, PathBuf::from("/tmp/output"));
    }
}
