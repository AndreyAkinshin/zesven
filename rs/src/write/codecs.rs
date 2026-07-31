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

/// How much of the machine one compression call may use for itself.
///
/// Two levels of parallelism are available to a writer: several entries
/// compressed alongside each other, and one stream cut into blocks that are
/// compressed alongside each other. Both claim cores, so exactly one of them
/// applies to any given call, and this is which.
///
/// The choice costs a little compression, which is why it is not simply "use
/// every core": a block is handed the window before it and so matches across
/// its own start, but a boundary still costs something. Splitting is therefore
/// reserved for a stream that has nothing to run alongside - a large entry
/// written on its own, or a solid block - where the alternative is one core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Concurrency {
    /// Other entries are being compressed at the same time.
    ///
    /// One thread and one unbroken stream: the cores are busy already, and the
    /// archive is smaller for not splitting this entry.
    Alongside,
    /// This stream is the only work there is.
    ///
    /// It may be cut into blocks and spread over this many threads. The number
    /// changes how long the call takes and never what it writes.
    Alone(usize),
}

impl Concurrency {
    /// The concurrency for a stream that has the machine to itself.
    ///
    /// Every thread the caller allowed. What the memory limit bounds is how
    /// many blocks are in flight, which the encoder works out from the block it
    /// is cutting at the time: reserving here instead would mean picking a
    /// block size before any is known and running that few threads for the
    /// whole stream.
    #[cfg_attr(
        not(all(feature = "lzma2", feature = "parallel")),
        allow(unused_variables)
    )]
    pub(crate) fn alone(options: &WriteOptions, _data_len: usize) -> Self {
        #[cfg(all(feature = "lzma2", feature = "parallel"))]
        {
            Self::Alone(options.threads.count())
        }
        #[cfg(not(all(feature = "lzma2", feature = "parallel")))]
        Self::Alone(1)
    }

    /// The concurrency for an entry compressed on its own terms.
    ///
    /// The blocking writer decides this by the path an entry takes: one large
    /// enough to be written on its own is [`Self::Alone`], and anything
    /// smaller waits in a batch alongside other entries and is left whole.
    /// Callers with no batch of their own - the async writer, which
    /// compresses one entry at a time - ask here instead of guessing, so that
    /// both APIs cut a stream at the same sizes and write the same archive.
    #[cfg(feature = "async")]
    pub(crate) fn for_entry(options: &WriteOptions, data_len: usize) -> Self {
        // Both halves of the blocking writer's question, since both decide
        // whether that entry is left whole: options that keep an entry off the
        // write-through path put it in a batch, and a batched entry is not
        // split however large it is.
        let large = data_len as u64 >= super::streaming_entry::STREAMING_THRESHOLD;
        if large && super::streaming_entry::can_stream(options) {
            Self::alone(options, data_len)
        } else {
            Self::Alongside
        }
    }

    /// Returns whether this call cuts its stream into blocks.
    ///
    /// Never when something else is already using the cores: the archive is
    /// smaller for leaving such an entry whole, and splitting it would only
    /// take threads from the entries alongside it.
    #[cfg(all(feature = "lzma2", feature = "parallel"))]
    fn is_chunked(
        self,
        options: &WriteOptions,
        encoder: &crate::codec::lzma::Lzma2EncoderOptions,
        data_len: usize,
    ) -> bool {
        match self {
            Self::Alongside => false,
            Self::Alone(_) => {
                // A caller with the data in hand knows whether it is long
                // enough to be cut, and a stream shorter than that comes out
                // identical either way - so the thread pool is not built for
                // it. The write-through path cannot ask this, since it has no
                // length until the stream ends.
                let dictionary = u64::from(encoder.dict_size.unwrap_or(0));
                data_len as u64 >= crate::codec::lzma2_chunked::shortest_split_stream(dictionary)
                    && lzma2_is_chunked(options, encoder)
            }
        }
    }

    /// Returns how many threads this call may use.
    #[cfg(all(feature = "lzma2", feature = "parallel"))]
    fn workers(self) -> usize {
        match self {
            Self::Alongside => 1,
            Self::Alone(workers) => workers.max(1),
        }
    }
}

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

