//! Compressing an entry straight into the sink.
//!
//! The ordinary path reads an entry into memory, compresses it, and writes the
//! result, which costs the size of the entry plus the size of its compressed
//! form. That is fine for the files most archives hold and impossible for the
//! ones 7z exists to handle: a 10 GB file needed 10 GB of memory to archive.
//!
//! This path compresses as it reads, so memory follows the dictionary rather
//! than the entry. It is used only where the chain from data to sink is a
//! single encoder - no filter, no encryption - because the entry is written as
//! it is compressed and there is no buffer left to hand to anything else. The
//! other cases keep the buffered path.

use std::io::{Read, Seek, Write};

use crate::codec::Encoder;
use crate::{ArchivePath, Error, Result};

use super::options::{EntryMeta, WriteOptions};
use super::{PendingEntry, Writer};

/// Entries at least this large are compressed straight into the sink.
///
/// Below it the buffered path is preferable: it can hand the entry to the
/// other entries being compressed alongside it, which is worth more than the
/// memory a small entry occupies.
pub(crate) const STREAMING_THRESHOLD: u64 = 64 * 1024 * 1024;

/// How much is read from the source at a time.
const READ_CHUNK: usize = 256 * 1024;

/// A writer that counts what passes through it.
///
/// The packed size of an entry is not known until it has been written, and
/// asking the sink for its position does not work for every sink: a multi
/// volume writer's position spans files.
struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W: Write> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Fills as much of `buffer` as the source will give before it runs dry.
///
/// A `Read` may return less than was asked for without being at the end, and a
/// short read here would mean a needless round through the encoder.
fn read_some(source: &mut dyn Read, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = source.read(&mut buffer[filled..]).map_err(Error::Io)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

/// Returns whether an entry of this size can be compressed straight to the sink.
///
/// Three things decide it: the entry is large enough to be worth it, the codec
/// has an encoder that writes through rather than handing back a buffer (`Copy`,
/// LZMA and LZMA2 do; Deflate, BZip2, PPMd, Zstd, LZ4 and Brotli do not), and
/// nothing needs the compressed bytes in hand - a filter ahead of the codec,
/// encryption behind it, or a solid block around it.
pub(crate) fn can_stream(options: &WriteOptions, size: u64) -> bool {
    if size < STREAMING_THRESHOLD {
        return false;
    }
    if options.solid.is_solid() || options.filter.is_active() {
        return false;
    }
    #[cfg(feature = "aes")]
    if options.is_data_encrypted() {
        return false;
    }

    encoder_is_available(options)
}

/// Returns whether the configured method has a streaming encoder here.
///
/// The codecs that do not are the ones whose wrappers hand back a buffer
/// rather than writing through; they keep the buffered path, which is correct
/// for them and merely uses more memory.
fn encoder_is_available(options: &WriteOptions) -> bool {
    use crate::codec::CodecMethod;

    match options.method {
        CodecMethod::Copy => true,
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => true,
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => true,
        _ => false,
    }
}

/// Passes bytes through unchanged, for the `Copy` method.
///
/// A stored entry has no encoder of its own, and reading a multi-gigabyte file
/// into memory only to write it back out unchanged is the worst case of what
/// this module exists to avoid.
struct StoreEncoder<W> {
    inner: W,
}

impl<W: Write + Send> Write for StoreEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write + Send> Encoder for StoreEncoder<W> {
    fn method_id(&self) -> &'static [u8] {
        crate::codec::method::COPY
    }

    fn finish(mut self: Box<Self>) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Builds the encoder for the configured method, writing into `output`.
///
/// Returns the encoder and the coder properties describing it, which are what
/// a reader needs and must come from the settings actually used.
fn encoder_for<'a, W: Write + Send + 'a>(
    options: &WriteOptions,
    output: W,
    #[cfg_attr(not(feature = "lzma"), allow(unused_variables))] size: u64,
) -> Result<(Box<dyn Encoder + 'a>, Vec<u8>)> {
    use crate::codec::CodecMethod;

    match options.method {
        CodecMethod::Copy => Ok((Box::new(StoreEncoder { inner: output }), Vec::new())),
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => {
            use crate::codec::lzma::{Lzma2Encoder, Lzma2EncoderOptions};

            let opts = Lzma2EncoderOptions {
                preset: options.level,
                dict_size: Some(super::codecs::dictionary_size(options, size as usize)),
            };
            let properties = opts.properties();
            Ok((Box::new(Lzma2Encoder::new(output, &opts)), properties))
        }
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => {
            use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

            let opts = LzmaEncoderOptions {
                preset: options.level,
                dict_size: Some(super::codecs::dictionary_size(options, size as usize)),
            };
            let properties = opts.properties();
            Ok((Box::new(LzmaEncoder::new(output, &opts)?), properties))
        }
        method => Err(Error::UnsupportedMethod {
            method_id: method.method_id(),
        }),
    }
}

impl<W: Write + Seek + Send> Writer<W> {
    /// Compresses an entry into the sink as it is read.
    ///
    /// `size` is what the entry is expected to hold; it selects the dictionary,
    /// and the actual number of bytes read is what gets recorded.
    pub(crate) fn compress_entry_streaming(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
        size: u64,
    ) -> Result<()> {
        // Entries still waiting in the batch were added first and have to reach
        // the sink before this one does.
        self.flush_buffered_entries()?;

        let mut buffer = vec![0u8; READ_CHUNK];

        // Read before building anything. A source can report a size and then
        // yield nothing, and an empty entry carries no stream at all: creating
        // the encoder first would write its framing into the sink and then
        // leave those bytes belonging to no folder, which moves every folder
        // after it.
        let first = read_some(source, &mut buffer)?;
        if first == 0 {
            self.entries.push(PendingEntry {
                path: archive_path,
                meta,
                uncompressed_size: 0,
            });
            return Ok(());
        }

        let mut crc = crc32fast::Hasher::new();
        let mut uncompressed_size = 0u64;

        // From here on the encoder writes into the sink as it goes, so any
        // failure leaves bytes behind that no folder accounts for. Every exit
        // below poisons the writer rather than returning a plain error and
        // letting the caller finish an archive that is already broken.
        let outcome = {
            let mut counting = CountingWriter::new(&mut self.sink);
            let (mut encoder, properties) = encoder_for(&self.options, &mut counting, size)?;

            let mut result = Ok(());
            let mut read = first;
            while read > 0 {
                crc.update(&buffer[..read]);
                uncompressed_size += read as u64;
                if let Err(e) = encoder.write_all(&buffer[..read]) {
                    result = Err(Error::Io(e));
                    break;
                }
                match read_some(source, &mut buffer) {
                    Ok(n) => read = n,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }

            // Finish regardless, so the encoder releases the borrow on the
            // sink; its own failure only matters if nothing failed already.
            let finished = encoder.finish().map_err(Error::Io);
            result
                .and(finished)
                .map(|()| (counting.written, properties))
        };

        let (packed_size, properties) = match outcome {
            Ok(values) => values,
            Err(e) => return self.fail(e),
        };

        self.entries.push(PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        });

        self.compressed_bytes += packed_size;
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.coder_methods.push(self.options.method);
        self.stream_info.coder_properties.push(properties);
        self.stream_info.crcs.push(None);
        self.stream_info.substream_sizes.push(uncompressed_size);
        self.stream_info.substream_crcs.push(crc.finalize());
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(None);
        self.stream_info.filter_info.push(None);
        self.stream_info.bcj2_folder_info.push(None);
        self.stream_info.num_unpack_streams_per_folder.push(1);

        Ok(())
    }
}
