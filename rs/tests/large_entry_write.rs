//! A large entry has nothing to be compressed alongside, so the codec splits
//! it into blocks and compresses those across cores instead. Three things have
//! to hold for that to be worth doing, and each is a property rather than a
//! case:
//!
//! - what comes back out is what went in, at every size around a block edge;
//! - the archive does not depend on how many cores produced it, so the same
//!   input is the same file on every machine;
//! - memory follows the window and not the entry, which is the whole reason
//!   this path exists and the one thing a naive implementation gets wrong.

#![cfg(all(feature = "lzma2", feature = "parallel"))]

use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, Writer};
use zesven::{ArchivePath, Threads};

/// Entries at or above this go through the write-through path.
///
/// Mirrors `write::streaming_entry::STREAMING_THRESHOLD`, which is internal.
/// A test that quietly stopped exercising that path would still pass, so the
/// sizes below are stated relative to this rather than as bare numbers.
const STREAMING_THRESHOLD: usize = 64 * 1024 * 1024;

/// Data that compresses a little, as a video container does.
///
/// Neither incompressible nor repetitive: the first would hide a matcher that
/// silently produced nothing, and the second compresses so far that a block
/// boundary stops being visible in the output at all.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x243F_6A88_85A3_08D3u64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
        if state % 8 == 0 {
            // Occasional structure, so matches exist to be found.
            data.extend_from_slice(b"........................");
        }
    }
    data.truncate(len);
    data
}

fn write_archive(data: &[u8], threads: Threads) -> Vec<u8> {
    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        .threads(threads);
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options);
    writer
        .add_bytes(ArchivePath::new("big.bin").expect("path"), data)
        .expect("adds");
    let (_result, cursor) = writer.finish_into_inner().expect("finishes");
    cursor.into_inner()
}

fn read_back(archive: &[u8]) -> Vec<u8> {
    let mut archive = Archive::open(Cursor::new(archive.to_vec())).expect("opens");
    archive.extract_to_vec("big.bin").expect("extracts")
}

/// Every size around a block edge has to survive the trip.
///
/// Blocks start at one dictionary and grow to four, and a block is handed to a
/// worker only once a quarter of the next one has arrived behind it. At level 1
/// the dictionary is 1 MiB, so the first eight blocks account for 12 MiB and
/// every one after them is 4 MiB: 64 MiB is exactly a boundary, and 69 MiB is
/// exactly where the block after it becomes worth dispatching. Off-by-one in
/// the split shows up at those two sizes and nowhere else.
#[test]
fn test_large_entries_round_trip_around_block_edges() {
    let mib = 1024 * 1024;

    for len in [
        STREAMING_THRESHOLD - 1,
        STREAMING_THRESHOLD,
        STREAMING_THRESHOLD + 1,
        STREAMING_THRESHOLD + 4 * mib,
        STREAMING_THRESHOLD + 5 * mib - 1,
        STREAMING_THRESHOLD + 5 * mib,
        STREAMING_THRESHOLD + 5 * mib + 1,
    ] {
        let data = payload(len);
        let archive = write_archive(&data, Threads::count_or_single(4));
        assert_eq!(read_back(&archive), data, "len={len}");
    }
}

/// What an entry claims to be must not reach the bytes of the archive.
///
/// The length in `EntryMeta` is whatever the caller had to hand: a `stat` taken
/// before the file was finished being written, a guess about a pipe, or nothing
/// at all. It decides which entries are worth reporting progress for and
/// nothing else - if it also decided the dictionary, the block size, or which
/// path the entry took, then the same bytes described differently would produce
/// different archives, and a length declared too small would send an entry far
/// larger than memory down the path that holds it whole.
#[test]
fn test_the_declared_size_does_not_reach_the_archive() {
    let data = payload(STREAMING_THRESHOLD + 6 * 1024 * 1024);

    let write = |declared: u64| {
        let options = WriteOptions::new()
            .level(1)
            .expect("level")
            .threads(Threads::count_or_single(4));
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .expect("writer")
            .options(options);
        writer
            .add_stream(
                ArchivePath::new("big.bin").expect("path"),
                &mut Cursor::new(data.clone()),
                EntryMeta::file(declared),
            )
            .expect("adds");
        let (_result, cursor) = writer.finish_into_inner().expect("finishes");
        cursor.into_inner()
    };

    let truthful = write(data.len() as u64);
    for declared in [0, 1, (STREAMING_THRESHOLD - 1) as u64, 4 << 30] {
        assert_eq!(
            write(declared),
            truthful,
            "a declared size of {declared} changed the archive",
        );
    }
    assert_eq!(read_back(&truthful), data);
}

