//! Compressing one LZMA2 stream across cores, as it is written.
//!
//! Writing a single large file otherwise runs at the speed of one core however
//! many the machine has, because one stream goes through one encoder. Here the
//! input is cut into fixed-length blocks, each is encoded on its own thread,
//! and the results are concatenated in input order.
//!
//! What makes the blocks independent of each other is not a dictionary reset
//! but a preset dictionary: a block is handed the input immediately before it,
//! so it starts with the window an unbroken stream would have had at that
//! point. It therefore asks the decoder to reset nothing, and the finished
//! stream carries exactly one reset, at its start, as any LZMA2 stream does.
//! A decoder needs to know nothing about any of this.
//!
//! Three properties are load-bearing.
//!
//! **The bytes depend on the input and nothing else.** Where a boundary falls
//! is a function of how many bytes have gone through the encoder, so the same
//! input at the same level produces the same archive whether one worker ran or
//! twenty, on a laptop or a server, from a file whose length was known or a
//! pipe whose length was not. A caller resolving to a single thread changes
//! the output, by asking for a single unbroken stream; nothing else does.
//!
//! **Memory is bounded by the window, not by the input.** Only so many blocks
//! are in flight, and how many follows from the budget the caller gave and the
//! size of the blocks being cut - which is what makes this usable for the
//! write-through path: a 10 GB file costs the same as a 100 MB one. The
//! multi-threaded writer
//! in `lzma-rust2` dispatches without bound and so queues the whole input,
//! which is the one thing the write-through path exists to avoid.
//!
//! **Compression is not traded away for it.** Each block is encoded with the
//! data before it as a preset dictionary, so it finds the matches an unbroken
//! stream would. That costs about as much again as encoding the block, and is
//! paid on every stream rather than only where it looks worthwhile, because
//! what a block can match against is not visible from the block.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::mpsc::{Receiver, sync_channel};

use super::lzma::{Lzma2Encoder, Lzma2EncoderOptions};
use super::{Encoder, method};

/// How many blocks may be in flight per worker.
///
/// One apiece would leave a worker idle from the moment it finishes until the
/// main thread collects that block, which it only does when the block at the
/// head of the queue is ready. A second block per worker covers that gap.
pub(crate) const BLOCKS_PER_WORKER: usize = 2;

/// How many blocks are cut at one size before the size doubles.
///
/// From measurement rather than taste. Growing every four blocks took a 200 MB
/// entry from twenty-five blocks to eleven, and with fewer pieces than the
/// machine has cores it ran at half the parallelism and took half again as
/// long: 25.5s against 17.2s on twenty cores. Twelve, sixteen and eight are
/// within the noise of each other on both a 200 MB entry and an 800 MB one, so
/// this is the middle of a flat region rather than a peak.
const BLOCKS_PER_STEP: u64 = 12;

/// The largest a block gets, in dictionaries.
///
/// Four, so the window a block is handed costs a quarter of what encoding the
/// block itself costs rather than all of it again.
const MAX_DICTIONARIES: u64 = 4;

/// How many dictionaries the block at `index` holds.
///
/// A block is encoded with the dictionary before it, which costs the encoder
/// about as much again as the block itself. Spreading that over a larger block
/// is what makes it cheap, so a long stream wants large blocks - but cutting a
/// stream barely past the threshold that coarsely would leave two blocks and
/// most of the machine idle.
///
/// So blocks start at one dictionary and double every [`BLOCKS_PER_STEP`]
/// until they reach [`MAX_DICTIONARIES`]: the beginning of a stream is cut
/// finely enough to fill the cores, and everything after it coarsely enough not
/// to pay for the window over and over.
///
/// The index is the only input. Not the machine, not how long the caller said
/// the stream would be, not how long it turns out to be - a stream and its own
/// first half are cut identically for as far as they agree.
fn dictionaries_at(index: u64) -> u64 {
    // 1, 2, 4: shifting by more would overflow on a long stream, and four is
    // where the growth stops anyway.
    const LAST_STEP: u64 = MAX_DICTIONARIES.trailing_zeros() as u64;
    1 << (index / BLOCKS_PER_STEP).min(LAST_STEP)
}

