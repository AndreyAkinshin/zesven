//! The writer's memory budget is a figure it reasons about, so the reasoning
//! can be wrong in ways every other test still passes: an archive written from
//! the right bytes in the right order says nothing about what was in memory
//! while it was written. This measures that directly, through the allocator, so
//! that a term left out of the account shows up as a number rather than as an
//! argument.
//!
//! It has a global allocator of its own, which is why it is a file of its own:
//! the counter is process-wide, and a test sharing the process with others
//! would measure them too.

#![cfg(all(feature = "lzma2", feature = "parallel"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use zesven::codec::CodecMethod;
use zesven::write::{EntryMeta, WriteOptions, Writer};
use zesven::{ArchivePath, MemoryLimit, Threads};

/// Bytes currently allocated, and the high-water mark since it was last reset.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counted;

impl Counted {
    fn note(live: usize) {
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            Self::note(LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            // A block that came back at a different address was copied, so both
            // the old one and the new one were live while that happened. A
            // block that came back at the same one was extended where it lay,
            // and no such moment existed. Recording the first as the difference
            // alone would miss the highest point of a growing buffer, which is
            // exactly what the batch's output is.
            if !std::ptr::eq(moved, pointer) {
                Self::note(LIVE.load(Ordering::Relaxed) + new_size);
            }
            let live = if new_size >= layout.size() {
                let by = new_size - layout.size();
                LIVE.fetch_add(by, Ordering::Relaxed) + by
            } else {
                let by = layout.size() - new_size;
                LIVE.fetch_sub(by, Ordering::Relaxed) - by
            };
            Self::note(live);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

/// What is live now, and the peak reset to it.
fn start_watching() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

/// How far the peak rose above where watching started.
fn risen_since(baseline: usize) -> usize {
    PEAK.load(Ordering::Relaxed).saturating_sub(baseline)
}

/// A sink that keeps a position and throws the bytes away.
///
/// The archive would otherwise be counted as the writer's own memory, and an
/// entry that does not compress is as large again as itself.
struct NullSink {
    position: u64,
    end: u64,
}

impl Write for NullSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.position += buf.len() as u64;
        self.end = self.end.max(self.position);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for NullSink {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.position = match to {
            SeekFrom::Start(at) => at,
            SeekFrom::End(offset) => self.end.saturating_add_signed(offset),
            SeekFrom::Current(offset) => self.position.saturating_add_signed(offset),
        };
        Ok(self.position)
    }
}

/// Data that does not compress, which is the case every bound has to hold for.
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

/// A source that hands out a slice without holding a second copy of it.
struct SliceReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl Read for SliceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let taken = (self.data.len() - self.position).min(buf.len());
        buf[..taken].copy_from_slice(&self.data[self.position..self.position + taken]);
        self.position += taken;
        Ok(taken)
    }
}

/// What the writer peaks at over a given corpus, in bytes above where it
/// started.
fn peak_of(budget: usize, batch: &[Vec<u8>], large: &[&[u8]], method: CodecMethod) -> usize {
    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        .method(method)
        .memory_limit(MemoryLimit::bytes_or_auto(budget as u64))
        .threads(Threads::count_or_single(4));
    let mut writer = Writer::create(NullSink {
        position: 0,
        end: 0,
    })
    .expect("writer")
    .options(options);

    let baseline = start_watching();
    for (i, data) in batch.iter().enumerate() {
        writer
            .add_bytes(ArchivePath::new(&format!("s{i}.bin")).expect("path"), data)
            .expect("adds");
    }
    for (i, data) in large.iter().enumerate() {
        writer
            .add_stream(
                ArchivePath::new(&format!("big{i}.bin")).expect("path"),
                &mut SliceReader { data, position: 0 },
                EntryMeta::file(data.len() as u64),
            )
            .expect("adds");
    }
    let _ = writer.finish_into_inner().expect("finishes");
    risen_since(baseline)
}

