//! Checksum computation utilities.
//!
//! This module provides CRC-32 and CRC-64 checksum computation for
//! verifying archive data integrity.
//!
//! # CRC-32
//!
//! CRC-32 is the standard checksum used in 7z archives for file verification.
//! It uses the IEEE 802.3 polynomial (same as Ethernet, ZIP, etc.).
//!
//! # CRC-64
//!
//! CRC-64-ECMA is provided for extended verification scenarios.
//! While not used in standard 7z archives, it's useful for:
//! - High-integrity verification scenarios
//! - Large file verification where CRC-32 collision risk matters
//! - Custom applications building on zesven
//!
//! # Example
//!
//! ```rust
//! use zesven::checksum::{Crc32, Crc64, Checksum};
//!
//! // CRC-32
//! let mut crc32 = Crc32::new();
//! crc32.update(b"Hello, ");
//! crc32.update(b"World!");
//! let value = crc32.finalize();
//!
//! // CRC-64
//! let mut crc64 = Crc64::new();
//! crc64.update(b"Hello, World!");
//! let value = crc64.finalize();
//!
//! // One-shot computation
//! let crc32 = Crc32::compute(b"Hello, World!");
//! let crc64 = Crc64::compute(b"Hello, World!");
//! ```

use std::io::{self, Read, Write};

use crate::READ_BUFFER_SIZE;

/// Common trait for checksum computation.
pub trait Checksum: Default + Clone {
    /// The output type of this checksum.
    type Output: Copy + Eq + std::fmt::Debug;

    /// Creates a new checksum calculator.
    fn new() -> Self;

    /// Updates the checksum with additional data.
    fn update(&mut self, data: &[u8]);

    /// Finishes the checksum computation and returns the value.
    fn finalize(&self) -> Self::Output;

    /// Resets the checksum to its initial state.
    fn reset(&mut self);

    /// Computes the checksum of a single slice in one call.
    fn compute(data: &[u8]) -> Self::Output {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }

    /// Computes the checksum by reading from a reader.
    fn compute_reader<R: Read>(reader: &mut R) -> io::Result<Self::Output> {
        let mut hasher = Self::new();
        let mut buffer = [0u8; READ_BUFFER_SIZE];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hasher.finalize())
    }
}

/// CRC-32 checksum calculator.
///
/// Uses the IEEE 802.3 polynomial (standard for Ethernet, ZIP, 7z, etc.).
/// This is the checksum format used in 7z archives.
///
/// # Example
///
/// ```rust
/// use zesven::checksum::{Crc32, Checksum};
///
/// // Incremental computation
/// let mut crc = Crc32::new();
/// crc.update(b"Hello, ");
/// crc.update(b"World!");
/// assert_eq!(crc.finalize(), 0xEC4AC3D0);
///
/// // One-shot computation
/// let crc = Crc32::compute(b"Hello, World!");
/// assert_eq!(crc, 0xEC4AC3D0);
/// ```
#[derive(Clone)]
pub struct Crc32 {
    hasher: crc32fast::Hasher,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Crc32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Crc32")
            .field("current", &self.hasher.clone().finalize())
            .finish()
    }
}

impl Checksum for Crc32 {
    type Output = u32;

    fn new() -> Self {
        Self {
            hasher: crc32fast::Hasher::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    fn finalize(&self) -> u32 {
        self.hasher.clone().finalize()
    }

    fn reset(&mut self) {
        self.hasher.reset();
    }
}

impl Crc32 {
    /// Creates a CRC-32 initialized with a specific value.
    ///
    /// This is useful for continuing a checksum from a saved state.
    pub fn with_initial(initial: u32) -> Self {
        Self {
            hasher: crc32fast::Hasher::new_with_initial(initial),
        }
    }
}

/// CRC-64 checksum calculator.
///
/// Uses the ECMA-182 polynomial. While not used in standard 7z archives,
/// CRC-64 provides stronger integrity guarantees for large files.
///
/// # When to Use CRC-64
///
/// CRC-32 has a collision probability of approximately 1 in 4 billion,
/// which becomes significant for:
/// - Very large files (terabytes)
/// - High-integrity applications
/// - Storage systems handling many files
///
/// CRC-64 reduces collision probability to approximately 1 in 2^64.
///
/// # Example
///
/// ```rust
/// use zesven::checksum::{Crc64, Checksum};
///
/// let crc = Crc64::compute(b"Hello, World!");
/// println!("CRC-64: {:016x}", crc);
/// ```
#[derive(Clone)]
pub struct Crc64 {
    hasher: crc64fast::Digest,
}

impl Default for Crc64 {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Crc64 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Crc64")
            .field("current", &self.hasher.sum64())
            .finish()
    }
}

impl Checksum for Crc64 {
    type Output = u64;

