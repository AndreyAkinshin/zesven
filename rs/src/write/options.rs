//! Write options and configuration for archive creation.

use crate::codec::CodecMethod;
use crate::format::streams::ResourceLimits;

#[cfg(feature = "aes")]
use crate::crypto::{NoncePolicy, Password};

/// Pre-compression filter for improving compression of specific data types.
///
/// BCJ (Branch/Call/Jump) filters transform executable code addresses from
/// relative to absolute form before compression. This improves compression
/// ratios by 5-15% for executable files because the resulting byte patterns
/// compress better.
///
/// The Delta filter computes differences between consecutive samples,
/// improving compression for audio, image, and other structured data.
///
/// # Example
///
/// ```rust
/// use zesven::write::{WriteOptions, WriteFilter};
///
/// // Use BCJ x86 filter for compressing Windows/Linux executables
/// let options = WriteOptions::new().filter(WriteFilter::BcjX86);
///
/// // Use Delta filter for audio data (2-byte samples)
/// let options = WriteOptions::new().filter(WriteFilter::Delta { distance: 2 });
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum WriteFilter {
    /// No filter applied.
    #[default]
    None,
    /// BCJ x86 filter for 32-bit and 64-bit x86 executables.
    BcjX86,
    /// BCJ ARM filter for 32-bit ARM executables.
    BcjArm,
    /// BCJ ARM64/AArch64 filter for 64-bit ARM executables.
    BcjArm64,
    /// BCJ ARM Thumb filter for ARM Thumb mode executables.
    BcjArmThumb,
    /// BCJ PowerPC filter.
    BcjPpc,
    /// BCJ SPARC filter.
    BcjSparc,
    /// BCJ IA-64 (Itanium) filter.
    BcjIa64,
    /// BCJ RISC-V filter.
    BcjRiscv,
    /// BCJ2 4-stream filter for x86 executables.
    ///
    /// BCJ2 splits x86 code into 4 streams (main, call, jump, range)
    /// for improved compression. Unlike simple BCJ filters, BCJ2 uses
    /// a complex 4-input coder structure.
    ///
    /// Note: BCJ2 is only supported in non-solid mode.
    Bcj2,
    /// Delta filter for structured data (audio, images).
    ///
    /// The `distance` parameter specifies the sample size in bytes.
    /// Common values:
    /// - 1: byte-level differences
    /// - 2: 16-bit audio samples
    /// - 4: 32-bit values or RGBA pixels
    ///
    /// Use [`WriteFilter::delta()`] to construct with validation.
    Delta {
        /// Sample distance in bytes (1-255).
        distance: u8,
    },
}

impl WriteFilter {
    /// Creates a Delta filter with the given sample distance.
    ///
    /// # Arguments
    ///
    /// * `distance` - Sample size in bytes (1-255)
    ///
    /// # Panics
    ///
    /// Panics if `distance` is 0. Use a non-zero distance value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::write::WriteFilter;
    ///
    /// let filter = WriteFilter::delta(2); // 16-bit samples
    /// ```
    pub fn delta(distance: u8) -> Self {
        assert!(
            distance > 0,
            "Delta filter distance must be non-zero (1-255)"
        );
        Self::Delta { distance }
    }
}

