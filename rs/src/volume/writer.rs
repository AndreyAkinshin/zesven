//! Multi-volume archive writer.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;

use super::VolumeConfig;
use crate::{Error, Result};

/// A writer that automatically splits output across multiple volume files.
///
/// This writer creates a new volume file when the current volume reaches
/// the configured size limit, seamlessly handling the transition.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::volume::{VolumeConfig, MultiVolumeWriter};
///
/// let config = VolumeConfig::new("archive.7z", 50 * 1024 * 1024); // 50 MB volumes
/// let mut writer = MultiVolumeWriter::create(config)?;
///
/// // Write data - automatically splits across volumes
/// writer.write_all(&large_data)?;
///
/// // Finish and get volume sizes
/// let sizes = writer.finish()?;
/// println!("Created {} volumes", sizes.len());
/// ```
pub struct MultiVolumeWriter {
    /// Configuration for volume generation.
    config: VolumeConfig,
    /// First volume file (kept open for header updates).
    first_volume: Option<BufWriter<File>>,
    /// Current volume file (if not volume 1).
    current_file: Option<BufWriter<File>>,
    /// Current volume number (1-indexed).
    current_volume: u32,
    /// Bytes written to current volume.
    current_volume_written: u64,
    /// Bytes written to first volume.
    first_volume_written: u64,
    /// Total bytes written across all volumes.
    total_written: u64,
    /// Sizes of completed volumes.
    completed_sizes: Vec<u64>,
    /// Current seek position in first volume (for header rewrites).
    first_volume_position: u64,
    /// Whether we're in header rewrite mode (seeking back into first volume).
    header_rewrite_mode: bool,
}

impl MultiVolumeWriter {
    /// Creates a new multi-volume writer.
    ///
    /// # Arguments
    ///
    /// * `config` - Volume configuration specifying size and base path
    ///
    /// # Errors
    ///
    /// Returns an error if the first volume file cannot be created.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::volume::{VolumeConfig, MultiVolumeWriter};
    ///
    /// let config = VolumeConfig::new("output.7z", 100 * 1024 * 1024);
    /// let writer = MultiVolumeWriter::create(config)?;
    /// ```
    pub fn create(config: VolumeConfig) -> Result<Self> {
        let path = config.volume_path(1);
        let file = File::create(&path).map_err(|e| {
            Error::Io(io::Error::new(
                e.kind(),
                format!("Failed to create volume {}: {}", path.display(), e),
            ))
        })?;

        Ok(Self {
            config,
            first_volume: Some(BufWriter::new(file)),
            current_file: None,
            current_volume: 1,
            current_volume_written: 0,
            first_volume_written: 0,
            total_written: 0,
            completed_sizes: Vec::new(),
            first_volume_position: 0,
            header_rewrite_mode: false,
        })
    }

    /// Opens the next volume file.
    fn open_next_volume(&mut self) -> Result<()> {
        // Close current volume if open (but not first volume - it stays open)
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            self.completed_sizes.push(self.current_volume_written);
        }

        // If we're moving from volume 1, record its size
        if self.current_volume == 1 {
            // Flush first volume but don't close it
            if let Some(ref mut first) = self.first_volume {
                first.flush()?;
            }
            self.first_volume_written = self.current_volume_written;
            self.completed_sizes.push(self.current_volume_written);
        }

        // Move to next volume
        self.current_volume += 1;

        let path = self.config.volume_path(self.current_volume);
        let file = File::create(&path).map_err(|e| {
            Error::Io(io::Error::new(
                e.kind(),
                format!("Failed to create volume {}: {}", path.display(), e),
            ))
        })?;

        self.current_file = Some(BufWriter::new(file));
        self.current_volume_written = 0;