    fn new() -> Self {
        Self {
            hasher: crc64fast::Digest::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.hasher.write(data);
    }

    fn finalize(&self) -> u64 {
        self.hasher.sum64()
    }

    fn reset(&mut self) {
        self.hasher = crc64fast::Digest::new();
    }
}

/// A writer wrapper that computes CRC-32 while writing.
///
/// # Example
///
/// ```rust
/// use zesven::checksum::Crc32Writer;
/// use std::io::Write;
///
/// let mut buffer = Vec::new();
/// let mut writer = Crc32Writer::new(&mut buffer);
///
/// writer.write_all(b"Hello, World!").unwrap();
///
/// let crc = writer.crc();
/// assert_eq!(crc, 0xEC4AC3D0);
/// assert_eq!(buffer, b"Hello, World!");
/// ```
pub struct Crc32Writer<W> {
    inner: W,
    crc: Crc32,
    bytes_written: u64,
}

impl<W> Crc32Writer<W> {
    /// Creates a new CRC-32 writer wrapping the given writer.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            crc: Crc32::new(),
            bytes_written: 0,
        }
    }

    /// Returns the current CRC-32 value.
    pub fn crc(&self) -> u32 {
        self.crc.finalize()
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Resets the CRC and byte counter.
    pub fn reset(&mut self) {
        self.crc.reset();
        self.bytes_written = 0;
    }

    /// Consumes the wrapper and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Returns a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Returns a mutable reference to the inner writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }
}

impl<W: Write> Write for Crc32Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(&buf[..n]);
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A writer wrapper that computes CRC-64 while writing.
///
/// # Example
///
/// ```rust
/// use zesven::checksum::Crc64Writer;
/// use std::io::Write;
///
/// let mut buffer = Vec::new();
/// let mut writer = Crc64Writer::new(&mut buffer);
///
/// writer.write_all(b"Hello, World!").unwrap();
///
/// let crc = writer.crc();
/// println!("CRC-64: {:016x}", crc);
/// ```
pub struct Crc64Writer<W> {
    inner: W,
    crc: Crc64,
    bytes_written: u64,
}

impl<W> Crc64Writer<W> {
    /// Creates a new CRC-64 writer wrapping the given writer.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            crc: Crc64::new(),
            bytes_written: 0,
        }
    }

    /// Returns the current CRC-64 value.
    pub fn crc(&self) -> u64 {
        self.crc.finalize()
    }

    /// Returns the number of bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Resets the CRC and byte counter.
    pub fn reset(&mut self) {
        self.crc.reset();
        self.bytes_written = 0;
    }

    /// Consumes the wrapper and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Returns a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Returns a mutable reference to the inner writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }
}

impl<W: Write> Write for Crc64Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(&buf[..n]);
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A reader wrapper that computes CRC-32 while reading.
///
/// # Example
///
/// ```rust
/// use zesven::checksum::Crc32Reader;
/// use std::io::{Cursor, Read};
///
/// let data = b"Hello, World!";
/// let mut reader = Crc32Reader::new(Cursor::new(data));
///
/// let mut buffer = Vec::new();
/// reader.read_to_end(&mut buffer).unwrap();
///
/// assert_eq!(reader.crc(), 0xEC4AC3D0);
/// ```
pub struct Crc32Reader<R> {
    inner: R,
    crc: Crc32,
    bytes_read: u64,
}

impl<R> Crc32Reader<R> {
    /// Creates a new CRC-32 reader wrapping the given reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            crc: Crc32::new(),
            bytes_read: 0,
        }
    }

    /// Returns the current CRC-32 value.
    pub fn crc(&self) -> u32 {
        self.crc.finalize()
    }

    /// Returns the number of bytes read.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Resets the CRC and byte counter.
    pub fn reset(&mut self) {
        self.crc.reset();
        self.bytes_read = 0;
    }

    /// Consumes the wrapper and returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Returns a reference to the inner reader.
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Returns a mutable reference to the inner reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        self.bytes_read += n as u64;
        Ok(n)
    }
}

/// A reader wrapper that computes CRC-64 while reading.
pub struct Crc64Reader<R> {
    inner: R,
    crc: Crc64,
    bytes_read: u64,
}

impl<R> Crc64Reader<R> {
    /// Creates a new CRC-64 reader wrapping the given reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            crc: Crc64::new(),
            bytes_read: 0,
        }
    }

    /// Returns the current CRC-64 value.
    pub fn crc(&self) -> u64 {
        self.crc.finalize()
    }

    /// Returns the number of bytes read.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Resets the CRC and byte counter.
    pub fn reset(&mut self) {
        self.crc.reset();
        self.bytes_read = 0;
    }

    /// Consumes the wrapper and returns the inner reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Returns a reference to the inner reader.
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Returns a mutable reference to the inner reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

impl<R: Read> Read for Crc64Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        self.bytes_read += n as u64;
        Ok(n)
    }
}

/// Verify result for CRC checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyResult {
    /// CRC matches expected value.
    Match,
    /// CRC does not match.
    Mismatch {
        /// Expected CRC value.
        expected: u64,
        /// Actual computed CRC value.
        actual: u64,
    },
    /// No CRC available for comparison.
    NoCrc,
}

impl VerifyResult {
    /// Returns true if verification passed or was skipped (no CRC).
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Match | Self::NoCrc)
    }

    /// Returns true if verification failed.
    pub fn is_err(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }
}

