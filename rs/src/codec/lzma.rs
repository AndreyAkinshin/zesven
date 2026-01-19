//! LZMA and LZMA2 codec implementations.

use crate::{Error, Result};
use std::io::{self, Read, Write};

use super::{Decoder, Encoder, method};

/// LZMA decoder.
pub struct LzmaDecoder<R> {
    inner: lzma_rust2::LzmaReader<R>,
}

impl<R> std::fmt::Debug for LzmaDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LzmaDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> LzmaDecoder<R> {
    /// Creates a new LZMA decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - LZMA properties (5 bytes: 1 byte props + 4 byte dict size)
    /// * `uncompressed_size` - Expected uncompressed size
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid.
    pub fn new(input: R, properties: &[u8], uncompressed_size: u64) -> Result<Self> {
        if properties.len() < 5 {
            return Err(Error::InvalidFormat(
                "LZMA properties too short (need 5 bytes)".into(),
            ));
        }

        let props_byte = properties[0];
        let dict_size = u32::from_le_bytes(properties[1..5].try_into().unwrap());

        let reader = lzma_rust2::LzmaReader::new_with_props(
            input,
            uncompressed_size,
            props_byte,
            dict_size,
            None,
        )
        .map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string())))?;

        Ok(Self { inner: reader })
    }
}

impl<R: Read + Send> Read for LzmaDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for LzmaDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA
    }
}

/// LZMA2 decoder.
pub struct Lzma2Decoder<R> {
    inner: lzma_rust2::Lzma2Reader<R>,
}

impl<R> std::fmt::Debug for Lzma2Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lzma2Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> Lzma2Decoder<R> {
    /// Creates a new LZMA2 decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - LZMA2 properties (1 byte encoding dictionary size)
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid.
    pub fn new(input: R, properties: &[u8]) -> Result<Self> {
        if properties.is_empty() {
            return Err(Error::InvalidFormat("LZMA2 properties missing".into()));
        }

        let dict_size = decode_lzma2_dict_size(properties[0])?;

        let reader = lzma_rust2::Lzma2Reader::new(input, dict_size, None);

        Ok(Self { inner: reader })
    }
}

impl<R: Read + Send> Read for Lzma2Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for Lzma2Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA2
    }
}

/// Multi-threaded LZMA2 decoder.
///
/// Uses multiple worker threads to decompress LZMA2 streams in parallel.
/// This can provide significant speedups for large files on multi-core systems.
///
/// # Feature
///
/// Requires the `parallel` feature to be enabled.
#[cfg(feature = "parallel")]
pub struct Lzma2DecoderMt<R: Read> {
    inner: lzma_rust2::Lzma2ReaderMt<R>,
}

#[cfg(feature = "parallel")]
impl<R: Read> std::fmt::Debug for Lzma2DecoderMt<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lzma2DecoderMt").finish_non_exhaustive()
    }
}

#[cfg(feature = "parallel")]
impl<R: Read + Send> Lzma2DecoderMt<R> {
    /// Creates a new multi-threaded LZMA2 decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - LZMA2 properties (1 byte encoding dictionary size)
    /// * `num_threads` - Number of worker threads (capped at 256)
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid.
    pub fn new(input: R, properties: &[u8], num_threads: u32) -> Result<Self> {
        if properties.is_empty() {
            return Err(Error::InvalidFormat("LZMA2 properties missing".into()));
        }

        let dict_size = decode_lzma2_dict_size(properties[0])?;
        let threads = num_threads.clamp(1, 256);

        let reader = lzma_rust2::Lzma2ReaderMt::new(input, dict_size, None, threads);

        Ok(Self { inner: reader })
    }

    /// Creates a new multi-threaded LZMA2 decoder using available CPU cores.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    /// * `properties` - LZMA2 properties (1 byte encoding dictionary size)
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid.
    pub fn new_auto(input: R, properties: &[u8]) -> Result<Self> {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);
        Self::new(input, properties, num_threads)
    }
}

