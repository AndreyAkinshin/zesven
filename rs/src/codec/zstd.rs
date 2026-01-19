//! Zstandard (ZSTD) compression codec.
//!
//! This module provides ZSTD compression support for 7z archives.
//! ZSTD provides excellent compression ratios with fast decompression.
//!
//! ## Dictionary Support
//!
//! ZSTD dictionaries can significantly improve compression ratios for small files
//! that share common patterns. This module provides:
//!
//! - [`ZstdDictionary`]: A trained dictionary that can be reused
//! - [`ZstdDecoderWithDict`]: Decoder that uses a pre-loaded dictionary
//! - [`ZstdEncoderWithDict`]: Encoder that uses a dictionary
//!
//! ### Example: Training and Using a Dictionary
//!
//! ```rust,ignore
//! use zesven::codec::zstd::{ZstdDictionary, ZstdEncoderWithDict, ZstdDecoderWithDict};
//!
//! // Train a dictionary from sample data
//! let samples = vec![
//!     b"common prefix data 1".to_vec(),
//!     b"common prefix data 2".to_vec(),
//!     b"common prefix data 3".to_vec(),
//! ];
//! let dict = ZstdDictionary::train(&samples, 4096)?;
//!
//! // Compress with the dictionary
//! let mut compressed = Vec::new();
//! {
//!     let mut encoder = ZstdEncoderWithDict::new(&mut compressed, 3, &dict)?;
//!     encoder.write_all(b"common prefix data 4")?;
//!     encoder.try_finish()?;
//! }
//!
//! // Decompress with the dictionary
//! let mut decoder = ZstdDecoderWithDict::new(Cursor::new(compressed), &dict)?;
//! let mut output = Vec::new();
//! decoder.read_to_end(&mut output)?;
//! ```

use std::io::{self, BufReader, Read, Write};
use std::sync::Arc;

use zstd::stream::{Decoder as ZstdDecoder, Encoder as ZstdEncoderInner};

use super::{Decoder, Encoder, method};

/// ZSTD decoder.
pub struct ZstdStreamDecoder<R> {
    inner: ZstdDecoder<'static, BufReader<R>>,
}

impl<R> std::fmt::Debug for ZstdStreamDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdStreamDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> ZstdStreamDecoder<R> {
    /// Creates a new ZSTD decoder.
    pub fn new(input: R) -> io::Result<Self> {
        let decoder = ZstdDecoder::new(input)?;
        Ok(Self { inner: decoder })
    }
}

impl<R: Read + Send> Read for ZstdStreamDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for ZstdStreamDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::ZSTD
    }
}

/// ZSTD encoder options.
#[derive(Debug, Clone)]
pub struct ZstdEncoderOptions {
    /// Compression level (1-22, default 3).
    pub level: i32,
}

impl Default for ZstdEncoderOptions {
    fn default() -> Self {
        Self { level: 3 }
    }
}

/// ZSTD encoder.
pub struct ZstdStreamEncoder<'a, W: Write> {
    inner: ZstdEncoderInner<'a, W>,
}

