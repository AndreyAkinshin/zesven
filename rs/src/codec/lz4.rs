//! LZ4 compression codec.
//!
//! This module provides LZ4 compression support for 7z archives.
//! LZ4 is a fast compression algorithm optimized for speed.

use std::io::{self, Read, Write};

use lz4_flex::frame::{FrameDecoder, FrameEncoder};

use super::{Decoder, Encoder, method};

/// LZ4 decoder.
pub struct Lz4Decoder<R: Read> {
    inner: FrameDecoder<R>,
}

impl<R: Read> std::fmt::Debug for Lz4Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lz4Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> Lz4Decoder<R> {
    /// Creates a new LZ4 decoder.
    pub fn new(input: R) -> Self {
        Self {
            inner: FrameDecoder::new(input),
        }
    }
}

impl<R: Read + Send> Read for Lz4Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Send> Decoder for Lz4Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ4
    }
}

/// LZ4 encoder options.
#[derive(Debug, Clone, Default)]
pub struct Lz4EncoderOptions {
    // LZ4 has limited configuration options in lz4_flex
}

/// LZ4 encoder.
pub struct Lz4Encoder<W: Write> {
    inner: FrameEncoder<W>,
}

impl<W: Write + Send> Lz4Encoder<W> {
    /// Creates a new LZ4 encoder.
    pub fn new(output: W, _options: &Lz4EncoderOptions) -> Self {
        Self {
            inner: FrameEncoder::new(output),
        }
    }

    /// Finishes encoding and returns the underlying writer.
    pub fn try_finish(self) -> io::Result<W> {
        self.inner.finish().map_err(io::Error::other)
    }
}

impl<W: Write + Send> Write for Lz4Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for Lz4Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZ4
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
    fn test_lz4_round_trip() {
        let original = b"Hello, World! This is a test of LZ4 compression.";

        // Compress
        let mut compressed = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut compressed, &Lz4EncoderOptions::default());
            encoder.write_all(original).unwrap();
            encoder.try_finish().unwrap();
        }

        // Decompress
        let mut decoder = Lz4Decoder::new(Cursor::new(compressed));
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz4_decoder_method_id() {
        let data = vec![0u8; 16];
        let decoder = Lz4Decoder::new(Cursor::new(data));
        assert_eq!(decoder.method_id(), method::LZ4);
    }
}