impl WriteFilter {
    /// Returns the 7z method ID bytes for this filter.
    pub fn method_id(&self) -> Option<&'static [u8]> {
        use crate::codec::method;
        match self {
            Self::None => None,
            Self::BcjX86 => Some(method::BCJ_X86),
            Self::BcjArm => Some(method::BCJ_ARM),
            Self::BcjArm64 => Some(method::BCJ_ARM64),
            Self::BcjArmThumb => Some(method::BCJ_ARM_THUMB),
            Self::BcjPpc => Some(method::BCJ_PPC),
            Self::BcjSparc => Some(method::BCJ_SPARC),
            Self::BcjIa64 => Some(method::BCJ_IA64),
            Self::BcjRiscv => Some(method::BCJ_RISCV),
            Self::Bcj2 => Some(method::BCJ2),
            Self::Delta { .. } => Some(method::DELTA),
        }
    }

    /// Returns whether this is the BCJ2 filter.
    ///
    /// BCJ2 requires special handling as it produces 4 output streams.
    pub fn is_bcj2(&self) -> bool {
        matches!(self, Self::Bcj2)
    }

    /// Returns whether this filter is active (not None).
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Returns the filter properties to encode in the archive header.
    ///
    /// Most BCJ filters have no properties. Delta filter has a 1-byte property
    /// encoding `distance - 1` (so distance 1 is stored as 0, distance 255 as 254).
    pub fn properties(&self) -> Option<Vec<u8>> {
        match self {
            Self::Delta { distance } => {
                // Delta properties: single byte = distance - 1
                // distance is guaranteed to be >= 1 via delta() constructor
                Some(vec![distance - 1])
            }
            _ => None,
        }
    }
}

/// Options for creating archives.
#[derive(Clone)]
pub struct WriteOptions {
    /// Compression method to use.
    pub method: CodecMethod,
    /// Compression level (0-9).
    pub level: u32,
    /// LZMA2 encoder variant (standard or fast).
    pub lzma2_variant: Lzma2Variant,
    /// Pre-compression filter.
    pub filter: WriteFilter,
    /// Solid archive options.
    pub solid: SolidOptions,
    /// Resource limits.
    pub limits: ResourceLimits,
    /// Whether to produce deterministic output.
    pub deterministic: bool,
    /// Archive comment.
    pub comment: Option<String>,
    /// Password for encryption (requires "aes" feature).
    #[cfg(feature = "aes")]
    pub password: Option<Password>,
    /// Nonce policy for encryption.
    #[cfg(feature = "aes")]
    pub nonce_policy: NoncePolicy,
    /// Whether to encrypt the header (file names).
    ///
    /// When enabled, the archive header is encrypted, hiding file names
    /// from users who don't have the password. This provides additional
    /// security beyond just encrypting file contents.
    ///
    /// Requires a password to be set.
    #[cfg(feature = "aes")]
    pub encrypt_header: bool,
    /// Whether to encrypt file contents.
    ///
    /// When enabled, each file's compressed data is encrypted with AES-256.
    /// This encrypts the actual file contents, not just the header/filenames.
    ///
    /// Requires a password to be set.
    ///
    /// Note: Can be combined with `encrypt_header` for full encryption
    /// (both file contents and file names hidden).
    #[cfg(feature = "aes")]
    pub encrypt_data: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            method: CodecMethod::Lzma2,
            level: 5,
            lzma2_variant: Lzma2Variant::Standard,
            filter: WriteFilter::None,
            solid: SolidOptions::default(),
            limits: ResourceLimits::default(),
            deterministic: false,
            comment: None,
            #[cfg(feature = "aes")]
            password: None,
            #[cfg(feature = "aes")]
            nonce_policy: NoncePolicy::default(),
            #[cfg(feature = "aes")]
            encrypt_header: false,
            #[cfg(feature = "aes")]
            encrypt_data: false,
        }
    }
}

impl std::fmt::Debug for WriteOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("WriteOptions");
        s.field("method", &self.method)
            .field("level", &self.level)
            .field("lzma2_variant", &self.lzma2_variant)
            .field("filter", &self.filter)
            .field("solid", &self.solid)
            .field("deterministic", &self.deterministic)
            .field("comment", &self.comment);
        #[cfg(feature = "aes")]
        s.field("has_password", &self.password.is_some());
        s.finish()
    }
}

impl WriteOptions {
    /// Creates new write options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the compression method.
    pub fn method(mut self, method: CodecMethod) -> Self {
        self.method = method;
        self
    }