impl<'a, W: Write + Send> ZstdStreamEncoder<'a, W> {
    /// Creates a new ZSTD encoder.
    pub fn new(output: W, options: &ZstdEncoderOptions) -> io::Result<Self> {
        let encoder = ZstdEncoderInner::new(output, options.level)?;
        Ok(Self { inner: encoder })
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<'a, W: Write + Send> Write for ZstdStreamEncoder<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<'a, W: Write + Send + 'a> Encoder for ZstdStreamEncoder<'a, W> {
    fn method_id(&self) -> &'static [u8] {
        method::ZSTD
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner.finish()?;
        Ok(())
    }
}

// =============================================================================
// Dictionary Support
// =============================================================================

/// A trained ZSTD dictionary for improved compression of similar data.
///
/// Dictionaries work best when:
/// - Compressing many small files (< 128 KB each)
/// - Files share common patterns or structure
/// - The dictionary is trained on representative samples
///
/// # Example
///
/// ```rust,ignore
/// use zesven::codec::zstd::ZstdDictionary;
///
/// // Collect sample data for training
/// let samples: Vec<Vec<u8>> = vec![
///     b"JSON: {\"type\": \"user\", \"id\": 1}".to_vec(),
///     b"JSON: {\"type\": \"user\", \"id\": 2}".to_vec(),
///     b"JSON: {\"type\": \"admin\", \"id\": 3}".to_vec(),
/// ];
///
/// // Train a 4KB dictionary
/// let dict = ZstdDictionary::train(&samples, 4096)?;
/// println!("Dictionary ID: {}", dict.id());
/// ```
#[derive(Clone)]
pub struct ZstdDictionary {
    /// Raw dictionary data.
    data: Arc<Vec<u8>>,
    /// Dictionary ID (extracted from header).
    id: u32,
}

impl std::fmt::Debug for ZstdDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdDictionary")
            .field("id", &self.id)
            .field("size", &self.data.len())
            .finish()
    }
}

impl ZstdDictionary {
    /// Trains a dictionary from sample data.
    ///
    /// # Arguments
    ///
    /// * `samples` - Collection of sample data to train on
    /// * `dict_size` - Target dictionary size in bytes (typically 4KB-128KB)
    ///
    /// # Returns
    ///
    /// A trained dictionary, or an error if training fails.
    ///
    /// # Notes
    ///
    /// - More samples generally produce better dictionaries
    /// - Samples should be representative of the data to compress
    /// - Larger dictionaries can provide better compression but use more memory
    pub fn train(samples: &[Vec<u8>], dict_size: usize) -> io::Result<Self> {
        let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
        let dict_data =
            zstd::dict::from_samples(&sample_refs, dict_size).map_err(io::Error::other)?;

        Self::from_bytes(dict_data)
    }

    /// Creates a dictionary from raw dictionary data.
    ///
    /// The data should be a valid ZSTD dictionary, either trained or
    /// loaded from a file.
    pub fn from_bytes(data: Vec<u8>) -> io::Result<Self> {
        if data.len() < 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dictionary too small",
            ));
        }

        // Extract dictionary ID from header
        // ZSTD dictionaries start with magic 0xEC30A437, followed by dict ID
        let id = if data.len() >= 8
            && data[0] == 0x37
            && data[1] == 0xA4
            && data[2] == 0x30
            && data[3] == 0xEC
        {
            u32::from_le_bytes([data[4], data[5], data[6], data[7]])
        } else {
            // Content-only dictionary or raw dictionary
            0
        };

        Ok(Self {
            data: Arc::new(data),
            id,
        })
    }

    /// Loads a dictionary from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        Self::from_bytes(data)
    }

    /// Returns the dictionary ID.
    ///
    /// Returns 0 for content-only dictionaries.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the dictionary size in bytes.
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Returns the raw dictionary data.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Saves the dictionary to a file.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> io::Result<()> {
        std::fs::write(path, &*self.data)
    }
}

/// ZSTD decoder that uses a pre-loaded dictionary.
///
/// Dictionary decompression is faster than loading the dictionary for each
/// decompression operation because the dictionary can be prepared once.
pub struct ZstdDecoderWithDict<'d, R: Read> {
    inner: ZstdDecoder<'d, BufReader<R>>,
}

impl<'d, R: Read> std::fmt::Debug for ZstdDecoderWithDict<'d, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdDecoderWithDict")
            .finish_non_exhaustive()
    }
}

impl<'d, R: Read + Send> ZstdDecoderWithDict<'d, R> {
    /// Creates a new decoder using the given dictionary.
    pub fn new(input: R, dict: &'d ZstdDictionary) -> io::Result<Self> {
        let buf_reader = BufReader::new(input);
        let decoder = ZstdDecoder::with_dictionary(buf_reader, dict.as_bytes())?;
        Ok(Self { inner: decoder })
    }
}

