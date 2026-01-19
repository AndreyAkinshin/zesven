//! Archive format detection utilities.
//!
//! This module provides automatic detection of archive formats based on
//! file signatures (magic bytes) and file extensions.

use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

/// Detected archive format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    /// 7z archive.
    SevenZip,
    /// ZIP archive.
    Zip,
    /// RAR archive (v4).
    Rar,
    /// RAR5 archive (v5+).
    Rar5,
    /// gzip compressed file.
    Gzip,
    /// TAR archive.
    Tar,
    /// XZ compressed file.
    Xz,
    /// bzip2 compressed file.
    Bzip2,
    /// LZMA compressed file.
    Lzma,
    /// Zstandard compressed file.
    Zstd,
    /// LZ4 compressed file.
    Lz4,
    /// Brotli compressed file.
    Brotli,
    /// Unknown or unrecognized format.
    Unknown,
}

impl ArchiveFormat {
    /// Returns the typical file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZip => "7z",
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Rar => "rar",
            ArchiveFormat::Rar5 => "rar",
            ArchiveFormat::Gzip => "gz",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::Xz => "xz",
            ArchiveFormat::Bzip2 => "bz2",
            ArchiveFormat::Lzma => "lzma",
            ArchiveFormat::Zstd => "zst",
            ArchiveFormat::Lz4 => "lz4",
            ArchiveFormat::Brotli => "br",
            ArchiveFormat::Unknown => "",
        }
    }

    /// Returns a human-readable name for this format.
    pub fn name(&self) -> &'static str {
        match self {
            ArchiveFormat::SevenZip => "7-Zip",
            ArchiveFormat::Zip => "ZIP",
            ArchiveFormat::Rar => "RAR",
            ArchiveFormat::Rar5 => "RAR5",
            ArchiveFormat::Gzip => "gzip",
            ArchiveFormat::Tar => "TAR",
            ArchiveFormat::Xz => "XZ",
            ArchiveFormat::Bzip2 => "bzip2",
            ArchiveFormat::Lzma => "LZMA",
            ArchiveFormat::Zstd => "Zstandard",
            ArchiveFormat::Lz4 => "LZ4",
            ArchiveFormat::Brotli => "Brotli",
            ArchiveFormat::Unknown => "Unknown",
        }
    }

    /// Returns whether this format is supported for reading by zesven.
    ///
    /// Currently only 7z archives are fully supported.
    pub fn is_supported(&self) -> bool {
        matches!(self, ArchiveFormat::SevenZip)
    }
}

impl std::fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Format detection result.
#[derive(Debug, Clone)]
pub struct FormatInfo {
    /// Detected archive format.
    pub format: ArchiveFormat,
    /// Offset to archive data (non-zero for SFX archives).
    pub offset: u64,
    /// Confidence level (0.0-1.0).
    ///
    /// - 1.0: Exact signature match
    /// - 0.5: Extension-based detection only
    /// - 0.0: Unknown format
    pub confidence: f32,
}

impl FormatInfo {
    /// Creates a new FormatInfo with signature-based detection (high confidence).
    pub fn from_signature(format: ArchiveFormat, offset: u64) -> Self {
        Self {
            format,
            offset,
            confidence: 1.0,
        }
    }

    /// Creates a new FormatInfo with extension-based detection (medium confidence).
    pub fn from_extension(format: ArchiveFormat) -> Self {
        Self {
            format,
            offset: 0,
            confidence: 0.5,
        }
    }

    /// Creates a FormatInfo for unknown format.
    pub fn unknown() -> Self {
        Self {
            format: ArchiveFormat::Unknown,
            offset: 0,
            confidence: 0.0,
        }
    }
}

/// Known archive format signatures.
///
/// Each entry is (signature bytes, format, optional version check).
const SIGNATURES: &[(&[u8], ArchiveFormat)] = &[
    // 7z: '7' 'z' 0xBC 0xAF 0x27 0x1C
    (
        &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
        ArchiveFormat::SevenZip,
    ),
    // ZIP: 'P' 'K' 0x03 0x04 (local file header)
    (&[0x50, 0x4B, 0x03, 0x04], ArchiveFormat::Zip),
    // ZIP: 'P' 'K' 0x05 0x06 (empty archive)
    (&[0x50, 0x4B, 0x05, 0x06], ArchiveFormat::Zip),
    // RAR: 'R' 'a' 'r' '!' 0x1A 0x07 0x00
    (
        &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00],
        ArchiveFormat::Rar,
    ),
    // RAR5: 'R' 'a' 'r' '!' 0x1A 0x07 0x01 0x00
    (
        &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00],
        ArchiveFormat::Rar5,
    ),
    // gzip: 0x1F 0x8B
    (&[0x1F, 0x8B], ArchiveFormat::Gzip),
    // XZ: 0xFD '7' 'z' 'X' 'Z' 0x00
    (&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00], ArchiveFormat::Xz),
    // bzip2: 'B' 'Z' 'h'
    (&[0x42, 0x5A, 0x68], ArchiveFormat::Bzip2),
    // Zstd: 0x28 0xB5 0x2F 0xFD
    (&[0x28, 0xB5, 0x2F, 0xFD], ArchiveFormat::Zstd),
    // LZ4: 0x04 0x22 0x4D 0x18 (frame format)
    (&[0x04, 0x22, 0x4D, 0x18], ArchiveFormat::Lz4),
];