/// Compressing a batch alongside a large entry must cost what it is charged.
///
/// One test in this file, because the counter is process-wide and tests run
/// alongside each other by default: a second test allocating here would be
/// counted as part of what this one is measuring. The round trip at the end is
/// part of the same test for that reason, and is there so the figures above are
/// known to be of a writer that works rather than one that gave up early.
///
/// The overlap is granted a quarter of the budget - an eighth for the batch and
/// an eighth for what is held with the buffer it swaps with - and taken off the
/// window the entry is given, so it should cost nothing over writing that entry
/// alone. What this catches is a term left out of that account: the batch's
/// outputs sitting beside its inputs, or output held past the point the writer
/// stops to collect. Every figure here is for data that does not compress,
/// which is what the account has to be right for.
#[test]
fn test_the_overlap_costs_what_it_is_charged_for() {
    // Roomy enough to admit a batch whose data is large against the margin
    // this allows itself: a batch of a few megabytes would leave every way of
    // mis-charging it - its output, or the room the vector holding that output
    // reserves past what it holds - inside the margin and invisible.
    let budget = 2 << 30;
    // Sized so that mis-charging it is visible rather than lost in a margin.
    // A batch is admitted only if what it will occupy fits an eighth of the
    // budget, and this one fits that when its data is charged twice and does
    // not when it is charged for what its output grows into as well. Admitted
    // on the smaller figure it goes on to spend past the share it was let in
    // on, which is the whole of what admitting it was meant to bound.
    let batch: Vec<Vec<u8>> = (0..3).map(|_| incompressible(30 << 20)).collect();
    // Well past the write-through threshold, so it is cut into blocks and its
    // output is held back while the batch is compressed.
    let large = incompressible(192 << 20);

    let alone = peak_of(budget, &[], &[&large], CodecMethod::Lzma2);
    let alongside = peak_of(budget, &batch, &[&large], CodecMethod::Lzma2);

    // What the batch may add: the share it was admitted on, and a margin for
    // the allocator's own noise. A batch admitted on a figure that leaves
    // something out spends more than the share, which is the whole of what
    // admitting it was meant to bound.
    let batch_bytes: usize = batch.iter().map(Vec::len).sum();
    let share = budget / 8;
    let allowed = alone + share + (16 << 20);
    assert!(
        alongside <= allowed,
        "the same entry peaked at {} MiB alone and {} MiB with a {} MiB batch \
         alongside it, over an allowance of {} MiB: something the overlap \
         spends is not being charged for",
        alone >> 20,
        alongside >> 20,
        batch_bytes >> 20,
        allowed >> 20,
    );

    // And a batch small enough to be admitted, which is the case the bound
    // exists for rather than the case that tests the bound: this one really is
    // compressed alongside the entry, and what the two of them occupy together
    // still has to sit inside the entry's own peak plus the share. It asserts
    // the promise admission makes rather than hunting a mistake - a batch that
    // fits its share has room to spare against it, by design.
    let admitted: Vec<Vec<u8>> = (0..3).map(|_| incompressible(12 << 20)).collect();
    let admitted_bytes: usize = admitted.iter().map(Vec::len).sum();
    let with_admitted = peak_of(budget, &admitted, &[&large], CodecMethod::Lzma2);
    assert!(
        with_admitted <= allowed,
        "a {} MiB batch that was admitted to run alongside took the peak to {} \
         MiB against {} MiB for the entry alone, over an allowance of {} MiB",
        admitted_bytes >> 20,
        with_admitted >> 20,
        alone >> 20,
        allowed >> 20,
    );

    // Again with a codec that writes its input straight through, and on a
    // budget small enough for the holding area to be smaller than the prefix.
    // `Copy` produces output as fast as it is fed, so anything handed to it in
    // one piece arrives in the holding area in one piece - and the prefix read
    // to recognise a large entry is up to the whole write-through threshold.
    // The budget matters as much as the codec: where a cap is larger than the
    // prefix, handing it over whole is within the cap and there is nothing to
    // see.
    let tight = 256 << 20;
    let tight_share = tight / 8;
    let stored_batch: Vec<Vec<u8>> = (0..3).map(|_| incompressible(2 << 20)).collect();
    let stored_alone = peak_of(tight, &[], &[&large], CodecMethod::Copy);
    let stored_alongside = peak_of(tight, &stored_batch, &[&large], CodecMethod::Copy);
    let stored_allowed = stored_alone + tight_share + (16 << 20);
    assert!(
        stored_alongside <= stored_allowed,
        "stored rather than compressed, the same entry peaked at {} MiB alone \
         and {} MiB with a batch alongside it, over an allowance of {} MiB: \
         output is reaching the holding area faster than it is being looked at",
        stored_alone >> 20,
        stored_alongside >> 20,
        stored_allowed >> 20,
    );

    // An entry whose last blocks are still being compressed while the next
    // entry is read is the other side of the same account, and the one where
    // getting it wrong is least visible: the two are never both at their own
    // peak, so a run that looks fine says nothing until the figure is compared
    // against what the tail was granted. What it may add is the share it is
    // allowed to keep, and no more - a tail handed over holding everything it
    // had at the end of its stream would double this.
    let second = incompressible(192 << 20);
    let one_after_another = peak_of(budget, &[], &[&large, &second], CodecMethod::Lzma2);
    let tail_allowed = alone + budget / 2 + (16 << 20);
    assert!(
        one_after_another <= tail_allowed,
        "one large entry peaked at {} MiB and two in a row at {} MiB, over an \
         allowance of {} MiB: an entry left finishing is holding more than the \
         share the entry behind it was told to work around",
        alone >> 20,
        one_after_another >> 20,
        tail_allowed >> 20,
    );

    // And the whole of it stays inside the budget the caller set, which is the
    // promise the shares are an implementation of.
    assert!(
        one_after_another <= budget + (64 << 20),
        "two large entries in a row peaked at {} MiB against a {} MiB budget",
        one_after_another >> 20,
        budget >> 20,
    );

    // The same shapes on a budget too small to hold a second encoder. A tail
    // is not handed over there at all, so two entries in a row have to cost
    // what one does: the floor of writing is one encoder, and the overlap must
    // not quietly make it two on a machine where a caller asked for little.
    let small = 64 << 20;
    let lean = incompressible(96 << 20);
    let lean_two = incompressible(96 << 20);
    let one_lean = peak_of(small, &[], &[&lean], CodecMethod::Lzma2);
    let two_lean = peak_of(small, &[], &[&lean, &lean_two], CodecMethod::Lzma2);
    eprintln!(
        "tight budget: one entry {} MiB, two in a row {} MiB",
        one_lean >> 20,
        two_lean >> 20
    );
    assert!(
        two_lean <= one_lean + (32 << 20),
        "on a {} MiB budget one entry peaked at {} MiB and two in a row at {} \
         MiB: an entry left finishing is costing a second encoder on a budget \
         that was never told it could have one",
        small >> 20,
        one_lean >> 20,
        two_lean >> 20,
    );

    round_trips();
}

/// The same shapes written to a sink that keeps its bytes, and read back.
fn round_trips() {
    let data = incompressible(70 << 20);
    let options = WriteOptions::new()
        .level(1)
        .expect("level")
        .memory_limit(MemoryLimit::bytes_or_auto(256 << 20))
        .threads(Threads::count_or_single(4));
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options);
    writer
        .add_bytes(ArchivePath::new("s0.bin").expect("path"), b"small")
        .expect("adds");
    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut SliceReader {
                data: &data,
                position: 0,
            },
            EntryMeta::file(data.len() as u64),
        )
        .expect("adds");
    let archive = writer.finish_into_inner().expect("finishes").1.into_inner();

    let mut read = zesven::read::Archive::open(Cursor::new(archive)).expect("opens");
    assert_eq!(read.extract_to_vec("s0.bin").expect("extracts"), b"small");
    assert_eq!(read.extract_to_vec("big.bin").expect("extracts"), data);
}