/// Returns how long a stream has to be before it is cut at all.
///
/// Below this the encoder produces one block, which is byte for byte what an
/// unsplit stream is - so a caller who knows the length can skip building a
/// thread pool for it without changing anything it would have written.
///
/// It is exactly the first block's dispatch threshold, and has to stay that
/// way: a caller comparing with `>` where the encoder cuts at `>=` disagrees
/// about a stream of precisely this length, and the two write different
/// archives for it. That is not hypothetical - at level 9 it was an 80 MiB
/// entry, which is an ordinary size for a thing to be.
pub(crate) fn shortest_split_stream(dictionary: u64) -> u64 {
    dictionary.saturating_add(dictionary / 4)
}

/// Data a worker refuses to compress, so a test can watch a real one panic.
#[cfg(test)]
const PANIC_SENTINEL: &[u8] = b"zesven: panic in this block";

/// Compresses `data` into one LZMA2 block, without the stream terminator.
///
/// `context` is the data immediately before this block, which the encoder is
/// given as a preset dictionary. It is what makes splitting safe: a block that
/// starts with an empty dictionary cannot match anything before it, so every
/// match reaching back past a boundary is lost. With the preceding window in
/// hand it matches across the boundary as an unbroken stream would, while
/// still being encoded without waiting for the block before it.
///
/// It is carried always, and deliberately not only where it looks worthwhile.
/// Indexing the window costs the encoder as much as the block itself, so it is
/// tempting to skip it for data that appears to have no matches - but whether
/// matches exist cannot be told from a sample. Random bytes repeated at a
/// period just under the block size look incompressible by every local
/// measure and compress sixteenfold, entirely through matches that reach
/// past a block's start; skipping the window there produced an archive
/// fourteen times larger than one thread produces.
///
/// The decoder needs nothing special. A block encoded with a preset dictionary
/// does not ask for a dictionary reset, so a reader working through the stream
/// in order already holds exactly those bytes.
///
/// `flush` rather than `finish`: the terminating zero byte belongs to the
/// stream as a whole and is written once, after the last block. Encoding each
/// block with its own terminator and then trimming the trailing byte back off
/// is how the previous attempt at this did it, and it left the correctness of
/// the format resting on a `pop()`.
fn compress_block(
    data: &[u8],
    context: Option<Vec<u8>>,
    options: &Lzma2EncoderOptions,
) -> io::Result<Vec<u8>> {
    // A panic here is the thing `dispatch` catches, and a test that arranges
    // one anywhere else proves nothing about that. This is how a test reaches
    // into a worker: the sentinel cannot occur by accident and does not exist
    // outside a test build.
    #[cfg(test)]
    if data.starts_with(PANIC_SENTINEL) {
        panic!("worker asked to blow up");
    }

    let mut out = Vec::new();
    let mut encoder = Lzma2Encoder::with_preset_dict(&mut out, options, context);
    encoder.write_all(data)?;
    encoder.flush()?;
    Ok(out)
}

