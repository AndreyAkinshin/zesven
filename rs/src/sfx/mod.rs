//! Self-extracting archive (SFX) creation.
//!
//! This module provides functionality to create self-extracting archives
//! by combining an executable stub with a 7z archive.
//!
//! # Supported Formats
//!
//! - **Windows PE**: Creates `.exe` files that run on Windows
//! - **Linux ELF**: Creates executable binaries for Linux
//! - **macOS Mach-O**: Creates executables for macOS
//! - **Generic**: Simple concatenation for custom stubs
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::sfx::{SfxBuilder, SfxConfig, SfxFormat, SfxStub};
//! use std::fs::File;
//!
//! // Load a stub executable
//! let stub = SfxStub::from_file("7zS.sfx")?;
//!
//! // Configure the SFX behavior
//! let config = SfxConfig::new()
//!     .title("My Installer")
//!     .run_program("setup.exe")
//!     .progress(true);
//!
//! // Create the archive data first
//! let archive_data = create_archive()?;
//!
//! // Build the SFX
//! let mut output = File::create("installer.exe")?;
//! let result = SfxBuilder::new()
//!     .stub(stub)
//!     .config(config)
//!     .build(&mut output, &archive_data)?;
//!
//! println!("Created SFX: {} bytes (stub: {}, archive: {})",
//!     result.total_size, result.stub_size, result.archive_size);
//! ```
//!
//! # Architecture
//!
//! An SFX archive has the following structure:
//!
//! ```text
//! +------------------+
//! |   Stub (.exe)    |  <- Executable that extracts the archive
//! +------------------+
//! |  Config Block    |  <- Optional: ;!@Install@!UTF-8! format
//! +------------------+
//! |   7z Archive     |  <- The actual compressed data
//! +------------------+
//! ```
//!
//! The stub executable contains code that:
//! 1. Finds the 7z archive appended to itself
//! 2. Extracts the archive contents
//! 3. Optionally runs a program after extraction

pub mod config;
pub mod stub;

pub use config::SfxConfig;
pub use stub::{SfxFormat, SfxStub};

use std::io::Write;

use crate::{Error, Result};

/// Result of creating an SFX archive.
#[must_use = "SFX result should be checked to verify creation completed successfully"]
#[derive(Debug, Clone)]
pub struct SfxResult {
    /// Total size of the SFX file in bytes.
    pub total_size: u64,
    /// Size of the stub executable in bytes.
    pub stub_size: u64,
    /// Size of the configuration block in bytes.
    pub config_size: u64,
    /// Size of the 7z archive in bytes.
    pub archive_size: u64,
}

impl SfxResult {
    /// Returns the overhead (stub + config) as a percentage of total size.
    pub fn overhead_percent(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            ((self.stub_size + self.config_size) as f64 / self.total_size as f64) * 100.0
        }
    }
}

/// Builder for creating self-extracting archives.
///
/// Use this to combine a stub executable, configuration, and 7z archive
/// into a single SFX file.
#[derive(Debug, Default)]
pub struct SfxBuilder {
    stub: Option<SfxStub>,
    config: SfxConfig,
    validate_stub: bool,
}

impl SfxBuilder {
    /// Creates a new SFX builder with default settings.
    pub fn new() -> Self {
        Self {
            stub: None,
            config: SfxConfig::default(),
            validate_stub: true,
        }
    }

    /// Sets the stub executable to use.
    pub fn stub(mut self, stub: SfxStub) -> Self {
        self.stub = Some(stub);
        self
    }

    /// Sets the SFX configuration.
    pub fn config(mut self, config: SfxConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets whether to validate the stub executable format.
    ///
    /// Default is true. Set to false to skip validation for custom stubs.
    pub fn validate_stub(mut self, validate: bool) -> Self {
        self.validate_stub = validate;
        self
    }

    /// Builds the SFX archive and writes it to the output.
    ///
    /// # Arguments
    ///
    /// * `output` - The writer to write the SFX to
    /// * `archive_data` - The 7z archive data to embed
    ///
    /// # Returns
    ///
    /// Returns information about the created SFX including sizes.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No stub is provided
    /// - The stub fails validation (if enabled)
    /// - Writing to output fails
    pub fn build<W: Write>(self, output: &mut W, archive_data: &[u8]) -> Result<SfxResult> {
        let stub = self
            .stub
            .ok_or_else(|| Error::InvalidFormat("no SFX stub provided".into()))?;

        // Validate stub if requested
        if self.validate_stub {
            stub.validate()?;
        }

        // Encode config
        let config_data = self.config.encode();

        // Write stub
        let stub_size = stub.write_to(output)?;

        // Write config (if any)
        if !config_data.is_empty() {
            output.write_all(&config_data).map_err(Error::Io)?;
        }

        // Write archive
        output.write_all(archive_data).map_err(Error::Io)?;

        let config_size = config_data.len() as u64;
        let archive_size = archive_data.len() as u64;
        let total_size = stub_size + config_size + archive_size;

        Ok(SfxResult {
            total_size,
            stub_size,
            config_size,
            archive_size,
        })
    }

    /// Builds the SFX archive to a file path.
    ///
    /// This is a convenience method that creates the file and calls `build`.
    pub fn build_to_path(
        self,
        path: impl AsRef<std::path::Path>,
        archive_data: &[u8],
    ) -> Result<SfxResult> {
        let mut file = std::fs::File::create(path).map_err(Error::Io)?;
        let result = self.build(&mut file, archive_data)?;

        // Set executable permission on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata().map_err(Error::Io)?.permissions();
            perms.set_mode(perms.mode() | 0o111); // Add execute permission
            file.set_permissions(perms).map_err(Error::Io)?;
        }

        Ok(result)
    }
}