#[cfg(feature = "parallel")]
impl<R: Read> Read for Lzma2DecoderMt<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(feature = "parallel")]
impl<R: Read + Send> Decoder for Lzma2DecoderMt<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA2
    }
}

/// Decodes the LZMA2 dictionary size from the property byte.
///
/// The encoding is:
/// - 0-39: Various dictionary sizes from 4KB to 4GB
/// - 40: Indicates dictionary size of 4GB - 1
///
/// # Arguments
///
/// * `prop` - The property byte from LZMA2 coder properties
fn decode_lzma2_dict_size(prop: u8) -> Result<u32> {
    if prop > 40 {
        return Err(Error::InvalidFormat(format!(
            "invalid LZMA2 dictionary size property: {}",
            prop
        )));
    }

    if prop == 40 {
        // Special case: 4GB - 1
        return Ok(0xFFFF_FFFF);
    }

    // Dictionary size = 2^(prop/2 + 12) or 3 * 2^(prop/2 + 11)
    let base_log = (prop as u32) / 2 + 12;
    let dict_size = if prop % 2 == 0 {
        1u32 << base_log
    } else {
        3u32 << (base_log - 1)
    };

    Ok(dict_size)
}

/// Encodes a dictionary size into the LZMA2 property byte.
///
/// Returns the property byte (0-40) for the given dictionary size.
/// The function rounds up to the nearest valid dictionary size.
///
/// # Arguments
///
/// * `dict_size` - The dictionary size in bytes
pub fn encode_lzma2_dict_size(dict_size: u32) -> u8 {
    if dict_size == u32::MAX {
        return 40;
    }

    // Find the smallest property value that gives a dict_size >= requested
    for prop in 0..=40u8 {
        let size = decode_lzma2_dict_size(prop).unwrap();
        if size >= dict_size {
            return prop;
        }
    }

    40
}

/// LZMA encoder options.
#[derive(Debug, Clone)]
pub struct LzmaEncoderOptions {
    /// Compression preset level (0-9, default 6).
    pub preset: u32,
    /// Dictionary size in bytes (optional, uses preset default if None).
    pub dict_size: Option<u32>,
}

impl Default for LzmaEncoderOptions {
    fn default() -> Self {
        Self {
            preset: 6,
            dict_size: None,
        }
    }
}

impl LzmaEncoderOptions {
    /// Creates options with the given preset level.
    pub fn with_preset(preset: u32) -> Self {
        Self {
            preset: preset.min(9),
            dict_size: None,
        }
    }

    /// Sets a custom dictionary size.
    pub fn with_dict_size(mut self, dict_size: u32) -> Self {
        self.dict_size = Some(dict_size);
        self
    }

    /// Converts to lzma_rust2 options.
    fn to_lzma_options(&self) -> lzma_rust2::LzmaOptions {
        let mut opts = lzma_rust2::LzmaOptions::with_preset(self.preset);
        if let Some(dict_size) = self.dict_size {
            opts.dict_size = dict_size;
        }
        opts
    }

    /// Returns LZMA properties (5 bytes: props byte + dict size).
    pub fn properties(&self) -> Vec<u8> {
        let opts = self.to_lzma_options();
        let mut props = vec![opts.get_props()];
        props.extend_from_slice(&opts.dict_size.to_le_bytes());
        props
    }
}

/// LZMA encoder.
pub struct LzmaEncoder<W: Write> {
    inner: lzma_rust2::LzmaWriter<W>,
}

