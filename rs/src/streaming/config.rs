//! Configuration for streaming decompression operations.
//!
//! This module provides [`StreamingConfig`] for controlling memory usage
//! and behavior during streaming decompression.

/// Configuration for streaming decompression with memory bounds.
///
/// This configuration allows fine-tuning memory usage during streaming
/// operations, making it possible to process large archives with bounded
/// memory consumption.
///
/// # Example
///
/// ```rust
/// use zesven::streaming::StreamingConfig;
///
/// // Default configuration (64 MiB buffer, 64 KiB read buffer)
/// let config = StreamingConfig::default();
///
/// // Custom configuration for constrained environments
/// let config = StreamingConfig::new()
///     .max_memory_buffer(32 * 1024 * 1024)  // 32 MiB
///     .read_buffer_size(32 * 1024)           // 32 KiB
///     .verify_crc(true);
/// ```
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Maximum memory buffer size for decompression (bytes).
    ///
    /// This limits the total memory used for decompression buffers.
    /// Default: 64 MiB.
    pub max_memory_buffer: usize,

    /// Buffer size for streaming reads (bytes).
    ///
    /// Smaller buffers reduce memory usage but may decrease performance.
    /// Default: 64 KiB.
    pub read_buffer_size: usize,

    /// Enable CRC verification during streaming.
    ///
    /// When enabled, CRC checksums are verified as data is extracted.
    /// This adds overhead but ensures data integrity.
    /// Default: true.
    pub verify_crc: bool,

    /// Enable progress tracking.
    ///
    /// When enabled, progress information is available during extraction.
    /// Default: true.
    pub track_progress: bool,

    /// Maximum number of entries to process.
    ///
    /// Provides protection against archives with excessive entry counts.
    /// Default: 1,000,000.
    pub max_entries: usize,

    /// Maximum compression ratio allowed.
    ///
    /// Provides protection against compression bombs.
    /// A ratio of 1000 means 1 byte compressed can expand to at most 1000 bytes.
    /// Default: 1000.
    pub max_compression_ratio: u32,

    /// Decoder pool capacity for solid archive optimization.
    ///
    /// The decoder pool caches decompression streams to avoid re-decompressing
    /// from the start of solid blocks when accessing multiple files.
    ///
    /// - `0` (default): Auto-size based on CPU count
    /// - `Some(n)`: Use exactly n decoders
    /// - `None`: Disable pooling (not recommended for solid archives)
    ///
    /// Default: 0 (auto-sized to CPU count).
    pub decoder_pool_capacity: Option<usize>,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_memory_buffer: 64 * 1024 * 1024, // 64 MiB
            read_buffer_size: 64 * 1024,         // 64 KiB
            verify_crc: true,
            track_progress: true,
            max_entries: 1_000_000,
            max_compression_ratio: 1000,
            decoder_pool_capacity: Some(0), // Auto-size based on CPU count
        }
    }
}