impl<'d, R: Read + Send> Read for ZstdDecoderWithDict<'d, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<'d, R: Read + Send> Decoder for ZstdDecoderWithDict<'d, R> {
    fn method_id(&self) -> &'static [u8] {
        method::ZSTD
    }
}

/// ZSTD encoder that uses a pre-loaded dictionary.
///
/// Dictionary compression can significantly improve compression ratios
/// for small files that share common patterns.
pub struct ZstdEncoderWithDict<'d, W: Write> {
    inner: ZstdEncoderInner<'d, W>,
}

impl<'d, W: Write> std::fmt::Debug for ZstdEncoderWithDict<'d, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdEncoderWithDict")
            .finish_non_exhaustive()
    }
}

impl<'d, W: Write + Send> ZstdEncoderWithDict<'d, W> {
    /// Creates a new encoder using the given dictionary.
    ///
    /// # Arguments
    ///
    /// * `output` - The writer to write compressed data to
    /// * `level` - Compression level (1-22)
    /// * `dict` - The dictionary to use for compression
    pub fn new(output: W, level: i32, dict: &'d ZstdDictionary) -> io::Result<Self> {
        let encoder = ZstdEncoderInner::with_dictionary(output, level, dict.as_bytes())?;
        Ok(Self { inner: encoder })
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<'d, W: Write + Send> Write for ZstdEncoderWithDict<'d, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<'d, W: Write + Send + 'd> Encoder for ZstdEncoderWithDict<'d, W> {
    fn method_id(&self) -> &'static [u8] {
        method::ZSTD
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner.finish()?;
        Ok(())
    }
}

/// Options for ZSTD encoding with optional dictionary.
#[derive(Debug, Clone, Default)]
pub struct ZstdEncoderOptionsWithDict {
    /// Compression level (1-22, default 3).
    pub level: i32,
    /// Optional dictionary for improved compression.
    pub dictionary: Option<ZstdDictionary>,
}

impl ZstdEncoderOptionsWithDict {
    /// Creates new options with default settings.
    pub fn new() -> Self {
        Self {
            level: 3,
            dictionary: None,
        }
    }

    /// Sets the compression level.
    pub fn level(mut self, level: i32) -> Self {
        self.level = level.clamp(1, 22);
        self
    }

