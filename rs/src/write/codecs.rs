//! Codec-specific compression implementations.
//!
//! This module provides compression functions for each supported codec.
//!
//! Every function returns the coder properties alongside the compressed bytes.
//! The properties are what a reader needs in order to decode the stream, so
//! they have to describe the settings the data was actually encoded with;
//! deriving them a second time from the options is how they drift apart.

// Only the codecs behind feature flags write through this.
#[allow(unused_imports)]
use std::io::Write;

#[allow(unused_imports)]
use crate::Result;

#[allow(unused_imports)]
use super::options::WriteOptions;

/// Compressed bytes together with the coder properties describing them.
pub(crate) struct Compressed {
    /// The compressed bytes.
    pub data: Vec<u8>,
    /// Coder properties, empty for codecs that have none.
    pub properties: Vec<u8>,
}

impl Compressed {
    /// Wraps output from a codec that has no properties.
    pub(crate) fn without_properties(data: Vec<u8>) -> Self {
        Self {
            data,
            properties: Vec::new(),
        }
    }
}

/// Returns the dictionary to compress `data_len` bytes with.
///
/// The compression level picks a dictionary, but a dictionary larger than
/// the data is only a memory cost: a reader allocates whatever the archive
/// declares, and no match can reach further back than the input's start.
#[cfg(feature = "lzma")]
pub(crate) fn dictionary_size(options: &WriteOptions, data_len: usize) -> u32 {
    use crate::codec::lzma::{dict_size_covering, preset_dict_size};

    preset_dict_size(options.level).min(dict_size_covering(data_len as u64))
}

/// Returns roughly what one encoder of the configured method will hold.
///
/// Each codec reserves something different: LZMA keeps a match finder several
/// times its dictionary, PPMd allocates a model whose size comes from the
/// level alone, and the rest work in buffers small enough not to matter here.
/// Estimating them all as LZMA would understate PPMd by a factor of sixty at
/// the top level, which is how a memory budget stops meaning anything.
#[cfg(feature = "parallel")]
pub(crate) fn encoder_memory_usage(
    options: &WriteOptions,
    #[cfg_attr(not(feature = "lzma"), allow(unused_variables))] data_len: usize,
) -> u64 {
    #[allow(unused_imports)]
    use crate::codec::CodecMethod;

    // Everything not named below streams through buffers measured in tens of
    // kilobytes; a megabyte apiece is a generous stand-in.
    const SMALL_ENCODER: u64 = 1 << 20;

    #[allow(clippy::match_single_binding)]
    match options.method {
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => crate::codec::lzma::encoder_memory_usage(
            options.level,
            dictionary_size(options, data_len),
        ),
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => crate::codec::lzma::encoder_memory_usage(
            options.level,
            dictionary_size(options, data_len),
        ),
        #[cfg(feature = "ppmd")]
        CodecMethod::PPMd => {
            // The model is allocated up front at exactly this size.
            let (_, mem_size) = ppmd_settings(options);
            u64::from(mem_size) + SMALL_ENCODER
        }
        #[cfg(feature = "bzip2")]
        CodecMethod::BZip2 => {
            // Block size is 100 KiB per level, and the encoder works in
            // several buffers of that size at once.
            let block = u64::from(options.level.clamp(1, 9)) * 100 * 1024;
            block * 8 + SMALL_ENCODER
        }
        _ => SMALL_ENCODER,
    }
}

/// Compresses data using LZMA2.
#[cfg(feature = "lzma2")]
pub(crate) fn compress_lzma2(
    options: &WriteOptions,
    data: &[u8],
    may_thread: bool,
) -> Result<Compressed> {
    use crate::codec::lzma::{Lzma2Encoder, Lzma2EncoderOptions};

    let opts = Lzma2EncoderOptions {
        preset: options.level,
        dict_size: Some(dictionary_size(options, data.len())),
    };

    #[cfg(feature = "parallel")]
    if may_thread {
        if let Some(compressed) = compress_lzma2_parallel(options, data, &opts)? {
            return Ok(compressed);
        }
    }
    #[cfg(not(feature = "parallel"))]
    let _ = may_thread;

    let mut output = Vec::new();
    {
        let mut encoder = Lzma2Encoder::new(&mut output, &opts);
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed {
        data: output,
        properties: opts.properties(),
    })
}

