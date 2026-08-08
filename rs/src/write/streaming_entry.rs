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
#[cfg(feature = "parallel")]
pub(crate) struct InFlightBatch {
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
fn batch_footprint(batch: &[super::BufferedEntry], options: &WriteOptions, reserved: u64) -> u64 {
    let entries: u64 = batch.iter().map(|entry| entry.data.len() as u64).sum();
    let largest = batch
        .iter()
        .map(|entry| entry.data.len())
        .max()
        .unwrap_or(0);
    let workers = super::entry_compression::workers_within_budget(options, largest, reserved)
        .min(batch.len())
        .max(1) as u64;

    entries.saturating_mul(4).saturating_add(
        workers.saturating_mul(super::codecs::encoder_memory_usage(options, largest)),
    )
}

/// What a holding area is holding, and whether anything more is coming.
#[derive(Default)]
struct Held {
    bytes: Vec<u8>,
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    /// How much may wait before a producer has to stop and let it be drained.
    ///
    /// `None` while the writer's own thread is the producer, and that is not a
    /// missing bound: the writer is also the only thing that drains, so a limit
    /// it could reach inside a single `write_all` would be a deadlock rather
    /// than a limit. A bound appears only when the area is handed to a thread
    /// of its own, which is the case where someone else is draining.
    cap: Option<usize>,
    /// Set once nothing further will be written.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    done: bool,
    /// Set when these bytes are going nowhere, so a producer stops waiting.
    ///
    /// A writer that has given up still has to get its threads back, and a
    /// producer blocked on an area nobody will drain never returns.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    abandoned: bool,
}

/// Where a streamed entry's compressed bytes go before the sink will take them.
///
/// Shared with the encoder rather than borrowed from the writer, so that the
/// writer stays free to write what is ahead of this entry while the encoder is
/// still going - and, once the entry has been read, so that the encoder can be
/// finished on a thread of its own while the writer gets on with the next entry.
#[derive(Clone)]
struct HoldingArea {
    shared: std::sync::Arc<(std::sync::Mutex<Held>, std::sync::Condvar)>,
}

/// A poisoned lock, reported the way every caller here has to handle it.
fn lock_failed() -> Error {
    Error::Io(std::io::Error::other("compression thread failed"))
}

impl HoldingArea {
    fn new() -> Self {
        Self {
            shared: std::sync::Arc::new((
                std::sync::Mutex::new(Held::default()),
                std::sync::Condvar::new(),
            )),
        }
    }

    /// Bounds how much may wait, for an area about to be filled by a thread.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    fn bound_to(&self, cap: usize) {
        let (lock, _) = &*self.shared;
        if let Ok(mut held) = lock.lock() {
            held.cap = Some(cap.max(1));
        }
    }

    /// Says that nothing further will be written, and wakes whoever is draining.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    fn no_more(&self) {
        let (lock, ready) = &*self.shared;
        if let Ok(mut held) = lock.lock() {
            held.done = true;
        }
        ready.notify_all();
    }

    /// Gives up on these bytes and releases anyone waiting to add more.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    fn abandon(&self) {
        let (lock, ready) = &*self.shared;
        if let Ok(mut held) = lock.lock() {
            held.abandoned = true;
            held.bytes = Vec::new();
        }
        ready.notify_all();
    }

    /// How much is waiting. A poisoned lock reads as nothing waiting, which
    /// only means the next write reports the failure rather than this call.
    fn waiting(&self) -> usize {
        let (lock, _) = &*self.shared;
        lock.lock().map_or(0, |held| held.bytes.len())
    }

    /// Swaps everything held so far into `taker`, leaving the area empty.
    ///
    /// A swap rather than a hand-over so that both buffers keep the capacity
    /// they have grown: this runs between every pair of reads, and taking the
    /// vector would leave the encoder to allocate its output afresh each time.
    fn swap_into(&self, taker: &mut Vec<u8>) -> Result<()> {
        let (lock, ready) = &*self.shared;
        let mut held = lock.lock().map_err(|_| lock_failed())?;
        std::mem::swap(&mut held.bytes, taker);
        drop(held);
        ready.notify_all();
        Ok(())
    }

    /// Waits until there is something to take or nothing more is coming.
    ///
    /// Answers whether that was the last of it. `done` is set only after the
    /// producer's final write, so observing it here means what has just been
    /// taken completes the stream.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    fn take_when_ready(&self, taker: &mut Vec<u8>) -> Result<bool> {
        let (lock, ready) = &*self.shared;
        let mut held = lock.lock().map_err(|_| lock_failed())?;
        while held.bytes.is_empty() && !held.done {
            held = ready.wait(held).map_err(|_| lock_failed())?;
        }
        taker.clear();
        std::mem::swap(&mut held.bytes, taker);
        let complete = held.done;
        drop(held);
        ready.notify_all();
        Ok(complete)
    }
}

