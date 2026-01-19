//! Multi-volume archive reader.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::format::SIGNATURE_HEADER_SIZE;
use crate::format::header::StartHeader;
use crate::{Error, Result};

/// Trait for readers that can report volume information.
pub trait VolumeReader: Read + Seek {
    /// Returns the total number of volumes.
    fn volume_count(&self) -> u32;

    /// Returns the sizes of all volumes in bytes.
    fn volume_sizes(&self) -> &[u64];

    /// Returns the current volume number (1-indexed).
    fn current_volume(&self) -> u32;

    /// Returns the total logical size across all volumes.
    fn total_size(&self) -> u64;
}

/// A reader that seamlessly reads across multiple volume files.
///
/// This reader presents a unified view of a multi-volume archive,
/// automatically switching between volumes as needed during read operations.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::volume::MultiVolumeReader;
///
/// // Open the first volume - other volumes are discovered automatically
/// let mut reader = MultiVolumeReader::open("archive.7z.001")?;
///
/// // Read data seamlessly across volume boundaries
/// let mut buffer = vec![0u8; 1024];
/// reader.read_exact(&mut buffer)?;
/// ```
pub struct MultiVolumeReader {
    /// Volume file handles (opened lazily).
    volumes: Vec<Option<BufReader<File>>>,
    /// Size of each volume in bytes.
    volume_sizes: Vec<u64>,
    /// Base path for volumes (without .NNN extension).
    base_path: PathBuf,
    /// Current position in the logical stream.
    position: u64,
    /// Current volume index (0-based).
    current_volume: usize,
    /// Position within the current volume.
    volume_position: u64,
    /// Total size across all volumes.
    total_size: u64,
}

impl MultiVolumeReader {
    /// Opens a multi-volume archive.
    ///
    /// Accepts the path to the first volume (`.7z.001`) or the base path
    /// (`.7z`). All volumes are detected automatically.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the first volume or base path
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No volume files are found
    /// - The first volume cannot be read
    /// - The path format is not recognized
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::volume::MultiVolumeReader;
    ///
    /// // Open by first volume
    /// let reader = MultiVolumeReader::open("archive.7z.001")?;
    ///
    /// // Or by base path (if first volume exists)
    /// let reader = MultiVolumeReader::open("archive.7z")?;
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let base_path = Self::detect_base_path(path)?;
        let (volume_sizes, total_size) = Self::detect_volumes(&base_path)?;

        if volume_sizes.is_empty() {
            return Err(Error::InvalidFormat("No volume files found".to_string()));
        }

        // Validate that we have all required volumes by reading the signature header
        Self::validate_complete_archive(&base_path, &volume_sizes, total_size)?;