    /// Sets the compression level (strict validation).
    ///
    /// Valid values are 0-9, where:
    /// - 0: No compression (store only)
    /// - 1-3: Fast compression, lower ratio
    /// - 4-6: Balanced compression (default is 5)
    /// - 7-9: Maximum compression, slower
    ///
    /// Use [`level_clamped`] instead if you want invalid values to be silently
    /// clamped to 9 rather than returning an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidCompressionLevel`] if level is greater than 9.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use zesven::write::WriteOptions;
    ///
    /// // Valid levels succeed
    /// let opts = WriteOptions::new().level(9)?;
    /// assert_eq!(opts.level, 9);
    ///
    /// // Invalid levels return an error
    /// let result = WriteOptions::new().level(15);
    /// assert!(result.is_err());
    /// # Ok::<(), zesven::Error>(())
    /// ```
    ///
    /// [`level_clamped`]: Self::level_clamped
    /// [`Error::InvalidCompressionLevel`]: crate::Error::InvalidCompressionLevel
    pub fn level(mut self, level: u32) -> crate::Result<Self> {
        if level > 9 {
            return Err(crate::Error::InvalidCompressionLevel { level });
        }
        self.level = level;
        Ok(self)
    }

    /// Sets the compression level, clamping values above 9 (lenient validation).
    ///
    /// This method clamps the level to the valid range 0-9.
    /// Use this when you want to accept any level and automatically
    /// use the maximum when exceeded.
    ///
    /// Use [`level`] instead if you want invalid values to return an error
    /// rather than being silently clamped.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use zesven::write::WriteOptions;
    ///
    /// // Normal usage
    /// let opts = WriteOptions::new().level_clamped(7);
    /// assert_eq!(opts.level, 7);
    ///
    /// // Values above 9 are clamped to 9
    /// let opts = WriteOptions::new().level_clamped(15);
    /// assert_eq!(opts.level, 9);
    /// ```
    ///
    /// [`level`]: Self::level
    pub fn level_clamped(mut self, level: u32) -> Self {
        self.level = level.min(9);
        self
    }

    /// Sets the LZMA2 encoder variant.
    ///
    /// # Arguments
    ///
    /// * `variant` - The LZMA2 variant to use
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::write::{WriteOptions, Lzma2Variant};
    ///
    /// let options = WriteOptions::new()
    ///     .lzma2_variant(Lzma2Variant::Fast);
    /// ```
    pub fn lzma2_variant(mut self, variant: Lzma2Variant) -> Self {
        self.lzma2_variant = variant;
        self
    }

    /// Enables fast LZMA2 encoding (requires `fast-lzma2` feature).
    ///
    /// This is a convenience method equivalent to:
    /// ```rust,ignore
    /// options.lzma2_variant(Lzma2Variant::Fast)
    /// ```
    pub fn fast_lzma2(self) -> Self {
        self.lzma2_variant(Lzma2Variant::Fast)
    }