/// The archive must not depend on how many cores wrote it.
///
/// Block boundaries come from the dictionary alone, so a two-core laptop and a
/// sixty-four-core server have to produce the same file. Without this the
/// worker count would leak into the output and an archive would stop being
/// reproducible off the machine that made it.
#[test]
fn test_output_does_not_depend_on_worker_count() {
    let data = payload(STREAMING_THRESHOLD + 3 * 1024 * 1024);

    let reference = write_archive(&data, Threads::count_or_single(2));
    for threads in [4, 8, 16] {
        assert_eq!(
            write_archive(&data, Threads::count_or_single(threads)),
            reference,
            "{threads} workers changed the archive",
        );
    }
    assert_eq!(read_back(&reference), data);
}

/// A single thread writes one unbroken stream, which still has to read back.
///
/// That output is deliberately different - it is the smallest the level can
/// produce - so the guarantee here is the round trip, not the bytes.
#[test]
fn test_single_thread_round_trips() {
    let data = payload(STREAMING_THRESHOLD + 1024 * 1024);
    let archive = write_archive(&data, Threads::Single);
    assert_eq!(read_back(&archive), data);
}

/// A sink that records how much has reached it.
struct CountingSink {
    inner: Cursor<Vec<u8>>,
    written: Arc<AtomicU64>,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written.fetch_add(n as u64, Ordering::Release);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl std::io::Seek for CountingSink {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A source that records how much has been taken from it, and notes the
/// largest gap between what it has given out and what has reached the sink.
struct GapReader<'a> {
    data: &'a [u8],
    position: usize,
    written: Arc<AtomicU64>,
    largest_gap: u64,
}

impl Read for GapReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.data.len() - self.position);
        buf[..n].copy_from_slice(&self.data[self.position..self.position + n]);
        self.position += n;

        let gap = (self.position as u64).saturating_sub(self.written.load(Ordering::Acquire));
        self.largest_gap = self.largest_gap.max(gap);

        Ok(n)
    }
}

/// Data with nothing to find in it, so that what is written tracks what was
/// read.
///
/// The measurement below is read-position minus bytes-written. On data that
/// compresses, most of that difference is the compression itself and grows
/// with the entry whatever the writer holds, which would make the test pass
/// for the wrong reason. Incompressible data makes the difference the
/// writer's own backlog and nothing else.
fn incompressible(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Runs one entry through the writer and returns the largest backlog seen.
fn largest_backlog(data: &[u8]) -> u64 {
    let written = Arc::new(AtomicU64::new(0));
    let sink = CountingSink {
        inner: Cursor::new(Vec::new()),
        written: Arc::clone(&written),
    };
    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        .threads(Threads::count_or_single(4));
    let mut writer = Writer::create(sink).expect("writer").options(options);

    let mut source = GapReader {
        data,
        position: 0,
        written: Arc::clone(&written),
        largest_gap: 0,
    };

    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut source,
            EntryMeta::file(data.len() as u64),
        )
        .expect("adds");
    let gap = source.largest_gap;
    let _ = writer.finish_into_inner().expect("finishes");
    gap
}

