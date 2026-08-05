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

/// A batch being compressed while the writer streams the entry after it.
///
/// The batch came first and must reach the sink first, so nothing it produces
/// can be written until it is collected. What it buys is the other cores: a
/// batch whose largest entry is the only one left compressing holds a single
/// core, and the entry behind it can have the rest of the machine meanwhile.
struct InFlightBatch {
    handle: std::thread::JoinHandle<Result<Vec<super::entry_compression::BatchOutcome>>>,
    /// The options the batch was accepted under, which are what writing it
    /// needs; the current ones may already be something else.
    options: WriteOptions,
    /// What it occupies while it runs, so the entry compressed alongside it can
    /// be given the rest and no more.
    footprint: u64,
}

/// What a batch about to be compressed will occupy: its entries, and the
/// encoders that will run over them.
///
/// Measured from the batch in hand rather than taken as the ceiling a batch is
/// allowed to reach. Where the pipeline is worth the most - a batch of two or
/// three entries that cannot fill the machine on its own - the two figures are
/// a long way apart, and charging the ceiling would take the window away from
/// the entry that has the cores to use it.
///
/// The data counts four times over, which is the worst the moment of growing
/// that output can be. The input is one: it is released as soon as its output
/// exists, but not before, and a batch small enough to be worth overlapping is
/// one where every entry is at that point together. The output is another, an
/// entry's output being at worst about the size of its input - data that does
/// not compress is what this has to be right for.
///
/// The other two are the vector it is collected into. It grows as the encoder
/// writes, so where an output just outgrows the input it came from, the vector
/// holding it has as much again reserved past what it holds; and a growth that
/// has to move rather than extend has the old one and the new one live together
/// while it copies. Measured, a batch spends about three and a half times its
/// data, so three would sit on the line rather than above it.
///
/// Reserving the input's size up front instead would be the right guess only
/// for data that does not compress, and would hold a hundred megabytes for text
/// that compresses to twenty.
#[cfg(feature = "parallel")]
fn batch_footprint(batch: &[super::BufferedEntry], options: &WriteOptions) -> u64 {
    let entries: u64 = batch.iter().map(|entry| entry.data.len() as u64).sum();
    let largest = batch
        .iter()
        .map(|entry| entry.data.len())
        .max()
        .unwrap_or(0);
    let workers = super::entry_compression::workers_within_budget(options, largest)
        .min(batch.len())
        .max(1) as u64;

    entries.saturating_mul(4).saturating_add(
        workers.saturating_mul(super::codecs::encoder_memory_usage(options, largest)),
    )
}