    /// Sets the pre-compression filter.
    ///
    /// Filters transform data before compression to improve compression ratios
    /// for specific data types. BCJ filters are effective for executables,
    /// while Delta is useful for audio and image data.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::write::{WriteOptions, WriteFilter};
    ///
    /// let options = WriteOptions::new()
    ///     .filter(WriteFilter::BcjX86);
    /// ```
    pub fn filter(mut self, filter: WriteFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Enables BCJ x86 filter for executable compression.
    ///
    /// This is a convenience method for compressing x86/x64 executables.
    /// Equivalent to `.filter(WriteFilter::BcjX86)`.
    pub fn bcj_x86(self) -> Self {
        self.filter(WriteFilter::BcjX86)
    }

    /// Enables BCJ ARM filter for ARM executable compression.
    ///
    /// Equivalent to `.filter(WriteFilter::BcjArm)`.
    pub fn bcj_arm(self) -> Self {
        self.filter(WriteFilter::BcjArm)
    }

    /// Enables BCJ ARM64 filter for AArch64 executable compression.
    ///
    /// Equivalent to `.filter(WriteFilter::BcjArm64)`.
    pub fn bcj_arm64(self) -> Self {
        self.filter(WriteFilter::BcjArm64)
    }

    /// Enables BCJ2 4-stream filter for x86 executable compression.
    ///
    /// BCJ2 splits x86 code into 4 streams for better compression.
    /// Equivalent to `.filter(WriteFilter::Bcj2)`.
    ///
    /// Note: BCJ2 is only supported in non-solid mode.
    pub fn bcj2(self) -> Self {
        self.filter(WriteFilter::Bcj2)
    }

    /// Enables Delta filter with the specified sample distance.
    ///
    /// The delta filter computes differences between consecutive samples,
    /// which can improve compression for audio, image, and other structured data.
    ///
    /// # Arguments
    ///
    /// * `distance` - Sample size in bytes (1-255). Common values:
    ///   - 1: byte-level differences
    ///   - 2: 16-bit audio samples
    ///   - 4: 32-bit values or RGBA pixels
    ///
    /// # Panics
    ///
    /// Panics if `distance` is 0.
    ///
    /// # Example
    ///
    /// ```rust
    /// use zesven::write::WriteOptions;
    ///
    /// // For 16-bit audio samples
    /// let options = WriteOptions::new().delta(2);
    /// ```
    pub fn delta(self, distance: u8) -> Self {
        self.filter(WriteFilter::delta(distance))
    }

    /// Returns whether a pre-compression filter is enabled.
    pub fn has_filter(&self) -> bool {
        self.filter.is_active()
    }

    /// Enables solid compression with default settings.
    pub fn solid(mut self) -> Self {
        self.solid = SolidOptions::enabled();
        self
    }

    /// Sets solid compression options.
    pub fn solid_options(mut self, options: SolidOptions) -> Self {
        self.solid = options;
        self
    }

    /// Enables deterministic mode for reproducible archives.
    pub fn deterministic(mut self, enabled: bool) -> Self {
        self.deterministic = enabled;
        self
    }

    /// Sets an archive comment.
    ///
    /// The comment will be stored as UTF-16LE in the archive and can be
    /// retrieved when reading the archive.
    pub fn comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Sets the password for encryption.
    #[cfg(feature = "aes")]
    pub fn password(mut self, password: impl Into<Password>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the nonce policy for encryption.
    #[cfg(feature = "aes")]
    pub fn nonce_policy(mut self, policy: NoncePolicy) -> Self {
        self.nonce_policy = policy;
        self
    }

    /// Enables header encryption (hides file names).
    ///
    /// When enabled, the archive header is encrypted along with the data,
    /// preventing users without the password from seeing file names.
    ///
    /// Note: This only has effect if a password is also set.
    #[cfg(feature = "aes")]
    pub fn encrypt_header(mut self, encrypt: bool) -> Self {
        self.encrypt_header = encrypt;
        self
    }

    /// Enables content encryption (encrypts file data).
    ///
    /// When enabled, each file's compressed data is encrypted with AES-256.
    /// This encrypts the actual file contents, protecting the data itself.
    ///
    /// Note: This only has effect if a password is also set.
    #[cfg(feature = "aes")]
    pub fn encrypt_data(mut self, encrypt: bool) -> Self {
        self.encrypt_data = encrypt;
        self
    }

    /// Returns whether encryption is enabled.
    #[cfg(feature = "aes")]
    pub fn is_encrypted(&self) -> bool {
        self.password.is_some()
    }

    /// Returns whether header encryption is enabled.
    #[cfg(feature = "aes")]
    pub fn is_header_encrypted(&self) -> bool {
        self.encrypt_header && self.password.is_some()
    }

    /// Returns whether data (content) encryption is enabled.
    #[cfg(feature = "aes")]
    pub fn is_data_encrypted(&self) -> bool {
        self.encrypt_data && self.password.is_some()
    }

    /// Returns whether header encryption is enabled (always false without aes feature).
    #[cfg(not(feature = "aes"))]
    pub fn is_header_encrypted(&self) -> bool {
        false
    }

    /// Returns whether data encryption is enabled (always false without aes feature).
    #[cfg(not(feature = "aes"))]
    pub fn is_data_encrypted(&self) -> bool {
        false
    }

    /// Returns whether encryption is enabled (always false without aes feature).
    #[cfg(not(feature = "aes"))]
    pub fn is_encrypted(&self) -> bool {
        false
    }
}

/// Options for solid archive compression.
#[derive(Debug, Clone, Default)]
pub struct SolidOptions {
    /// Whether solid compression is enabled.
    pub enabled: bool,
    /// Maximum bytes per solid block (None = unlimited).
    pub block_size: Option<u64>,
    /// Maximum files per solid block (None = unlimited).
    pub files_per_block: Option<usize>,
}

impl SolidOptions {
    /// Creates disabled solid options.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            block_size: None,
            files_per_block: None,
        }
    }