/// TAR USTAR signature at offset 257.
const TAR_USTAR_SIGNATURE: &[u8] = b"ustar";

/// Detects the archive format from a reader by examining magic bytes.
///
/// This function reads the first few bytes of the input to identify the
/// archive format based on known signatures.
///
/// # Arguments
///
/// * `reader` - A seekable reader positioned at the start of the file
///
/// # Returns
///
/// Returns `FormatInfo` containing the detected format, offset, and confidence.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::format::detect::detect_format;
/// use std::fs::File;
///
/// let mut file = File::open("archive.7z")?;
/// let info = detect_format(&mut file)?;
/// println!("Detected format: {} (confidence: {:.0}%)",
///          info.format, info.confidence * 100.0);
/// ```
pub fn detect_format<R: Read + Seek>(reader: &mut R) -> Result<FormatInfo> {
    // Save the current position
    let start_pos = reader.stream_position().map_err(Error::Io)?;

    // Read first 16 bytes for signature detection
    let mut header = [0u8; 16];
    let bytes_read = reader.read(&mut header).map_err(Error::Io)?;

    // Try to match against known signatures
    for (signature, format) in SIGNATURES {
        if bytes_read >= signature.len() && header.starts_with(signature) {
            // Restore position
            reader.seek(SeekFrom::Start(start_pos)).map_err(Error::Io)?;
            return Ok(FormatInfo::from_signature(*format, 0));
        }
    }

    // Check for TAR format (USTAR signature at offset 257)
    if bytes_read >= 16 {
        reader
            .seek(SeekFrom::Start(start_pos + 257))
            .map_err(Error::Io)?;
        let mut tar_header = [0u8; 5];
        if reader.read(&mut tar_header).map_err(Error::Io)? == 5
            && tar_header == *TAR_USTAR_SIGNATURE
        {
            reader.seek(SeekFrom::Start(start_pos)).map_err(Error::Io)?;
            return Ok(FormatInfo::from_signature(ArchiveFormat::Tar, 0));
        }
    }

    // Check for LZMA (no magic, but first byte often 0x5D for default props)
    // This is a heuristic detection
    if bytes_read >= 5 && header[0] == 0x5D && header[1] == 0x00 && header[2] == 0x00 {
        reader.seek(SeekFrom::Start(start_pos)).map_err(Error::Io)?;
        return Ok(FormatInfo {
            format: ArchiveFormat::Lzma,
            offset: 0,
            confidence: 0.7, // Lower confidence for heuristic detection
        });
    }

    // Restore position
    reader.seek(SeekFrom::Start(start_pos)).map_err(Error::Io)?;

    // No signature matched
    Ok(FormatInfo::unknown())
}

/// Detects the archive format from a file extension.
///
/// # Arguments
///
/// * `extension` - The file extension (without the leading dot)
///
/// # Returns
///
/// Returns the detected `ArchiveFormat`.
///
/// # Example
///
/// ```rust
/// use zesven::format::detect::{detect_format_from_extension, ArchiveFormat};
///
/// assert_eq!(detect_format_from_extension("7z"), ArchiveFormat::SevenZip);
/// assert_eq!(detect_format_from_extension("zip"), ArchiveFormat::Zip);
/// assert_eq!(detect_format_from_extension("unknown"), ArchiveFormat::Unknown);
/// ```
pub fn detect_format_from_extension(extension: &str) -> ArchiveFormat {
    match extension.to_lowercase().as_str() {
        "7z" => ArchiveFormat::SevenZip,
        "zip" | "jar" | "war" | "apk" | "ipa" => ArchiveFormat::Zip,
        "rar" => ArchiveFormat::Rar,
        "gz" | "gzip" | "tgz" => ArchiveFormat::Gzip,
        "tar" => ArchiveFormat::Tar,
        "xz" | "txz" => ArchiveFormat::Xz,
        "bz2" | "bzip2" | "tbz2" => ArchiveFormat::Bzip2,
        "lzma" => ArchiveFormat::Lzma,
        "zst" | "zstd" => ArchiveFormat::Zstd,
        "lz4" => ArchiveFormat::Lz4,
        "br" | "brotli" => ArchiveFormat::Brotli,
        _ => ArchiveFormat::Unknown,
    }
}