/// Writes an LZMA2 stream whose blocks are compressed on several threads.
///
/// Blocks reach the sink in the order they were taken from the input, because
/// the queue is drained from the front: a block that finishes early waits for
/// the ones before it rather than being reordered afterwards.
pub(crate) struct ChunkedLzma2Encoder<W: Write> {
    sink: W,
    options: Lzma2EncoderOptions,
    /// How many blocks have been handed to a worker.
    ///
    /// The block size is a function of this, so it is also the position in the
    /// stream: it is what makes the boundaries depend on the input alone.
    emitted: u64,
    /// The block being filled, plus whatever has arrived beyond it.
    staging: Vec<u8>,
    /// Blocks handed to workers, oldest first, each with what it is costing.
    ///
    /// The cost travels with the block rather than being recomputed when it is
    /// collected: the block size grows as the stream goes on, so a figure
    /// worked out at collection time would subtract more than was ever added.
    in_flight: VecDeque<(Receiver<io::Result<Vec<u8>>>, u64)>,
    /// What the blocks in flight are costing at this moment.
    ///
    /// Published rather than kept private because a caller that overlaps this
    /// encoder with other work has to budget around what it holds, and the
    /// alternative - assuming it holds everything it was allowed - is the whole
    /// budget, which leaves nothing for anything to overlap with. Shared so it
    /// can be read while the encoder is being finished on another thread.
    held: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The most blocks that may ever be in flight.
    ///
    /// Two per worker, so that a worker which finishes early has something to
    /// start on while the main thread is still collecting the block in front.
    /// More than that is queueing, not parallelism.
    window_ceiling: usize,
    /// What the blocks in flight may occupy between them.
    ///
    /// Counted in bytes rather than in blocks because a block is not a fixed
    /// size: early ones are a quarter of what later ones are, and a window
    /// fixed in blocks would either hold a quarter of the memory it was
    /// allowed at the start of a stream, or four times it at the end.
    budget: u64,
    /// What something else is holding, read afresh whenever the window is
    /// worked out.
    ///
    /// A stream compressed alongside the tail of the one before it starts with
    /// most of the budget spoken for, and gets it back as that tail drains.
    /// Taken as a fixed figure instead, the stream would run for the whole of
    /// its length in the window it had in its first second.
    spoken_for: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    pool: rayon::ThreadPool,
    /// How much of each block is kept as the next one's window.
    ///
    /// The dictionary, which is as far back as a match can reach. Resolved
    /// once at construction rather than read from the options per block: the
    /// options carry it as an `Option`, and a `None` there would silently mean
    /// no window at all - every block starting cold, and the compression this
    /// encoder exists to preserve quietly gone.
    window_bytes: usize,
    /// The tail of the previous block, given to the next one as its window.
    ///
    /// Empty before the first block, which has nothing behind it.
    context: Vec<u8>,
}

impl<W: Write> ChunkedLzma2Encoder<W> {
    /// Creates an encoder writing into `sink`.
    ///
    /// `workers` is how many blocks may be compressed at once and `budget` what
    /// the blocks in flight may occupy; both change only how long the stream
    /// takes to write, never what is written. `options` must state its
    /// dictionary: that is what each block is given of the one before it and
    /// what the block size is measured in, and without it the split would cost
    /// the compression it is meant to keep.
    pub(crate) fn new(
        sink: W,
        options: &Lzma2EncoderOptions,
        workers: usize,
        budget: u64,
        spoken_for: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    ) -> crate::Result<Self> {
        let window_bytes = options
            .dict_size
            .filter(|size| *size > 0)
            .and_then(|size| usize::try_from(size).ok())
            .ok_or_else(|| {
                crate::Error::InvalidFormat(
                    "a split LZMA2 stream needs a stated dictionary size".into(),
                )
            })?;
        let workers = workers.max(1);

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .map_err(|e| crate::Error::Io(io::Error::other(e)))?;

        Ok(Self {
            sink,
            options: options.clone(),
            emitted: 0,
            staging: Vec::new(),
            in_flight: VecDeque::with_capacity(workers * BLOCKS_PER_WORKER),
            held: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            window_ceiling: workers * BLOCKS_PER_WORKER,
            budget,
            spoken_for,
            pool,
            window_bytes,
            context: Vec::new(),
        })
    }

    /// A handle on what this encoder's blocks in flight are costing.
    ///
    /// Read by whoever is budgeting around this encoder. It only ever falls
    /// once the stream has been fully written, so a figure taken then bounds
    /// what the encoder holds for the rest of its life.
    pub(crate) fn held(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        std::sync::Arc::clone(&self.held)
    }

    /// What one block of `len` bytes costs while it is in flight.
    ///
    /// The encoder compressing it, the block itself, the compressed block it
    /// produces - which for incompressible data is about the size of the block
    /// again - and the copy of the window it was handed, which is a dictionary.
    ///
    /// An encoder apiece overstates a queue longer than the worker count, since
    /// a block waiting for a worker has no encoder yet. Counting them against
    /// the workers instead is the truer figure and was measured to be worth
    /// nothing: on the mixed corpus it moved the wall time by less than the run
    /// to run spread, because what bounds these streams is how many blocks they
    /// are cut into and not what the blocks cost.
    fn block_cost(&self, len: usize) -> u64 {
        crate::codec::lzma::encoder_memory_usage(
            self.options.preset,
            self.options.dict_size.unwrap_or(0),
        )
        .saturating_add(2 * len as u64)
        .saturating_add(self.window_bytes as u64)
    }

