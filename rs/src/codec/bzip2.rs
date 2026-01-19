//! BZip2 codec implementation.

use std::io::{self, Read, Write};

use bzip2::Compression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;

use super::{Decoder, Encoder, method};

/// BZip2 decoder.
pub struct Bzip2Decoder<R> {
    inner: BzDecoder<R>,
}

impl<R> std::fmt::Debug for Bzip2Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bzip2Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> Bzip2Decoder<R> {
    /// Creates a new BZip2 decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The compressed data source
    pub fn new(input: R) -> Self {
        Self {
            inner: BzDecoder::new(input),
        }
    }
}

impl<R: Read + Send> Read for Bzip2Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for Bzip2Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BZIP2
    }
}

/// BZip2 encoder options.
#[derive(Debug, Clone)]
pub struct Bzip2EncoderOptions {
    /// Compression level (1-9, default 9).
    pub level: u32,
}

impl Default for Bzip2EncoderOptions {
    fn default() -> Self {
        Self { level: 9 }
    }
}

impl Bzip2EncoderOptions {
    /// Creates options with the given compression level.
    pub fn with_level(level: u32) -> Self {
        Self {
            level: level.clamp(1, 9),
        }
    }
}

/// BZip2 encoder.
pub struct Bzip2Encoder<W: Write> {
    inner: BzEncoder<W>,
}

impl<W: Write> std::fmt::Debug for Bzip2Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bzip2Encoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> Bzip2Encoder<W> {
    /// Creates a new BZip2 encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for compressed data
    /// * `options` - Encoder options
    pub fn new(output: W, options: &Bzip2EncoderOptions) -> Self {
        Self {
            inner: BzEncoder::new(output, Compression::new(options.level)),
        }
    }

    /// Finishes encoding and flushes all data.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<W: Write + Send> Write for Bzip2Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for Bzip2Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BZIP2
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        self.inner.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_bzip2_roundtrip() {
        let data = b"Hello, World! This is a test of BZip2 compression.";

        // Compress
        let mut compressed = Vec::new();
        let opts = Bzip2EncoderOptions::default();
        {
            let mut encoder = Bzip2Encoder::new(Cursor::new(&mut compressed), &opts);
            encoder.write_all(data).unwrap();
            Box::new(encoder).finish().unwrap();
        }

        // Decompress
        let reader = Cursor::new(&compressed);
        let mut decoder = Bzip2Decoder::new(reader);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_bzip2_encoder_options() {
        let opts = Bzip2EncoderOptions::default();
        assert_eq!(opts.level, 9);

        let opts = Bzip2EncoderOptions::with_level(5);
        assert_eq!(opts.level, 5);

        let opts = Bzip2EncoderOptions::with_level(0);
        assert_eq!(opts.level, 1); // Clamped

        let opts = Bzip2EncoderOptions::with_level(100);
        assert_eq!(opts.level, 9); // Clamped
    }

    #[test]
    fn test_bzip2_method_id() {
        let reader = Cursor::new(Vec::<u8>::new());
        let decoder = Bzip2Decoder::new(reader);
        assert_eq!(decoder.method_id(), method::BZIP2);
    }
}