impl Write for HoldingArea {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (lock, ready) = &*self.shared;
        #[cfg_attr(not(all(feature = "parallel", feature = "lzma2")), allow(unused_mut))]
        let mut held = lock
            .lock()
            .map_err(|_| std::io::Error::other("compression thread failed"))?;
        // Only a thread of its own can be made to wait, and only such a thread
        // is ever given a cap. Where the writer is the one filling this, it is
        // also the only thing that drains it, so waiting here would be a
        // deadlock rather than a limit.
        #[cfg(all(feature = "parallel", feature = "lzma2"))]
        {
            while !held.abandoned
                && held
                    .cap
                    .is_some_and(|cap| held.bytes.len() >= cap && !held.bytes.is_empty())
            {
                held = ready
                    .wait(held)
                    .map_err(|_| std::io::Error::other("compression thread failed"))?;
            }
            // Bytes nobody will write are dropped rather than queued: the
            // producer is being wound down and only needs to be able to finish.
            if held.abandoned {
                drop(held);
                ready.notify_all();
                return Ok(buf.len());
            }
        }
        held.bytes.extend_from_slice(buf);
        drop(held);
        ready.notify_all();
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

/// One entry written straight through, and what recording it needs.
///
/// Gathered rather than pushed field by field at the end of the write, because
/// the order folders reach the header has to follow the order the entries were
/// accepted, and an entry whose bytes are held back has to carry this until its
/// turn comes. Today nothing is held back past the end of the entry, and this
/// is recorded at once.
pub(crate) struct StreamedFolder {
    path: ArchivePath,
    meta: EntryMeta,
    uncompressed_size: u64,
    crc: u32,
    packed_size: u64,
    properties: Vec<u8>,
    /// Carried rather than read from the options, which may have changed since
    /// this entry was accepted.
    method: crate::codec::CodecMethod,
}

/// Everything a streamed entry's folder needs except how long its bytes turned
/// out to be, which is settled only once the encoder has been finished.
struct FolderSoFar {
    path: ArchivePath,
    meta: EntryMeta,
    uncompressed_size: u64,
    crc: u32,
    properties: Vec<u8>,
    method: crate::codec::CodecMethod,
}

impl FolderSoFar {
    fn packed(self, packed_size: u64) -> StreamedFolder {
        StreamedFolder {
            path: self.path,
            meta: self.meta,
            uncompressed_size: self.uncompressed_size,
            crc: self.crc,
            packed_size,
            properties: self.properties,
            method: self.method,
        }
    }
}

/// An entry that has been read to the end and is still being compressed.
///
/// What is left of it is the blocks in flight when the last byte arrived, which
/// are the largest ones a stream cuts and the ones with nothing behind them to
/// keep the machine busy. Finishing them on a thread of its own is what lets
/// the entry after this one start against that idle time instead of after it.
#[cfg(all(feature = "parallel", feature = "lzma2"))]
pub(crate) struct StreamedTail {
    worker: TailWorker,
    produced: std::sync::Arc<std::sync::atomic::AtomicU64>,
    folder: FolderSoFar,
    called_off: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// What its blocks in flight were costing when it was handed over.
    ///
    /// Read rather than assumed: an encoder allowed the whole budget is not one
    /// using it, and charging what it was allowed would leave nothing for the
    /// entry meant to run alongside. It only falls from here - the stream has
    /// been fully written, so nothing further is dispatched - which is what
    /// makes a figure taken once a bound for the tail's whole life.
    held: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// The thread finishing an entry, and the area it is writing into.
///
/// Kept apart from what the entry is so that winding the thread down is what
/// dropping this does, and so the rest of the tail can be moved out from under
/// it once it has been waited for.
#[cfg(all(feature = "parallel", feature = "lzma2"))]
struct TailWorker {
    /// Taken once joined, so that dropping a worker already waited for does not
    /// wait again.
    handle: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    holding: HoldingArea,
}

#[cfg(all(feature = "parallel", feature = "lzma2"))]
impl TailWorker {
    /// Waits for the thread and reports what finishing the entry did.
    ///
    /// A panic is turned into an error rather than resumed: it reaches the
    /// caller as a writer that cannot be finished, which is what it is.
    fn join(&mut self) -> std::io::Result<()> {
        match self.handle.take() {
            None => Ok(()),
            Some(handle) => handle
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("finishing an entry panicked"))),
        }
    }
}