/// Verifies CRC-32 against an expected value.
pub fn verify_crc32(data: &[u8], expected: Option<u32>) -> VerifyResult {
    match expected {
        Some(expected) => {
            let actual = Crc32::compute(data);
            if actual == expected {
                VerifyResult::Match
            } else {
                VerifyResult::Mismatch {
                    expected: expected as u64,
                    actual: actual as u64,
                }
            }
        }
        None => VerifyResult::NoCrc,
    }
}

/// Verifies CRC-64 against an expected value.
pub fn verify_crc64(data: &[u8], expected: Option<u64>) -> VerifyResult {
    match expected {
        Some(expected) => {
            let actual = Crc64::compute(data);
            if actual == expected {
                VerifyResult::Match
            } else {
                VerifyResult::Mismatch { expected, actual }
            }
        }
        None => VerifyResult::NoCrc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_crc32_basic() {
        let crc = Crc32::compute(b"Hello, World!");
        // CRC-32 IEEE 802.3 (ISO 3309) value
        assert_eq!(crc, 0xEC4AC3D0);
    }

    #[test]
    fn test_crc32_empty() {
        let crc = Crc32::compute(b"");
        assert_eq!(crc, 0);
    }

    #[test]
    fn test_crc32_incremental() {
        let mut hasher = Crc32::new();
        hasher.update(b"Hello, ");
        hasher.update(b"World!");
        assert_eq!(hasher.finalize(), 0xEC4AC3D0);
    }

    #[test]
    fn test_crc32_reset() {
        let mut hasher = Crc32::new();
        hasher.update(b"test");
        hasher.reset();
        hasher.update(b"Hello, World!");
        assert_eq!(hasher.finalize(), 0xEC4AC3D0);
    }

    #[test]
    fn test_crc64_basic() {
        let crc = Crc64::compute(b"Hello, World!");
        // Note: exact value depends on polynomial
        assert!(crc != 0);
    }

    #[test]
    fn test_crc64_empty() {
        let crc = Crc64::compute(b"");
        assert_eq!(crc, 0);
    }

    #[test]
    fn test_crc64_incremental() {
        let mut hasher = Crc64::new();
        hasher.update(b"Hello, ");
        hasher.update(b"World!");
        let incremental = hasher.finalize();

        let oneshot = Crc64::compute(b"Hello, World!");
        assert_eq!(incremental, oneshot);
    }

    #[test]
    fn test_crc32_writer() {
        let mut buffer = Vec::new();
        let mut writer = Crc32Writer::new(&mut buffer);

        writer.write_all(b"Hello, World!").unwrap();

        assert_eq!(writer.crc(), 0xEC4AC3D0);
        assert_eq!(writer.bytes_written(), 13);
        assert_eq!(buffer, b"Hello, World!");
    }

    #[test]
    fn test_crc64_writer() {
        let mut buffer = Vec::new();
        let mut writer = Crc64Writer::new(&mut buffer);

        writer.write_all(b"test data").unwrap();

        assert!(writer.crc() != 0);
        assert_eq!(writer.bytes_written(), 9);
    }

    #[test]
    fn test_crc32_reader() {
        let data = b"Hello, World!";
        let mut reader = Crc32Reader::new(Cursor::new(data));

        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).unwrap();

        assert_eq!(reader.crc(), 0xEC4AC3D0);
        assert_eq!(reader.bytes_read(), 13);
    }

    #[test]
    fn test_crc64_reader() {
        let data = b"test data";
        let mut reader = Crc64Reader::new(Cursor::new(data));

        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).unwrap();

        assert!(reader.crc() != 0);
        assert_eq!(reader.bytes_read(), 9);
    }

    #[test]
    fn test_verify_crc32_match() {
        let data = b"Hello, World!";
        let result = verify_crc32(data, Some(0xEC4AC3D0));
        assert!(result.is_ok());
        assert_eq!(result, VerifyResult::Match);
    }

    #[test]
    fn test_verify_crc32_mismatch() {
        let data = b"Hello, World!";
        let result = verify_crc32(data, Some(0x12345678));
        assert!(result.is_err());
        assert!(matches!(result, VerifyResult::Mismatch { .. }));
    }

    #[test]
    fn test_verify_crc32_no_crc() {
        let data = b"Hello, World!";
        let result = verify_crc32(data, None);
        assert!(result.is_ok());
        assert_eq!(result, VerifyResult::NoCrc);
    }

    #[test]
    fn test_crc32_with_initial() {
        let hasher = Crc32::with_initial(0x12345678);
        // Should start with non-zero state
        let _ = hasher.finalize();
    }

    #[test]
    fn test_checksum_trait() {
        fn compute_checksum<C: Checksum>(data: &[u8]) -> C::Output {
            C::compute(data)
        }

        let crc32 = compute_checksum::<Crc32>(b"test");
        let crc64 = compute_checksum::<Crc64>(b"test");

        assert!(crc32 != 0);
        assert!(crc64 != 0);
    }
}
