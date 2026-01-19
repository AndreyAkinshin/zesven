//! PPMd codec implementation.
//!
//! PPMd (PPMdH variant) is used by 7z archives with method ID 0x030401.
//! The properties are 5 bytes: 1 byte for order, 4 bytes for memory size (little endian).

use crate::{Error, Result};
use std::io::{self, Read, Write};

use ppmd_rust::{PPMD7_MAX_MEM_SIZE, PPMD7_MAX_ORDER, PPMD7_MIN_MEM_SIZE, PPMD7_MIN_ORDER};
use ppmd_rust::{Ppmd7Decoder as RustPpmdDecoder, Ppmd7Encoder as RustPpmdEncoder};

use super::{Decoder, Encoder, method};

/// PPMd decoder.
pub struct PpmdDecoder<R: Read> {
    inner: RustPpmdDecoder<R>,
}

/// PPMd decoder with size limit.
///
/// PPMd doesn't have an end-of-stream marker, so we need to track
/// how many bytes have been read and stop at the expected size.
pub struct SizedPpmdDecoder<R: Read> {
    inner: RustPpmdDecoder<R>,
    remaining: u64,
}

impl<R: Read> std::fmt::Debug for PpmdDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PpmdDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> PpmdDecoder<R> {
    /// Creates a new PPMd decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - PPMd properties (5 bytes: order + mem_size)
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid or decoder initialization fails.
    pub fn new(input: R, properties: &[u8]) -> Result<Self> {
        if properties.len() < 5 {
            return Err(Error::InvalidFormat(
                "PPMd properties too short (need 5 bytes)".into(),
            ));
        }

        let order = properties[0] as u32;
        let mem_size = u32::from_le_bytes(properties[1..5].try_into().unwrap());

        Self::new_with_params(input, order, mem_size)
    }

    /// Creates a new PPMd decoder with explicit parameters.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `order` - Model order (2-64)
    /// * `mem_size` - Memory size in bytes
    ///
    /// # Errors
    ///
    /// Returns an error if parameters are out of range or decoder initialization fails.
    pub fn new_with_params(input: R, order: u32, mem_size: u32) -> Result<Self> {
        if !(PPMD7_MIN_ORDER..=PPMD7_MAX_ORDER).contains(&order) {
            return Err(Error::InvalidFormat(format!(
                "PPMd order {} out of range [{}-{}]",
                order, PPMD7_MIN_ORDER, PPMD7_MAX_ORDER
            )));
        }

        if !(PPMD7_MIN_MEM_SIZE..=PPMD7_MAX_MEM_SIZE).contains(&mem_size) {
            return Err(Error::InvalidFormat(format!(
                "PPMd memory size {} out of range [{}-{}]",
                mem_size, PPMD7_MIN_MEM_SIZE, PPMD7_MAX_MEM_SIZE
            )));
        }

        let decoder = RustPpmdDecoder::new(input, order, mem_size).map_err(|e| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{:?}", e),
            ))
        })?;

        Ok(Self { inner: decoder })
    }
}

impl<R: Read + Send> Read for PpmdDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for PpmdDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::PPMD
    }
}

impl<R: Read> std::fmt::Debug for SizedPpmdDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SizedPpmdDecoder")
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

impl<R: Read + Send> SizedPpmdDecoder<R> {
    /// Creates a new size-limited PPMd decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - PPMd properties (5 bytes: order + mem_size)
    /// * `uncompressed_size` - Expected uncompressed size (decoder will stop after this many bytes)
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid or decoder initialization fails.
    pub fn new(input: R, properties: &[u8], uncompressed_size: u64) -> Result<Self> {
        if properties.len() < 5 {
            return Err(Error::InvalidFormat(
                "PPMd properties too short (need 5 bytes)".into(),
            ));
        }

        let order = properties[0] as u32;
        let mem_size = u32::from_le_bytes(properties[1..5].try_into().unwrap());

        Self::new_with_params(input, order, mem_size, uncompressed_size)
    }

    /// Creates a new size-limited PPMd decoder with explicit parameters.
    pub fn new_with_params(
        input: R,
        order: u32,
        mem_size: u32,
        uncompressed_size: u64,
    ) -> Result<Self> {
        if !(PPMD7_MIN_ORDER..=PPMD7_MAX_ORDER).contains(&order) {
            return Err(Error::InvalidFormat(format!(
                "PPMd order {} out of range [{}-{}]",
                order, PPMD7_MIN_ORDER, PPMD7_MAX_ORDER
            )));
        }

        if !(PPMD7_MIN_MEM_SIZE..=PPMD7_MAX_MEM_SIZE).contains(&mem_size) {
            return Err(Error::InvalidFormat(format!(
                "PPMd memory size {} out of range [{}-{}]",
                mem_size, PPMD7_MIN_MEM_SIZE, PPMD7_MAX_MEM_SIZE
            )));
        }

        let decoder = RustPpmdDecoder::new(input, order, mem_size).map_err(|e| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{:?}", e),
            ))
        })?;

        Ok(Self {
            inner: decoder,
            remaining: uncompressed_size,
        })
    }
}