        Self::create_unchecked(base_path, volume_sizes, total_size)
    }

    /// Opens a multi-volume archive without validating completeness.
    ///
    /// This is useful for testing or when you know the archive is complete.
    #[cfg(test)]
    pub(crate) fn open_unchecked(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let base_path = Self::detect_base_path(path)?;
        let (volume_sizes, total_size) = Self::detect_volumes(&base_path)?;

        if volume_sizes.is_empty() {
            return Err(Error::InvalidFormat("No volume files found".to_string()));
        }

        Self::create_unchecked(base_path, volume_sizes, total_size)
    }

    /// Creates the reader with pre-computed volume information.
    fn create_unchecked(
        base_path: PathBuf,
        volume_sizes: Vec<u64>,
        total_size: u64,
    ) -> Result<Self> {
        let mut volumes = Vec::with_capacity(volume_sizes.len());
        for _ in 0..volume_sizes.len() {
            volumes.push(None);
        }

        Ok(Self {
            volumes,
            volume_sizes,
            base_path,
            position: 0,
            current_volume: 0,
            volume_position: 0,
            total_size,
        })
    }

    /// Validates that all required volumes are present.
    ///
    /// Reads the signature header from the first volume to determine the
    /// expected archive size, then checks if all volumes exist.
    fn validate_complete_archive(
        base_path: &Path,
        volume_sizes: &[u64],
        total_size: u64,
    ) -> Result<()> {
        // Open the first volume and read the signature header
        let first_volume_path = Self::volume_path_for(base_path, 1);
        let mut file = File::open(&first_volume_path).map_err(Error::Io)?;
        let start_header = StartHeader::parse(&mut file)?;

        // Calculate expected archive size
        let expected_size =
            SIGNATURE_HEADER_SIZE + start_header.next_header_offset + start_header.next_header_size;

        // Check if we have enough data
        if total_size < expected_size {
            // Find which volume is missing by calculating the expected volume number
            let mut cumulative: u64 = 0;
            for &size in volume_sizes.iter() {
                cumulative += size;
                if cumulative >= expected_size {
                    // All necessary volumes exist
                    return Ok(());
                }
            }

            // We're missing volumes - calculate which one
            let missing_volume = (volume_sizes.len() + 1) as u32;
            let missing_path = Self::volume_path_for(base_path, missing_volume);
            return Err(Error::VolumeMissing {
                volume: missing_volume,
                path: missing_path.to_string_lossy().to_string(),
                source: io::Error::new(io::ErrorKind::NotFound, "Volume file not found"),
            });
        }

        Ok(())
    }

    /// Detects the base path from various input formats.
    ///
    /// Handles:
    /// - `.7z.001`, `.7z.002`, etc. -> extracts base `.7z` path
    /// - `.7z` -> uses as-is
    fn detect_base_path(path: &Path) -> Result<PathBuf> {
        let path_str = path.to_string_lossy();

        // Handle .7z.NNN format
        if let Some(pos) = path_str.rfind(".7z.") {
            // Check if the part after .7z. is a number
            let suffix = &path_str[pos + 4..];
            if suffix.chars().all(|c| c.is_ascii_digit()) && !suffix.is_empty() {
                let base = &path_str[..pos + 3]; // Include .7z
                return Ok(PathBuf::from(base));
            }
        }

        // Handle plain .7z extension - check if .7z.001 exists
        if path_str.ends_with(".7z") {
            let first_volume = PathBuf::from(format!("{}.001", path_str));
            if first_volume.exists() {
                return Ok(path.to_path_buf());
            }
            // If .7z.001 doesn't exist, this might be a regular (non-multi-volume) archive
            return Err(Error::InvalidFormat(
                "Not a multi-volume archive (no .7z.001 found)".to_string(),
            ));
        }

        Err(Error::InvalidFormat(
            "Could not determine volume base path".to_string(),
        ))
    }

    /// Detects all volumes and their sizes.
    fn detect_volumes(base_path: &Path) -> Result<(Vec<u64>, u64)> {
        let mut sizes = Vec::new();
        let mut total = 0u64;
        let mut volume_num = 1u32;

        loop {
            let volume_path = Self::volume_path_for(base_path, volume_num);
            match std::fs::metadata(&volume_path) {
                Ok(meta) => {
                    let size = meta.len();
                    sizes.push(size);
                    total += size;
                    volume_num += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) => {
                    return Err(Error::Io(e));
                }
            }
        }

        Ok((sizes, total))
    }

    /// Generates volume path for a given base path and volume number.
    fn volume_path_for(base: &Path, num: u32) -> PathBuf {
        let base_str = base.to_string_lossy();
        PathBuf::from(format!("{}.{:03}", base_str, num))
    }

    /// Opens a volume file lazily.
    fn open_volume(&mut self, index: usize) -> Result<&mut BufReader<File>> {
        if self.volumes[index].is_none() {
            let path = Self::volume_path_for(&self.base_path, (index + 1) as u32);
            let file = File::open(&path).map_err(|e| Error::VolumeMissing {
                volume: (index + 1) as u32,
                path: path.to_string_lossy().to_string(),
                source: e,
            })?;
            self.volumes[index] = Some(BufReader::new(file));
        }
        Ok(self.volumes[index].as_mut().unwrap())
    }

    /// Calculates volume index and offset for a logical position.
    fn position_to_volume(&self, pos: u64) -> (usize, u64) {
        let mut remaining = pos;
        for (i, &size) in self.volume_sizes.iter().enumerate() {
            if remaining < size {
                return (i, remaining);
            }
            remaining -= size;
        }
        // Position is at or beyond end
        let last = self.volume_sizes.len().saturating_sub(1);
        (last, self.volume_sizes.get(last).copied().unwrap_or(0))
    }

    /// Returns the base path for this archive.
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Returns the path of a specific volume.
    pub fn get_volume_path(&self, volume_number: u32) -> PathBuf {
        Self::volume_path_for(&self.base_path, volume_number)
    }

    /// Checks if all volumes are present and accessible.
    pub fn verify_volumes(&self) -> Result<()> {
        for i in 0..self.volume_sizes.len() {
            let path = Self::volume_path_for(&self.base_path, (i + 1) as u32);
            if !path.exists() {
                return Err(Error::VolumeMissing {
                    volume: (i + 1) as u32,
                    path: path.to_string_lossy().to_string(),
                    source: io::Error::new(io::ErrorKind::NotFound, "Volume file not found"),
                });
            }
        }
        Ok(())
    }
}