/// Memory has to follow the window, not the size of the entry.
///
/// What the writer is holding is the gap between what it has read and what it
/// has written. The invariant is that this does not grow with the entry:
/// doubling the input must not double the backlog, which is what makes a file
/// larger than memory archivable at all. A multi-threaded encoder without
/// backpressure queues the whole entry, and that is what this catches.
///
/// Stated as a comparison between two sizes rather than as a fraction of one.
/// A fraction is satisfied by anything that buffers a constant share of the
/// input - including buffering all of it - so it would have passed for a
/// writer with no bound at all.
#[test]
fn test_memory_follows_the_window_not_the_entry() {
    let small = incompressible(STREAMING_THRESHOLD + 8 * 1024 * 1024);
    let large = incompressible(STREAMING_THRESHOLD + 136 * 1024 * 1024);

    let small_backlog = largest_backlog(&small);
    let large_backlog = largest_backlog(&large);

    // The entry grew by 128 MiB. The backlog is allowed to differ by a little
    // - it is sampled, and the last blocks of a run land differently - but not
    // to follow the entry.
    let slack = 16 * 1024 * 1024;
    assert!(
        large_backlog <= small_backlog + slack,
        "backlog went from {small_backlog} bytes on a {} byte entry to \
         {large_backlog} on a {} byte one: it is following the entry rather \
         than the window",
        small.len(),
        large.len(),
    );

    // And it really is bounded, not merely growing slowly. The bound is the
    // threshold - which is read before the entry can be recognised as large -
    // plus the blocks in flight behind it. What it is not is a share of the
    // entry: on the larger entry here that would allow four times as much.
    let bound = (STREAMING_THRESHOLD + 64 * 1024 * 1024) as u64;
    for (backlog, data) in [(small_backlog, &small), (large_backlog, &large)] {
        assert!(
            backlog < bound,
            "backlog {backlog} on a {} byte entry, over a bound of {bound}",
            data.len(),
        );
    }
}

/// A wrong declared size must not cost memory either.
///
/// The bound above holds when the caller knows how long the entry is. An entry
/// that claims to be small and is not has to be recognised by reading, or the
/// writer holds a file it was told would fit and does not.
#[test]
fn test_a_size_declared_too_small_still_bounds_memory() {
    let data = incompressible(STREAMING_THRESHOLD + 136 * 1024 * 1024);

    let written = Arc::new(AtomicU64::new(0));
    let sink = CountingSink {
        inner: Cursor::new(Vec::new()),
        written: Arc::clone(&written),
    };
    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        .threads(Threads::count_or_single(4));
    let mut writer = Writer::create(sink).expect("writer").options(options);

    let mut source = GapReader {
        data: &data,
        position: 0,
        written: Arc::clone(&written),
        largest_gap: 0,
    };

    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut source,
            // A tenth of the truth.
            EntryMeta::file(data.len() as u64 / 10),
        )
        .expect("adds");
    let backlog = source.largest_gap;
    let _ = writer.finish_into_inner().expect("finishes");

    assert!(
        backlog < (STREAMING_THRESHOLD + 64 * 1024 * 1024) as u64,
        "backlog {backlog} on a {} byte entry declared as {}: the declared size \
         decided how the entry was held",
        data.len(),
        data.len() / 10,
    );
}