    /// The length of the block currently being filled.
    fn block_size(&self) -> usize {
        // The dictionary fits in a `usize` - it was read from one - so this
        // product only saturates on a target where it could not be held anyway.
        let block = dictionaries_at(self.emitted).saturating_mul(self.window_bytes as u64);
        usize::try_from(block).unwrap_or(usize::MAX)
    }

    /// How much has to have arrived before a block is handed to a worker.
    ///
    /// A block is dispatched only once there is enough behind it to be worth a
    /// block of its own. Otherwise a stream one byte past a boundary is cut
    /// into a full block and a one-byte block, and that second block costs an
    /// entire encoder - a match finder twelve times the dictionary - to
    /// compress one byte: at level 9 that was a doubling of peak memory to do
    /// nothing at all. The remainder stays on the last block instead, which is
    /// therefore between one and one and a quarter blocks long.
    fn dispatch_threshold(&self) -> usize {
        let block = self.block_size();
        block.saturating_add(block / 4)
    }

    /// How many blocks of the current size the budget allows in flight.
    ///
    /// A worker holds its encoder, the block it was handed, and the compressed
    /// block it is producing, which for incompressible data is about the size
    /// of the block again. Dividing the budget by that is what keeps the whole
    /// window inside it as blocks grow, rather than reserving up front for the
    /// largest block a stream might reach and running that few workers from the
    /// start - which on a 200 MB entry was half the parallelism for the whole
    /// of it.
    fn window(&self) -> usize {
        let per_block = self.block_cost(self.block_size());
        let available = self.budget.saturating_sub(
            self.spoken_for
                .as_ref()
                .map_or(0, |held| held.load(std::sync::atomic::Ordering::Relaxed)),
        );

        let affordable = available
            .checked_div(per_block)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(self.window_ceiling);
        affordable.clamp(1, self.window_ceiling)
    }

    /// Hands the staged block to a worker, waiting first if the window is full.
    ///
    /// Waiting here is the whole point: the source is a file being read at
    /// gigabytes a second and the workers compress at megabytes a second, so
    /// without a bound the input would be queued in its entirety.
    fn dispatch(&mut self, len: usize) -> io::Result<()> {
        let len = len.min(self.staging.len());
        if len == 0 {
            return Ok(());
        }

        let window = self.window();
        while self.in_flight.len() >= window {
            self.collect_one()?;
        }

        // What stays behind is the beginning of the next block, so it moves
        // rather than being copied; the block itself is handed over whole.
        let rest = self.staging.split_off(len);
        let block = std::mem::replace(&mut self.staging, rest);
        self.emitted += 1;
        let options = self.options.clone();
        let (tx, rx) = sync_channel(1);

        // The window this block sees, and then the window the next one will:
        // both are the last dictionary's worth of input, which is as far back
        // as a match can reach. Taken here rather than in the worker so that
        // it follows the input order rather than the order blocks finish in.
        let context = (!self.context.is_empty()).then(|| std::mem::take(&mut self.context));
        let keep = self.window_bytes.min(block.len());
        self.context = block[block.len() - keep..].to_vec();
        // From the block as it actually is, not from the size the schedule
        // calls for: the last block of a stream is between one and one and a
        // quarter of that, and the remainder is real memory like the rest.
        let cost = self.block_cost(block.len());

        self.pool.spawn(move || {
            // Caught here rather than left to unwind. A rayon pool built
            // without a panic handler aborts the process when a task panics,
            // so a bug in the encoder - or a debug assertion in it - would
            // take the caller's program down rather than returning the error
            // that `collect_one` is written to receive.
            let compressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compress_block(&block, context, &options)
            }))
            .unwrap_or_else(|_| {
                Err(io::Error::other(
                    "LZMA2 worker panicked compressing a block",
                ))
            });