/// Creates an SFX archive from raw components.
///
/// This is a lower-level function for direct SFX creation without
/// the builder pattern.
///
/// # Arguments
///
/// * `output` - The writer to write the SFX to
/// * `stub` - The stub executable data
/// * `config` - Optional configuration (pass `None` for no config)
/// * `archive` - The 7z archive data
///
/// # Returns
///
/// Returns the total number of bytes written.
pub fn create_sfx<W: Write>(
    output: &mut W,
    stub: &[u8],
    config: Option<&SfxConfig>,
    archive: &[u8],
) -> Result<u64> {
    // Write stub
    output.write_all(stub).map_err(Error::Io)?;
    let mut total = stub.len() as u64;

    // Write config if provided
    if let Some(cfg) = config {
        let config_data = cfg.encode();
        if !config_data.is_empty() {
            output.write_all(&config_data).map_err(Error::Io)?;
            total += config_data.len() as u64;
        }
    }

    // Write archive
    output.write_all(archive).map_err(Error::Io)?;
    total += archive.len() as u64;

    Ok(total)
}

/// Extracts the embedded 7z archive from an SFX file.
///
/// This reads an SFX file and returns just the 7z archive portion,
/// stripping the stub and config.
///
/// # Arguments
///
/// * `sfx_data` - The complete SFX file data
///
/// # Returns
///
/// Returns the 7z archive data and information about the SFX structure.
pub fn extract_archive_from_sfx(sfx_data: &[u8]) -> Result<(Vec<u8>, SfxInfo)> {
    use std::io::Cursor;

    let mut cursor = Cursor::new(sfx_data);
    let sfx_info = crate::format::header::detect_sfx(&mut cursor)?;

    let archive_offset = match sfx_info {
        Some(info) => info.archive_offset,
        None => 0, // Regular archive, no stub
    };

    if archive_offset as usize >= sfx_data.len() {
        return Err(Error::InvalidFormat(
            "archive offset beyond file end".into(),
        ));
    }

    let archive_data = sfx_data[archive_offset as usize..].to_vec();

    Ok((
        archive_data,
        SfxInfo {
            archive_offset,
            stub_size: archive_offset,
            format: SfxFormat::detect(sfx_data),
        },
    ))
}