#[cfg(all(feature = "parallel", feature = "lzma2"))]
impl Drop for TailWorker {
    /// Waits for the thread, first telling it to stop queueing output.
    ///
    /// A writer dropped without being finished still has to get its threads
    /// back, and one blocked on a holding area nobody is going to drain would
    /// never return.
    fn drop(&mut self) {
        self.holding.abandon();
        drop(self.join());
    }
}

/// Work already accepted whose bytes have to reach the sink before anything
/// written after it.
///
/// One at a time. Two would need the memory of both held at once against a
/// budget sized for one, and what a second buys is the tail of a tail.
#[cfg(feature = "parallel")]
pub(crate) enum Ahead {
    /// A batch of small entries being compressed on a thread of its own.
    Batch(InFlightBatch),
    /// A streamed entry whose last blocks are still being compressed.
    ///
    /// Only the chunked LZMA2 encoder keeps blocks in flight, so this is the
    /// one build where an entry has a tail to leave behind.
    #[cfg(feature = "lzma2")]
    Tail(StreamedTail),
}

/// An entry read to the end, with its encoder not yet finished.
struct Pumped {
    encoder: Box<dyn Encoder>,
    produced: std::sync::Arc<std::sync::atomic::AtomicU64>,
    properties: Vec<u8>,
    /// What the encoder still holds, where it holds anything. `None` means
    /// finishing it is arithmetic rather than work, and there is nothing to
    /// gain by moving it off this thread.
    held: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
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

/// What an encoder may not spend, in two parts.
///
/// The fixed part is settled when the encoder is built. The live part is read
/// again whenever the encoder works out its window, because what it stands for
/// gives its memory back as it finishes: an entry compressed alongside the tail
/// of the one before it starts with most of the budget spoken for and should
/// widen as that tail drains, rather than running for the whole of its length
/// in the window it had in its first second.
struct Reservation {
    fixed: u64,
    live: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

impl Reservation {
    /// Nothing spoken for, which is what a build with nothing to run ahead has.
    fn none() -> Self {
        Self {
            fixed: 0,
            live: None,
        }
    }
}

/// What an entry is compressed under, beyond the data itself.
///
/// Gathered into one place because they travel together and are decided
/// together: which branch an entry takes settles all three at once.
struct PumpSetup<'a> {
    options: &'a WriteOptions,
    /// Memory already spoken for by work running alongside.
    reserved: Reservation,
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
        if let Some(watcher) = self.watcher.as_ref() {
            if let Ok(mut held) = watcher.reporter.lock() {
                called_off = !held.on_progress(written, watcher.declared);
            }
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

/// What an entry's own buffering costs while it is being compressed.
///
/// Its output is held rather than written straight through, so that the encoder
/// owns what it writes into and can be handed to a thread when the entry has
/// been read. Two buffers survive a pour - the area the encoder fills and the
/// one it is swapped with, so that neither has to be allocated again - and each
/// is capped at half a share.
fn own_buffering(options: &WriteOptions) -> u64 {
    overlap_share(options) as u64
}

/// How much a streamed entry may still be holding when it is left to a thread.
///
/// A quarter of the budget. Whatever it keeps, the entry behind it does not
/// get, and an entry left with nothing compresses one block at a time - which
/// is far slower than not overlapping at all. A quarter leaves that entry the
/// half it needs to fill the machine while keeping enough of this one in flight
/// to be worth handing over.
#[cfg(all(feature = "parallel", feature = "lzma2"))]
fn tail_share(options: &WriteOptions) -> u64 {
    options.memory_limit.bytes() / 2
}

/// Whether the budget can afford to have two encoders alive at once.
///
/// Handing a tail over means the entry behind it starts while that one is still
/// finishing, and each of them holds an encoder whatever else it is doing: a
/// match finder several times the dictionary, which is the floor of what
/// compressing costs at all. Where the share a tail is allowed does not cover
/// even that, the overlap would take the writer's smallest footprint from one
/// encoder to two - measured, from 258 MB to 364 MB on a 16 MiB budget. A
/// caller who sets a budget that small means it, and the overlap is not worth
/// having on a machine that cannot afford the second encoder anyway.
#[cfg(all(feature = "parallel", feature = "lzma2"))]
fn can_afford_a_tail(options: &WriteOptions) -> bool {
    let encoder = crate::codec::lzma::encoder_memory_usage(
        options.level,
        super::codecs::stream_dictionary_size(options),
    );
    tail_share(options) >= encoder
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
/// What building an encoder produced: the encoder, what describes its output,
/// and - where it has any - what its blocks in flight are costing.
struct BuiltEncoder {
    encoder: Box<dyn Encoder>,
    properties: Vec<u8>,
    /// Present only for an encoder that keeps work in flight, which is the only
    /// kind whose `finish` is worth putting on a thread of its own. `None` is
    /// therefore also the answer to "is there anything to overlap here".
    held: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

fn encoder_for<W: Write + Send + 'static>(
    options: &WriteOptions,
    output: W,
    reserved: Reservation,
) -> Result<BuiltEncoder> {
    use crate::codec::CodecMethod;

    // Destructured rather than read through, so that the live half counts as
    // used in a build where nothing consults it. Only the chunked LZMA2
    // encoder has blocks in flight for either half to come off.
    let Reservation { fixed, live } = reserved;
    #[cfg(not(all(feature = "parallel", feature = "lzma2")))]
    let _ = (fixed, live);

    let plain = |encoder: Box<dyn Encoder>, properties: Vec<u8>| BuiltEncoder {
        encoder,
        properties,
        held: None,
    };

    match options.method {
        CodecMethod::Copy => Ok(plain(Box::new(StoreEncoder { inner: output }), Vec::new())),
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
                    options.memory_limit.bytes().saturating_sub(fixed),
                    live,
                )?;
                let held = encoder.held();
                return Ok(BuiltEncoder {
                    encoder: Box::new(encoder),
                    properties,
                    held: Some(held),
                });
            }

            Ok(plain(
                Box::new(Lzma2Encoder::new(output, &opts)),
                properties,
            ))
        }
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => {
            use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

            let opts = LzmaEncoderOptions {
                preset: options.level,
                dict_size: Some(super::codecs::stream_dictionary_size(options)),
            };
            let properties = opts.properties();
            Ok(plain(
                Box::new(LzmaEncoder::new(output, &opts)?),
                properties,
            ))
        }
        method => Err(Error::UnsupportedMethod {
            method_id: method.method_id(),
        }),
    }
}

impl<W: Write + Seek> Writer<W> {
    /// Records a folder whose bytes have already reached the sink.
    ///
    /// Everything a header needs about it, in the one order that is allowed:
    /// the position of an entry in the file list is what binds it to its data,
    /// so these go in as the entries were accepted and never as they finished.
    pub(crate) fn record_streamed_folder(&mut self, folder: StreamedFolder) {
        let StreamedFolder {
            path,
            meta,
            uncompressed_size,
            crc,
            packed_size,
            properties,
            method,
        } = folder;

        let pending = PendingEntry {
            path,
            meta,
            uncompressed_size,
        };

        self.compressed_bytes += packed_size;
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.coder_methods.push(method);
        self.stream_info.coder_properties.push(properties);
        self.stream_info.crcs.push(None);
        self.stream_info.substream_sizes.push(uncompressed_size);
        self.stream_info.substream_crcs.push(crc);
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(None);
        self.stream_info.filter_info.push(None);
        self.stream_info.bcj2_folder_info.push(None);
        self.stream_info.num_unpack_streams_per_folder.push(1);

        self.record_entry(pending);
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

    /// What the work already accepted but not yet written is costing.
    ///
    /// Anything starting now spends out of what is left of the budget, not out
    /// of the whole of it: the two run at the same time by design, and a second
    /// encoder given the full budget is how a limit stops being one.
    ///
    /// A tail is charged in two parts. Its blocks in flight are read from the
    /// encoder rather than assumed - an encoder allowed the whole budget is not
    /// one using it - and handed on as the live part, so that the entry running
    /// alongside widens again as they are written out. The buffering it is
    /// drained through is fixed and stays charged for as long as it exists.
    #[cfg(feature = "parallel")]
    fn ahead_reservation(&self) -> Reservation {
        match self.ahead.as_ref() {
            None => Reservation::none(),
            // Its entries and the encoders running over them, which it holds
            // until the last of them is compressed.
            Some(Ahead::Batch(batch)) => Reservation {
                fixed: batch.footprint,
                live: None,
            },
            #[cfg(feature = "lzma2")]
            Some(Ahead::Tail(tail)) => Reservation {
                fixed: own_buffering(&self.options),
                live: Some(std::sync::Arc::clone(&tail.held)),
            },
        }
    }

    /// What is spoken for, as one figure, for a caller that cannot re-read it.
    ///
    /// A batch is compressed in one call and cannot widen partway through, so
    /// it is given the live part as it stands when it starts.
    #[cfg(feature = "parallel")]
    pub(crate) fn ahead_reservation_now(&self) -> u64 {
        let reservation = self.ahead_reservation();
        reservation.fixed.saturating_add(
            reservation
                .live
                .map_or(0, |held| held.load(std::sync::atomic::Ordering::Relaxed)),
        )
    }

    /// Whether what is ahead has finished and is only waiting to be written.
    #[cfg(feature = "parallel")]
    fn ahead_is_finished(&self) -> bool {
        match self.ahead.as_ref() {
            None => false,
            Some(Ahead::Batch(batch)) => batch.handle.is_finished(),
            #[cfg(feature = "lzma2")]
            Some(Ahead::Tail(tail)) => tail
                .worker
                .handle
                .as_ref()
                .is_some_and(|handle| handle.is_finished()),
        }
    }

    /// Writes out whatever is ahead and records it, waiting if it is still
    /// running.
    ///
    /// The one way work leaves the pipeline. Everything that writes to the sink
    /// or records an entry goes through here first, because both are ordered
    /// against what was accepted before them and neither can be allowed to
    /// overtake it.
    #[cfg(feature = "parallel")]
    pub(crate) fn settle_ahead(&mut self) -> Result<()> {
        match self.ahead.take() {
            None => Ok(()),
            Some(Ahead::Batch(batch)) => self.settle_batch(batch),
            #[cfg(feature = "lzma2")]
            Some(Ahead::Tail(tail)) => self.settle_tail(tail),
        }
    }

    /// Waits for a batch sent ahead and writes it.
    ///
    /// A thread that panicked is reported as a failure rather than left to
    /// produce an archive missing the entries it was given.
    #[cfg(feature = "parallel")]
    fn settle_batch(&mut self, batch: InFlightBatch) -> Result<()> {
        let InFlightBatch {
            handle, options, ..
        } = batch;
        let outcomes = handle
            .join()
            .map_err(|_| Error::Io(std::io::Error::other("compressing a batch panicked")))??;
        self.write_batch_outcomes(outcomes, &options)
    }

    /// Pours out a streamed entry's remaining bytes and records its folder.
    ///
    /// Draining runs alongside the thread rather than after it: the holding
    /// area is bounded, so a tail whose output nobody took would stop at that
    /// bound and wait. Joining first and draining afterwards is therefore not a
    /// slower version of this - it is a deadlock.
    #[cfg(all(feature = "parallel", feature = "lzma2"))]
    fn settle_tail(&mut self, tail: StreamedTail) -> Result<()> {
        let StreamedTail {
            mut worker,
            produced,
            folder,
            called_off,
            ..
        } = tail;

        let mut scratch = Vec::new();
        loop {
            let complete = match worker.holding.take_when_ready(&mut scratch) {
                Ok(complete) => complete,
                Err(e) => return self.fail(e),
            };
            if !scratch.is_empty() {
                if let Err(e) = self.sink.write_all(&scratch) {
                    return self.fail(Error::Io(e));
                }
            }
            if complete {
                break;
            }
        }

        if let Err(e) = worker.join() {
            // A reporter that called the write off is told that, rather than
            // the write error the refusal had to travel out as.
            let e = if called_off.load(std::sync::atomic::Ordering::Relaxed) {
                Error::Cancelled
            } else {
                Error::Io(e)
            };
            return self.fail(e);
        }

        let packed_size = produced.load(std::sync::atomic::Ordering::Relaxed);
        self.record_streamed_folder(folder.packed(packed_size));
        Ok(())
    }

    /// Gives up on whatever is ahead, for a writer that is already failing.
    ///
    /// Its own failure is nothing to report, and this exists only so the thread
    /// and everything it holds are gone before the call returns.
    #[cfg(feature = "parallel")]
    pub(crate) fn abandon_ahead(&mut self) {
        match self.ahead.take() {
            None => {}
            Some(Ahead::Batch(batch)) => drop(batch.handle.join()),
            // Dropped rather than joined here: its own `Drop` releases the
            // thread from the holding area first, which a plain join would not.
            #[cfg(feature = "lzma2")]
            Some(Ahead::Tail(tail)) => drop(tail),
        }
    }

    /// Whether anything accepted earlier is still to reach the sink.
    #[cfg(feature = "parallel")]
    fn something_ahead(&self) -> bool {
        self.ahead.is_some()
    }
}

impl<W: Write + Seek + Send> Writer<W> {
    /// Starts compressing the pending batch on a thread of its own.
    ///
    /// The entries go with it, so the writer is left holding nothing of them:
    /// the batch is written from what comes back, in the order it came in.
    /// Does nothing where there is nothing to send or nothing to gain - a
    /// build without threads compresses the batch on this thread either way,
    /// and starting one for it would only add a hand-off.
    fn send_batch_ahead(&mut self) -> Result<()> {
        #[cfg(not(feature = "parallel"))]
        return Ok(());

        #[cfg(feature = "parallel")]
        {
            if self.pending_batch.is_empty() {
                return Ok(());
            }
            // Something is already outstanding, and only one thing may be:
            // this batch has to be written after it and would need its own
            // memory held alongside. Left where it is, it is compressed in
            // order - which still runs against whatever is ahead, since a
            // batch is compressed before any of it is written.
            if self.ahead.is_some() {
                return Ok(());
            }
            // A solid block waiting behind the batch would have to be written
            // between the batch and this entry, and cannot be while the batch
            // is still being compressed. It is rare enough - the options must
            // have changed from solid to non-solid mid-archive - to be worth
            // no more than falling back to compressing in order.
            if !self.solid_buffer.is_empty() {
                return Ok(());
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
                return Ok(());
            }
            let batch = std::mem::take(&mut self.pending_batch);
            self.pending_batch_size = 0;
            // What the batch occupies is spent alongside the entry behind it
            // rather than before it, so it has to come out of the same budget -
            // and what it takes, the entry with the cores to use it does not
            // get. A batch too large for the overlap to be paid for is
            // compressed in order, as it was before: it is large enough to be
            // busying the cores on its own, which is what the overlap was for.
            let footprint = batch_footprint(&batch, &options, 0);
            if footprint > overlap_share(&options) as u64 {
                self.pending_batch = batch;
                self.pending_batch_size =
                    self.pending_batch.iter().map(|e| e.data.len() as u64).sum();
                return Ok(());
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
                .spawn(move || {
                    super::entry_compression::compress_batch_owned(batch, &for_thread, 0)
                }) {
                Ok(handle) => handle,
                // The entries went into the closure and go with it, so there is
                // nothing to put back and no way to write them. Failing the
                // writer is what stops that from becoming an archive quietly
                // missing everything the batch held: the caller is told, and a
                // caller that ignores being told cannot finish anyway.
                Err(e) => return self.fail(Error::Io(e)),
            };

            self.ahead = Some(Ahead::Batch(InFlightBatch {
                handle,
                options,
                footprint,
            }));
            Ok(())
        }
    }

    /// Reads the entry through the encoder, running `between` after each read.
    ///
    /// The encoder is built over a holding area rather than over the sink, so
    /// that it owns everything it writes into and can be finished somewhere
    /// other than here. `between` is what moves those bytes on: it writes out
    /// whatever is ahead of this entry, and pours the entry's own output once
    /// nothing is.
    ///
    /// It is handed what the encoder has produced so far rather than what has
    /// been read. Reading runs ahead of compressing by the whole window - on a
    /// default level that can be most of an entry - so a caller told about
    /// reads would watch a bar reach the end in a second and then wait out the
    /// rest in silence.
    ///
    /// The encoder comes back unfinished. Finishing it is where a chunked
    /// stream compresses its last and largest blocks, with nothing behind them
    /// to fill the machine, and the caller is the one that decides whether that
    /// happens here or on a thread of its own.
    fn pump(
        source: &mut dyn Read,
        prefix: Vec<u8>,
        holding: HoldingArea,
        setup: PumpSetup<'_>,
        state: &mut StreamedEntry,
        mut between: impl FnMut(u64) -> Result<()>,
    ) -> Result<Pumped> {
        let PumpSetup {
            options,
            reserved,
            watcher,
        } = setup;
        let mut buffer = vec![0u8; READ_CHUNK];
        let counting = CountingWriter::new(holding).watched(watcher);
        let produced = std::sync::Arc::clone(&counting.written);
        let BuiltEncoder {
            mut encoder,
            properties,
            held,
        } = encoder_for(options, counting, reserved)?;

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

        // Dropped rather than finished when something has already gone wrong.
        // Finishing would compress and emit the rest of a stream nobody is
        // going to write, which is the whole tail of an entry spent on output
        // that is discarded; dropping gives the threads back just as surely.
        if let Err(e) = result {
            drop(encoder);
            return Err(e);
        }

        Ok(Pumped {
            encoder,
            produced,
            properties,
            held,
        })
    }

    /// Compresses an entry into the sink as it is read.
    ///
    /// `prefix` is what has already been read from `source` in order to decide
    /// that the entry belongs on this path, and is compressed ahead of the
    /// rest. The number of bytes that actually arrive is what gets recorded.
    ///
    /// The call can return with the entry still being compressed. What is left
    /// of it by then is the blocks that were in flight when the last byte
    /// arrived - the largest a stream cuts, and the ones with nothing behind
    /// them - so finishing them here would be the machine idling at the end of
    /// every entry. They are left to a thread instead, and the entry after this
    /// one is compressed against them.
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
        self.send_batch_ahead()?;
        // Anything left waiting is dealt with before this entry starts. It
        // writes as it goes, so it settles whatever is ahead of it on the way -
        // and if it fails, that has to be wound down rather than left running
        // against a writer that has given up.
        if let Err(e) = self.flush_buffered_entries() {
            self.abandon_ahead();
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
        // What is already spoken for: whatever is ahead, and this entry's own
        // holding area. Read before the encoder is built, because it is what
        // the encoder may not have.
        let mut reserved = self.ahead_reservation();
        reserved.fixed = reserved.fixed.saturating_add(own_buffering(&options));

        // What the entry says it holds, which is what a caller has to measure
        // against. It is a claim rather than a fact - the archive is built from
        // what arrives, not from this - so it bounds the fraction reported and
        // nothing else.
        let declared = meta.size;
        // This is the path where telling the caller matters most: one entry
        // here can be the whole archive, and the call that accepts it returns
        // only when it has been read.
        self.announce_entries(vec![(archive_path.as_str().to_string(), declared)]);
        // Shared with the counter behind the encoder, which reports each block
        // as it is produced. The writer keeps its own handle: what is ahead of
        // this entry is written while the encoder runs, and those entries have
        // to be reported as they go in.
        let shared = self.progress.clone();
        // Raised by the counter behind the encoder when the reporter refuses to
        // let the write go on, and read once the encoder has been finished.
        let called_off = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = shared.as_ref().map(|reporter| Watcher {
            reporter: std::sync::Arc::clone(reporter),
            declared,
            called_off: std::sync::Arc::clone(&called_off),
        });

        // From here on the encoder's output is bytes no folder accounts for
        // until this entry is recorded, so any failure leaves the archive
        // unfinishable. Every exit below poisons the writer rather than
        // returning a plain error and letting the caller finish it anyway.
        //
        // The encoder writes into a holding area whether or not anything is
        // ahead. Pouring it after every read keeps that no dearer than writing
        // to the sink, and it is what lets the encoder be handed to a thread at
        // the end: one that had borrowed the sink could not go anywhere.
        let outcome = Self::pump(
            source,
            prefix,
            holding.clone(),
            PumpSetup {
                options: &options,
                reserved,
                watcher,
            },
            &mut state,
            |_| {
                if self.something_ahead()
                    && time_to_collect(self.ahead_is_finished(), holding.waiting(), held_limit)
                {
                    self.settle_ahead()?;
                }
                if !self.something_ahead() {
                    self.pour(&holding, &mut scratch)?;
                }
                Ok(())
            },
        );

        // The counter behind the encoder keeps its own handle for as long as
        // the encoder lives, so this one is only the writer's.
        drop(shared);

        let pumped = match outcome {
            Ok(pumped) => pumped,
            Err(e) => {
                // Whatever is ahead is waited for even though nothing more will
                // be written: it holds every entry it was given, and leaving it
                // to run on after the writer has given up would keep that
                // memory until the process ended.
                self.abandon_ahead();
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

        // Anything still ahead is written now, and then everything this entry
        // has produced so far: the tail spawned below appends to the same area,
        // so leaving bytes in it would put them after the ones the tail adds.
        let settled = self
            .settle_ahead()
            .and_then(|()| self.pour(&holding, &mut scratch));
        if let Err(e) = settled {
            self.abandon_ahead();
            return self.fail(e);
        }

        let StreamedEntry {
            crc,
            uncompressed_size,
        } = state;
        let folder = FolderSoFar {
            path: archive_path,
            meta,
            uncompressed_size,
            crc: crc.finalize(),
            properties: pumped.properties,
            method: options.method,
        };

        // `mut` only where there is an encoder to drain a block at a time,
        // which is only where one keeps blocks in flight.
        #[cfg_attr(not(all(feature = "parallel", feature = "lzma2")), allow(unused_mut))]
        let Pumped {
            mut encoder,
            produced,
            held,
            ..
        } = pumped;

        // An encoder holding nothing has nothing to finish: closing it is
        // arithmetic and a terminating byte, and a thread for that would cost
        // more than it saves.
        let Some(held) = held else {
            return self.finish_entry_here(encoder, &produced, folder, &holding, &mut scratch);
        };

        // Only the chunked LZMA2 encoder ever holds anything, so in a build
        // without it the branch above is the only one there is. This exists so
        // that build compiles rather than because it can be reached.
        #[cfg(not(all(feature = "parallel", feature = "lzma2")))]
        {
            let _ = (held, called_off, &options, held_limit);
            return self.finish_entry_here(encoder, &produced, folder, &holding, &mut scratch);
        }

        #[cfg(all(feature = "parallel", feature = "lzma2"))]
        {
            // Nothing is handed over where the budget cannot hold two encoders
            // at once: finishing here keeps the writer's footprint what it was.
            if !can_afford_a_tail(&options) {
                return self.finish_entry_here(encoder, &produced, folder, &holding, &mut scratch);
            }

            // A stream is handed to the encoder far faster than it is compressed,
            // so at the end of one what is in flight is not a tail at all: it is
            // most of the entry, and handing that to a thread would leave the entry
            // behind it a budget already spent - one block at a time, slower than
            // not overlapping in the first place. Brought down here, on this
            // thread, to what the next entry can afford to leave spoken for. What
            // stays is the last blocks, which are the largest a stream cuts and the
            // ones with nothing behind them to keep the machine busy.
            //
            // Poured between blocks rather than after them all: the area is still
            // unbounded at this point, and collecting a backlog into it without
            // draining would hold every one of those blocks' output at once.
            let target = tail_share(&options);
            while held.load(std::sync::atomic::Ordering::Relaxed) > target {
                match encoder.drain_one_block() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => return self.fail(Error::Io(e)),
                }
                if let Err(e) = self.pour(&holding, &mut scratch) {
                    return self.fail(e);
                }
            }

            // Bounded from here on, because the writer is no longer the thing
            // filling it: the thread below is, and without a bound it would queue
            // the whole of what is left while the next entry was being read.
            holding.bound_to(held_limit);
            let for_thread = holding.clone();
            let handle = std::thread::Builder::new()
                .name("zesven-tail".into())
                .spawn(move || {
                    // Whatever happens, the writer has to be told nothing more is
                    // coming, or it waits for bytes that will never arrive. A guard
                    // rather than a call at the end, so a panic says it too.
                    let _closing = ClosingArea(for_thread);
                    encoder.finish()
                });

            match handle {
                Ok(handle) => {
                    self.ahead = Some(Ahead::Tail(StreamedTail {
                        worker: TailWorker {
                            handle: Some(handle),
                            holding,
                        },
                        produced,
                        folder,
                        called_off,
                        held,
                    }));
                    Ok(())
                }
                // The encoder went into the closure and is gone with it, so the
                // rest of this entry can never be written. The writer is finished
                // rather than left to produce an archive whose last folder stops
                // partway through its stream.
                Err(e) => self.fail(Error::Io(e)),
            }
        }
    }

    /// Finishes the encoder on this thread and records what it produced.
    ///
    /// For an encoder with nothing in flight, which is every one but the
    /// chunked LZMA2 encoder: there is no work to move off this thread, only a
    /// terminator to write.
    fn finish_entry_here(
        &mut self,
        encoder: Box<dyn Encoder>,
        produced: &std::sync::atomic::AtomicU64,
        folder: FolderSoFar,
        holding: &HoldingArea,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        let finished = encoder.finish().map_err(Error::Io);
        let settled = finished.and_then(|()| self.pour(holding, scratch));
        if let Err(e) = settled {
            return self.fail(e);
        }
        let packed_size = produced.load(std::sync::atomic::Ordering::Relaxed);
        self.record_streamed_folder(folder.packed(packed_size));
        Ok(())
    }
}

/// Says that nothing more is coming, however the thread ended.
#[cfg(all(feature = "parallel", feature = "lzma2"))]
struct ClosingArea(HoldingArea);

#[cfg(all(feature = "parallel", feature = "lzma2"))]
impl Drop for ClosingArea {
    fn drop(&mut self) {
        self.0.no_more();
    }
}

#[cfg(test)]
mod tests {
    use super::time_to_collect;

    /// What an entry buffers for itself covers both buffers it needs.
    ///
    /// Its output is held rather than written straight through, and two buffers
    /// survive a pour: the area the encoder fills and the one it is swapped
    /// with. Each is capped at half a share, so a share covers the pair, and
    /// charging less would hand the encoder a window it cannot have - which is
    /// how a budget stops bounding anything.
    #[test]
    fn test_an_entry_is_charged_for_both_buffers_it_pours_through() {
        use super::{overlap_share, own_buffering};
        use crate::write::options::WriteOptions;

        let options = WriteOptions::new();
        let share = overlap_share(&options) as u64;

        assert_eq!(super::holding_cap(&options) as u64 * 2, share);
        assert_eq!(own_buffering(&options), share);
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

        let grown = batch_footprint(&full, &options, 0) - batch_footprint(&lean, &options, 0);
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

/// Without threads nothing is ever compressed ahead of the entry being
/// written, so there is nothing to settle, nothing to wind down and nothing
/// spoken for. The names exist so that the paths that order themselves against
/// the pipeline read the same in every build.
#[cfg(not(feature = "parallel"))]
impl<W: Write + Seek> Writer<W> {
    pub(crate) fn ahead_reservation_now(&self) -> u64 {
        0
    }

    fn ahead_reservation(&self) -> Reservation {
        Reservation::none()
    }

    fn something_ahead(&self) -> bool {
        false
    }

    fn ahead_is_finished(&self) -> bool {
        false
    }

    pub(crate) fn settle_ahead(&mut self) -> Result<()> {
        Ok(())
    }

    pub(crate) fn abandon_ahead(&mut self) {}
}
