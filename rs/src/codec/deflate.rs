//! Deflate codec implementation.

use std::io::{self, Read, Write};

use flate2::Compression;
use flate2::bufread::DeflateDecoder as FlateDecoder;
use flate2::write::DeflateEncoder as FlateEncoder;

use super::{Decoder, Encoder, method};

/// Deflate decoder.
pub struct DeflateDecoder<R> {
    inner: FlateDecoder<R>,
}

impl<R> std::fmt::Debug for DeflateDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeflateDecoder").finish_non_exhaustive()
    }
}

impl<R: io::BufRead + Send> DeflateDecoder<R> {
    /// Creates a new Deflate decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source (must implement BufRead)
    pub fn new(input: R) -> Self {
        Self {
            inner: FlateDecoder::new(input),
        }
    }
}

impl<R: io::BufRead + Send> Read for DeflateDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: io::BufRead + Send> Decoder for DeflateDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::DEFLATE
    }
}

/// Deflate encoder options.
#[derive(Debug, Clone)]
pub struct DeflateEncoderOptions {
    /// Compression level (0-9, default 6).
    pub level: u32,
}

impl Default for DeflateEncoderOptions {
    fn default() -> Self {
        Self { level: 6 }
    }
}

impl DeflateEncoderOptions {
    /// Creates options with the given compression level.
    pub fn with_level(level: u32) -> Self {
        Self {
            level: level.min(9),
        }
    }
}

/// Deflate encoder.
pub struct DeflateEncoder<W: Write> {
    inner: FlateEncoder<W>,
}

impl<W: Write> std::fmt::Debug for DeflateEncoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeflateEncoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> DeflateEncoder<W> {
    /// Creates a new Deflate encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for compressed data
    /// * `options` - Encoder options
    pub fn new(output: W, options: &DeflateEncoderOptions) -> Self {
        Self {
            inner: FlateEncoder::new(output, Compression::new(options.level)),
        }
    }

    /// Finishes encoding and flushes all data.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<W: Write + Send> Write for DeflateEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for DeflateEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::DEFLATE
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn test_deflate_roundtrip() {
        let data = b"Hello, World! This is a test of Deflate compression.";

        // Compress
        let mut compressed = Vec::new();
        let opts = DeflateEncoderOptions::default();
        {
            let mut encoder = DeflateEncoder::new(Cursor::new(&mut compressed), &opts);
            encoder.write_all(data).unwrap();
            Box::new(encoder).finish().unwrap();
        }

        // Decompress
        let reader = BufReader::new(Cursor::new(&compressed));
        let mut decoder = DeflateDecoder::new(reader);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_deflate_encoder_options() {
        let opts = DeflateEncoderOptions::default();
        assert_eq!(opts.level, 6);

        let opts = DeflateEncoderOptions::with_level(9);
        assert_eq!(opts.level, 9);

        let opts = DeflateEncoderOptions::with_level(100);
        assert_eq!(opts.level, 9); // Clamped
    }

    #[test]
    fn test_deflate_method_id() {
        let reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
        let decoder = DeflateDecoder::new(reader);
        assert_eq!(decoder.method_id(), method::DEFLATE);
    }
}