impl<W: Write> std::fmt::Debug for LzmaEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LzmaEncoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> LzmaEncoder<W> {
    /// Creates a new LZMA encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for compressed data
    /// * `options` - Encoder options
    ///
    /// # Errors
    ///
    /// Returns an error if the encoder cannot be initialized.
    pub fn new(output: W, options: &LzmaEncoderOptions) -> Result<Self> {
        let lzma_opts = options.to_lzma_options();
        // For 7z archives, we don't use the .lzma header (raw stream)
        // We also use end marker since size is tracked separately
        let writer = lzma_rust2::LzmaWriter::new_no_header(output, &lzma_opts, true)
            .map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string())))?;

        Ok(Self { inner: writer })
    }

    /// Returns the LZMA properties for this encoder (5 bytes).
    pub fn properties(options: &LzmaEncoderOptions) -> Vec<u8> {
        options.properties()
    }

    /// Finishes encoding and flushes all data.
    pub fn try_finish(self) -> io::Result<()> {
        self.inner
            .finish()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}

impl<W: Write + Send> Write for LzmaEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for LzmaEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner
            .finish()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}

/// LZMA2 encoder options.
#[derive(Debug, Clone)]
pub struct Lzma2EncoderOptions {
    /// Compression preset level (0-9, default 6).
    pub preset: u32,
    /// Dictionary size in bytes (optional, uses preset default if None).
    pub dict_size: Option<u32>,
}

impl Default for Lzma2EncoderOptions {
    fn default() -> Self {
        Self {
            preset: 6,
            dict_size: None,
        }
    }
}

impl Lzma2EncoderOptions {
    /// Creates options with the given preset level.
    pub fn with_preset(preset: u32) -> Self {
        Self {
            preset: preset.min(9),
            dict_size: None,
        }
    }

    /// Sets a custom dictionary size.
    pub fn with_dict_size(mut self, dict_size: u32) -> Self {
        self.dict_size = Some(dict_size);
        self
    }

    /// Converts to lzma_rust2 options.
    fn to_lzma2_options(&self) -> lzma_rust2::Lzma2Options {
        let mut opts = lzma_rust2::Lzma2Options::with_preset(self.preset);
        if let Some(dict_size) = self.dict_size {
            opts.lzma_options.dict_size = dict_size;
        }
        opts
    }

    /// Returns LZMA2 properties (1 byte: encoded dict size).
    pub fn properties(&self) -> Vec<u8> {
        let opts = self.to_lzma2_options();
        vec![encode_lzma2_dict_size(opts.lzma_options.dict_size)]
    }
}

/// LZMA2 encoder.
pub struct Lzma2Encoder<W: Write> {
    inner: lzma_rust2::Lzma2Writer<W>,
}

impl<W: Write> std::fmt::Debug for Lzma2Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lzma2Encoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> Lzma2Encoder<W> {
    /// Creates a new LZMA2 encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for compressed data
    /// * `options` - Encoder options
    pub fn new(output: W, options: &Lzma2EncoderOptions) -> Self {
        let lzma2_opts = options.to_lzma2_options();
        let writer = lzma_rust2::Lzma2Writer::new(output, lzma2_opts);

        Self { inner: writer }
    }

    /// Returns the LZMA2 properties for this encoder (1 byte).
    pub fn properties(options: &Lzma2EncoderOptions) -> Vec<u8> {
        options.properties()
    }

    /// Finishes encoding and flushes all data.
    pub fn try_finish(self) -> io::Result<()> {
        self.inner
            .finish()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}

impl<W: Write + Send> Write for Lzma2Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for Lzma2Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA2
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner
            .finish()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_lzma2_dict_size() {
        // Prop 0: 2^12 = 4KB
        assert_eq!(decode_lzma2_dict_size(0).unwrap(), 4096);
        // Prop 1: 3 * 2^11 = 6KB
        assert_eq!(decode_lzma2_dict_size(1).unwrap(), 6144);
        // Prop 2: 2^13 = 8KB
        assert_eq!(decode_lzma2_dict_size(2).unwrap(), 8192);
        // Prop 3: 3 * 2^12 = 12KB
        assert_eq!(decode_lzma2_dict_size(3).unwrap(), 12288);
        // Prop 18: 2^21 = 2MB
        assert_eq!(decode_lzma2_dict_size(18).unwrap(), 2 * 1024 * 1024);
        // Prop 40: 4GB - 1
        assert_eq!(decode_lzma2_dict_size(40).unwrap(), 0xFFFF_FFFF);
    }