            // A send failure means the encoder was dropped without collecting,
            // which is not this worker's problem to report.
            let _ = tx.send(compressed);
        });

        self.held
            .fetch_add(cost, std::sync::atomic::Ordering::Relaxed);
        self.in_flight.push_back((rx, cost));
        Ok(())
    }

    /// Waits for the oldest block in flight and writes it out.
    fn collect_one(&mut self) -> io::Result<()> {
        let Some((rx, cost)) = self.in_flight.pop_front() else {
            return Ok(());
        };
        self.held
            .fetch_sub(cost, std::sync::atomic::Ordering::Relaxed);

        // A worker that neither sent nor is alive is gone; its block will
        // never arrive, and continuing would write a stream missing a block in
        // the middle.
        let block = rx.recv().map_err(|_| {
            io::Error::other("LZMA2 worker thread stopped before finishing a block")
        })?;
        self.sink.write_all(&block?)
    }

    /// Finishes the stream and returns the sink.
    pub(crate) fn finish(mut self) -> io::Result<W> {
        // Everything still staged is the last block, however long it is: there
        // is nothing behind it to justify cutting it further.
        self.dispatch(self.staging.len())?;
        while !self.in_flight.is_empty() {
            self.collect_one()?;
        }

        // The terminator closes the stream. An input that produced no blocks
        // at all leaves an empty stream, which is a terminator and nothing
        // else, and is what a reader expects of a zero-length entry.
        self.sink.write_all(&[0x00])?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

impl<W: Write> Write for ChunkedLzma2Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut rest = buf;
        while !rest.is_empty() {
            // Taken a block at a time rather than all at once: a caller handing
            // over a whole file in one call would otherwise stage the file,
            // which is the memory this encoder exists not to use.
            let threshold = self.dispatch_threshold();
            let take = (threshold - self.staging.len()).min(rest.len());
            self.staging.extend_from_slice(&rest[..take]);
            rest = &rest[take..];

            if self.staging.len() >= threshold {
                self.dispatch(self.block_size())?;
            }
        }
        Ok(buf.len())
    }

    /// Writes out every block already finished, and nothing else.
    ///
    /// The staged block is deliberately left alone: dispatching a short block
    /// here would put a boundary where the block size does not call for one,
    /// and the output would then depend on when the caller happened to flush.
    fn flush(&mut self) -> io::Result<()> {
        while let Some((rx, cost)) = self.in_flight.front() {
            let cost = *cost;
            match rx.try_recv() {
                Ok(block) => {
                    self.in_flight.pop_front();
                    self.held
                        .fetch_sub(cost, std::sync::atomic::Ordering::Relaxed);
                    self.sink.write_all(&block?)?;
                }
                Err(_) => break,
            }
        }
        self.sink.flush()
    }
}

impl<W: Write + Send> Encoder for ChunkedLzma2Encoder<W> {
    fn method_id(&self) -> &'static [u8] {
        method::LZMA2
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        (*self).finish().map(|_| ())
    }

    fn drain_one_block(&mut self) -> io::Result<bool> {
        if self.in_flight.is_empty() {
            return Ok(false);
        }
        self.collect_one()?;
        Ok(true)
    }
}

