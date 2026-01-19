//! Pre-processing filter codecs (BCJ, Delta).
//!
//! These filters are applied before/after compression to improve compression
//! ratios for specific data types like executables (BCJ) or audio/image data (Delta).

use std::io::{self, Read};

use lzma_rust2::filter::bcj::BcjReader;
use lzma_rust2::filter::delta::DeltaReader;

use super::{Decoder, method};

/// BCJ x86 filter decoder.
///
/// Applies x86 Branch/Call/Jump filtering which improves compression
/// of x86 executables by converting relative addresses to absolute ones.
pub struct BcjX86Decoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjX86Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjX86Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjX86Decoder<R> {
    /// Creates a new BCJ x86 filter decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The data source (typically output from a decompressor)
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_x86(input, 0),
        }
    }

    /// Creates a new BCJ x86 filter decoder with start position.
    ///
    /// # Arguments
    ///
    /// * `input` - The data source
    /// * `start_pos` - The starting position for address calculation
    pub fn new_with_start_pos(input: R, start_pos: usize) -> Self {
        Self {
            inner: BcjReader::new_x86(input, start_pos),
        }
    }
}

impl<R: Read + Send> Read for BcjX86Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjX86Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_X86
    }
}

/// BCJ ARM filter decoder.
///
/// Applies ARM Branch/Call/Jump filtering which improves compression
/// of ARM executables.
pub struct BcjArmDecoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjArmDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArmDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjArmDecoder<R> {
    /// Creates a new BCJ ARM filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_arm(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjArmDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjArmDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM
    }
}

/// BCJ ARM64 filter decoder.
///
/// Applies ARM64/AArch64 Branch/Call/Jump filtering.
pub struct BcjArm64Decoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjArm64Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArm64Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjArm64Decoder<R> {
    /// Creates a new BCJ ARM64 filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_arm64(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjArm64Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjArm64Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM64
    }
}

/// BCJ ARM Thumb filter decoder.
///
/// Applies ARM Thumb mode Branch/Call/Jump filtering.
pub struct BcjArmThumbDecoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjArmThumbDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjArmThumbDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjArmThumbDecoder<R> {
    /// Creates a new BCJ ARM Thumb filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_arm_thumb(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjArmThumbDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjArmThumbDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_ARM_THUMB
    }
}

/// BCJ PowerPC filter decoder.
///
/// Applies PowerPC Branch/Call/Jump filtering.
pub struct BcjPpcDecoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjPpcDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjPpcDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjPpcDecoder<R> {
    /// Creates a new BCJ PowerPC filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_ppc(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjPpcDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjPpcDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_PPC
    }
}

/// BCJ SPARC filter decoder.
///
/// Applies SPARC Branch/Call/Jump filtering.
pub struct BcjSparcDecoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjSparcDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjSparcDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjSparcDecoder<R> {
    /// Creates a new BCJ SPARC filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_sparc(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjSparcDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjSparcDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_SPARC
    }
}

/// BCJ IA64 filter decoder.
///
/// Applies IA64/Itanium Branch/Call/Jump filtering.
pub struct BcjIa64Decoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjIa64Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjIa64Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjIa64Decoder<R> {
    /// Creates a new BCJ IA64 filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_ia64(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjIa64Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjIa64Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_IA64
    }
}

/// BCJ RISC-V filter decoder.
///
/// Applies RISC-V Branch/Call/Jump filtering.
pub struct BcjRiscvDecoder<R> {
    inner: BcjReader<R>,
}

impl<R> std::fmt::Debug for BcjRiscvDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BcjRiscvDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BcjRiscvDecoder<R> {
    /// Creates a new BCJ RISC-V filter decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: BcjReader::new_riscv(input, 0),
        }
    }
}

impl<R: Read + Send> Read for BcjRiscvDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BcjRiscvDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BCJ_RISCV
    }
}

/// Delta filter decoder.
///
/// Applies delta filtering which stores the difference between consecutive
/// bytes. Useful for audio/image data where consecutive samples are similar.
pub struct DeltaDecoder<R> {
    inner: DeltaReader<R>,
}

impl<R> std::fmt::Debug for DeltaDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> DeltaDecoder<R> {
    /// Creates a new Delta filter decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The data source (typically output from a decompressor)
    /// * `properties` - Delta properties (1 byte: distance - 1)
    ///
    /// The distance is encoded as (stored_value + 1), so property byte 0 means distance 1.
    pub fn new(input: R, properties: &[u8]) -> Self {
        let distance = properties.first().map(|b| *b as usize + 1).unwrap_or(1);
        Self::new_with_distance(input, distance)
    }

    /// Creates a new Delta filter decoder with explicit distance.
    ///
    /// # Arguments
    ///
    /// * `input` - The data source
    /// * `distance` - The delta distance (1-256)
    pub fn new_with_distance(input: R, distance: usize) -> Self {
        Self {
            inner: DeltaReader::new(input, distance),
        }
    }
}

impl<R: Read + Send> Read for DeltaDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for DeltaDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::DELTA
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_bcj_x86_decoder_method_id() {
        let data = vec![0u8; 16];
        let decoder = BcjX86Decoder::new(Cursor::new(data));
        assert_eq!(decoder.method_id(), method::BCJ_X86);
    }

    #[test]
    fn test_delta_decoder_method_id() {
        let data = vec![0u8; 16];
        let decoder = DeltaDecoder::new(Cursor::new(data), &[0]);
        assert_eq!(decoder.method_id(), method::DELTA);
    }

    #[test]
    fn test_delta_decoder_distance_parsing() {
        // Property byte 0 means distance 1
        let data = vec![1, 2, 3, 4];
        let mut decoder = DeltaDecoder::new(Cursor::new(data), &[0]);
        let mut output = vec![0u8; 4];
        decoder.read_exact(&mut output).unwrap();
        // With delta distance 1:
        // out[0] = 1 + history[255] = 1 + 0 = 1
        // out[1] = 2 + out[0] = 2 + 1 = 3
        // out[2] = 3 + out[1] = 3 + 3 = 6
        // out[3] = 4 + out[2] = 4 + 6 = 10
        assert_eq!(output, [1, 3, 6, 10]);
    }

    #[test]
    fn test_delta_decoder_empty_properties() {
        // Empty properties should default to distance 1
        let data = vec![1, 1, 1, 1];
        let mut decoder = DeltaDecoder::new(Cursor::new(data), &[]);
        let mut output = vec![0u8; 4];
        decoder.read_exact(&mut output).unwrap();
        // 1, 1+1=2, 1+2=3, 1+3=4
        assert_eq!(output, [1, 2, 3, 4]);
    }
}