    #[test]
    fn test_decode_lzma2_dict_size_invalid() {
        assert!(decode_lzma2_dict_size(41).is_err());
        assert!(decode_lzma2_dict_size(255).is_err());
    }

    #[test]
    fn test_lzma_decoder_properties_too_short() {
        use std::io::Cursor;

        let input = Cursor::new(vec![]);
        let err = LzmaDecoder::new(input, &[0x5D], 0).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_lzma2_decoder_properties_missing() {
        use std::io::Cursor;

        let input = Cursor::new(vec![]);
        let err = Lzma2Decoder::new(input, &[]).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_encode_lzma2_dict_size() {
        // Exact matches should return same prop
        assert_eq!(encode_lzma2_dict_size(4096), 0); // 4KB
        assert_eq!(encode_lzma2_dict_size(8192), 2); // 8KB

        // Values between should round up
        assert_eq!(encode_lzma2_dict_size(5000), 1); // rounds to 6KB (prop 1)
        assert_eq!(encode_lzma2_dict_size(7000), 2); // rounds to 8KB (prop 2)

        // Max value
        assert_eq!(encode_lzma2_dict_size(0xFFFF_FFFF), 40);
    }

    #[test]
    fn test_encode_decode_lzma2_roundtrip() {
        // For exact sizes, encode then decode should give same size
        for prop in 0..=40u8 {
            let size = decode_lzma2_dict_size(prop).unwrap();
            let encoded_prop = encode_lzma2_dict_size(size);
            assert_eq!(encoded_prop, prop, "roundtrip failed for prop {}", prop);
        }
    }

    #[test]
    fn test_lzma_encoder_options_default() {
        let opts = LzmaEncoderOptions::default();
        assert_eq!(opts.preset, 6);
        assert!(opts.dict_size.is_none());
    }

    #[test]
    fn test_lzma_encoder_options_properties() {
        let opts = LzmaEncoderOptions::with_preset(0);
        let props = opts.properties();
        assert_eq!(props.len(), 5);
        // First byte is LZMA properties byte
        // Next 4 bytes are dict size (little endian)
    }

    #[test]
    fn test_lzma2_encoder_options_default() {
        let opts = Lzma2EncoderOptions::default();
        assert_eq!(opts.preset, 6);
        assert!(opts.dict_size.is_none());
    }

    #[test]
    fn test_lzma2_encoder_options_properties() {
        let opts = Lzma2EncoderOptions::with_preset(0);
        let props = opts.properties();
        assert_eq!(props.len(), 1);
        // The property byte encodes dictionary size
    }

    #[test]
    fn test_lzma2_encoder_roundtrip() {
        use std::io::Cursor;

        let data = b"Hello, World! This is a test of LZMA2 compression.";

        // Compress
        let mut compressed = Vec::new();
        let opts = Lzma2EncoderOptions::with_preset(0);
        {
            let mut encoder = Lzma2Encoder::new(Cursor::new(&mut compressed), &opts);
            encoder.write_all(data).unwrap();
            Box::new(encoder).finish().unwrap();
        }

        // Decompress
        let props = opts.properties();
        let reader = Cursor::new(&compressed);
        let mut decoder = Lzma2Decoder::new(reader, &props).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lzma_encoder_roundtrip() {
        use std::io::Cursor;

        let data = b"Hello, World! This is a test of LZMA compression.";

        // Compress
        let mut compressed = Vec::new();
        let opts = LzmaEncoderOptions::with_preset(0);
        {
            let mut encoder = LzmaEncoder::new(Cursor::new(&mut compressed), &opts).unwrap();
            encoder.write_all(data).unwrap();
            Box::new(encoder).finish().unwrap();
        }

        // Decompress
        let props = opts.properties();
        let reader = Cursor::new(&compressed);
        let mut decoder = LzmaDecoder::new(reader, &props, data.len() as u64).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }
}