impl Read for MultiVolumeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.total_size {
            return Ok(0);
        }

        let mut total_read = 0;
        let mut buf_offset = 0;

        while buf_offset < buf.len() && self.position < self.total_size {
            // Check if we need to move to the next volume
            let current_volume_size = self.volume_sizes[self.current_volume];
            let remaining_in_volume = current_volume_size - self.volume_position;

            if remaining_in_volume == 0 {
                // Move to next volume
                self.current_volume += 1;
                self.volume_position = 0;
                if self.current_volume >= self.volumes.len() {
                    break;
                }
                continue;
            }

            // Calculate how much to read before opening volume
            let to_read = (buf.len() - buf_offset).min(remaining_in_volume as usize);
            let seek_pos = self.volume_position;
            let current_vol = self.current_volume;

            // Open volume if needed and seek to position
            let volume = self.open_volume(current_vol).map_err(io::Error::other)?;
            volume.seek(SeekFrom::Start(seek_pos))?;

            // Read from current volume
            let n = volume.read(&mut buf[buf_offset..buf_offset + to_read])?;

            if n == 0 {
                // Unexpected end of volume
                break;
            }

            buf_offset += n;
            total_read += n;
            self.position += n as u64;
            self.volume_position += n as u64;
        }

        Ok(total_read)
    }
}

impl Seek for MultiVolumeReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(p) => self.total_size as i64 + p,
            SeekFrom::Current(p) => self.position as i64 + p,
        };

        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot seek before start of stream",
            ));
        }

        let new_pos = new_pos as u64;
        self.position = new_pos.min(self.total_size);

        let (vol_idx, vol_pos) = self.position_to_volume(self.position);
        self.current_volume = vol_idx;
        self.volume_position = vol_pos;

        Ok(self.position)
    }
}

impl VolumeReader for MultiVolumeReader {
    fn volume_count(&self) -> u32 {
        self.volume_sizes.len() as u32
    }

    fn volume_sizes(&self) -> &[u64] {
        &self.volume_sizes
    }

    fn current_volume(&self) -> u32 {
        (self.current_volume + 1) as u32
    }

    fn total_size(&self) -> u64 {
        self.total_size
    }
}

