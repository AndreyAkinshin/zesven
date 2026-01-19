//! Brotli compression codec.
//!
//! This module provides Brotli compression support for 7z archives.
//! Brotli is a compression algorithm developed by Google, optimized for web content.

use std::io::{self, Read, Write};

use brotli::CompressorWriter;
use brotli::Decompressor;
use brotli::enc::BrotliEncoderParams;

use super::{Decoder, Encoder, method};

/// Brotli decoder.
pub struct BrotliDecoder<R: Read> {
    inner: Decompressor<R>,
}

impl<R: Read> std::fmt::Debug for BrotliDecoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrotliDecoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> BrotliDecoder<R> {
    /// Creates a new Brotli decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: Decompressor::new(input, 4096),
        }
    }
}

impl<R: Read + Send> Read for BrotliDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for BrotliDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BROTLI
    }
}

/// Brotli encoder options.
#[derive(Debug, Clone)]
pub struct BrotliEncoderOptions {
    /// Compression quality (0-11, default 4).
    pub quality: u32,
    /// LG window size (10-24, default 22).
    pub lg_window_size: u32,
}

impl Default for BrotliEncoderOptions {
    fn default() -> Self {
        Self {
            quality: 4,
            lg_window_size: 22,
        }
    }
}

/// Brotli encoder.
pub struct BrotliEncoder<W: Write> {
    inner: CompressorWriter<W>,
}

impl<W: Write + Send> BrotliEncoder<W> {
    /// Creates a new Brotli encoder.
    pub fn new(output: W, options: &BrotliEncoderOptions) -> Self {
        let params = BrotliEncoderParams {
            quality: options.quality as i32,
            lgwin: options.lg_window_size as i32,
            ..Default::default()
        };
        Self {
            inner: CompressorWriter::with_params(output, 4096, &params),
        }
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        Ok(self.inner.into_inner())
    }
}

impl<W: Write + Send> Write for BrotliEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for BrotliEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BROTLI
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        let _ = self.inner.into_inner();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_brotli_round_trip() {
        let original = b"Hello, World! This is a test of Brotli compression.";

        // Compress
        let mut compressed = Vec::new();
        {
            let mut encoder = BrotliEncoder::new(&mut compressed, &BrotliEncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = BrotliDecoder::new(Cursor::new(compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_brotli_decoder_method_id() {
        // Compress some data to create valid brotli stream
        let mut compressed = Vec::new();
        {
            let mut encoder = BrotliEncoder::new(&mut compressed, &BrotliEncoderOptions::default());
            encoder.write_all(b"test").unwrap();
            encoder.try_finish().unwrap();
        }

        let decoder = BrotliDecoder::new(Cursor::new(compressed));
        assert_eq!(decoder.method_id(), method::BROTLI);
    }
}