/// Returns where the boundaries fall in a stream of `size` bytes.
///
/// The encoder does not need this - it cuts as it goes - but a caller reasoning
/// about memory or a test reasoning about boundaries does, and deriving it
/// twice is how the two drift apart.
#[cfg(test)]
fn boundaries_for(size: u64, dictionary: u64) -> Vec<u64> {
    let mut boundaries = Vec::new();
    let mut staged = size;
    let mut index = 0;
    loop {
        let block = dictionaries_at(index) * dictionary;
        if staged < block + block / 4 {
            break;
        }
        boundaries.push(block);
        staged -= block;
        index += 1;
    }
    if staged > 0 {
        boundaries.push(staged);
    }
    boundaries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::lzma::Lzma2Decoder;
    use std::io::Read;

    /// Large enough never to bind: these tests are about the bytes, and the
    /// budget only decides how many blocks are in flight at a time.
    const BUDGET: u64 = 1 << 40;

    fn options() -> Lzma2EncoderOptions {
        Lzma2EncoderOptions::with_preset(1).with_dict_size(1 << 16)
    }

    fn decode(compressed: &[u8], dict_size: u32) -> Vec<u8> {
        let properties = [crate::codec::lzma::encode_lzma2_dict_size(dict_size)];
        let mut decoder = Lzma2Decoder::new(compressed, &properties).expect("builds");
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).expect("decodes");
        out
    }

    /// Data with structure to find, which is what carries a window forward.
    fn compressible(len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut n = 0u64;
        while out.len() < len {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            out.extend_from_slice(
                format!("record {n}: status=active payload=abcdefghijklmnopqrstuvwxyz\n")
                    .as_bytes(),
            );
        }
        out.truncate(len);
        out
    }

    fn data(len: usize) -> Vec<u8> {
        // Neither random nor a repeated byte: the first defeats the matcher
        // entirely and the second is matched by anything, and both hide
        // mistakes that ordinary data would show.
        (0..len)
            .map(|i| {
                let x = (i as u64).wrapping_mul(2_654_435_761);
                (x >> ((i % 8) * 8)) as u8
            })
            .collect()
    }

    /// Data with nothing to find in it, as already-compressed data has.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        while out.len() < len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.extend_from_slice(&state.to_le_bytes());
        }
        out.truncate(len);
        out
    }

    fn roundtrip(len: usize, workers: usize) {
        let input = data(len);
        let mut out = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut out, &options(), workers, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        assert_eq!(decode(&out, 1 << 16), input, "len={len} workers={workers}");
    }

    #[test]
    fn test_roundtrip_across_block_counts() {
        for len in [0, 1, 1000, (1 << 16) - 1, 1 << 16, (1 << 16) + 1, 500_000] {
            roundtrip(len, 4);
        }
    }

    #[test]
    fn test_roundtrip_with_one_worker() {
        roundtrip(500_000, 1);
    }

    /// Data that skips the window still has to round trip.
    #[test]
    fn test_incompressible_data_round_trips() {
        let input = incompressible(500_000);
        let mut out = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut out, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");
        assert_eq!(decode(&out, 1 << 16), input);
    }

    /// The output must not depend on how many workers produced it.
    ///
    /// This is what lets the writer pick a worker count from the machine
    /// without the archive changing from one machine to the next.
    #[test]
    fn test_bytes_do_not_depend_on_worker_count() {
        let input = data(400_000);

        let encode = |workers: usize| {
            let mut out = Vec::new();
            let mut encoder = ChunkedLzma2Encoder::new(&mut out, &options(), workers, BUDGET, None)
                .expect("builds");
            encoder.write_all(&input).expect("writes");
            encoder.finish().expect("finishes");
            out
        };

        let one = encode(1);
        for workers in [2, 3, 8, 16] {
            assert_eq!(encode(workers), one, "worker count changed the output");
        }
    }

    /// Writing in small pieces must produce what writing at once produces.
    ///
    /// A boundary that moved with the caller's buffer size would make the
    /// archive depend on how the entry happened to be read.
    #[test]
    fn test_bytes_do_not_depend_on_write_size() {
        let input = data(300_000);

        let mut whole = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut whole, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        for piece in [1, 7, 4096, 100_000] {
            let mut split = Vec::new();
            let mut encoder =
                ChunkedLzma2Encoder::new(&mut split, &options(), 4, BUDGET, None).expect("builds");
            for part in input.chunks(piece) {
                encoder.write_all(part).expect("writes");
                encoder.flush().expect("flushes");
            }
            encoder.finish().expect("finishes");
            assert_eq!(split, whole, "write size {piece} changed the output");
        }
    }

    /// A single block must be byte-identical to the ordinary encoder.
    ///
    /// Below two blocks there is nothing to parallelise, and the writer only
    /// reaches for this encoder above that; if the two disagreed on one block,
    /// the threshold would be visible in the archive.
    #[test]
    fn test_one_block_matches_the_ordinary_encoder() {
        let input = data(40_000);

        let mut chunked = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut chunked, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        let mut plain = Vec::new();
        let mut ordinary = Lzma2Encoder::new(&mut plain, &options());
        ordinary.write_all(&input).expect("writes");
        ordinary.try_finish().expect("finishes");

        assert_eq!(chunked, plain);
    }

    /// Data that compresses must round trip too, and must do so identically
    /// however many workers ran.
    ///
    /// It takes the other path through the encoder: incompressible data is
    /// encoded block by block with nothing carried between them, while this
    /// hands each block the window before it. Both have to hold.
    #[test]
    fn test_compressible_data_round_trips_and_is_stable() {
        let input = compressible(400_000);

        let encode = |workers: usize| {
            let mut out = Vec::new();
            let mut encoder = ChunkedLzma2Encoder::new(&mut out, &options(), workers, BUDGET, None)
                .expect("builds");
            encoder.write_all(&input).expect("writes");
            encoder.finish().expect("finishes");
            out
        };

        let one = encode(1);
        assert_eq!(decode(&one, 1 << 16), input);
        for workers in [2, 4, 8] {
            assert_eq!(encode(workers), one, "worker count changed the output");
        }
    }

    /// Carrying the window has to be what makes compressible data smaller.
    ///
    /// Without it a block cannot match past its own start, and on text that is
    /// worth about a tenth of the archive. If this stops holding, the window is
    /// being carried and paid for without buying anything.
    #[test]
    fn test_carrying_the_window_pays_for_itself() {
        let input = compressible(600_000);

        let mut chunked = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut chunked, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        // The same split, with every block starting from an empty window.
        let mut isolated = Vec::new();
        for block in input.chunks(1 << 16) {
            isolated.extend_from_slice(&compress_block(block, None, &options()).expect("encodes"));
        }
        isolated.push(0x00);

        assert!(
            chunked.len() < isolated.len(),
            "carried window produced {} bytes against {} without it",
            chunked.len(),
            isolated.len(),
        );
        assert_eq!(decode(&chunked, 1 << 16), input);
    }

    /// Data whose matches are far apart must not lose them to the split.
    ///
    /// This is the case that rules out deciding whether to carry the window
    /// from a sample of the data. Random bytes repeated at a period longer
    /// than a block look incompressible by every local measure - every byte
    /// value is equally likely, and a block on its own holds barely one
    /// period - and yet they compress enormously, entirely through matches
    /// that reach back past a block's start. An encoder that dropped the
    /// window here produced an archive fourteen times larger.
    #[test]
    fn test_matches_reaching_past_a_block_survive() {
        // Random-looking, repeating at just under a block. A match then lies
        // further back than the start of the block it is found in, but still
        // within the dictionary, so only the window makes it reachable.
        let period: Vec<u8> = (0..(1 << 16) - 4096)
            .map(|i| {
                let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                x ^= x >> 29;
                x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                (x >> 32) as u8
            })
            .collect();
        let input: Vec<u8> = period
            .iter()
            .cycle()
            .take(period.len() * 8)
            .copied()
            .collect();

        let mut out = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut out, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        assert_eq!(decode(&out, 1 << 16), input);

        // Seven of the eight repeats have to be found. Without the window each
        // block sees at most one period and the archive stays about as large
        // as the input.
        assert!(
            out.len() < input.len() / 4,
            "{} bytes from {} of eightfold-repeated data: the split is losing \
             matches that reach past a block",
            out.len(),
            input.len(),
        );
    }

    /// A worker that panics has to surface as an error, not take the process.
    ///
    /// rayon aborts the process when a task panics and no panic handler is
    /// installed, so without catching it here a bug in the encoder - or a
    /// debug assertion inside it - would kill the caller's program outright
    /// rather than failing the archive.
    #[test]
    fn test_a_panicking_worker_becomes_an_error() {
        // Through the encoder itself, not a stand-in: what has to hold is that
        // the panic is caught where `dispatch` puts a worker, and a pool built
        // in the test would be a different pool with different handlers.
        let mut input = PANIC_SENTINEL.to_vec();
        input.extend_from_slice(&data(200_000));

        let mut out = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut out, &options(), 2, BUDGET, None).expect("builds");
        let error = encoder
            .write_all(&input)
            .and_then(|()| encoder.finish().map(|_| ()))
            .expect_err("a panicking worker has to fail the stream");

        assert!(
            error.to_string().contains("panicked"),
            "a panic has to arrive as an error saying so, got {error}"
        );
    }

    /// The blocks have to fall where the schedule says they do.
    ///
    /// Compared against the same input cut by [`boundaries_for`] and encoded
    /// block by block: if the encoder ever cut somewhere else - on a flush, on
    /// a write boundary, on a tail it should have kept - these disagree.
    #[test]
    fn test_blocks_fall_where_the_schedule_says() {
        let input = compressible(900_000);

        let mut chunked = Vec::new();
        let mut encoder =
            ChunkedLzma2Encoder::new(&mut chunked, &options(), 4, BUDGET, None).expect("builds");
        encoder.write_all(&input).expect("writes");
        encoder.finish().expect("finishes");

        let mut expected = Vec::new();
        let mut start = 0usize;
        for length in boundaries_for(input.len() as u64, 1 << 16) {
            let end = start + length as usize;
            let context = (start > 0).then(|| {
                let from = start.saturating_sub(1 << 16);
                input[from..start].to_vec()
            });
            expected.extend_from_slice(
                &compress_block(&input[start..end], context, &options()).expect("encodes"),
            );
            start = end;
        }
        expected.push(0x00);
        assert_eq!(start, input.len(), "the schedule has to cover the input");

        assert_eq!(chunked, expected, "the encoder cut somewhere unscheduled");
    }

    /// A stream barely past a boundary must not grow a block for the remainder.
    ///
    /// That block costs an encoder, a match finder twelve times the dictionary,
    /// and at level 9 doing that for one byte doubled the peak memory of the
    /// whole write.
    #[test]
    fn test_a_short_tail_stays_on_the_block_before_it() {
        let dictionary = 1u64 << 16;
        assert_eq!(
            boundaries_for(dictionary + 1, dictionary),
            vec![dictionary + 1]
        );
        assert_eq!(
            boundaries_for(dictionary * 2, dictionary),
            vec![dictionary, dictionary]
        );

        // Nothing shorter than a quarter of a block is ever cut off on its own.
        for size in [1u64, 100, dictionary - 1, dictionary * 3 + 7, 1_000_000] {
            let blocks = boundaries_for(size, dictionary);
            assert_eq!(blocks.iter().sum::<u64>(), size, "size {size}");
            if let Some(last) = blocks.last() {
                assert!(*last >= dictionary / 4 || blocks.len() == 1, "size {size}");
            }
        }
    }

    /// Blocks grow as the stream goes on, and stop growing at four dictionaries.
    #[test]
    fn test_blocks_grow_and_then_stop() {
        assert_eq!(dictionaries_at(0), 1);
        assert_eq!(dictionaries_at(BLOCKS_PER_STEP - 1), 1);
        assert_eq!(dictionaries_at(BLOCKS_PER_STEP), 2);
        assert_eq!(dictionaries_at(BLOCKS_PER_STEP * 2), 4);
        assert_eq!(dictionaries_at(BLOCKS_PER_STEP * 3), MAX_DICTIONARIES);
        assert_eq!(dictionaries_at(u64::MAX / 2), MAX_DICTIONARIES);
    }

    /// A stream and the start of a longer one have to be cut identically.
    ///
    /// This is what makes the boundaries independent of the length: the encoder
    /// never knows how much is still to come, so knowing it must not matter.
    #[test]
    fn test_a_prefix_is_cut_like_the_stream_it_starts() {
        let long = boundaries_for(5_000_000, 1 << 16);
        let short = boundaries_for(1_000_000, 1 << 16);

        let shared = short.len() - 1;
        assert_eq!(&long[..shared], &short[..shared]);
    }
}