/// Information about an SFX archive structure.
#[derive(Debug, Clone)]
pub struct SfxInfo {
    /// Offset where the 7z archive begins.
    pub archive_offset: u64,
    /// Size of the stub (same as archive_offset).
    pub stub_size: u64,
    /// Detected format of the stub executable.
    pub format: Option<SfxFormat>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn create_minimal_archive() -> Vec<u8> {
        // 7z signature + minimal headers (empty archive)
        let mut data = Vec::new();
        // 7z signature
        data.extend_from_slice(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]);
        // Version (0.4)
        data.extend_from_slice(&[0x00, 0x04]);
        // Start header CRC
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        // Next header offset (0)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Next header size (0)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Next header CRC
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        data
    }

    fn create_fake_pe_stub() -> Vec<u8> {
        let mut stub = vec![0u8; 256];
        // MZ signature
        stub[0] = b'M';
        stub[1] = b'Z';
        // PE header offset at 0x3C
        stub[0x3C] = 64;
        // PE signature at offset 64
        stub[64] = b'P';
        stub[65] = b'E';
        stub[66] = 0;
        stub[67] = 0;
        stub
    }

    fn create_fake_elf_stub() -> Vec<u8> {
        let mut stub = vec![0u8; 64];
        // ELF magic
        stub[0..4].copy_from_slice(b"\x7FELF");
        stub[4] = 2; // 64-bit
        stub[5] = 1; // Little endian
        stub[16] = 2; // Executable type
        stub[17] = 0;
        stub
    }

    #[test]
    fn test_sfx_builder_no_stub() {
        let builder = SfxBuilder::new();
        let archive = create_minimal_archive();
        let mut output = Vec::new();
        assert!(builder.build(&mut output, &archive).is_err());
    }

    #[test]
    fn test_sfx_builder_with_stub() {
        let stub_data = create_fake_pe_stub();
        let stub = SfxStub::with_format(stub_data.clone(), SfxFormat::WindowsPe);
        let archive = create_minimal_archive();

        let mut output = Vec::new();
        let result = SfxBuilder::new()
            .stub(stub)
            .validate_stub(false) // Skip validation for this test
            .build(&mut output, &archive)
            .unwrap();

        assert_eq!(result.stub_size, stub_data.len() as u64);
        assert_eq!(result.archive_size, archive.len() as u64);
        assert_eq!(result.config_size, 0);
        assert_eq!(
            result.total_size,
            stub_data.len() as u64 + archive.len() as u64
        );
    }

    #[test]
    fn test_sfx_builder_with_config() {
        let stub_data = create_fake_pe_stub();
        let stub = SfxStub::with_format(stub_data, SfxFormat::WindowsPe);
        let config = SfxConfig::new().title("Test Installer").progress(true);
        let archive = create_minimal_archive();

        let mut output = Vec::new();
        let result = SfxBuilder::new()
            .stub(stub)
            .config(config)
            .validate_stub(false)
            .build(&mut output, &archive)
            .unwrap();

        assert!(result.config_size > 0);
        assert!(result.total_size > result.stub_size + result.archive_size);

        // Verify config is in output
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains(";!@Install@!UTF-8!"));
        assert!(output_str.contains("Title=\"Test Installer\""));
    }

    #[test]
    fn test_sfx_result_overhead() {
        let result = SfxResult {
            total_size: 1000,
            stub_size: 100,
            config_size: 50,
            archive_size: 850,
        };

        assert!((result.overhead_percent() - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_create_sfx_function() {
        let stub = create_fake_elf_stub();
        let archive = create_minimal_archive();
        let config = SfxConfig::new().title("Test");

        let mut output = Vec::new();
        let total = create_sfx(&mut output, &stub, Some(&config), &archive).unwrap();

        assert!(total > (stub.len() + archive.len()) as u64); // Config adds bytes
        assert_eq!(output.len(), total as usize);
    }

    #[test]
    fn test_create_sfx_no_config() {
        let stub = vec![1, 2, 3, 4, 5];
        let archive = create_minimal_archive();

        let mut output = Vec::new();
        let total = create_sfx(&mut output, &stub, None, &archive).unwrap();

        assert_eq!(total, (stub.len() + archive.len()) as u64);
    }

    #[test]
    fn test_extract_archive_from_sfx() {
        let stub = create_fake_pe_stub();
        let archive = create_minimal_archive();

        // Create SFX
        let mut sfx_data = Vec::new();
        create_sfx(&mut sfx_data, &stub, None, &archive).unwrap();

        // Extract archive
        let (extracted, info) = extract_archive_from_sfx(&sfx_data).unwrap();

        assert_eq!(info.archive_offset, stub.len() as u64);
        assert_eq!(extracted, archive);
        assert_eq!(info.format, Some(SfxFormat::WindowsPe));
    }

    #[test]
    fn test_sfx_roundtrip_pe() {
        let stub_data = create_fake_pe_stub();
        let stub = SfxStub::with_format(stub_data.clone(), SfxFormat::WindowsPe);
        let archive = create_minimal_archive();

        // Create SFX
        let mut sfx_data = Vec::new();
        let _result = SfxBuilder::new()
            .stub(stub)
            .validate_stub(false)
            .build(&mut sfx_data, &archive)
            .unwrap();

        // Detect SFX
        let mut cursor = Cursor::new(&sfx_data);
        let detected = crate::format::header::detect_sfx(&mut cursor).unwrap();

        assert!(detected.is_some());
        let info = detected.unwrap();
        assert_eq!(info.archive_offset, stub_data.len() as u64);
    }

    #[test]
    fn test_sfx_roundtrip_elf() {
        let stub_data = create_fake_elf_stub();
        let stub = SfxStub::with_format(stub_data.clone(), SfxFormat::LinuxElf);
        let archive = create_minimal_archive();

        // Create SFX
        let mut sfx_data = Vec::new();
        let _result = SfxBuilder::new()
            .stub(stub)
            .validate_stub(false)
            .build(&mut sfx_data, &archive)
            .unwrap();

        // Verify we can find the archive
        let mut cursor = Cursor::new(&sfx_data);
        let detected = crate::format::header::detect_sfx(&mut cursor).unwrap();

        assert!(detected.is_some());
        assert_eq!(detected.unwrap().archive_offset, stub_data.len() as u64);
    }
}