impl StreamingConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a configuration optimized for low memory usage.
    ///
    /// This configuration uses smaller buffers suitable for
    /// memory-constrained environments.
    pub fn low_memory() -> Self {
        Self {
            max_memory_buffer: 8 * 1024 * 1024, // 8 MiB
            read_buffer_size: 16 * 1024,        // 16 KiB
            verify_crc: true,
            track_progress: false,
            max_entries: 100_000,
            max_compression_ratio: 1000,
            decoder_pool_capacity: Some(2), // Minimal pool
        }
    }

    /// Creates a configuration optimized for high performance.
    ///
    /// This configuration uses larger buffers for better throughput
    /// at the cost of higher memory usage.
    pub fn high_performance() -> Self {
        Self {
            max_memory_buffer: 256 * 1024 * 1024, // 256 MiB
            read_buffer_size: 256 * 1024,         // 256 KiB
            verify_crc: true,
            track_progress: true,
            max_entries: 10_000_000,
            max_compression_ratio: 10000,
            decoder_pool_capacity: Some(0), // Auto-size (uses CPU count)
        }
    }

    /// Creates a configuration automatically sized for the current system.
    ///
    /// This method detects available system RAM and configures memory buffers
    /// accordingly. The buffer is sized to use approximately 12.5% of available RAM,
    /// with a minimum of 32 MiB and maximum of 1 GiB.
    ///
    /// Requires the `sysinfo` feature to be enabled. Without the feature,
    /// falls back to the default configuration.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::streaming::StreamingConfig;
    ///
    /// // Auto-configure based on available system RAM
    /// let config = StreamingConfig::auto_sized();
    /// ```
    #[cfg(feature = "sysinfo")]
    pub fn auto_sized() -> Self {
        use sysinfo::System;

        let mut sys = System::new();
        sys.refresh_memory();

        let total_memory = sys.total_memory(); // in bytes
        let available_memory = sys.available_memory(); // in bytes

        // Use ~12.5% of available RAM, or ~6.25% of total if available is very low
        let target = (available_memory / 8).max(total_memory / 16);

        // Clamp to reasonable bounds: 32 MiB to 1 GiB
        let min_buffer = 32 * 1024 * 1024; // 32 MiB
        let max_buffer = 1024 * 1024 * 1024; // 1 GiB
        let buffer_size = (target as usize).clamp(min_buffer, max_buffer);

        // Scale read buffer with memory buffer (0.1% of memory buffer, clamped)
        let read_buffer = (buffer_size / 1000).clamp(32 * 1024, 512 * 1024);

        Self {
            max_memory_buffer: buffer_size,
            read_buffer_size: read_buffer,
            verify_crc: true,
            track_progress: true,
            max_entries: 1_000_000,
            max_compression_ratio: 1000,
            decoder_pool_capacity: Some(0), // Auto-size decoder pool based on CPU count
        }
    }

    /// Creates a configuration automatically sized for the current system.
    ///
    /// This is a fallback when the `sysinfo` feature is not enabled.
    /// Returns the default configuration.
    #[cfg(not(feature = "sysinfo"))]
    pub fn auto_sized() -> Self {
        Self::default()
    }

    /// Returns the detected system memory information.
    ///
    /// Returns `None` if the `sysinfo` feature is not enabled or if
    /// memory detection fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if let Some(info) = StreamingConfig::system_memory_info() {
    ///     println!("Total: {} bytes, Available: {} bytes", info.total, info.available);
    /// }
    /// ```
    #[cfg(feature = "sysinfo")]
    pub fn system_memory_info() -> Option<SystemMemoryInfo> {
        use sysinfo::System;

        let mut sys = System::new();
        sys.refresh_memory();

        Some(SystemMemoryInfo {
            total: sys.total_memory(),
            available: sys.available_memory(),
            used: sys.used_memory(),
        })
    }

    /// Returns system memory information.
    ///
    /// Returns `None` when the `sysinfo` feature is not enabled.
    #[cfg(not(feature = "sysinfo"))]
    pub fn system_memory_info() -> Option<SystemMemoryInfo> {
        None
    }

    /// Sets the maximum memory buffer size.
    pub fn max_memory_buffer(mut self, bytes: usize) -> Self {
        self.max_memory_buffer = bytes;
        self
    }

    /// Sets the read buffer size.
    pub fn read_buffer_size(mut self, bytes: usize) -> Self {
        self.read_buffer_size = bytes;
        self
    }

    /// Sets whether to verify CRC checksums.
    pub fn verify_crc(mut self, verify: bool) -> Self {
        self.verify_crc = verify;
        self
    }

    /// Sets whether to track progress.
    pub fn track_progress(mut self, track: bool) -> Self {
        self.track_progress = track;
        self
    }

    /// Sets the maximum number of entries.
    pub fn max_entries(mut self, count: usize) -> Self {
        self.max_entries = count;
        self
    }

    /// Sets the maximum compression ratio.
    pub fn max_compression_ratio(mut self, ratio: u32) -> Self {
        self.max_compression_ratio = ratio;
        self
    }

    /// Sets the decoder pool capacity.
    ///
    /// - `Some(0)`: Auto-size based on CPU count (default)
    /// - `Some(n)`: Use exactly n decoders
    /// - `None`: Disable pooling
    pub fn decoder_pool_capacity(mut self, capacity: Option<usize>) -> Self {
        self.decoder_pool_capacity = capacity;
        self
    }

    /// Disables the decoder pool.
    ///
    /// Not recommended for solid archives as it can significantly
    /// degrade performance when accessing multiple files.
    pub fn disable_decoder_pool(mut self) -> Self {
        self.decoder_pool_capacity = None;
        self
    }

    /// Resolves the decoder pool capacity to an actual value.
    ///
    /// - `Some(0)` → CPU count
    /// - `Some(n)` → n
    /// - `None` → 0 (disabled)
    pub fn resolved_decoder_pool_capacity(&self) -> usize {
        match self.decoder_pool_capacity {
            Some(0) => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            Some(n) => n,
            None => 0,
        }
    }

    /// Validates the configuration.
    ///
    /// Returns an error if any values are invalid.
    pub fn validate(&self) -> crate::Result<()> {
        if self.max_memory_buffer == 0 {
            return Err(crate::Error::InvalidFormat(
                "max_memory_buffer must be greater than 0".into(),
            ));
        }

        if self.read_buffer_size == 0 {
            return Err(crate::Error::InvalidFormat(
                "read_buffer_size must be greater than 0".into(),
            ));
        }

        if self.read_buffer_size > self.max_memory_buffer {
            return Err(crate::Error::InvalidFormat(
                "read_buffer_size cannot exceed max_memory_buffer".into(),
            ));
        }

        Ok(())
    }
}

