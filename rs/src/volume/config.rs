//! Configuration for multi-volume archives.

use std::path::{Path, PathBuf};

/// Configuration for multi-volume archives.
///
/// This struct defines the volume size and base path for multi-volume archive
/// operations.
///
/// # Example
///
/// ```rust
/// use zesven::volume::VolumeConfig;
///
/// // Create config for 100 MB volumes
/// let config = VolumeConfig::new("archive.7z", 100 * 1024 * 1024);
///
/// // Get paths for each volume
/// assert_eq!(config.volume_path(1).to_str().unwrap(), "archive.7z.001");
/// assert_eq!(config.volume_path(2).to_str().unwrap(), "archive.7z.002");
/// ```
#[derive(Debug, Clone)]
pub struct VolumeConfig {
    /// Size of each volume in bytes (except possibly the last).
    pub volume_size: u64,
    /// Base path for volume files (without volume extension).
    base_path: PathBuf,
}

impl VolumeConfig {
    /// Creates a new volume configuration.
    ///
    /// # Arguments
    ///
    /// * `base_path` - Base path for the archive (e.g., "archive.7z")
    /// * `volume_size` - Maximum size of each volume in bytes
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::volume::VolumeConfig;
    ///
    /// // 50 MB volumes
    /// let config = VolumeConfig::new("backup.7z", 50 * 1024 * 1024);
    /// ```
    pub fn new(base_path: impl AsRef<Path>, volume_size: u64) -> Self {
        Self {
            volume_size,
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Returns the base path for the archive.
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Generates the path for a specific volume number.
    ///
    /// Volume numbers are 1-indexed and formatted with 3 digits (e.g., 001, 002).
    ///
    /// # Arguments
    ///
    /// * `volume_number` - The volume number (1-indexed)
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::volume::VolumeConfig;
    ///
    /// let config = VolumeConfig::new("data.7z", 1024 * 1024);
    /// assert_eq!(config.volume_path(1).to_str().unwrap(), "data.7z.001");
    /// assert_eq!(config.volume_path(10).to_str().unwrap(), "data.7z.010");
    /// assert_eq!(config.volume_path(100).to_str().unwrap(), "data.7z.100");
    /// ```
    pub fn volume_path(&self, volume_number: u32) -> PathBuf {
        let base_str = self.base_path.to_string_lossy();
        PathBuf::from(format!("{}.{:03}", base_str, volume_number))
    }

    /// Returns the volume size in bytes.
    pub fn volume_size(&self) -> u64 {
        self.volume_size
    }

    /// Creates a config with the default volume size (100 MB).
    pub fn with_default_size(base_path: impl AsRef<Path>) -> Self {
        Self::new(base_path, 100 * 1024 * 1024)
    }

    /// Creates a config for DVD-sized volumes (~4.7 GB).
    pub fn dvd(base_path: impl AsRef<Path>) -> Self {
        Self::new(base_path, 4700 * 1024 * 1024) // 4700 MiB
    }

    /// Creates a config for CD-sized volumes (~700 MB).
    pub fn cd(base_path: impl AsRef<Path>) -> Self {
        Self::new(base_path, 700 * 1024 * 1024)
    }

    /// Creates a config for FAT32-compatible volumes (~4 GB).
    pub fn fat32(base_path: impl AsRef<Path>) -> Self {
        // FAT32 max file size is 4 GB - 1 byte
        Self::new(base_path, 4 * 1024 * 1024 * 1024 - 1)
    }
}

impl Default for VolumeConfig {
    fn default() -> Self {
        Self {
            volume_size: 100 * 1024 * 1024, // 100 MB
            base_path: PathBuf::from("archive.7z"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_path_generation() {
        let config = VolumeConfig::new("test.7z", 1024);

        assert_eq!(config.volume_path(1), PathBuf::from("test.7z.001"));
        assert_eq!(config.volume_path(2), PathBuf::from("test.7z.002"));
        assert_eq!(config.volume_path(10), PathBuf::from("test.7z.010"));
        assert_eq!(config.volume_path(100), PathBuf::from("test.7z.100"));
        assert_eq!(config.volume_path(999), PathBuf::from("test.7z.999"));
    }

    #[test]
    fn test_volume_path_with_directory() {
        let config = VolumeConfig::new("/path/to/archive.7z", 1024);

        assert_eq!(
            config.volume_path(1),
            PathBuf::from("/path/to/archive.7z.001")
        );
    }

    #[test]
    fn test_preset_sizes() {
        let dvd = VolumeConfig::dvd("archive.7z");
        assert_eq!(dvd.volume_size(), 4700 * 1024 * 1024);

        let cd = VolumeConfig::cd("archive.7z");
        assert_eq!(cd.volume_size(), 700 * 1024 * 1024);

        let fat32 = VolumeConfig::fat32("archive.7z");
        assert_eq!(fat32.volume_size(), 4 * 1024 * 1024 * 1024 - 1);
    }

    #[test]
    fn test_default() {
        let config = VolumeConfig::default();
        assert_eq!(config.volume_size(), 100 * 1024 * 1024);
        assert_eq!(config.base_path(), Path::new("archive.7z"));
    }
}