        Ok(())
    }

    /// Switches to the next volume.
    fn switch_to_next_volume(&mut self) -> Result<()> {
        self.open_next_volume()
    }

    /// Returns the current volume number (1-indexed).
    pub fn current_volume(&self) -> u32 {
        self.current_volume
    }

    /// Returns the total number of volumes created so far.
    pub fn volume_count(&self) -> u32 {
        self.current_volume
    }

    /// Returns the sizes of all completed volumes.
    pub fn completed_sizes(&self) -> &[u64] {
        &self.completed_sizes
    }

    /// Returns the total bytes written across all volumes.
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// Returns the bytes written to the current volume.
    pub fn current_volume_written(&self) -> u64 {
        self.current_volume_written
    }

    /// Returns the remaining space in the current volume.
    pub fn remaining_in_volume(&self) -> u64 {
        self.config
            .volume_size()
            .saturating_sub(self.current_volume_written)
    }

    /// Returns the path of the current volume.
    pub fn current_volume_path(&self) -> PathBuf {
        self.config.volume_path(self.current_volume)
    }

    /// Finishes writing and returns the sizes of all volumes.
    ///
    /// This flushes and closes all volume files.
    ///
    /// # Returns
    ///
    /// A vector containing the size of each volume in bytes.
    pub fn finish(mut self) -> Result<Vec<u64>> {
        // Flush and close first volume
        if let Some(mut file) = self.first_volume.take() {
            file.flush()?;
            // If we never left volume 1, use current_volume_written
            if self.current_volume == 1 {
                self.completed_sizes.push(self.current_volume_written);
            }
        }

        // Flush and close current volume (if not volume 1)
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            self.completed_sizes.push(self.current_volume_written);
        }

        Ok(self.completed_sizes)
    }
}

impl Write for MultiVolumeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Handle header rewrite mode - writing to a previously-written position in first volume
        if self.header_rewrite_mode {
            let file = self
                .first_volume
                .as_mut()
                .ok_or_else(|| io::Error::other("First volume file not open"))?;
            let n = file.write(buf)?;
            self.first_volume_position += n as u64;
            return Ok(n);
        }

        let remaining_in_volume = self
            .config
            .volume_size()
            .saturating_sub(self.current_volume_written);

        if remaining_in_volume == 0 {
            // Current volume is full, switch to next
            self.switch_to_next_volume().map_err(io::Error::other)?;
            return self.write(buf);
        }

        // Write what we can to the current volume
        let to_write = buf.len().min(remaining_in_volume as usize);

        let n = if self.current_volume == 1 {
            // Write to first volume
            let file = self
                .first_volume
                .as_mut()
                .ok_or_else(|| io::Error::other("First volume file not open"))?;
            file.write(&buf[..to_write])?
        } else {
            // Write to current volume
            let file = self
                .current_file
                .as_mut()
                .ok_or_else(|| io::Error::other("Current volume file not open"))?;
            file.write(&buf[..to_write])?
        };

        self.current_volume_written += n as u64;
        self.total_written += n as u64;

        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.first_volume.as_mut() {
            file.flush()?;
        }
        if let Some(file) = self.current_file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

impl Seek for MultiVolumeWriter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        // Multi-volume writer has limited seek support:
        // - Seeking within first volume (for header rewriting) - always allowed
        // - Getting current position
        match pos {
            SeekFrom::Start(p) => {
                // Seek within first volume
                let max_pos = if self.current_volume == 1 {
                    self.current_volume_written
                } else {
                    self.first_volume_written
                };

                if p <= max_pos {
                    if let Some(file) = self.first_volume.as_mut() {
                        file.seek(SeekFrom::Start(p))?;

                        // Only enter header rewrite mode if seeking backward
                        // (not at the append position)
                        if self.current_volume == 1 && p == self.current_volume_written {
                            // Seeking to current position - stay in normal mode
                            self.header_rewrite_mode = false;
                        } else {
                            // Seeking backward - enter header rewrite mode
                            self.first_volume_position = p;
                            self.header_rewrite_mode = true;
                        }
                        Ok(p)
                    } else {
                        Err(io::Error::other("First volume file not open"))
                    }
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "Cannot seek to position {} in first volume (max written: {})",
                            p, max_pos
                        ),
                    ))
                }
            }
            SeekFrom::Current(0) => {
                if self.header_rewrite_mode {
                    Ok(self.first_volume_position)
                } else {
                    Ok(self.total_written)
                }
            }
            SeekFrom::End(0) => Ok(self.total_written),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Multi-volume writer only supports seeking within first volume",
            )),
        }
    }
}