/// Memory usage estimate for an operation.
///
/// This struct provides minimum, typical, and maximum memory estimates
/// for decompression or compression operations.
///
/// # Example
///
/// ```rust
/// use zesven::streaming::MemoryEstimate;
///
/// let estimate = MemoryEstimate::new(1024, 4096, 16384);
/// println!("Memory: {} - {} bytes (typical: {})",
///     estimate.minimum, estimate.maximum, estimate.typical);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryEstimate {
    /// Minimum memory required (best case scenario).
    pub minimum: usize,
    /// Typical memory usage under normal conditions.
    pub typical: usize,
    /// Maximum memory that could be used (worst case).
    pub maximum: usize,
}

impl MemoryEstimate {
    /// Creates a new memory estimate.
    pub fn new(minimum: usize, typical: usize, maximum: usize) -> Self {
        Self {
            minimum,
            typical,
            maximum,
        }
    }

    /// Creates an estimate where min/typical/max are all the same.
    pub fn fixed(size: usize) -> Self {
        Self::new(size, size, size)
    }

    /// Adds another estimate to this one.
    pub fn add(&self, other: &MemoryEstimate) -> MemoryEstimate {
        MemoryEstimate {
            minimum: self.minimum.saturating_add(other.minimum),
            typical: self.typical.saturating_add(other.typical),
            maximum: self.maximum.saturating_add(other.maximum),
        }
    }

    /// Formats the estimate in human-readable form.
    pub fn format_human(&self) -> String {
        use crate::progress::format_bytes_iec_usize;
        format!(
            "{} - {} (typical: {})",
            format_bytes_iec_usize(self.minimum),
            format_bytes_iec_usize(self.maximum),
            format_bytes_iec_usize(self.typical)
        )
    }
}

/// Compression method identifier for memory estimation.
///
/// This enum mirrors [`crate::codec::CodecMethod`] but is specifically used
/// for memory estimation in the streaming API. You can convert from `CodecMethod`
/// using `From`:
///
/// ```rust,ignore
/// use zesven::codec::CodecMethod;
/// use zesven::streaming::CompressionMethod;
///
/// let method = CodecMethod::Lzma2;
/// let estimation_method: CompressionMethod = method.into();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMethod {
    /// Copy (no compression) - minimal memory.
    Copy,
    /// LZMA compression.
    Lzma,
    /// LZMA2 compression.
    Lzma2,
    /// Deflate compression.
    Deflate,
    /// BZip2 compression.
    Bzip2,
    /// PPMd compression.
    Ppmd,
    /// LZ4 compression.
    Lz4,
    /// ZSTD compression.
    Zstd,
    /// Brotli compression.
    Brotli,
}