impl<R: Read + Send> Read for SizedPpmdDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }

        // Limit read to remaining bytes
        let max_read = buf.len().min(self.remaining as usize);
        let n = self.inner.read(&mut buf[..max_read])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl<R: Read + Send> Decoder for SizedPpmdDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::PPMD
    }
}

/// PPMd encoder options.
#[derive(Debug, Clone)]
pub struct PpmdEncoderOptions {
    /// Model order (2-64, default 6).
    pub order: u32,
    /// Memory size in bytes (default 16MB).
    pub mem_size: u32,
}

impl Default for PpmdEncoderOptions {
    fn default() -> Self {
        Self {
            order: 6,
            mem_size: 16 * 1024 * 1024, // 16MB
        }
    }
}

impl PpmdEncoderOptions {
    /// Creates options with specified order and memory size.
    pub fn new(order: u32, mem_size: u32) -> Self {
        Self {
            order: order.clamp(PPMD7_MIN_ORDER, PPMD7_MAX_ORDER),
            mem_size: mem_size.clamp(PPMD7_MIN_MEM_SIZE, PPMD7_MAX_MEM_SIZE),
        }
    }

    /// Creates options with the given order (using default memory size).
    pub fn with_order(order: u32) -> Self {
        Self {
            order: order.clamp(PPMD7_MIN_ORDER, PPMD7_MAX_ORDER),
            ..Default::default()
        }
    }

    /// Returns PPMd properties (5 bytes: order + mem_size).
    pub fn properties(&self) -> Vec<u8> {
        let mut props = vec![self.order as u8];
        props.extend_from_slice(&self.mem_size.to_le_bytes());
        props
    }
}

/// PPMd encoder.
pub struct PpmdEncoder<W: Write> {
    inner: RustPpmdEncoder<W>,
}

impl<W: Write> std::fmt::Debug for PpmdEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PpmdEncoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> PpmdEncoder<W> {
    /// Creates a new PPMd encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for compressed data
    /// * `options` - Encoder options
    ///
    /// # Errors
    ///
    /// Returns an error if the encoder cannot be initialized.
    pub fn new(output: W, options: &PpmdEncoderOptions) -> Result<Self> {
        let encoder =
            RustPpmdEncoder::new(output, options.order, options.mem_size).map_err(|e| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{:?}", e),
                ))
            })?;

        Ok(Self { inner: encoder })
    }

    /// Returns the PPMd properties for this encoder (5 bytes).
    pub fn properties(options: &PpmdEncoderOptions) -> Vec<u8> {
        options.properties()
    }
}

impl<W: Write + Send> Write for PpmdEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for PpmdEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::PPMD
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        // 7z archives track size separately, so no end marker needed
        self.inner.finish(false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_ppmd_roundtrip() {
        let data = b"Hello, World! This is a test of PPMd compression. PPMd is a prediction by partial matching algorithm.";

        // Compress
        let mut compressed = Vec::new();
        let opts = PpmdEncoderOptions::new(6, 1024 * 1024); // 1MB memory
        {
            let mut encoder = PpmdEncoder::new(Cursor::new(&mut compressed), &opts).unwrap();
            encoder.write_all(data).unwrap();
            Box::new(encoder).finish().unwrap();
        }

        // Decompress - need to read exact size since no end marker
        let props = opts.properties();
        let reader = Cursor::new(&compressed);
        let mut decoder = PpmdDecoder::new(reader, &props).unwrap();
        let mut decompressed = vec![0u8; data.len()];
        decoder.read_exact(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_ppmd_encoder_options() {
        let opts = PpmdEncoderOptions::default();
        assert_eq!(opts.order, 6);
        assert_eq!(opts.mem_size, 16 * 1024 * 1024);

        let opts = PpmdEncoderOptions::with_order(8);
        assert_eq!(opts.order, 8);

        // Order clamping
        let opts = PpmdEncoderOptions::with_order(1);
        assert_eq!(opts.order, PPMD7_MIN_ORDER);

        let opts = PpmdEncoderOptions::with_order(100);
        assert_eq!(opts.order, PPMD7_MAX_ORDER);
    }

    #[test]
    fn test_ppmd_properties() {
        let opts = PpmdEncoderOptions::new(8, 0x00100000); // order 8, 1MB
        let props = opts.properties();

        assert_eq!(props.len(), 5);
        assert_eq!(props[0], 8); // order
        assert_eq!(&props[1..5], &0x00100000u32.to_le_bytes()); // mem_size
    }

    #[test]
    fn test_ppmd_decoder_invalid_properties() {
        let reader = Cursor::new(vec![]);

        // Too short
        let err = PpmdDecoder::new(reader.clone(), &[0x06]).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));

        // Invalid order (0)
        let err = PpmdDecoder::new(reader.clone(), &[0, 0, 0, 0x10, 0]).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));

        // Invalid order (too high)
        let err = PpmdDecoder::new(reader.clone(), &[100, 0, 0, 0x10, 0]).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_ppmd_method_id() {
        let opts = PpmdEncoderOptions::new(6, 1024 * 1024);
        let mut compressed = Vec::new();
        let encoder = PpmdEncoder::new(Cursor::new(&mut compressed), &opts).unwrap();
        assert_eq!(encoder.method_id(), method::PPMD);
    }
}