/// A batch compressed while the entry behind it streams must not reach the
/// archive.
///
/// Entries waiting in the batch are written before the large entry that
/// follows them, but they may be compressed at the same time as it - so what
/// the writer does with the batch depends on the memory budget, and the
/// archive must not. The budgets below land on either side of that: under the
/// cramped one the batch is too large a share to be worth overlapping and is
/// compressed in order, while the roomy one sends it off alongside the entry
/// and writes it when it comes back.
#[test]
fn test_a_batch_compressed_alongside_does_not_reach_the_archive() {
    use zesven::MemoryLimit;

    let small: Vec<Vec<u8>> = (0..4).map(|i| payload(1 << 20 << (i % 3))).collect();
    let large = payload(STREAMING_THRESHOLD + (4 << 20));

    let build = |limit: MemoryLimit, threads: Threads| {
        let options = WriteOptions::new()
            .level(1)
            .expect("level")
            .memory_limit(limit)
            .threads(threads);
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .expect("writer")
            .options(options);
        // Small entries, then a large one, twice: the second batch is the one
        // that starts while the first large entry is still being written.
        for round in 0..2 {
            for (i, data) in small.iter().enumerate() {
                writer
                    .add_bytes(
                        ArchivePath::new(&format!("{round}-{i}.bin")).expect("path"),
                        data,
                    )
                    .expect("adds");
            }
            writer
                .add_bytes(
                    ArchivePath::new(&format!("{round}-big.bin")).expect("path"),
                    &large,
                )
                .expect("adds");
        }
        writer.finish_into_inner().expect("finishes").1.into_inner()
    };

    let cramped = build(
        MemoryLimit::bytes_or_auto(16 << 20),
        Threads::count_or_single(2),
    );
    let roomy = build(
        MemoryLimit::bytes_or_auto(4 << 30),
        Threads::count_or_single(8),
    );

    assert_eq!(
        cramped, roomy,
        "when the batch ahead was collected reached the bytes",
    );

    // And the archive says what it should: every entry, in the order it was
    // added, holding what it was given.
    let mut archive = Archive::open(Cursor::new(cramped)).expect("opens");
    let names: Vec<String> = archive
        .entries()
        .iter()
        .map(|entry| entry.path.to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "0-0.bin",
            "0-1.bin",
            "0-2.bin",
            "0-3.bin",
            "0-big.bin",
            "1-0.bin",
            "1-1.bin",
            "1-2.bin",
            "1-3.bin",
            "1-big.bin",
        ],
    );
    assert_eq!(
        archive.extract_to_vec("0-2.bin").expect("extracts"),
        small[2]
    );
    assert_eq!(
        archive.extract_to_vec("1-big.bin").expect("extracts"),
        large
    );

    // A single thread takes neither path: the batch is not sent anywhere,
    // because a caller asking for one thread is asking for one. What that
    // produces is deliberately different - an unbroken stream - so the promise
    // here is that it still reads back.
    let single = build(MemoryLimit::bytes_or_auto(4 << 30), Threads::Single);
    let mut archive = Archive::open(Cursor::new(single)).expect("opens");
    assert_eq!(
        archive.extract_to_vec("0-2.bin").expect("extracts"),
        small[2]
    );
    assert_eq!(
        archive.extract_to_vec("1-big.bin").expect("extracts"),
        large
    );
}

/// A batch waiting to be written must not turn the entry behind it into a
/// buffer.
///
/// Its compressed output has to be held back until the batch reaches the sink,
/// and holding it without a bound would undo the one property this path exists
/// for: a 10 GB entry would be held whole because four small ones were still
/// being compressed. What is held is capped instead, and the batch is collected
/// the moment either it finishes or that cap is reached.
#[test]
fn test_a_batch_ahead_does_not_hold_the_entry() {
    use zesven::MemoryLimit;

    // Enough budget that the batch is worth sending ahead at all: below that
    // the writer compresses it in order and this measures nothing.
    let budget = MemoryLimit::bytes_or_auto(1 << 30);
    // Fewer entries than there are threads, so the batch is still waiting when
    // the large entry arrives rather than having flushed itself on the way in.
    let batch: Vec<Vec<u8>> = (0..3).map(|_| incompressible(2 << 20)).collect();

    let backlog_of = |entry: &[u8]| -> u64 {
        let written = Arc::new(AtomicU64::new(0));
        let sink = CountingSink {
            inner: Cursor::new(Vec::new()),
            written: Arc::clone(&written),
        };
        let options = WriteOptions::new()
            .level(1)
            .expect("level")
            .memory_limit(budget)
            .threads(Threads::count_or_single(4));
        let mut writer = Writer::create(sink).expect("writer").options(options);
        for (i, data) in batch.iter().enumerate() {
            writer
                .add_bytes(ArchivePath::new(&format!("s{i}.bin")).expect("path"), data)
                .expect("adds");
        }

        let mut source = GapReader {
            data: entry,
            position: 0,
            written: Arc::clone(&written),
            largest_gap: 0,
        };
        writer
            .add_stream(
                ArchivePath::new("big.bin").expect("path"),
                &mut source,
                EntryMeta::file(entry.len() as u64),
            )
            .expect("adds");
        let gap = source.largest_gap;
        let _ = writer.finish_into_inner().expect("finishes");
        gap
    };

    let small = incompressible(STREAMING_THRESHOLD + 8 * 1024 * 1024);
    let large = incompressible(STREAMING_THRESHOLD + 136 * 1024 * 1024);
    let small_backlog = backlog_of(&small);
    let large_backlog = backlog_of(&large);

    // The entry grew by 128 MiB behind the same batch. What is held back is
    // the batch and the cap on the holding area, neither of which knows how
    // large the entry is.
    let slack = 16 * 1024 * 1024;
    assert!(
        large_backlog <= small_backlog + slack,
        "backlog went from {small_backlog} on a {} byte entry to \
         {large_backlog} on a {} byte one, behind the same batch: the batch \
         ahead turned the entry into a buffer",
        small.len(),
        large.len(),
    );
}