    /// Creates enabled solid options with defaults.
    ///
    /// Default settings:
    /// - `block_size`: 64 MiB (files are grouped into 64 MB blocks)
    /// - `files_per_block`: None (unlimited files per block)
    ///
    /// The 64 MB default balances compression ratio against memory usage.
    /// Larger blocks generally compress better but require more memory
    /// during both compression and extraction.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            block_size: Some(64 * 1024 * 1024), // 64 MB default block size
            files_per_block: None,
        }
    }

    /// Sets the maximum block size.
    pub fn block_size(mut self, size: u64) -> Self {
        self.block_size = Some(size);
        self
    }

    /// Sets the maximum files per block.
    pub fn files_per_block(mut self, count: usize) -> Self {
        self.files_per_block = Some(count);
        self
    }

    /// Returns whether solid compression is enabled.
    pub fn is_solid(&self) -> bool {
        self.enabled
    }
}

/// LZMA2 encoder variant selection.
///
/// Allows choosing between standard and fast LZMA2 encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Lzma2Variant {
    /// Standard LZMA2 encoder using hash-chain match-finder.
    ///
    /// - Best compression ratio
    /// - Slower at higher compression levels
    /// - Default encoder
    #[default]
    Standard,

    /// Fast LZMA2 encoder using radix match-finder (experimental).
    ///
    /// - 1.5-3x faster compression at levels 5+
    /// - ~1-5% larger output
    /// - Requires `fast-lzma2` feature
    ///
    /// Note: Currently falls back to standard encoder.
    /// Full radix match-finder implementation is planned for future release.
    Fast,
}

impl Lzma2Variant {
    /// Returns true if this is the fast variant.
    pub fn is_fast(&self) -> bool {
        matches!(self, Self::Fast)
    }

    /// Returns true if this is the standard variant.
    pub fn is_standard(&self) -> bool {
        matches!(self, Self::Standard)
    }
}

/// Metadata for an entry being written.
#[derive(Debug, Clone, Default)]
pub struct EntryMeta {
    /// Whether this is a directory.
    pub is_directory: bool,
    /// File size (for files).
    pub size: u64,
    /// Modification time as Windows FILETIME.
    pub modification_time: Option<u64>,
    /// Creation time as Windows FILETIME.
    pub creation_time: Option<u64>,
    /// Access time as Windows FILETIME.
    pub access_time: Option<u64>,
    /// Windows file attributes.
    pub attributes: Option<u32>,
    /// Whether this is an anti-item (marks file for deletion in incremental backups).
    pub is_anti: bool,
}

impl EntryMeta {
    /// Creates metadata for a file.
    pub fn file(size: u64) -> Self {
        Self {
            is_directory: false,
            size,
            ..Default::default()
        }
    }

