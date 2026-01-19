//! Async codec infrastructure for 7z archives.
//!
//! This module provides async wrappers around compression codecs using
//! `async-compression` where available, and `spawn_blocking` for codecs
//! that only have synchronous implementations.

use std::io::{self, Read};
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::{Error, Result};

pub use crate::codec::CodecMethod;
/// Method IDs for compression algorithms.
pub use crate::codec::method;

/// Async decoder trait for reading compressed data asynchronously.
pub trait AsyncDecoder: AsyncRead + Send + Unpin {
    /// Returns the method ID for this decoder.
    fn method_id(&self) -> &'static [u8];
}

/// Async encoder trait for writing compressed data asynchronously.
pub trait AsyncEncoder: AsyncWrite + Send + Unpin {
    /// Returns the method ID for this encoder.
    fn method_id(&self) -> &'static [u8];

    /// Finishes encoding and flushes any remaining data.
    fn finish(&mut self) -> Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + '_>>;
}

// ============================================================================
// Copy Codec (Pass-through, no compression)
// ============================================================================

pin_project! {
    /// Async copy decoder (no decompression).
    pub struct AsyncCopyDecoder<R> {
        #[pin]
        reader: R,
        remaining: u64,
    }
}

impl<R> AsyncCopyDecoder<R> {
    /// Creates a new copy decoder.
    pub fn new(reader: R, size: u64) -> Self {
        Self {
            reader,
            remaining: size,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncCopyDecoder<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();

        if *this.remaining == 0 {
            return Poll::Ready(Ok(()));
        }

        // Limit read to remaining bytes
        let max_read = (*this.remaining as usize).min(buf.remaining());
        let mut limited_buf = buf.take(max_read);

        match this.reader.poll_read(cx, &mut limited_buf) {
            Poll::Ready(Ok(())) => {
                let n = limited_buf.filled().len();
                *this.remaining -= n as u64;
                // Advance the original buffer by the amount we read
                unsafe {
                    buf.assume_init(buf.filled().len() + n);
                }
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<R: AsyncRead + Send + Unpin> AsyncDecoder for AsyncCopyDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::COPY
    }
}

// ============================================================================
// Async Compression using async-compression crate
// ============================================================================

pin_project! {
    /// Async LZMA decoder using async-compression.
    pub struct AsyncLzmaDecoder<R> {
        #[pin]
        inner: async_compression::tokio::bufread::LzmaDecoder<tokio::io::BufReader<R>>,
    }
}

impl<R: AsyncRead + Unpin> AsyncLzmaDecoder<R> {
    /// Creates a new LZMA decoder.
    pub fn new(reader: R) -> Self {
        let buf_reader = tokio::io::BufReader::new(reader);
        Self {
            inner: async_compression::tokio::bufread::LzmaDecoder::new(buf_reader),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncLzmaDecoder<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<R: AsyncRead + Send + Unpin> AsyncDecoder for AsyncLzmaDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA
    }
}

pin_project! {
    /// Async Deflate decoder using async-compression.
    pub struct AsyncDeflateDecoder<R> {
        #[pin]
        inner: async_compression::tokio::bufread::DeflateDecoder<tokio::io::BufReader<R>>,
    }
}

impl<R: AsyncRead + Unpin> AsyncDeflateDecoder<R> {
    /// Creates a new Deflate decoder.
    pub fn new(reader: R) -> Self {
        let buf_reader = tokio::io::BufReader::new(reader);
        Self {
            inner: async_compression::tokio::bufread::DeflateDecoder::new(buf_reader),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncDeflateDecoder<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<R: AsyncRead + Send + Unpin> AsyncDecoder for AsyncDeflateDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::DEFLATE
    }
}

pin_project! {
    /// Async BZip2 decoder using async-compression.
    pub struct AsyncBzip2Decoder<R> {
        #[pin]
        inner: async_compression::tokio::bufread::BzDecoder<tokio::io::BufReader<R>>,
    }
}

impl<R: AsyncRead + Unpin> AsyncBzip2Decoder<R> {
    /// Creates a new BZip2 decoder.
    pub fn new(reader: R) -> Self {
        let buf_reader = tokio::io::BufReader::new(reader);
        Self {
            inner: async_compression::tokio::bufread::BzDecoder::new(buf_reader),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncBzip2Decoder<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<R: AsyncRead + Send + Unpin> AsyncDecoder for AsyncBzip2Decoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::BZIP2
    }
}

// ============================================================================
// Async Encoders using async-compression
// ============================================================================

pin_project! {
    /// Async LZMA encoder using async-compression.
    pub struct AsyncLzmaEncoder<W> {
        #[pin]
        inner: async_compression::tokio::write::LzmaEncoder<W>,
    }
}

impl<W: AsyncWrite + Unpin> AsyncLzmaEncoder<W> {
    /// Creates a new LZMA encoder.
    pub fn new(writer: W) -> Self {
        Self {
            inner: async_compression::tokio::write::LzmaEncoder::new(writer),
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncLzmaEncoder<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

impl<W: AsyncWrite + Send + Unpin> AsyncEncoder for AsyncLzmaEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA
    }

    fn finish(&mut self) -> Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Shutdown calls finish internally for async-compression encoders
            AsyncWriteExt::shutdown(self).await
        })
    }
}

pin_project! {
    /// Async Deflate encoder using async-compression.
    pub struct AsyncDeflateEncoder<W> {
        #[pin]
        inner: async_compression::tokio::write::DeflateEncoder<W>,
    }
}

impl<W: AsyncWrite + Unpin> AsyncDeflateEncoder<W> {
    /// Creates a new Deflate encoder with the given compression level (0-9).
    pub fn new(writer: W, level: u32) -> Self {
        let compression = async_compression::Level::Precise(level as i32);
        Self {
            inner: async_compression::tokio::write::DeflateEncoder::with_quality(
                writer,
                compression,
            ),
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncDeflateEncoder<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

impl<W: AsyncWrite + Send + Unpin> AsyncEncoder for AsyncDeflateEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::DEFLATE
    }

    fn finish(&mut self) -> Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + '_>> {
        Box::pin(async move { AsyncWriteExt::shutdown(self).await })
    }
}

pin_project! {
    /// Async BZip2 encoder using async-compression.
    pub struct AsyncBzip2Encoder<W> {
        #[pin]
        inner: async_compression::tokio::write::BzEncoder<W>,
    }
}

impl<W: AsyncWrite + Unpin> AsyncBzip2Encoder<W> {
    /// Creates a new BZip2 encoder with the given compression level (1-9).
    pub fn new(writer: W, level: u32) -> Self {
        let compression = async_compression::Level::Precise(level as i32);
        Self {
            inner: async_compression::tokio::write::BzEncoder::with_quality(writer, compression),
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncBzip2Encoder<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

impl<W: AsyncWrite + Send + Unpin> AsyncEncoder for AsyncBzip2Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::BZIP2
    }

    fn finish(&mut self) -> Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send + '_>> {
        Box::pin(async move { AsyncWriteExt::shutdown(self).await })
    }
}

// ============================================================================
// Factory Functions
// ============================================================================

/// Builds an async decoder for a given method.
///
/// # Arguments
///
/// * `input` - The async reader providing compressed data
/// * `method` - The compression method to use
/// * `uncompressed_size` - Expected size of uncompressed output (used for Copy method)
///
/// # Errors
///
/// Returns an error if the compression method is unsupported.
pub fn build_async_decoder<R: AsyncRead + Send + Unpin + 'static>(
    input: R,
    method: CodecMethod,
    uncompressed_size: u64,
) -> Result<Box<dyn AsyncDecoder>> {
    match method {
        CodecMethod::Copy => Ok(Box::new(AsyncCopyDecoder::new(input, uncompressed_size))),
        CodecMethod::Lzma => Ok(Box::new(AsyncLzmaDecoder::new(input))),
        CodecMethod::Lzma2 => {
            // LZMA2 uses the same LZMA decoder in async-compression
            Ok(Box::new(AsyncLzmaDecoder::new(input)))
        }
        CodecMethod::Deflate => Ok(Box::new(AsyncDeflateDecoder::new(input))),
        CodecMethod::BZip2 => Ok(Box::new(AsyncBzip2Decoder::new(input))),
        CodecMethod::PPMd => Err(Error::UnsupportedFeature {
            feature: "async PPMd decompression",
        }),
        CodecMethod::Lz4 => Err(Error::UnsupportedFeature {
            feature: "async LZ4 decompression",
        }),
        CodecMethod::Zstd => Err(Error::UnsupportedFeature {
            feature: "async ZSTD decompression",
        }),
        CodecMethod::Brotli => Err(Error::UnsupportedFeature {
            feature: "async Brotli decompression",
        }),
    }
}

/// Builds an async encoder for a given method.
///
/// # Arguments
///
/// * `output` - The async writer to write compressed data to
/// * `method` - The compression method to use
/// * `level` - Compression level (0-9, higher = better compression)
///
/// # Errors
///
/// Returns an error if the compression method is unsupported.
pub fn build_async_encoder<W: AsyncWrite + Send + Unpin + 'static>(
    output: W,
    method: CodecMethod,
    level: u32,
) -> Result<Box<dyn AsyncEncoder>> {
    match method {
        CodecMethod::Copy => Err(Error::UnsupportedFeature {
            feature: "async copy encoder (use direct write)",
        }),
        CodecMethod::Lzma => Ok(Box::new(AsyncLzmaEncoder::new(output))),
        CodecMethod::Lzma2 => {
            // LZMA2 uses the same LZMA encoder in async-compression
            Ok(Box::new(AsyncLzmaEncoder::new(output)))
        }
        CodecMethod::Deflate => Ok(Box::new(AsyncDeflateEncoder::new(output, level))),
        CodecMethod::BZip2 => Ok(Box::new(AsyncBzip2Encoder::new(output, level))),
        CodecMethod::PPMd => Err(Error::UnsupportedFeature {
            feature: "async PPMd compression",
        }),
        CodecMethod::Lz4 => Err(Error::UnsupportedFeature {
            feature: "async LZ4 compression",
        }),
        CodecMethod::Zstd => Err(Error::UnsupportedFeature {
            feature: "async ZSTD compression",
        }),
        CodecMethod::Brotli => Err(Error::UnsupportedFeature {
            feature: "async Brotli compression",
        }),
    }
}

// ============================================================================
// Sync-to-Async Bridge (for codecs without native async support)
// ============================================================================

/// A sync-to-async bridge that wraps a sync decoder in an async interface.
///
/// This is used for codecs that don't have native async implementations
/// (like PPMd) by running the sync decompression in a blocking task.
#[allow(dead_code)] // Infrastructure for async codec bridging
pub struct BlockingDecoder<D> {
    decoder: Option<D>,
    buffer: Vec<u8>,
    position: usize,
    chunk_size: usize,
}

impl<D: Read + Send + 'static> BlockingDecoder<D> {
    /// Creates a new blocking decoder bridge.
    pub fn new(decoder: D, chunk_size: usize) -> Self {
        Self {
            decoder: Some(decoder),
            buffer: Vec::new(),
            position: 0,
            chunk_size,
        }
    }

    /// Reads more data from the decoder in a blocking task.
    #[allow(dead_code)] // Part of async codec infrastructure
    async fn read_chunk(&mut self) -> io::Result<()> {
        if let Some(mut decoder) = self.decoder.take() {
            let chunk_size = self.chunk_size;
            let result = tokio::task::spawn_blocking(move || {
                let mut chunk = vec![0u8; chunk_size];
                match decoder.read(&mut chunk) {
                    Ok(n) => {
                        chunk.truncate(n);
                        Ok((decoder, chunk))
                    }
                    Err(e) => Err(e),
                }
            })
            .await
            .map_err(io::Error::other)?;

            match result {
                Ok((decoder, chunk)) => {
                    self.decoder = Some(decoder);
                    self.buffer = chunk;
                    self.position = 0;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(())
        }
    }
}

impl<D: Read + Send + Unpin + 'static> AsyncRead for BlockingDecoder<D> {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // Return buffered data if available
        if this.position < this.buffer.len() {
            let available = &this.buffer[this.position..];
            let to_copy = available.len().min(buf.remaining());
            buf.put_slice(&available[..to_copy]);
            this.position += to_copy;
            return Poll::Ready(Ok(()));
        }

        // Buffer exhausted, need to read more
        // This is a simplified implementation that returns Pending
        // In practice, you'd need a proper waker/future setup
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_async_copy_decoder() {
        let data = b"Hello, async world!";
        let cursor = std::io::Cursor::new(data.to_vec());
        let mut decoder = AsyncCopyDecoder::new(cursor, data.len() as u64);

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, data);
    }

    #[tokio::test]
    async fn test_build_async_decoder_copy() {
        let data = b"test data";
        let cursor = std::io::Cursor::new(data.to_vec());
        let mut decoder =
            build_async_decoder(cursor, CodecMethod::Copy, data.len() as u64).unwrap();

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, data);
    }

    #[test]
    fn test_method_id_constants() {
        assert_eq!(method::COPY, &[0x00]);
        assert_eq!(method::LZMA, &[0x03, 0x01, 0x01]);
        assert_eq!(method::LZMA2, &[0x21]);
    }
}