/// Where a streamed entry's compressed bytes go before the sink will take them.
///
/// Shared with the encoder rather than borrowed from the writer, so that the
/// writer stays free to collect the batch ahead of this entry and write it
/// while the encoder is still going.
#[derive(Clone)]
struct HoldingArea {
    held: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl HoldingArea {
    fn new() -> Self {
        Self {
            held: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// How much is waiting. A poisoned lock reads as nothing waiting, which
    /// only means the next write reports the failure rather than this call.
    fn waiting(&self) -> usize {
        self.held.lock().map_or(0, |held| held.len())
    }

    /// Swaps everything held so far into `taker`, leaving the area empty.
    ///
    /// A swap rather than a hand-over so that both buffers keep the capacity
    /// they have grown: this runs between every pair of reads, and taking the
    /// vector would leave the encoder to allocate its output afresh each time.
    fn swap_into(&self, taker: &mut Vec<u8>) -> Result<()> {
        let mut held = self
            .held
            .lock()
            .map_err(|_| Error::Io(std::io::Error::other("compression thread failed")))?;
        std::mem::swap(&mut *held, taker);
        Ok(())
    }
}

impl Write for HoldingArea {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut held = self
            .held
            .lock()
            .map_err(|_| std::io::Error::other("compression thread failed"))?;
        held.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether the batch running alongside this entry should be collected now.
///
/// Two reasons, and they are not the same one. A batch that has finished is
/// collected because there is nothing to wait for and holding its output any
/// longer only costs memory. A holding area at its cap is collected because
/// there is no more room to hold anything: that one blocks until the batch is
/// done, and it is what stops a large entry being buffered whole behind a batch
/// that turns out to be slower than it.
fn time_to_collect(batch_finished: bool, waiting: usize, cap: usize) -> bool {
    batch_finished || waiting >= cap
}

/// What reading an entry through the encoder works out as it goes.
struct StreamedEntry {
    crc: crc32fast::Hasher,
    uncompressed_size: u64,
}

/// A writer that counts what passes through it.
///
/// The packed size of an entry is not known until it has been written, and
/// asking the sink for its position does not work for every sink: a multi
/// volume writer's position spans files.
struct CountingWriter<W> {
    inner: W,
    /// Shared so the count can be read while the encoder holds the writer:
    /// what has been produced is what a caller watching wants to be told, and
    /// the encoder has the writer for the whole of an entry.
    written: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Told each time a block reaches the sink, where one is watching.
    ///
    /// This is the only place that hears about the work as it happens. An
    /// encoder that compresses across cores takes the entry in far faster than
    /// it compresses it and finishes the rest when it is closed, so a caller
    /// told between reads would hear nothing for the length of the entry and
    /// then that it was done.
    watcher: Option<Watcher>,
}

/// What an entry is compressed under, beyond the data itself.
///
/// Gathered into one place because they travel together and are decided
/// together: which branch an entry takes settles all three at once.
struct PumpSetup<'a> {
    options: &'a WriteOptions,
    /// Memory already spoken for by a batch running alongside.
    reserved: u64,
    /// Who to tell about the archive being produced, if anyone.
    watcher: Option<Watcher>,
}

/// Where a streamed entry's progress goes, and what it is measured against.
struct Watcher {
    reporter: std::sync::Arc<std::sync::Mutex<Box<dyn crate::progress::ProgressReporter>>>,
    declared: u64,
    /// Set when the reporter answered `on_progress` with a refusal.
    ///
    /// The refusal has to travel out through the encoder, which knows only
    /// `io::Error`, so it is raised as one and read back here. Without this the
    /// caller would be handed whatever the write failure looked like from the
    /// outside rather than the thing they asked for.
    called_off: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl<W: Write> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            written: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watcher: None,
        }
    }

    fn watched(mut self, watcher: Option<Watcher>) -> Self {
        self.watcher = watcher;
        self
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        let written = self
            .written
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed)
            + n as u64;
        let mut called_off = false;
        if let Some(watcher) = self.watcher.as_ref()
            && let Ok(mut held) = watcher.reporter.lock()
        {
            called_off = !held.on_progress(written, watcher.declared);
        }
        // A refusal ends the entry where it stands. The bytes just written are
        // lost with the rest of what the encoder has produced, which is the
        // point: an entry stopped partway is not one an archive can hold.
        if called_off {
            if let Some(watcher) = self.watcher.as_ref() {
                watcher
                    .called_off
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return Err(std::io::Error::other(
                "the progress reporter called the write off",
            ));
        }
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
pub(super) fn read_some(source: &mut dyn Read, buffer: &mut [u8]) -> Result<usize> {
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

/// Returns whether these options allow compressing straight to the sink.
///
/// Two things decide it: the codec has an encoder that writes through rather
/// than handing back a buffer (`Copy`, LZMA and LZMA2 do; Deflate, BZip2, PPMd,
/// Zstd, LZ4 and Brotli do not), and nothing needs the compressed bytes in
/// hand, which a filter ahead of the codec, encryption behind it, or a solid
/// block around it all do.
///
/// Size is the caller's half of the question, and is deliberately not asked
/// here: it is answered by reading, not by what an entry claims to hold.
pub(crate) fn can_stream(options: &WriteOptions) -> bool {
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

/// What compressing a batch alongside the entry behind it may cost, per side.
///
/// Two sides answer to it: what the batch itself occupies, which is what
/// decides whether it is sent ahead at all, and what the entry's compressed
/// output occupies while it waits for that batch. Both are spent while the
/// entry is being compressed, so both come off the window the entry has to work
/// in - an eighth each, which covers a batch worth overlapping and leaves the
/// entry three quarters of the budget it would otherwise have had to itself.
fn overlap_share(options: &WriteOptions) -> usize {
    usize::try_from(options.memory_limit.bytes() / 8).unwrap_or(usize::MAX)
}

/// What to take off the entry's window while a batch runs alongside it.
///
/// Everything the overlap spends: what the batch occupies, and the share the
/// holding area and the buffer it is swapped with take between them. The batch
/// figure is passed in rather than recomputed because it is measured from the
/// batch that was actually sent, which is the only thing that knows what it
/// holds.
fn overlap_reservation(batch_footprint: u64, options: &WriteOptions) -> u64 {
    batch_footprint.saturating_add(overlap_share(options) as u64)
}

/// How much output may be waiting before the batch has to be collected.
///
/// Half the share rather than all of it, because two buffers of that size
/// survive a pour: what the encoder writes into, and the one it is swapped with
/// so that neither has to be allocated again. Both keep the capacity they grew
/// to, so a limit of the whole share would occupy two of them.
///
/// It is where the writer stops and waits rather than a ceiling the encoder is
/// held to. A single write can carry a compressed block past it, and the amount
/// that can is bounded by what the encoder had in flight - which it gives up in
/// the same moment, since the block it hands over is one it no longer holds.
fn holding_cap(options: &WriteOptions) -> usize {
    overlap_share(options) / 2
}

/// Builds the encoder for the configured method, writing into `output`.
///
/// Returns the encoder and the coder properties describing it, which are what
/// a reader needs and must come from the settings actually used.
///
/// `reserved` is memory already spoken for elsewhere and is taken off what the
/// encoder may use for blocks in flight. It changes how fast an entry is
/// compressed, never how it is cut: which block a byte lands in follows from
/// its position in the stream. Only blocks in flight answer to it, so a build
/// without threads has nothing to take it off.
fn encoder_for<'a, W: Write + Send + 'a>(
    options: &WriteOptions,
    output: W,
    reserved: u64,
) -> Result<(Box<dyn Encoder + 'a>, Vec<u8>)> {
    use crate::codec::CodecMethod;

    // Only the chunked LZMA2 encoder has blocks in flight to take it off.
    #[cfg(not(all(feature = "parallel", feature = "lzma2")))]
    let _ = reserved;

    match options.method {
        CodecMethod::Copy => Ok((Box::new(StoreEncoder { inner: output }), Vec::new())),
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => {
            use crate::codec::lzma::{Lzma2Encoder, Lzma2EncoderOptions};

            let opts = Lzma2EncoderOptions {
                preset: options.level,
                dict_size: Some(super::codecs::stream_dictionary_size(options)),
            };
            let properties = opts.properties();

            // An entry on this path has nothing being compressed alongside it,
            // so one encoder here is one core busy and the rest idle. Cut into
            // blocks it is the whole machine, and the memory that costs is the
            // window rather than the entry: what makes this path usable for a
            // file larger than memory is preserved.
            #[cfg(feature = "parallel")]
            if super::codecs::lzma2_is_chunked(options, &opts) {
                use crate::codec::lzma2_chunked::ChunkedLzma2Encoder;

                let encoder = ChunkedLzma2Encoder::new(
                    output,
                    &opts,
                    options.threads.count(),
                    options.memory_limit.bytes().saturating_sub(reserved),
                )?;
                return Ok((Box::new(encoder), properties));
            }

            Ok((Box::new(Lzma2Encoder::new(output, &opts)), properties))
        }
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => {
            use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

            let opts = LzmaEncoderOptions {
                preset: options.level,
                dict_size: Some(super::codecs::stream_dictionary_size(options)),
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
    /// Starts compressing the pending batch on a thread of its own.
    ///
    /// The entries go with it, so the writer is left holding nothing of them:
    /// the batch is written from what comes back, in the order it came in.
    /// Returns `None` where there is nothing to send or nothing to gain - a
    /// build without threads compresses the batch on this thread either way,
    /// and starting one for it would only add a hand-off.
    fn send_batch_ahead(&mut self) -> Result<Option<InFlightBatch>> {
        #[cfg(not(feature = "parallel"))]
        return Ok(None);

        #[cfg(feature = "parallel")]
        {
            if self.pending_batch.is_empty() {
                return Ok(None);
            }
            // A solid block waiting behind the batch would have to be written
            // between the batch and this entry, and cannot be while the batch
            // is still being compressed. It is rare enough - the options must
            // have changed from solid to non-solid mid-archive - to be worth
            // no more than falling back to compressing in order.
            if !self.solid_buffer.is_empty() {
                return Ok(None);
            }

            let options = self
                .pending_batch
                .first()
                .map(|entry| (*entry.options).clone())
                .unwrap_or_else(|| (*self.active_options).clone());
            // One thread means one thread, and it is the batch's own setting
            // that says so - the same one it will be compressed under. The
            // overlap would not change the archive, since the batch is still
            // compressed in order and a stream written by a single thread is
            // still unbroken, but a caller who asked for one thread is bounding
            // what the writer costs the machine, and a second one compressing a
            // batch is not that.
            if options.threads.count() <= 1 {
                return Ok(None);
            }
            let batch = std::mem::take(&mut self.pending_batch);
            self.pending_batch_size = 0;
            // What the batch occupies is spent alongside the entry behind it
            // rather than before it, so it has to come out of the same budget -
            // and what it takes, the entry with the cores to use it does not
            // get. A batch too large for the overlap to be paid for is
            // compressed in order, as it was before: it is large enough to be
            // busying the cores on its own, which is what the overlap was for.
            let footprint = batch_footprint(&batch, &options);
            if footprint > overlap_share(&options) as u64 {
                self.pending_batch = batch;
                self.pending_batch_size =
                    self.pending_batch.iter().map(|e| e.data.len() as u64).sum();
                return Ok(None);
            }

            // Announced here rather than where a batch compressed in order is,
            // because this one never goes through that path. It is also the
            // longest a batch is ever outstanding - it runs alongside an entry
            // large enough to be written straight through - so a caller left
            // to hear about these entries as they finish would watch the whole
            // of that wait with only the large entry to show for it.
            self.announce_entries(
                batch
                    .iter()
                    .map(|entry| (entry.path.as_str().to_string(), entry.data.len() as u64))
                    .collect(),
            );

            let for_thread = options.clone();
            let handle = match std::thread::Builder::new()
                .name("zesven-batch".into())
                .spawn(move || super::entry_compression::compress_batch_owned(batch, &for_thread))
            {
                Ok(handle) => handle,
                // The entries went into the closure and go with it, so there is
                // nothing to put back and no way to write them. Failing the
                // writer is what stops that from becoming an archive quietly
                // missing everything the batch held: the caller is told, and a
                // caller that ignores being told cannot finish anyway.
                Err(e) => return self.fail(Error::Io(e)),
            };

            Ok(Some(InFlightBatch {
                handle,
                options,
                footprint,
            }))
        }
    }

    /// Waits for a batch sent ahead and writes it, if one is still out.
    ///
    /// A thread that panicked is reported as a failure rather than left to
    /// produce an archive missing the entries it was given.
    fn collect_batch(&mut self, in_flight: &mut Option<InFlightBatch>) -> Result<()> {
        let Some(InFlightBatch {
            handle, options, ..
        }) = in_flight.take()
        else {
            return Ok(());
        };
        let outcomes = handle
            .join()
            .map_err(|_| Error::Io(std::io::Error::other("compressing a batch panicked")))??;
        self.write_batch_outcomes(outcomes, &options)
    }

    /// Waits for a batch whose output is never going to be written.
    ///
    /// Its own failure is nothing to report: the writer is already failing, and
    /// this exists only so the thread and everything it holds are gone before
    /// the call returns.
    fn abandon_batch(&mut self, in_flight: &mut Option<InFlightBatch>) {
        if let Some(InFlightBatch { handle, .. }) = in_flight.take() {
            drop(handle.join());
        }
    }

    /// Reads the entry through the encoder, running `between` after each read.
    ///
    /// The encoder is built over `output`, which is what lets the same loop
    /// serve both an entry written straight to the sink and one held back while
    /// the batch ahead of it is compressed. `between` is where the second of
    /// those collects the batch and pours what has been held; for the first it
    /// does nothing, and the sink can stay borrowed by the encoder for the
    /// whole entry.
    ///
    /// It is handed what the encoder has produced so far rather than what has
    /// been read. Reading runs ahead of compressing by the whole window - on a
    /// default level that can be most of an entry - so a caller told about
    /// reads would watch a bar reach the end in a second and then wait out the
    /// rest in silence.
    fn pump<O: Write + Send>(
        source: &mut dyn Read,
        prefix: Vec<u8>,
        output: O,
        setup: PumpSetup<'_>,
        state: &mut StreamedEntry,
        mut between: impl FnMut(u64) -> Result<()>,
    ) -> Result<(u64, Vec<u8>)> {
        let PumpSetup {
            options,
            reserved,
            watcher,
        } = setup;
        let mut buffer = vec![0u8; READ_CHUNK];
        let mut counting = CountingWriter::new(output).watched(watcher);
        let produced = std::sync::Arc::clone(&counting.written);
        let (mut encoder, properties) = encoder_for(options, &mut counting, reserved)?;

        state.crc.update(&prefix);
        state.uncompressed_size += prefix.len() as u64;
        // In the same sized pieces as the rest of the entry, and not in one
        // go. The prefix is up to the whole write-through threshold, which for
        // a codec that writes its input through is that much output before
        // anything gets a chance to look at what is waiting - so handing it
        // over whole would put 64 MiB past a limit that may be a fraction of
        // it.
        let mut result = Ok(());
        for piece in prefix.chunks(READ_CHUNK) {
            if let Err(e) = encoder.write_all(piece) {
                result = Err(Error::Io(e));
                break;
            }
            if let Err(e) = between(produced.load(std::sync::atomic::Ordering::Relaxed)) {
                result = Err(e);
                break;
            }
        }
        // Handed over rather than held: the encoder has taken what it needs of
        // it into blocks, and keeping the copy would put the threshold on top
        // of the window for the rest of the entry.
        drop(prefix);

        while result.is_ok() {
            let read = match read_some(source, &mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            if read == 0 {
                break;
            }
            state.crc.update(&buffer[..read]);
            state.uncompressed_size += read as u64;
            if let Err(e) = encoder.write_all(&buffer[..read]) {
                result = Err(Error::Io(e));
                break;
            }
            if let Err(e) = between(produced.load(std::sync::atomic::Ordering::Relaxed)) {
                result = Err(e);
                break;
            }
        }

        // Finish regardless, so the encoder gives up its output; its own
        // failure only matters if nothing failed already.
        let finished = encoder.finish().map_err(Error::Io);
        result.and(finished)?;
        Ok((
            counting.written.load(std::sync::atomic::Ordering::Relaxed),
            properties,
        ))
    }

    /// Moves whatever the encoder has produced so far into the sink.
    fn pour(&mut self, holding: &HoldingArea, scratch: &mut Vec<u8>) -> Result<()> {
        scratch.clear();
        holding.swap_into(scratch)?;
        if scratch.is_empty() {
            return Ok(());
        }
        self.sink.write_all(scratch).map_err(Error::Io)
    }

    /// Compresses an entry into the sink as it is read.
    ///
    /// `prefix` is what has already been read from `source` in order to decide
    /// that the entry belongs on this path, and is compressed ahead of the
    /// rest. The number of bytes that actually arrive is what gets recorded.
    pub(crate) fn compress_entry_streaming(
        &mut self,
        archive_path: ArchivePath,
        prefix: Vec<u8>,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        // Entries still waiting were added first and have to reach the sink
        // before this one does. The batch of small entries can be compressed
        // while this one is, as long as it is written first; anything else
        // waiting has to be dealt with here and now.
        let mut in_flight = self.send_batch_ahead()?;
        // Anything left waiting is dealt with before this entry starts. It
        // cannot fail while a batch is out - a batch is only sent ahead when
        // nothing else is waiting - but if that ever stops being true, the
        // batch has to be waited for rather than left running against a writer
        // that has given up.
        if let Err(e) = self.flush_buffered_entries() {
            self.abandon_batch(&mut in_flight);
            return self.fail(e);
        }

        let mut state = StreamedEntry {
            crc: crc32fast::Hasher::new(),
            uncompressed_size: 0,
        };
        let holding = HoldingArea::new();
        let mut scratch = Vec::new();
        let held_limit = holding_cap(&self.options);
        // Taken rather than borrowed: the encoder is built from them and the
        // writer has to stay reachable while it runs.
        let options = self.options.clone();

        // What the entry says it holds, which is what a caller has to measure
        // against. It is a claim rather than a fact - the archive is built from
        // what arrives, not from this - so it bounds the fraction reported and
        // nothing else.
        let declared = meta.size;
        // This is the path where telling the caller matters most: one entry
        // here can be the whole archive, and the call that accepts it returns
        // only when it has been written.
        self.announce_entries(vec![(archive_path.as_str().to_string(), declared)]);
        // Shared with the counter behind the encoder, which reports each block
        // as it reaches the sink. The writer keeps its own handle: the batch
        // sent ahead of this entry is collected and written while the encoder
        // runs, and those entries have to be reported as they go in.
        let shared = self.progress.clone();
        // Raised by the counter behind the encoder when the reporter refuses to
        // let the write go on, and read once the encoder has given the sink up.
        let called_off = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = || {
            shared.as_ref().map(|reporter| Watcher {
                reporter: std::sync::Arc::clone(reporter),
                declared,
                called_off: std::sync::Arc::clone(&called_off),
            })
        };

        // From here on the encoder's output is bytes no folder accounts for
        // until this entry is recorded, so any failure leaves the archive
        // unfinishable. Every exit below poisons the writer rather than
        // returning a plain error and letting the caller finish it anyway.
        let outcome = match in_flight.as_ref() {
            // Nothing ahead of this entry: the encoder writes into the sink for
            // the whole of it, as it did before there was anything to overlap.
            None => Self::pump(
                source,
                prefix,
                &mut self.sink,
                PumpSetup {
                    options: &options,
                    reserved: 0,
                    watcher: watcher(),
                },
                &mut state,
                |_| Ok(()),
            ),
            // A batch is being compressed alongside: what this entry produces
            // is held until that batch has been written, and goes to the sink
            // from then on.
            Some(batch) => {
                let reserved = overlap_reservation(batch.footprint, &options);
                let output = holding.clone();
                Self::pump(
                    source,
                    prefix,
                    output,
                    PumpSetup {
                        options: &options,
                        reserved,
                        watcher: watcher(),
                    },
                    &mut state,
                    |_| {
                        if in_flight.as_ref().is_some_and(|batch| {
                            time_to_collect(
                                batch.handle.is_finished(),
                                holding.waiting(),
                                held_limit,
                            )
                        }) {
                            self.collect_batch(&mut in_flight)?;
                        }
                        if in_flight.is_none() {
                            self.pour(&holding, &mut scratch)?;
                        }
                        Ok(())
                    },
                )
            }
        };

        // The encoder and the counter behind it are both dropped with the
        // outcome above, so the only handle left from here on is the writer's.
        drop(shared);

        let (packed_size, properties) = match outcome {
            Ok(values) => values,
            Err(e) => {
                // The batch is waited for even though nothing will be written:
                // it holds every entry it was given, and leaving it to run on
                // after the writer has given up would keep that memory until
                // the process ended.
                self.abandon_batch(&mut in_flight);
                // A write the caller called off is reported as that rather than
                // as the write error it had to travel out as. The writer is
                // still finished with either way: an entry stopped partway has
                // left bytes in the sink that belong to no folder.
                let e = if called_off.load(std::sync::atomic::Ordering::Relaxed) {
                    Error::Cancelled
                } else {
                    e
                };
                return self.fail(e);
            }
        };

        // Whatever is left: the batch if the holding area never filled, and
        // then the rest of this entry.
        let settled = self
            .collect_batch(&mut in_flight)
            .and_then(|()| self.pour(&holding, &mut scratch));
        if let Err(e) = settled {
            self.abandon_batch(&mut in_flight);
            return self.fail(e);
        }

        let StreamedEntry {
            crc,
            uncompressed_size,
        } = state;

        let pending = PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        };

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

        self.record_entry(pending);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::time_to_collect;

    /// The window taken from an entry covers everything the overlap spends.
    ///
    /// Two things are spent while the entry is compressed and neither is the
    /// entry's: the batch running alongside it, and the output waiting to be
    /// written once the batch has been. Leaving either out gives the encoder a
    /// window it cannot have, which is how a budget stops bounding anything.
    #[test]
    fn test_the_window_taken_covers_both_sides_of_the_overlap() {
        use super::{overlap_reservation, overlap_share};
        use crate::write::options::WriteOptions;

        let options = WriteOptions::new();
        let footprint = 40 << 20;
        let share = overlap_share(&options) as u64;

        // The holding area and the buffer it is swapped with are each capped at
        // half a share, so a share covers the pair.
        assert_eq!(super::holding_cap(&options) as u64 * 2, share);
        assert_eq!(overlap_reservation(footprint, &options), footprint + share);
    }

    /// A batch is charged for its data four times over.
    ///
    /// Twice for existing twice: each entry's input is released as soon as its
    /// output is complete, which stops a finished entry from holding both - but
    /// not the entry being finished, and a batch worth overlapping is small
    /// enough that every one of its entries is being compressed at once. Twice
    /// again for the vector that output is collected into, which reserves as
    /// much again past what it holds and, where a growth has to move rather
    /// than extend, has both the old one and the new one live while it copies.
    /// Each of those left out in turn made the figure the overlap is granted
    /// against one the batch could exceed on its own.
    #[cfg(feature = "parallel")]
    #[test]
    fn test_a_batch_is_charged_for_its_output_as_well_as_its_input() {
        use super::super::BufferedEntry;
        use super::batch_footprint;
        use crate::ArchivePath;
        use crate::write::options::{EntryMeta, WriteOptions};

        let options = std::sync::Arc::new(WriteOptions::new());
        let entry = |name: &str, len: usize| BufferedEntry {
            path: ArchivePath::new(name).expect("path"),
            data: vec![0u8; len],
            meta: EntryMeta::file(len as u64),
            crc: 0,
            options: options.clone(),
        };
        // Two batches with the same largest entry and the same number of
        // entries, so the encoders they are charged for are identical and only
        // the data between them differs.
        let lean = vec![entry("a.bin", 16 << 20), entry("b.bin", 1)];
        let full = vec![entry("a.bin", 16 << 20), entry("b.bin", 8 << 20)];
        let more = ((8 << 20) - 1) as u64;

        let grown = batch_footprint(&full, &options) - batch_footprint(&lean, &options);
        assert_eq!(
            grown,
            more * 4,
            "{more} more bytes in a batch grew what it is charged by {grown}: \
             its output, or the room the vector holding that output reserves \
             past it, is not being charged beside its input",
        );
    }

    /// A finished batch is collected whatever is waiting, and a full holding
    /// area is collected whatever the batch is doing.
    ///
    /// The second is the one that is hard to reach from outside: it needs a
    /// batch slower than the entry behind it produces output, which depends on
    /// what the two are and how many cores each got. Reaching it by timing
    /// would make a test that passes for reasons it cannot state.
    #[test]
    fn test_a_batch_is_collected_when_it_ends_or_when_there_is_no_room() {
        // Nothing to collect for yet: still running, and room to spare.
        assert!(!time_to_collect(false, 0, 64));
        assert!(!time_to_collect(false, 63, 64));

        // Finished, so there is nothing to gain by holding on.
        assert!(time_to_collect(true, 0, 64));

        // Out of room, so there is no choice but to wait for it.
        assert!(time_to_collect(false, 64, 64));
        assert!(time_to_collect(false, 65, 64));
    }
}