impl std::fmt::Debug for MultiVolumeWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiVolumeWriter")
            .field("config", &self.config)
            .field("current_volume", &self.current_volume)
            .field("current_volume_written", &self.current_volume_written)
            .field("total_written", &self.total_written)
            .field("completed_volumes", &self.completed_sizes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn test_create_single_volume() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 1024);

        let mut writer = MultiVolumeWriter::create(config).unwrap();

        // Write less than one volume
        let data = vec![42u8; 100];
        writer.write_all(&data).unwrap();

        let sizes = writer.finish().unwrap();
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes[0], 100);

        // Verify file exists
        let volume_path = PathBuf::from(format!("{}.001", base_path.display()));
        assert!(volume_path.exists());
    }

    #[test]
    fn test_create_multiple_volumes() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 100); // 100 byte volumes

        let mut writer = MultiVolumeWriter::create(config).unwrap();

        // Write 250 bytes - should create 3 volumes
        let data = vec![42u8; 250];
        writer.write_all(&data).unwrap();

        let sizes = writer.finish().unwrap();
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0], 100);
        assert_eq!(sizes[1], 100);
        assert_eq!(sizes[2], 50);

        // Verify all files exist
        for i in 1..=3 {
            let volume_path = PathBuf::from(format!("{}.{:03}", base_path.display(), i));
            assert!(volume_path.exists());
        }
    }

    #[test]
    fn test_write_across_boundaries() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 100);

        let mut writer = MultiVolumeWriter::create(config).unwrap();

        // Write data that spans boundaries
        for i in 0..5 {
            let data = vec![i as u8; 50];
            writer.write_all(&data).unwrap();
        }

        let sizes = writer.finish().unwrap();
        assert_eq!(sizes.len(), 3); // 250 bytes / 100 = 3 volumes

        // Verify data integrity by reading back
        for vol in 1..=3 {
            let volume_path = PathBuf::from(format!("{}.{:03}", base_path.display(), vol));
            let mut file = File::open(&volume_path).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            assert_eq!(data.len(), sizes[(vol - 1) as usize] as usize);
        }
    }

    #[test]
    fn test_volume_count_tracking() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 100);

        let mut writer = MultiVolumeWriter::create(config).unwrap();
        assert_eq!(writer.volume_count(), 1);
        assert_eq!(writer.current_volume(), 1);

        writer.write_all(&[0u8; 150]).unwrap();
        assert_eq!(writer.volume_count(), 2);
        assert_eq!(writer.current_volume(), 2);
    }

    #[test]
    fn test_total_written_tracking() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 100);

        let mut writer = MultiVolumeWriter::create(config).unwrap();

        writer.write_all(&[0u8; 50]).unwrap();
        assert_eq!(writer.total_written(), 50);

        writer.write_all(&[0u8; 100]).unwrap();
        assert_eq!(writer.total_written(), 150);

        assert_eq!(writer.current_volume_written(), 50);
    }

    #[test]
    fn test_remaining_in_volume() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 100);

        let mut writer = MultiVolumeWriter::create(config).unwrap();
        assert_eq!(writer.remaining_in_volume(), 100);

        writer.write_all(&[0u8; 30]).unwrap();
        assert_eq!(writer.remaining_in_volume(), 70);

        writer.write_all(&[0u8; 70]).unwrap();
        assert_eq!(writer.remaining_in_volume(), 0);
    }

    #[test]
    fn test_seek_to_start_first_volume() {
        let dir = TempDir::new().unwrap();
        let base_path = dir.path().join("test.7z");
        let config = VolumeConfig::new(&base_path, 1000);

        let mut writer = MultiVolumeWriter::create(config).unwrap();

        // Write some data
        writer.write_all(&[1u8; 100]).unwrap();

        // Seek to start and overwrite
        writer.seek(SeekFrom::Start(0)).unwrap();
        writer.write_all(&[2u8; 50]).unwrap();

        writer.finish().unwrap();

        // Read back and verify
        let volume_path = PathBuf::from(format!("{}.001", base_path.display()));
        let mut file = File::open(&volume_path).unwrap();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();

        // First 50 bytes should be 2s, remaining should be 1s
        assert_eq!(&data[0..50], &vec![2u8; 50][..]);
        assert_eq!(&data[50..100], &vec![1u8; 50][..]);
    }
}