impl CompressionMethod {
    /// Estimates decoder memory for this compression method.
    ///
    /// # Arguments
    ///
    /// * `dict_size` - Dictionary size in bytes (for LZMA/LZMA2)
    pub fn estimate_decoder_memory(&self, dict_size: Option<u32>) -> MemoryEstimate {
        match self {
            Self::Copy => MemoryEstimate::fixed(0),

            Self::Lzma | Self::Lzma2 => {
                // LZMA decoder needs: dictionary buffer + range coder state + match finder state
                // Dictionary is the dominant factor
                let dict = dict_size.unwrap_or(8 * 1024 * 1024) as usize; // Default 8 MiB

                // Base state: ~20 KB for internal structures
                let base_state = 20 * 1024;

                // Minimum: just dict + base state
                let minimum = dict + base_state;

                // Typical: dict + base state + some working buffers
                let typical = dict + base_state + 64 * 1024;

                // Maximum: dict + base state + generous buffers
                let maximum = dict + base_state + 256 * 1024;

                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Deflate => {
                // Deflate uses 32 KB sliding window + Huffman tables
                let minimum = 32 * 1024 + 8 * 1024;
                let typical = 64 * 1024;
                let maximum = 128 * 1024;
                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Bzip2 => {
                // BZip2 uses up to 900 KB per block (level 9)
                let minimum = 100 * 1024; // Level 1
                let typical = 400 * 1024; // Level 5
                let maximum = 900 * 1024; // Level 9
                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Ppmd => {
                // PPMd memory depends on model order and memory size setting
                let minimum = 1024 * 1024;
                let typical = 16 * 1024 * 1024;
                let maximum = 256 * 1024 * 1024;
                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Lz4 => {
                // LZ4 is designed for low memory usage
                let minimum = 16 * 1024;
                let typical = 64 * 1024;
                let maximum = 256 * 1024;
                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Zstd => {
                // ZSTD decoder memory depends on window size
                // Default window is up to 8 MiB
                let minimum = 128 * 1024;
                let typical = 1024 * 1024;
                let maximum = 128 * 1024 * 1024; // 128 MiB window
                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Brotli => {
                // Brotli uses sliding window up to 16 MiB
                let minimum = 256 * 1024;
                let typical = 4 * 1024 * 1024;
                let maximum = 16 * 1024 * 1024;
                MemoryEstimate::new(minimum, typical, maximum)
            }
        }
    }

    /// Estimates encoder memory for this compression method.
    ///
    /// Encoder memory is typically 5-10x higher than decoder memory.
    ///
    /// # Arguments
    ///
    /// * `dict_size` - Dictionary size in bytes (for LZMA/LZMA2)
    /// * `level` - Compression level (0-9)
    pub fn estimate_encoder_memory(&self, dict_size: Option<u32>, level: u32) -> MemoryEstimate {
        let level = level.min(9);

        match self {
            Self::Copy => MemoryEstimate::fixed(0),

            Self::Lzma | Self::Lzma2 => {
                // LZMA encoder needs: dictionary + hash tables + match finder
                // Hash tables scale with dictionary size (roughly 4-8x dictionary)
                let dict =
                    dict_size.unwrap_or_else(|| Self::lzma_default_dict_size(level)) as usize;

                // Match finder hash tables (binary tree or hash chain)
                // BT4 uses 4 hash arrays totaling ~4x dict size
                let hash_tables = dict * 4;

                // Internal buffers
                let buffers = 64 * 1024;

                let base = dict + hash_tables + buffers;

                // Level affects search depth, not memory much
                let level_factor = 1.0 + (level as f64 * 0.1);

                let minimum = base;
                let typical = (base as f64 * level_factor) as usize;
                let maximum = (base as f64 * 1.5) as usize;

                MemoryEstimate::new(minimum, typical, maximum)
            }

            Self::Deflate => {
                // Deflate encoder uses ~256 KB to ~1 MB depending on level
                let base = match level {
                    0..=3 => 128 * 1024,
                    4..=6 => 256 * 1024,
                    _ => 512 * 1024,
                };
                MemoryEstimate::new(base, base + 64 * 1024, base * 2)
            }

            Self::Bzip2 => {
                // BZip2 encoder uses ~8x block size
                let block_size = (level.max(1) * 100 * 1024) as usize;
                let encoder_mem = block_size * 8;
                MemoryEstimate::new(encoder_mem, encoder_mem + 100 * 1024, encoder_mem * 2)
            }

            Self::Ppmd => {
                // PPMd encoder memory matches decoder
                self.estimate_decoder_memory(None)
            }

            Self::Lz4 => {
                // LZ4 encoder uses hash table + working buffers
                let base = match level {
                    0..=3 => 16 * 1024,
                    4..=6 => 64 * 1024,
                    _ => 256 * 1024,
                };
                MemoryEstimate::new(base, base * 2, base * 4)
            }

            Self::Zstd => {
                // ZSTD encoder memory scales significantly with level
                let base = match level {
                    0..=3 => 1024 * 1024,
                    4..=6 => 8 * 1024 * 1024,
                    7..=9 => 64 * 1024 * 1024,
                    _ => 128 * 1024 * 1024,
                };
                MemoryEstimate::new(base / 2, base, base * 2)
            }

            Self::Brotli => {
                // Brotli encoder memory depends on quality level
                let base = match level {
                    0..=4 => 1024 * 1024,
                    5..=7 => 4 * 1024 * 1024,
                    _ => 16 * 1024 * 1024,
                };
                MemoryEstimate::new(base / 2, base, base * 2)
            }
        }
    }

    /// Returns the default LZMA dictionary size for a given compression level.
    fn lzma_default_dict_size(level: u32) -> u32 {
        match level {
            0 => 64 * 1024,        // 64 KB
            1 => 256 * 1024,       // 256 KB
            2 => 1024 * 1024,      // 1 MB
            3 => 2 * 1024 * 1024,  // 2 MB
            4 => 4 * 1024 * 1024,  // 4 MB
            5 => 8 * 1024 * 1024,  // 8 MB
            6 => 8 * 1024 * 1024,  // 8 MB
            7 => 16 * 1024 * 1024, // 16 MB
            8 => 32 * 1024 * 1024, // 32 MB
            _ => 64 * 1024 * 1024, // 64 MB
        }
    }
}

impl From<crate::codec::CodecMethod> for CompressionMethod {
    fn from(method: crate::codec::CodecMethod) -> Self {
        match method {
            crate::codec::CodecMethod::Copy => Self::Copy,
            crate::codec::CodecMethod::Lzma => Self::Lzma,
            crate::codec::CodecMethod::Lzma2 => Self::Lzma2,
            crate::codec::CodecMethod::Deflate => Self::Deflate,
            crate::codec::CodecMethod::BZip2 => Self::Bzip2,
            crate::codec::CodecMethod::PPMd => Self::Ppmd,
            crate::codec::CodecMethod::Lz4 => Self::Lz4,
            crate::codec::CodecMethod::Zstd => Self::Zstd,
            crate::codec::CodecMethod::Brotli => Self::Brotli,
        }
    }
}

impl StreamingConfig {
    /// Estimates total memory usage for this configuration.
    ///
    /// This includes buffer memory and expected decoder memory.
    ///
    /// # Arguments
    ///
    /// * `method` - The compression method being used
    /// * `dict_size` - Dictionary size (for LZMA/LZMA2)
    pub fn estimate_memory(
        &self,
        method: CompressionMethod,
        dict_size: Option<u32>,
    ) -> MemoryEstimate {
        // Buffer memory from config
        let buffer_mem = MemoryEstimate::fixed(self.max_memory_buffer + self.read_buffer_size);

        // Decoder memory
        let decoder_mem = method.estimate_decoder_memory(dict_size);

        // Combine
        buffer_mem.add(&decoder_mem)
    }

    /// Estimates memory for a streaming extraction operation.
    ///
    /// This provides a conservative estimate that accounts for:
    /// - Read buffers
    /// - Decompression buffers
    /// - Decoder internal state
    /// - Output buffering
    pub fn estimate_extraction_memory(&self) -> MemoryEstimate {
        // Default to LZMA2 with typical dict size as worst case
        let decoder = CompressionMethod::Lzma2.estimate_decoder_memory(Some(16 * 1024 * 1024));

        let buffers = MemoryEstimate::new(
            self.read_buffer_size,
            self.read_buffer_size + 64 * 1024,
            self.max_memory_buffer,
        );

        decoder.add(&buffers)
    }
}

/// Information about system memory.
///
/// This struct provides total, available, and used memory information
/// from the system when the `sysinfo` feature is enabled.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::streaming::{StreamingConfig, SystemMemoryInfo};
///
/// if let Some(info) = StreamingConfig::system_memory_info() {
///     println!("Total: {} bytes", info.total);
///     println!("Available: {} bytes", info.available);
///     println!("Used: {} bytes", info.used);
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemMemoryInfo {
    /// Total physical memory in bytes.
    pub total: u64,
    /// Available (free) memory in bytes.
    pub available: u64,
    /// Used memory in bytes.
    pub used: u64,
}

impl SystemMemoryInfo {
    /// Returns the percentage of memory currently in use.
    pub fn usage_percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64) * 100.0
        }
    }

    /// Formats memory values in human-readable form.
    pub fn format_human(&self) -> String {
        use crate::progress::format_bytes_iec_usize;
        format!(
            "Total: {}, Available: {}, Used: {} ({:.1}%)",
            format_bytes_iec_usize(self.total as usize),
            format_bytes_iec_usize(self.available as usize),
            format_bytes_iec_usize(self.used as usize),
            self.usage_percent()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_estimate_add() {
        let a = MemoryEstimate::new(100, 200, 300);
        let b = MemoryEstimate::new(10, 20, 30);
        let sum = a.add(&b);
        assert_eq!(sum.minimum, 110);
        assert_eq!(sum.typical, 220);
        assert_eq!(sum.maximum, 330);
    }

    #[test]
    fn test_memory_estimate_fixed() {
        let fixed = MemoryEstimate::fixed(1024);
        assert_eq!(fixed.minimum, 1024);
        assert_eq!(fixed.typical, 1024);
        assert_eq!(fixed.maximum, 1024);
    }

    #[test]
    fn test_format_bytes() {
        use crate::progress::format_bytes_iec_usize;
        // Tests for the centralized format_bytes_iec_usize function
        assert_eq!(format_bytes_iec_usize(512), "512 B");
        assert_eq!(format_bytes_iec_usize(1024), "1.0 KiB");
        assert_eq!(format_bytes_iec_usize(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes_iec_usize(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn test_lzma_decoder_memory() {
        let estimate = CompressionMethod::Lzma2.estimate_decoder_memory(Some(8 * 1024 * 1024));
        // Should be at least dictionary size
        assert!(estimate.minimum >= 8 * 1024 * 1024);
    }

    #[test]
    fn test_config_estimate_memory() {
        let config = StreamingConfig::default();
        let estimate = config.estimate_memory(CompressionMethod::Lzma2, None);
        // Should be positive
        assert!(estimate.minimum > 0);
        assert!(estimate.typical >= estimate.minimum);
        assert!(estimate.maximum >= estimate.typical);
    }

    #[test]
    fn test_default_config() {
        let config = StreamingConfig::default();
        assert_eq!(config.max_memory_buffer, 64 * 1024 * 1024);
        assert_eq!(config.read_buffer_size, 64 * 1024);
        assert!(config.verify_crc);
        assert!(config.track_progress);
    }

    #[test]
    fn test_low_memory_config() {
        let config = StreamingConfig::low_memory();
        assert_eq!(config.max_memory_buffer, 8 * 1024 * 1024);
        assert!(config.max_memory_buffer < StreamingConfig::default().max_memory_buffer);
    }

    #[test]
    fn test_high_performance_config() {
        let config = StreamingConfig::high_performance();
        assert_eq!(config.max_memory_buffer, 256 * 1024 * 1024);
        assert!(config.max_memory_buffer > StreamingConfig::default().max_memory_buffer);
    }

    #[test]
    fn test_builder_pattern() {
        let config = StreamingConfig::new()
            .max_memory_buffer(16 * 1024 * 1024)
            .read_buffer_size(32 * 1024)
            .verify_crc(false)
            .track_progress(false);

        assert_eq!(config.max_memory_buffer, 16 * 1024 * 1024);
        assert_eq!(config.read_buffer_size, 32 * 1024);
        assert!(!config.verify_crc);
        assert!(!config.track_progress);
    }

    #[test]
    fn test_validation_success() {
        let config = StreamingConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validation_zero_memory_buffer() {
        let config = StreamingConfig::new().max_memory_buffer(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_zero_read_buffer() {
        let config = StreamingConfig::new().read_buffer_size(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_read_buffer_exceeds_max() {
        let config = StreamingConfig::new()
            .max_memory_buffer(1024)
            .read_buffer_size(2048);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_decoder_pool_capacity_default() {
        let config = StreamingConfig::default();
        assert_eq!(config.decoder_pool_capacity, Some(0)); // Auto-size
        assert!(config.resolved_decoder_pool_capacity() >= 1);
    }

    #[test]
    fn test_decoder_pool_capacity_explicit() {
        let config = StreamingConfig::new().decoder_pool_capacity(Some(8));
        assert_eq!(config.resolved_decoder_pool_capacity(), 8);
    }

    #[test]
    fn test_decoder_pool_capacity_disabled() {
        let config = StreamingConfig::new().disable_decoder_pool();
        assert_eq!(config.decoder_pool_capacity, None);
        assert_eq!(config.resolved_decoder_pool_capacity(), 0);
    }

    #[test]
    fn test_low_memory_decoder_pool() {
        let config = StreamingConfig::low_memory();
        assert_eq!(config.decoder_pool_capacity, Some(2));
        assert_eq!(config.resolved_decoder_pool_capacity(), 2);
    }

    #[test]
    fn test_auto_sized() {
        let config = StreamingConfig::auto_sized();
        // Auto-sized should have valid bounds
        assert!(config.max_memory_buffer >= 32 * 1024 * 1024); // At least 32 MiB
        assert!(config.max_memory_buffer <= 1024 * 1024 * 1024); // At most 1 GiB
        assert!(config.read_buffer_size >= 32 * 1024); // At least 32 KiB
        assert!(config.read_buffer_size <= 512 * 1024); // At most 512 KiB
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_system_memory_info() {
        // Test that system_memory_info returns Some when sysinfo is enabled
        let info_opt = StreamingConfig::system_memory_info();

        #[cfg(feature = "sysinfo")]
        {
            let info = info_opt.expect("sysinfo feature enabled but no memory info");
            assert!(info.total > 0, "Total memory should be > 0");
            // available + used should roughly equal total (with some tolerance for measurement timing)
            // used memory should be <= total
            assert!(info.used <= info.total, "Used should be <= total");
        }

        #[cfg(not(feature = "sysinfo"))]
        {
            assert!(info_opt.is_none(), "Without sysinfo, should return None");
        }
    }

    #[test]
    fn test_system_memory_info_usage_percent() {
        let info = SystemMemoryInfo {
            total: 16 * 1024 * 1024 * 1024,    // 16 GiB
            available: 8 * 1024 * 1024 * 1024, // 8 GiB
            used: 8 * 1024 * 1024 * 1024,      // 8 GiB
        };
        assert!((info.usage_percent() - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_system_memory_info_zero_total() {
        let info = SystemMemoryInfo {
            total: 0,
            available: 0,
            used: 0,
        };
        assert!((info.usage_percent() - 0.0).abs() < f64::EPSILON);
    }
}
