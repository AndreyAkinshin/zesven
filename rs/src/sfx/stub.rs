//! SFX stub executable format handling.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::{Error, Result};

/// Supported SFX executable formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfxFormat {
    /// Windows PE executable (.exe).
    WindowsPe,
    /// Linux ELF binary.
    LinuxElf,
    /// macOS Mach-O binary.
    MacOsMachO,
    /// Generic format - just prepend stub without validation.
    Generic,
}

impl SfxFormat {
    /// Detects the format of a stub binary from its magic bytes.
    pub fn detect(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }

        // PE: starts with "MZ"
        if data.len() >= 2 && &data[0..2] == b"MZ" {
            return Some(Self::WindowsPe);
        }

        // ELF: starts with 0x7F "ELF"
        if data.len() >= 4 && &data[0..4] == b"\x7FELF" {
            return Some(Self::LinuxElf);
        }

        // Mach-O: various magic numbers
        if data.len() >= 4 {
            let magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            match magic {
                0xFEEDFACE | 0xFEEDFACF | 0xCAFEBABE | 0xBEBAFECA => {
                    return Some(Self::MacOsMachO);
                }
                _ => {}
            }
            // Little-endian Mach-O
            let magic_le = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            if magic_le == 0xFEEDFACE || magic_le == 0xFEEDFACF {
                return Some(Self::MacOsMachO);
            }
        }

        None
    }

    /// Returns the file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::WindowsPe => "exe",
            Self::LinuxElf => "",
            Self::MacOsMachO => "",
            Self::Generic => "",
        }
    }

    /// Returns a human-readable name for this format.
    pub fn name(&self) -> &'static str {
        match self {
            Self::WindowsPe => "Windows PE",
            Self::LinuxElf => "Linux ELF",
            Self::MacOsMachO => "macOS Mach-O",
            Self::Generic => "Generic",
        }
    }
}

/// Represents an SFX stub executable.
#[derive(Debug, Clone)]
pub struct SfxStub {
    /// The detected or specified format.
    pub format: SfxFormat,
    /// Raw stub executable data.
    pub data: Vec<u8>,
}

impl SfxStub {
    /// Creates a new SFX stub from raw data.
    ///
    /// The format is auto-detected from the data.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let format = SfxFormat::detect(&data).unwrap_or(SfxFormat::Generic);
        Ok(Self { format, data })
    }

    /// Creates a new SFX stub with an explicit format.
    pub fn with_format(data: Vec<u8>, format: SfxFormat) -> Self {
        Self { format, data }
    }

    /// Loads a stub from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let data = std::fs::read(path.as_ref()).map_err(Error::Io)?;
        Self::new(data)
    }

    /// Loads a stub from a reader.
    pub fn from_reader<R: Read>(mut reader: R) -> Result<Self> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data).map_err(Error::Io)?;
        Self::new(data)
    }

    /// Returns the size of the stub in bytes.
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Validates that the stub appears to be a valid executable.
    pub fn validate(&self) -> Result<()> {
        match self.format {
            SfxFormat::WindowsPe => self.validate_pe(),
            SfxFormat::LinuxElf => self.validate_elf(),
            SfxFormat::MacOsMachO => self.validate_macho(),
            SfxFormat::Generic => Ok(()), // No validation for generic
        }
    }

    /// Validates a PE executable structure.
    fn validate_pe(&self) -> Result<()> {
        if self.data.len() < 64 {
            return Err(Error::InvalidFormat("PE stub too small".into()));
        }

        // Check MZ signature
        if &self.data[0..2] != b"MZ" {
            return Err(Error::InvalidFormat("invalid PE signature".into()));
        }

        // Get PE header offset (at offset 0x3C)
        let pe_offset = u32::from_le_bytes([
            self.data[0x3C],
            self.data[0x3D],
            self.data[0x3E],
            self.data[0x3F],
        ]) as usize;

        if pe_offset + 4 > self.data.len() {
            return Err(Error::InvalidFormat(
                "PE header offset out of bounds".into(),
            ));
        }

        // Check PE signature "PE\0\0"
        if &self.data[pe_offset..pe_offset + 4] != b"PE\0\0" {
            return Err(Error::InvalidFormat("invalid PE header signature".into()));
        }

        Ok(())
    }

    /// Validates an ELF executable structure.
    fn validate_elf(&self) -> Result<()> {
        if self.data.len() < 52 {
            // Minimum ELF header size (32-bit)
            return Err(Error::InvalidFormat("ELF stub too small".into()));
        }

        // Check ELF magic
        if &self.data[0..4] != b"\x7FELF" {
            return Err(Error::InvalidFormat("invalid ELF magic".into()));
        }

        // Check ELF class (32 or 64 bit)
        let class = self.data[4];
        if class != 1 && class != 2 {
            return Err(Error::InvalidFormat("invalid ELF class".into()));
        }

        // Check ELF type (should be executable = 2 or shared object = 3)
        let elf_type = u16::from_le_bytes([self.data[16], self.data[17]]);
        if elf_type != 2 && elf_type != 3 {
            return Err(Error::InvalidFormat(
                "ELF is not an executable or shared object".into(),
            ));
        }

        Ok(())
    }

    /// Validates a Mach-O executable structure.
    fn validate_macho(&self) -> Result<()> {
        if self.data.len() < 28 {
            // Minimum Mach-O header size
            return Err(Error::InvalidFormat("Mach-O stub too small".into()));
        }

        let magic = u32::from_be_bytes([self.data[0], self.data[1], self.data[2], self.data[3]]);
        let magic_le = u32::from_le_bytes([self.data[0], self.data[1], self.data[2], self.data[3]]);

        let is_valid_magic = matches!(magic, 0xFEEDFACE | 0xFEEDFACF | 0xCAFEBABE | 0xBEBAFECA)
            || matches!(magic_le, 0xFEEDFACE | 0xFEEDFACF);

        if !is_valid_magic {
            return Err(Error::InvalidFormat("invalid Mach-O magic".into()));
        }

        Ok(())
    }

    /// Writes the stub to a writer.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<u64> {
        writer.write_all(&self.data).map_err(Error::Io)?;
        Ok(self.data.len() as u64)
    }
}