/// A sink that records how far the source had been read when it first
/// received anything.
struct FirstWriteWatcher {
    inner: Cursor<Vec<u8>>,
    read_so_far: Arc<AtomicU64>,
    read_at_first_write: Arc<AtomicU64>,
    seen_a_write: bool,
}

impl Write for FirstWriteWatcher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Only once the entry has started arriving: the signature header is
        // written before anything is read, and is not what this is watching
        // for.
        let read = self.read_so_far.load(Ordering::Acquire);
        if !self.seen_a_write && !buf.is_empty() && read > 0 {
            self.seen_a_write = true;
            self.read_at_first_write.store(read, Ordering::Release);
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl std::io::Seek for FirstWriteWatcher {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A source that records how much has been taken from it.
struct WatchedReader<'a> {
    data: &'a [u8],
    position: usize,
    read_so_far: Arc<AtomicU64>,
}

impl Read for WatchedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let taken = (self.data.len() - self.position).min(buf.len());
        buf[..taken].copy_from_slice(&self.data[self.position..self.position + taken]);
        self.position += taken;
        self.read_so_far
            .store(self.position as u64, Ordering::Release);
        Ok(taken)
    }
}

/// The batch really is compressed alongside the entry, not before it.
///
/// Everything else here holds whether or not the two overlap - an archive
/// written in order is a correct archive - so without this the whole thing
/// could stop happening and every other test would still pass.
///
/// What gives it away is when the batch reaches the sink. Compressed in turn it
/// is written before the entry is read at all, so nothing has been taken from
/// the source past the prefix that was already read to recognise it.
/// Compressed alongside, the entry goes on being read while the batch is still
/// going, and the source is well past that by the time anything is written.
///
/// The batch is one entry, so it holds one core and takes far longer than the
/// entry does on four; that is the shape the overlap exists for, and the shape
/// that makes this measurable.
#[test]
fn test_the_batch_is_compressed_while_the_entry_is_read() {
    use zesven::MemoryLimit;

    let read_so_far = Arc::new(AtomicU64::new(0));
    let read_at_first_write = Arc::new(AtomicU64::new(0));

    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        // Roomy enough that a batch this size is worth sending ahead.
        .memory_limit(MemoryLimit::bytes_or_auto(4 << 30))
        .threads(Threads::count_or_single(4));
    let mut writer = Writer::create(FirstWriteWatcher {
        inner: Cursor::new(Vec::new()),
        read_so_far: Arc::clone(&read_so_far),
        read_at_first_write: Arc::clone(&read_at_first_write),
        seen_a_write: false,
    })
    .expect("writer")
    .options(options);

    // Two entries just under the write-through threshold - past it they would
    // be streamed rather than batched - so the batch is twice the work the
    // prefix is, and stays the slower of the two even where there are not
    // enough cores to run them side by side.
    let waiting = incompressible(STREAMING_THRESHOLD - (4 << 20));
    for name in ["batch-0.bin", "batch-1.bin"] {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), &waiting)
            .expect("adds");
    }

    let large = incompressible(110 << 20);
    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut WatchedReader {
                data: &large,
                position: 0,
                read_so_far: Arc::clone(&read_so_far),
            },
            EntryMeta::file(large.len() as u64),
        )
        .expect("adds");
    let archive = writer
        .finish_into_inner()
        .expect("finishes")
        .1
        .inner
        .into_inner();

    let at_first_write = read_at_first_write.load(Ordering::Acquire);
    let prefix = STREAMING_THRESHOLD as u64;
    assert!(
        at_first_write > prefix,
        "the first bytes reached the sink after {at_first_write} bytes had been \
         read, against a {prefix} byte prefix that is read before the entry is \
         even recognised: the batch was compressed before the entry rather than \
         alongside it",
    );

    // And the archive is still an archive.
    let mut archive = Archive::open(Cursor::new(archive)).expect("opens");
    assert_eq!(archive.extract_to_vec("big.bin").expect("extracts"), large);
}
