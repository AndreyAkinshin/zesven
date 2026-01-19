//! Codec-specific compression implementations.
//!
//! This module provides compression functions for each supported codec.

use std::io::{Seek, Write};

#[allow(unused_imports)]
use crate::Result;

use super::Writer;

impl<W: Write + Seek> Writer<W> {
    /// Compresses data using LZMA2.
    #[cfg(feature = "lzma2")]
    pub(crate) fn compress_lzma2(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::lzma::{Lzma2Encoder, Lzma2EncoderOptions};

        let opts = Lzma2EncoderOptions {
            dict_size: Some(1 << (16 + self.options.level.min(7))),
            ..Default::default()
        };
        let mut output = Vec::new();
        {
            let mut encoder = Lzma2Encoder::new(&mut output, &opts);
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using LZMA.
    #[cfg(feature = "lzma")]
    pub(crate) fn compress_lzma(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

        let opts = LzmaEncoderOptions {
            dict_size: Some(1 << (16 + self.options.level.min(7))),
            ..Default::default()
        };
        let mut output = Vec::new();
        {
            let mut encoder = LzmaEncoder::new(&mut output, &opts)?;
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using Deflate.
    #[cfg(feature = "deflate")]
    pub(crate) fn compress_deflate(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::deflate::{DeflateEncoder, DeflateEncoderOptions};

        let opts = DeflateEncoderOptions {
            level: self.options.level,
        };
        let mut output = Vec::new();
        {
            let mut encoder = DeflateEncoder::new(&mut output, &opts);
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using BZip2.
    #[cfg(feature = "bzip2")]
    pub(crate) fn compress_bzip2(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::bzip2::{Bzip2Encoder, Bzip2EncoderOptions};

        let opts = Bzip2EncoderOptions {
            level: self.options.level,
        };
        let mut output = Vec::new();
        {
            let mut encoder = Bzip2Encoder::new(&mut output, &opts);
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using Zstd.
    #[cfg(feature = "zstd")]
    pub(crate) fn compress_zstd(&self, data: &[u8]) -> Result<Vec<u8>> {
        use super::ZSTD_LEVEL_MAP;
        use crate::codec::zstd::{ZstdEncoderOptions, ZstdStreamEncoder};

        let zstd_level = ZSTD_LEVEL_MAP[self.options.level.min(9) as usize];

        let opts = ZstdEncoderOptions { level: zstd_level };
        let mut output = Vec::new();
        {
            let mut encoder = ZstdStreamEncoder::new(&mut output, &opts)
                .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using LZ4.
    #[cfg(feature = "lz4")]
    pub(crate) fn compress_lz4(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::lz4::{Lz4Encoder, Lz4EncoderOptions};

        let opts = Lz4EncoderOptions::default();
        let mut output = Vec::new();
        {
            let mut encoder = Lz4Encoder::new(&mut output, &opts);
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using Brotli.
    #[cfg(feature = "brotli")]
    pub(crate) fn compress_brotli(&self, data: &[u8]) -> Result<Vec<u8>> {
        use super::BROTLI_QUALITY_MAP;
        use crate::codec::brotli::{BrotliEncoder, BrotliEncoderOptions};

        let quality = BROTLI_QUALITY_MAP[self.options.level.min(9) as usize];

        let opts = BrotliEncoderOptions {
            quality,
            lg_window_size: 22,
        };
        let mut output = Vec::new();
        {
            let mut encoder = BrotliEncoder::new(&mut output, &opts);
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.try_finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }

    /// Compresses data using PPMd.
    #[cfg(feature = "ppmd")]
    pub(crate) fn compress_ppmd(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::Encoder;
        use crate::codec::ppmd::{PpmdEncoder, PpmdEncoderOptions};

        // Map compression level to PPMd order and memory size
        // Higher levels use higher order and more memory
        let (order, mem_size) = match self.options.level {
            0..=2 => (4, 4 * 1024 * 1024),  // 4MB
            3..=4 => (6, 8 * 1024 * 1024),  // 8MB
            5..=6 => (6, 16 * 1024 * 1024), // 16MB
            7..=8 => (8, 32 * 1024 * 1024), // 32MB
            _ => (8, 64 * 1024 * 1024),     // 64MB
        };

        let opts = PpmdEncoderOptions::new(order, mem_size);
        let mut output = Vec::new();
        {
            let mut encoder = PpmdEncoder::new(&mut output, &opts)?;
            encoder.write_all(data).map_err(crate::Error::Io)?;
            Box::new(encoder).finish().map_err(crate::Error::Io)?;
        }
        Ok(output)
    }
}