    /// Creates metadata for a directory.
    pub fn directory() -> Self {
        Self {
            is_directory: true,
            size: 0,
            ..Default::default()
        }
    }

    /// Creates metadata from a filesystem path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the path cannot be read.
    ///
    /// [`Error::Io`]: crate::Error::Io
    pub fn from_path(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self::from_metadata(&metadata))
    }

    /// Creates metadata from std::fs::Metadata.
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            is_directory: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            modification_time: metadata.modified().ok().map(system_time_to_filetime),
            creation_time: metadata.created().ok().map(system_time_to_filetime),
            access_time: metadata.accessed().ok().map(system_time_to_filetime),
            attributes: None, // Platform-specific
            is_anti: false,
        }
    }

    /// Sets modification time.
    pub fn modification_time(mut self, time: u64) -> Self {
        self.modification_time = Some(time);
        self
    }

    /// Sets creation time.
    pub fn creation_time(mut self, time: u64) -> Self {
        self.creation_time = Some(time);
        self
    }

    /// Sets attributes.
    pub fn attributes(mut self, attrs: u32) -> Self {
        self.attributes = Some(attrs);
        self
    }

    /// Creates metadata for an anti-item (file marked for deletion).
    ///
    /// Anti-items are used in incremental backups to mark files that
    /// should be deleted when the archive is applied.
    pub fn anti_item() -> Self {
        Self {
            is_directory: false,
            is_anti: true,
            size: 0,
            ..Default::default()
        }
    }

    /// Creates metadata for an anti-item directory.
    ///
    /// Anti-item directories mark entire directories for deletion.
    pub fn anti_directory() -> Self {
        Self {
            is_directory: true,
            is_anti: true,
            size: 0,
            ..Default::default()
        }
    }

    /// Marks this entry as an anti-item.
    pub fn as_anti(mut self) -> Self {
        self.is_anti = true;
        self.size = 0; // Anti-items have no content
        self
    }
}

/// Converts a SystemTime to Windows FILETIME.
fn system_time_to_filetime(time: std::time::SystemTime) -> u64 {
    use std::time::UNIX_EPOCH;

    // Windows FILETIME is 100-nanosecond intervals since January 1, 1601
    // Unix epoch is January 1, 1970
    // Difference: 116444736000000000 (100-ns intervals)
    const FILETIME_UNIX_DIFF: u64 = 116444736000000000;

    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let hundred_nanos = duration.as_nanos() / 100;
            FILETIME_UNIX_DIFF + hundred_nanos as u64
        }
        Err(e) => {
            // Time is before Unix epoch
            let diff = e.duration();
            let hundred_nanos = diff.as_nanos() / 100;
            FILETIME_UNIX_DIFF.saturating_sub(hundred_nanos as u64)
        }
    }
}

/// Result of writing an archive.
///
/// This struct contains statistics about the archive creation, including
/// the number of entries written, total and compressed sizes, and volume
/// information for multi-volume archives.
#[must_use = "write results should be checked to ensure archive was created successfully"]
#[derive(Debug, Clone, Default)]
pub struct WriteResult {
    /// Number of entries written.
    pub entries_written: usize,
    /// Number of directories written.
    pub directories_written: usize,
    /// Total uncompressed bytes.
    pub total_size: u64,
    /// Total compressed bytes.
    pub compressed_size: u64,
    /// Number of volumes written (1 for single-file archives).
    pub volume_count: u32,
    /// Size of each volume in bytes.
    pub volume_sizes: Vec<u64>,
}

impl WriteResult {
    /// Returns the compression ratio (compressed / uncompressed).
    pub fn compression_ratio(&self) -> f64 {
        if self.total_size == 0 {
            1.0
        } else {
            self.compressed_size as f64 / self.total_size as f64
        }
    }