/// Returns the dictionary for a stream compressed as it is read.
///
/// The level's dictionary, not narrowed to the data the way a buffered entry's
/// is. Nothing on that path knows how long the stream will turn out to be, and
/// narrowing it to the length the caller declared would let a wrong declaration
/// change the bytes of the archive: a stale `stat`, or a file still being
/// written. An entry only reaches that path once it is past the streaming
/// threshold, and no level's dictionary is larger than that, so wherever both
/// paths could apply they agree.
#[cfg(feature = "lzma")]
pub(crate) fn stream_dictionary_size(options: &WriteOptions) -> u32 {
    crate::codec::lzma::preset_dict_size(options.level)
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

/// Returns the LZMA2 encoder settings for compressing `data_len` bytes.
#[cfg(feature = "lzma2")]
pub(crate) fn lzma2_options(
    options: &WriteOptions,
    data_len: usize,
) -> crate::codec::lzma::Lzma2EncoderOptions {
    crate::codec::lzma::Lzma2EncoderOptions {
        preset: options.level,
        dict_size: Some(dictionary_size(options, data_len)),
    }
}

/// Returns whether an LZMA2 stream is cut into blocks rather than left whole.
///
/// One unbroken stream is the smaller output - a boundary costs a little even
/// though each block is handed the window before it - and it is what a caller
/// who asked for a single thread gets. Where the blocks fall when there are any is decided by the
/// encoder, from the bytes that have gone through it - not here, and not from
/// any figure a caller supplied.
#[cfg(all(feature = "lzma2", feature = "parallel"))]
pub(crate) fn lzma2_is_chunked(
    options: &WriteOptions,
    encoder: &crate::codec::lzma::Lzma2EncoderOptions,
) -> bool {
    // A caller who asked for one thread gets one stream, which is both the
    // smallest this level can produce and the only form that is identical on
    // every machine.
    !options.threads.is_single() && encoder.dict_size.is_some_and(|size| size > 0)
}

/// Compresses data using LZMA2.
#[cfg(feature = "lzma2")]
pub(crate) fn compress_lzma2(
    options: &WriteOptions,
    data: &[u8],
    #[cfg_attr(not(feature = "parallel"), allow(unused_variables))] concurrency: Concurrency,
) -> Result<Compressed> {
    use crate::codec::lzma::Lzma2Encoder;

    let opts = lzma2_options(options, data.len());

    #[cfg(feature = "parallel")]
    if concurrency.is_chunked(options, &opts, data.len()) {
        use crate::codec::lzma2_chunked::ChunkedLzma2Encoder;

        let mut output = Vec::new();
        {
            let mut encoder = ChunkedLzma2Encoder::new(
                &mut output,
                &opts,
                concurrency.workers(),
                options.memory_limit.bytes(),
            )?;
            encoder.write_all(data).map_err(crate::Error::Io)?;
            encoder.finish().map_err(crate::Error::Io)?;
        }
        return Ok(Compressed {
            data: output,
            properties: opts.properties(),
        });
    }

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

#[cfg(all(test, feature = "lzma2", feature = "parallel"))]
mod tests {
    use super::*;
    use crate::Threads;

    fn is_chunked(options: &WriteOptions) -> bool {
        let encoder = lzma2_options(options, usize::MAX);
        lzma2_is_chunked(options, &encoder)
    }

    /// Nothing about the machine may reach the decision to split.
    ///
    /// Where a stream is cut decides the bytes; if the core count or the memory
    /// budget got into it, the same input would produce different archives on
    /// different hardware.
    #[test]
    fn test_splitting_does_not_depend_on_the_machine() {
        for threads in [2usize, 4, 16, 64] {
            for limit in [64 << 20, 512 << 20, 8u64 << 30] {
                let options = WriteOptions::new()
                    .level(5)
                    .expect("level")
                    .threads(Threads::count_or_single(threads))
                    .memory_limit(crate::MemoryLimit::bytes_or_auto(limit));
                assert!(is_chunked(&options), "threads={threads} limit={limit}");
            }
        }
    }

    /// One thread means one unbroken stream, however it was asked for.
    ///
    /// `Count(1)` is not a separate case that happens to behave the same: it
    /// resolves to one thread, and it is resolving to one thread that decides
    /// this. `Auto` on a single-core machine lands here too.
    #[test]
    fn test_a_single_thread_is_never_split() {
        for threads in [Threads::Single, Threads::count_or_single(1)] {
            let options = WriteOptions::new()
                .level(5)
                .expect("level")
                .threads(threads);
            assert!(!is_chunked(&options), "{threads:?} was split");
        }
    }

    /// The two ways of asking "is this stream cut?" have to give one answer.
    ///
    /// A caller with the data in hand asks by length; the encoder cuts as the
    /// bytes arrive. The two comparisons have to agree at every length, and
    /// they did not: `>` here against `>=` in the encoder, which is one length
    /// in the whole range - at level 9 an 80 MiB entry, ordinary enough for
    /// someone to have one - where the blocking and async writers produced
    /// different archives from the same input.
    ///
    /// Checked against the encoder itself rather than against a second copy of
    /// the rule, at a dictionary small enough to be free.
    #[test]
    fn test_the_split_threshold_matches_where_the_encoder_cuts() {
        use crate::codec::lzma::Lzma2EncoderOptions;
        use crate::codec::lzma2_chunked::{ChunkedLzma2Encoder, shortest_split_stream};
        use std::io::Write;

        let dictionary = 1u64 << 16;
        let encoder_options = Lzma2EncoderOptions::with_preset(1).with_dict_size(dictionary as u32);
        let boundary = shortest_split_stream(dictionary);

        // What one unbroken stream of this input looks like.
        let unsplit = |len: usize| {
            let mut out = Vec::new();
            let mut encoder = crate::codec::lzma::Lzma2Encoder::new(&mut out, &encoder_options);
            encoder.write_all(&vec![0u8; len]).expect("writes");
            encoder.try_finish().expect("finishes");
            out
        };
        let chunked = |len: usize| {
            let mut out = Vec::new();
            let mut encoder =
                ChunkedLzma2Encoder::new(&mut out, &encoder_options, 4, u64::MAX).expect("builds");
            encoder.write_all(&vec![0u8; len]).expect("writes");
            encoder.finish().expect("finishes");
            out
        };

        // The options a caller who reached this decision would hold.
        let options = WriteOptions::new()
            .level(1)
            .expect("level")
            .threads(Threads::count_or_single(4));

        for len in [boundary - 1, boundary, boundary + 1] {
            let cut = chunked(len as usize) != unsplit(len as usize);
            // The production predicate itself, not a second copy of its rule -
            // a copy agrees with the encoder while the code that decides for
            // real does not, which is exactly how the 80 MiB divergence
            // survived a test that looked like this one.
            let predicted =
                Concurrency::Alone(4).is_chunked(&options, &encoder_options, len as usize);
            assert_eq!(
                cut,
                predicted,
                "at {len} bytes the encoder {} but the writer says it {}",
                if cut { "cut" } else { "did not cut" },
                if predicted { "would" } else { "would not" },
            );
        }
    }

    /// A stream compressed as it is read must use the level's own dictionary.
    ///
    /// Narrowing it to the data is right for an entry held in memory, whose
    /// length is known, and impossible for one that is not: the only length
    /// available there is the one the caller declared, which may be wrong.
    /// These agree wherever both could apply, which is what keeps an entry's
    /// bytes the same whichever path it took.
    #[test]
    fn test_the_streaming_dictionary_matches_the_buffered_one() {
        use super::super::streaming_entry::STREAMING_THRESHOLD;

        for level in 0..=9u32 {
            let options = WriteOptions::new().level(level).expect("level");
            let streamed = stream_dictionary_size(&options);
            assert!(
                u64::from(streamed) <= STREAMING_THRESHOLD,
                "level {level} wants a dictionary larger than the threshold, so the \
                 two paths would disagree on an entry just past it",
            );
            for size in [STREAMING_THRESHOLD + 1, STREAMING_THRESHOLD * 4, 4 << 30] {
                assert_eq!(
                    dictionary_size(&options, size as usize),
                    streamed,
                    "level {level} at {size} bytes",
                );
            }
        }
    }
}