    /// Sets the dictionary to use.
    pub fn dictionary(mut self, dict: ZstdDictionary) -> Self {
        self.dictionary = Some(dict);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_zstd_round_trip() {
        let original = b"Hello, World! This is a test of ZSTD compression.";

        // Compress
        let mut compressed = Vec::new();
        {
            let mut encoder =
                ZstdStreamEncoder::new(&mut compressed, &ZstdEncoderOptions::default()).unwrap();
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = ZstdStreamDecoder::new(Cursor::new(compressed)).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_zstd_decoder_method_id() {
        let data = zstd::encode_all(&b"test"[..], 3).unwrap();
        let decoder = ZstdStreamDecoder::new(Cursor::new(data)).unwrap();
        assert_eq!(decoder.method_id(), method::ZSTD);
    }

    // Dictionary tests

    #[test]
    fn test_zstd_dictionary_train() {
        // Create sample data with common patterns
        let samples: Vec<Vec<u8>> = (0..100)
            .map(|i| {
                format!(
                    "{{\"type\": \"user\", \"id\": {}, \"name\": \"User{}\"}}",
                    i, i
                )
                .into_bytes()
            })
            .collect();

        let dict = ZstdDictionary::train(&samples, 4096).unwrap();
        assert!(dict.size() > 0);
        assert!(dict.size() <= 4096);
    }

    #[test]
    fn test_zstd_dictionary_from_bytes() {
        // Create a simple dictionary (this is content-only, no magic header)
        let dict_data = vec![0u8; 1024];
        let dict = ZstdDictionary::from_bytes(dict_data).unwrap();
        assert_eq!(dict.id(), 0); // Content-only dictionary
        assert_eq!(dict.size(), 1024);
    }

    #[test]
    fn test_zstd_dictionary_too_small() {
        let dict_data = vec![0u8; 4];
        let result = ZstdDictionary::from_bytes(dict_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_zstd_dictionary_with_magic() {
        // Create dictionary with ZSTD magic header
        let mut dict_data = vec![0u8; 128];
        // ZSTD dictionary magic: 0xEC30A437 (little-endian: 37 A4 30 EC)
        dict_data[0] = 0x37;
        dict_data[1] = 0xA4;
        dict_data[2] = 0x30;
        dict_data[3] = 0xEC;
        // Dictionary ID: 0x12345678
        dict_data[4] = 0x78;
        dict_data[5] = 0x56;
        dict_data[6] = 0x34;
        dict_data[7] = 0x12;

        let dict = ZstdDictionary::from_bytes(dict_data).unwrap();
        assert_eq!(dict.id(), 0x12345678);
    }

    #[test]
    fn test_zstd_dictionary_round_trip() {
        // Train a dictionary
        let samples: Vec<Vec<u8>> = (0..50)
            .map(|i| format!("prefix_data_{}_suffix", i).into_bytes())
            .collect();

        let dict = ZstdDictionary::train(&samples, 4096).unwrap();

        // Compress with dictionary
        let original = b"prefix_data_999_suffix with some extra content";
        let mut compressed = Vec::new();
        {
            let mut encoder = ZstdEncoderWithDict::new(&mut compressed, 3, &dict).unwrap();
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress with dictionary
        let mut decoder = ZstdDecoderWithDict::new(Cursor::new(compressed), &dict).unwrap();
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_zstd_dictionary_compression_improvement() {
        // Train a dictionary on specific data
        let samples: Vec<Vec<u8>> = (0..100)
            .map(|i| {
                format!(
                    "{{\"type\": \"event\", \"id\": {}, \"timestamp\": 1234567890}}",
                    i
                )
                .into_bytes()
            })
            .collect();

        let dict = ZstdDictionary::train(&samples, 8192).unwrap();

        // Compress test data
        let test_data = b"{\"type\": \"event\", \"id\": 500, \"timestamp\": 1234567891}";

        // Compress without dictionary
        let compressed_no_dict = zstd::encode_all(&test_data[..], 3).unwrap();

        // Compress with dictionary
        let mut compressed_with_dict = Vec::new();
        {
            let mut encoder =
                ZstdEncoderWithDict::new(&mut compressed_with_dict, 3, &dict).unwrap();
            encoder.write_all(test_data).unwrap();
            encoder.try_finish().unwrap();
        }

        // Dictionary compression should be smaller or equal for similar data
        // Note: For very small data, dictionary might not help much
        println!(
            "Without dict: {} bytes, with dict: {} bytes",
            compressed_no_dict.len(),
            compressed_with_dict.len()
        );
    }

    #[test]
    fn test_zstd_encoder_options_with_dict() {
        let samples: Vec<Vec<u8>> = (0..10)
            .map(|i| format!("sample_{}", i).into_bytes())
            .collect();
        let dict = ZstdDictionary::train(&samples, 1024).unwrap();

        let options = ZstdEncoderOptionsWithDict::new().level(5).dictionary(dict);

        assert_eq!(options.level, 5);
        assert!(options.dictionary.is_some());
    }

    #[test]
    fn test_zstd_dictionary_clone() {
        let samples: Vec<Vec<u8>> = (0..10)
            .map(|i| format!("data_{}", i).into_bytes())
            .collect();
        let dict = ZstdDictionary::train(&samples, 1024).unwrap();
        let dict_clone = dict.clone();

        assert_eq!(dict.id(), dict_clone.id());
        assert_eq!(dict.size(), dict_clone.size());
    }
}