/// Detects the archive format from a file path.
///
/// This function first tries signature-based detection by reading the file,
/// then falls back to extension-based detection if the signature is unknown.
///
/// # Arguments
///
/// * `reader` - A seekable reader
/// * `path` - Optional file path for extension-based fallback
///
/// # Returns
///
/// Returns `FormatInfo` with the best available detection result.
pub fn detect_format_with_fallback<R: Read + Seek>(
    reader: &mut R,
    extension: Option<&str>,
) -> Result<FormatInfo> {
    // Try signature-based detection first
    let info = detect_format(reader)?;

    if info.format != ArchiveFormat::Unknown {
        return Ok(info);
    }

    // Fall back to extension-based detection
    if let Some(ext) = extension {
        let format = detect_format_from_extension(ext);
        if format != ArchiveFormat::Unknown {
            return Ok(FormatInfo::from_extension(format));
        }
    }

    Ok(FormatInfo::unknown())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_detect_7z_signature() {
        let data = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x04];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::SevenZip);
        assert_eq!(info.confidence, 1.0);
    }

    #[test]
    fn test_detect_zip_signature() {
        let data = [0x50, 0x4B, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Zip);
        assert_eq!(info.confidence, 1.0);
    }

    #[test]
    fn test_detect_rar_signature() {
        let data = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Rar);
    }

    #[test]
    fn test_detect_rar5_signature() {
        let data = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Rar5);
    }

    #[test]
    fn test_detect_gzip_signature() {
        let data = [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Gzip);
    }

    #[test]
    fn test_detect_xz_signature() {
        let data = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Xz);
    }

    #[test]
    fn test_detect_bzip2_signature() {
        let data = [0x42, 0x5A, 0x68, 0x39, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Bzip2);
    }

    #[test]
    fn test_detect_zstd_signature() {
        let data = [0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Zstd);
    }

    #[test]
    fn test_detect_lz4_signature() {
        let data = [0x04, 0x22, 0x4D, 0x18, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Lz4);
    }

    #[test]
    fn test_detect_unknown() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format(&mut cursor).unwrap();
        assert_eq!(info.format, ArchiveFormat::Unknown);
        assert_eq!(info.confidence, 0.0);
    }

    #[test]
    fn test_detect_format_from_extension() {
        assert_eq!(detect_format_from_extension("7z"), ArchiveFormat::SevenZip);
        assert_eq!(detect_format_from_extension("zip"), ArchiveFormat::Zip);
        assert_eq!(detect_format_from_extension("ZIP"), ArchiveFormat::Zip);
        assert_eq!(detect_format_from_extension("rar"), ArchiveFormat::Rar);
        assert_eq!(detect_format_from_extension("gz"), ArchiveFormat::Gzip);
        assert_eq!(detect_format_from_extension("tgz"), ArchiveFormat::Gzip);
        assert_eq!(detect_format_from_extension("tar"), ArchiveFormat::Tar);
        assert_eq!(detect_format_from_extension("xz"), ArchiveFormat::Xz);
        assert_eq!(detect_format_from_extension("bz2"), ArchiveFormat::Bzip2);
        assert_eq!(detect_format_from_extension("lzma"), ArchiveFormat::Lzma);
        assert_eq!(detect_format_from_extension("zst"), ArchiveFormat::Zstd);
        assert_eq!(detect_format_from_extension("lz4"), ArchiveFormat::Lz4);
        assert_eq!(detect_format_from_extension("br"), ArchiveFormat::Brotli);
        assert_eq!(
            detect_format_from_extension("unknown"),
            ArchiveFormat::Unknown
        );
    }

    #[test]
    fn test_detect_with_fallback() {
        // Signature match takes precedence
        let data = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x04];
        let mut cursor = Cursor::new(&data);
        let info = detect_format_with_fallback(&mut cursor, Some("zip")).unwrap();
        assert_eq!(info.format, ArchiveFormat::SevenZip);
        assert_eq!(info.confidence, 1.0);

        // Extension fallback when signature unknown
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&data);
        let info = detect_format_with_fallback(&mut cursor, Some("7z")).unwrap();
        assert_eq!(info.format, ArchiveFormat::SevenZip);
        assert_eq!(info.confidence, 0.5);
    }

    #[test]
    fn test_format_display() {
        assert_eq!(format!("{}", ArchiveFormat::SevenZip), "7-Zip");
        assert_eq!(format!("{}", ArchiveFormat::Zip), "ZIP");
    }

    #[test]
    fn test_format_extension() {
        assert_eq!(ArchiveFormat::SevenZip.extension(), "7z");
        assert_eq!(ArchiveFormat::Zip.extension(), "zip");
    }

    #[test]
    fn test_format_is_supported() {
        assert!(ArchiveFormat::SevenZip.is_supported());
        assert!(!ArchiveFormat::Zip.is_supported());
        assert!(!ArchiveFormat::Unknown.is_supported());
    }

    #[test]
    fn test_reader_position_restored() {
        let data = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x04];
        let mut cursor = Cursor::new(&data);

        // Set position to 2
        cursor.seek(SeekFrom::Start(2)).unwrap();

        let _info = detect_format(&mut cursor).unwrap();

        // Position should be restored
        assert_eq!(cursor.position(), 2);
    }
}