impl std::fmt::Debug for MultiVolumeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiVolumeReader")
            .field("base_path", &self.base_path)
            .field("volume_count", &self.volume_sizes.len())
            .field("total_size", &self.total_size)
            .field("position", &self.position)
            .field("current_volume", &(self.current_volume + 1))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_volumes(dir: &Path, base_name: &str, sizes: &[usize]) -> PathBuf {
        let base_path = dir.join(base_name);
        for (i, &size) in sizes.iter().enumerate() {
            let volume_path = PathBuf::from(format!("{}.{:03}", base_path.display(), i + 1));
            let mut file = File::create(&volume_path).unwrap();
            // Write sequential bytes so we can verify reading
            let data: Vec<u8> = (0..size).map(|j| ((i * 256 + j) % 256) as u8).collect();
            file.write_all(&data).unwrap();
        }
        base_path
    }

    #[test]
    fn test_detect_base_path() {
        // Test .7z.001 format
        let result = MultiVolumeReader::detect_base_path(Path::new("archive.7z.001"));
        assert_eq!(result.unwrap(), PathBuf::from("archive.7z"));

        // Test .7z.002 format
        let result = MultiVolumeReader::detect_base_path(Path::new("archive.7z.002"));
        assert_eq!(result.unwrap(), PathBuf::from("archive.7z"));

        // Test .7z.123 format
        let result = MultiVolumeReader::detect_base_path(Path::new("/path/to/archive.7z.123"));
        assert_eq!(result.unwrap(), PathBuf::from("/path/to/archive.7z"));
    }

    #[test]
    fn test_volume_path_generation() {
        let base = PathBuf::from("test.7z");
        assert_eq!(
            MultiVolumeReader::volume_path_for(&base, 1),
            PathBuf::from("test.7z.001")
        );
        assert_eq!(
            MultiVolumeReader::volume_path_for(&base, 10),
            PathBuf::from("test.7z.010")
        );
        assert_eq!(
            MultiVolumeReader::volume_path_for(&base, 100),
            PathBuf::from("test.7z.100")
        );
    }

    #[test]
    fn test_open_multivolume() {
        let dir = TempDir::new().unwrap();
        let base_path = create_test_volumes(dir.path(), "test.7z", &[100, 100, 50]);

        // Use open_unchecked since test files don't have valid 7z signatures
        let reader =
            MultiVolumeReader::open_unchecked(format!("{}.001", base_path.display())).unwrap();

        assert_eq!(reader.volume_count(), 3);
        assert_eq!(reader.volume_sizes(), &[100, 100, 50]);
        assert_eq!(reader.total_size(), 250);
        assert_eq!(reader.current_volume(), 1);
    }

    #[test]
    fn test_read_across_volumes() {
        let dir = TempDir::new().unwrap();
        let base_path = create_test_volumes(dir.path(), "test.7z", &[100, 100, 50]);

        // Use open_unchecked since test files don't have valid 7z signatures
        let mut reader =
            MultiVolumeReader::open_unchecked(format!("{}.001", base_path.display())).unwrap();

        // Read all data
        let mut buffer = vec![0u8; 250];
        let n = reader.read(&mut buffer).unwrap();
        assert_eq!(n, 250);

        // Verify data integrity (each volume starts with different byte pattern)
        assert_eq!(buffer[0], 0); // First byte of first volume
        assert_eq!(buffer[100], 0); // First byte of second volume (100 offset in that vol's data)
        assert_eq!(buffer[200], 0); // First byte of third volume (200 offset in that vol's data)
    }

    #[test]
    fn test_seek_operations() {
        let dir = TempDir::new().unwrap();
        let base_path = create_test_volumes(dir.path(), "test.7z", &[100, 100, 50]);

        // Use open_unchecked since test files don't have valid 7z signatures
        let mut reader =
            MultiVolumeReader::open_unchecked(format!("{}.001", base_path.display())).unwrap();

        // Seek to middle of second volume
        let pos = reader.seek(SeekFrom::Start(150)).unwrap();
        assert_eq!(pos, 150);
        assert_eq!(reader.current_volume(), 2);

        // Seek to start
        let pos = reader.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(pos, 0);
        assert_eq!(reader.current_volume(), 1);

        // Seek from end
        let pos = reader.seek(SeekFrom::End(-50)).unwrap();
        assert_eq!(pos, 200);
        assert_eq!(reader.current_volume(), 3);

        // Seek relative
        reader.seek(SeekFrom::Start(100)).unwrap();
        let pos = reader.seek(SeekFrom::Current(25)).unwrap();
        assert_eq!(pos, 125);
    }

    #[test]
    fn test_no_volumes_error() {
        let dir = TempDir::new().unwrap();
        let result = MultiVolumeReader::open(dir.path().join("nonexistent.7z.001"));
        assert!(result.is_err());
    }

    #[test]
    fn test_position_to_volume() {
        let dir = TempDir::new().unwrap();
        let base_path = create_test_volumes(dir.path(), "test.7z", &[100, 100, 50]);
        // Use open_unchecked since test files don't have valid 7z signatures
        let reader =
            MultiVolumeReader::open_unchecked(format!("{}.001", base_path.display())).unwrap();

        // Position in first volume
        let (vol, off) = reader.position_to_volume(50);
        assert_eq!(vol, 0);
        assert_eq!(off, 50);

        // Position at boundary
        let (vol, off) = reader.position_to_volume(100);
        assert_eq!(vol, 1);
        assert_eq!(off, 0);

        // Position in third volume
        let (vol, off) = reader.position_to_volume(225);
        assert_eq!(vol, 2);
        assert_eq!(off, 25);
    }
}