/// Compresses data on several threads, if there is enough of it to split.
///
/// Returns `None` when the data does not reach two chunks, in which case
/// there is nothing to run in parallel and the single-threaded encoder
/// produces a better result.
#[cfg(all(feature = "lzma2", feature = "parallel"))]
fn compress_lzma2_parallel(
    options: &WriteOptions,
    data: &[u8],
    opts: &crate::codec::lzma::Lzma2EncoderOptions,
) -> Result<Option<Compressed>> {
    use crate::codec::lzma::Lzma2EncoderMt;

    // A caller who asked for one thread gets one stream, which is both the
    // smallest this level can produce and the only form that is identical on
    // every machine.
    if options.threads.is_single() {
        return Ok(None);
    }

    // A chunk cannot match against the chunk before it, so chunking costs
    // compression, and larger chunks cost less of it. They also split the work
    // into fewer pieces: at four dictionaries per chunk, a default 64 MiB solid
    // block is two chunks and two cores, which is not enough to pay for the
    // dictionary the level asks for - that combination compressed a block of
    // incompressible data slower than release 1.2.0 did. One dictionary per
    // chunk keeps every core busy and still produces a smaller archive than
    // 1.2.0; the entries of a non-solid archive are compressed in parallel
    // with each other instead, and do not come through here.
    const DICTIONARIES_PER_CHUNK: u64 = 1;

    let dictionary = u64::from(opts.dict_size.unwrap_or(0));
    let chunk_size = dictionary.saturating_mul(DICTIONARIES_PER_CHUNK);
    if chunk_size == 0 {
        return Ok(None);
    }

    let chunks = (data.len() as u64).div_ceil(chunk_size);
    if chunks < 2 {
        return Ok(None);
    }

    // How many workers actually run follows from the machine and the memory
    // budget, and deliberately does not decide *whether* to chunk: chunk
    // boundaries come from the dictionary alone, so the bytes are the same
    // whether one worker or twenty produced them. Deciding to chunk by core
    // count instead would mean a small machine writing a different archive
    // from a large one.
    let workers = chunks
        .min(super::entry_compression::workers_within_budget(options, data.len()) as u64)
        .max(1);

    let mut output = Vec::new();
    {
        let mut encoder = Lzma2EncoderMt::new(&mut output, opts, chunk_size, workers as u32)?;
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Some(Compressed {
        data: output,
        properties: opts.properties(),
    }))
}

/// Compresses data using LZMA.
#[cfg(feature = "lzma")]
pub(crate) fn compress_lzma(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

    let opts = LzmaEncoderOptions {
        preset: options.level,
        dict_size: Some(dictionary_size(options, data.len())),
    };
    let mut output = Vec::new();
    {
        let mut encoder = LzmaEncoder::new(&mut output, &opts)?;
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed {
        data: output,
        properties: opts.properties(),
    })
}

/// Compresses data using Deflate.
#[cfg(feature = "deflate")]
pub(crate) fn compress_deflate(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use crate::codec::deflate::{DeflateEncoder, DeflateEncoderOptions};

    let opts = DeflateEncoderOptions {
        level: options.level,
    };
    let mut output = Vec::new();
    {
        let mut encoder = DeflateEncoder::new(&mut output, &opts);
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed::without_properties(output))
}

/// Compresses data using BZip2.
#[cfg(feature = "bzip2")]
pub(crate) fn compress_bzip2(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use crate::codec::bzip2::{Bzip2Encoder, Bzip2EncoderOptions};

    let opts = Bzip2EncoderOptions {
        level: options.level,
    };
    let mut output = Vec::new();
    {
        let mut encoder = Bzip2Encoder::new(&mut output, &opts);
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed::without_properties(output))
}

/// Compresses data using Zstd.
#[cfg(feature = "zstd")]
pub(crate) fn compress_zstd(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use super::ZSTD_LEVEL_MAP;
    use crate::codec::zstd::{ZstdEncoderOptions, ZstdStreamEncoder};

    let zstd_level = ZSTD_LEVEL_MAP[options.level.min(9) as usize];

    let opts = ZstdEncoderOptions { level: zstd_level };
    let mut output = Vec::new();
    {
        let mut encoder = ZstdStreamEncoder::new(&mut output, &opts)
            .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed::without_properties(output))
}

/// Compresses data using LZ4.
#[cfg(feature = "lz4")]
pub(crate) fn compress_lz4(_options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use crate::codec::lz4::{Lz4Encoder, Lz4EncoderOptions};

    let opts = Lz4EncoderOptions::default();
    let mut output = Vec::new();
    {
        let mut encoder = Lz4Encoder::new(&mut output, &opts);
        encoder.write_all(data).map_err(crate::Error::Io)?;
        encoder.try_finish().map_err(crate::Error::Io)?;
    }
    Ok(Compressed::without_properties(output))
}

/// Compresses data using Brotli.
#[cfg(feature = "brotli")]
pub(crate) fn compress_brotli(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use super::BROTLI_QUALITY_MAP;
    use crate::codec::brotli::{BrotliEncoder, BrotliEncoderOptions};

    let quality = BROTLI_QUALITY_MAP[options.level.min(9) as usize];

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
    Ok(Compressed::without_properties(output))
}

/// Returns the PPMd order and memory size for the configured level.
#[cfg(feature = "ppmd")]
fn ppmd_settings(options: &WriteOptions) -> (u32, u32) {
    // Higher levels use higher order and more memory.
    match options.level {
        0..=2 => (4, 4 * 1024 * 1024),
        3..=4 => (6, 8 * 1024 * 1024),
        5..=6 => (6, 16 * 1024 * 1024),
        7..=8 => (8, 32 * 1024 * 1024),
        _ => (8, 64 * 1024 * 1024),
    }
}

/// Compresses data using PPMd.
#[cfg(feature = "ppmd")]
pub(crate) fn compress_ppmd(options: &WriteOptions, data: &[u8]) -> Result<Compressed> {
    use crate::codec::Encoder;
    use crate::codec::ppmd::{PpmdEncoder, PpmdEncoderOptions};

    let (order, mem_size) = ppmd_settings(options);

    let opts = PpmdEncoderOptions::new(order, mem_size);
    let mut output = Vec::new();
    {
        let mut encoder = PpmdEncoder::new(&mut output, &opts)?;
        encoder.write_all(data).map_err(crate::Error::Io)?;
        Box::new(encoder).finish().map_err(crate::Error::Io)?;
    }

    let mut properties = vec![order as u8];
    properties.extend_from_slice(&mem_size.to_le_bytes());

    Ok(Compressed {
        data: output,
        properties,
    })
}