/// Gets the archive offset from an SFX file.
///
/// This reads the stub size from format-specific locations or falls back
/// to searching for the 7z signature.
pub fn get_archive_offset<R: Read + Seek>(reader: &mut R, format: SfxFormat) -> Result<u64> {
    match format {
        SfxFormat::WindowsPe => get_pe_archive_offset(reader),
        SfxFormat::LinuxElf => get_elf_archive_offset(reader),
        SfxFormat::MacOsMachO => get_macho_archive_offset(reader),
        SfxFormat::Generic => {
            // Fall back to signature search
            crate::format::header::find_signature(reader, None)?
                .ok_or_else(|| Error::InvalidFormat("no 7z signature found".into()))
        }
    }
}

/// Gets archive offset from a PE executable.
fn get_pe_archive_offset<R: Read + Seek>(reader: &mut R) -> Result<u64> {
    // For PE files, we typically just search for the 7z signature
    // as the archive is appended after the PE structure
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
    crate::format::header::find_signature(reader, None)?
        .ok_or_else(|| Error::InvalidFormat("no 7z signature found in PE".into()))
}

/// Gets archive offset from an ELF executable.
fn get_elf_archive_offset<R: Read + Seek>(reader: &mut R) -> Result<u64> {
    // For ELF, search for signature after the ELF structure
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
    crate::format::header::find_signature(reader, None)?
        .ok_or_else(|| Error::InvalidFormat("no 7z signature found in ELF".into()))
}

/// Gets archive offset from a Mach-O executable.
fn get_macho_archive_offset<R: Read + Seek>(reader: &mut R) -> Result<u64> {
    // For Mach-O, search for signature after the Mach-O structure
    reader.seek(SeekFrom::Start(0)).map_err(Error::Io)?;
    crate::format::header::find_signature(reader, None)?
        .ok_or_else(|| Error::InvalidFormat("no 7z signature found in Mach-O".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_detection_pe() {
        // Minimal MZ header
        let mut pe_data = vec![0u8; 128];
        pe_data[0] = b'M';
        pe_data[1] = b'Z';
        pe_data[0x3C] = 64; // PE header offset
        pe_data[64] = b'P';
        pe_data[65] = b'E';
        pe_data[66] = 0;
        pe_data[67] = 0;

        assert_eq!(SfxFormat::detect(&pe_data), Some(SfxFormat::WindowsPe));
    }

    #[test]
    fn test_format_detection_elf() {
        let elf_data = b"\x7FELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00";
        assert_eq!(SfxFormat::detect(elf_data), Some(SfxFormat::LinuxElf));
    }

    #[test]
    fn test_format_detection_macho_64() {
        let macho_data = [0xFE, 0xED, 0xFA, 0xCF]; // 64-bit big-endian
        assert_eq!(SfxFormat::detect(&macho_data), Some(SfxFormat::MacOsMachO));
    }

    #[test]
    fn test_format_detection_macho_fat() {
        let macho_data = [0xCA, 0xFE, 0xBA, 0xBE]; // Fat binary
        assert_eq!(SfxFormat::detect(&macho_data), Some(SfxFormat::MacOsMachO));
    }

    #[test]
    fn test_format_detection_unknown() {
        let random_data = [0x00, 0x01, 0x02, 0x03];
        assert_eq!(SfxFormat::detect(&random_data), None);
    }

    #[test]
    fn test_stub_new() {
        let elf_data = b"\x7FELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00".to_vec();
        let stub = SfxStub::new(elf_data).unwrap();
        assert_eq!(stub.format, SfxFormat::LinuxElf);
    }

    #[test]
    fn test_stub_with_format() {
        let data = vec![0u8; 100];
        let stub = SfxStub::with_format(data.clone(), SfxFormat::Generic);
        assert_eq!(stub.format, SfxFormat::Generic);
        assert_eq!(stub.size(), 100);
    }

    #[test]
    fn test_format_extensions() {
        assert_eq!(SfxFormat::WindowsPe.extension(), "exe");
        assert_eq!(SfxFormat::LinuxElf.extension(), "");
        assert_eq!(SfxFormat::MacOsMachO.extension(), "");
    }

    #[test]
    fn test_pe_validation_too_small() {
        let stub = SfxStub::with_format(vec![b'M', b'Z'], SfxFormat::WindowsPe);
        assert!(stub.validate().is_err());
    }

    #[test]
    fn test_elf_validation() {
        // Valid ELF header (64-bit executable)
        let mut elf_data = vec![0u8; 64];
        elf_data[0..4].copy_from_slice(b"\x7FELF");
        elf_data[4] = 2; // 64-bit
        elf_data[5] = 1; // Little endian
        elf_data[16] = 2; // Executable
        elf_data[17] = 0;

        let stub = SfxStub::with_format(elf_data, SfxFormat::LinuxElf);
        assert!(stub.validate().is_ok());
    }

    #[test]
    fn test_write_to() {
        let data = vec![1, 2, 3, 4, 5];
        let stub = SfxStub::with_format(data.clone(), SfxFormat::Generic);

        let mut output = Vec::new();
        let written = stub.write_to(&mut output).unwrap();

        assert_eq!(written, 5);
        assert_eq!(output, data);
    }
}