    /// Returns the space savings percentage.
    pub fn space_savings(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            1.0 - self.compression_ratio()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_options_default() {
        let opts = WriteOptions::default();
        assert_eq!(opts.level, 5);
        assert!(!opts.solid.is_solid());
        assert!(!opts.deterministic);
    }

    #[test]
    fn test_write_options_builder() {
        let opts = WriteOptions::new()
            .method(CodecMethod::Lzma)
            .level(9)
            .unwrap()
            .solid()
            .deterministic(true);

        assert_eq!(opts.method, CodecMethod::Lzma);
        assert_eq!(opts.level, 9);
        assert!(opts.solid.is_solid());
        assert!(opts.deterministic);
    }

    #[test]
    fn test_level_valid() {
        // Valid levels (0-9) should succeed
        for level in 0..=9 {
            let result = WriteOptions::new().level(level);
            assert!(result.is_ok(), "level {} should be valid", level);
            assert_eq!(result.unwrap().level, level);
        }
    }

    #[test]
    fn test_level_invalid() {
        // Levels above 9 should return an error
        for level in [10, 15, 100, u32::MAX] {
            let result = WriteOptions::new().level(level);
            assert!(result.is_err(), "level {} should be invalid", level);
            assert!(matches!(
                result.unwrap_err(),
                crate::Error::InvalidCompressionLevel { level: l } if l == level
            ));
        }
    }

    #[test]
    fn test_level_clamped() {
        // Valid levels pass through unchanged
        assert_eq!(WriteOptions::new().level_clamped(0).level, 0);
        assert_eq!(WriteOptions::new().level_clamped(5).level, 5);
        assert_eq!(WriteOptions::new().level_clamped(9).level, 9);

        // Levels above 9 are clamped to 9
        assert_eq!(WriteOptions::new().level_clamped(10).level, 9);
        assert_eq!(WriteOptions::new().level_clamped(15).level, 9);
        assert_eq!(WriteOptions::new().level_clamped(100).level, 9);
        assert_eq!(WriteOptions::new().level_clamped(u32::MAX).level, 9);
    }

    #[test]
    fn test_delta_filter_valid() {
        // Valid distances (1-255) should work
        let filter = WriteFilter::delta(1);
        assert!(matches!(filter, WriteFilter::Delta { distance: 1 }));

        let filter = WriteFilter::delta(2);
        assert!(matches!(filter, WriteFilter::Delta { distance: 2 }));

        let filter = WriteFilter::delta(255);
        assert!(matches!(filter, WriteFilter::Delta { distance: 255 }));
    }

    #[test]
    #[should_panic(expected = "Delta filter distance must be non-zero")]
    fn test_delta_filter_zero_panics() {
        WriteFilter::delta(0);
    }

    #[test]
    fn test_solid_options() {
        let opts = SolidOptions::enabled()
            .block_size(1024 * 1024)
            .files_per_block(100);

        assert!(opts.is_solid());
        assert_eq!(opts.block_size, Some(1024 * 1024));
        assert_eq!(opts.files_per_block, Some(100));
    }

    #[test]
    fn test_entry_meta_file() {
        let meta = EntryMeta::file(1000);
        assert!(!meta.is_directory);
        assert_eq!(meta.size, 1000);
    }

    #[test]
    fn test_entry_meta_directory() {
        let meta = EntryMeta::directory();
        assert!(meta.is_directory);
        assert_eq!(meta.size, 0);
    }

    #[test]
    fn test_write_result() {
        let result = WriteResult {
            entries_written: 10,
            directories_written: 2,
            total_size: 1000,
            compressed_size: 500,
            volume_count: 1,
            volume_sizes: vec![500],
        };
        assert!((result.compression_ratio() - 0.5).abs() < 0.001);
        assert!((result.space_savings() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_write_result_empty() {
        let result = WriteResult::default();
        assert!((result.compression_ratio() - 1.0).abs() < 0.001);
        assert!((result.space_savings() - 0.0).abs() < 0.001);
    }
}
