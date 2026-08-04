//! Writing an archive is silent from the outside unless it is asked not to be.
//!
//! Entries are gathered and compressed together, so the call that accepts one
//! usually returns before anything has been compressed, and the work lands on
//! whichever call fills the batch or on `finish`. That is why the reporting
//! here is worth having and why it has to say more than "a call took a long
//! time": what a caller needs to know is which entries are being worked on, how
//! far the work has got, and when each one is in.

#![cfg(all(feature = "lzma2", feature = "parallel"))]

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use zesven::progress::ProgressReporter;
use zesven::write::{EntryMeta, WriteOptions, Writer};
use zesven::{ArchivePath, Error, Threads};

/// Entries at or above this go through the write-through path.
const STREAMING_THRESHOLD: usize = 64 * 1024 * 1024;

/// Data that compresses a little, so blocks are real work rather than nothing.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x243F_6A88_85A3_08D3u64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
        if state % 8 == 0 {
            data.extend_from_slice(b"........................");
        }
    }
    data.truncate(len);
    data
}

/// Every call the writer makes, in the order it made them.
#[derive(Debug, PartialEq, Eq)]
enum Said {
    Start(String, u64),
    Progress(u64, u64),
    Ratio(u64, u64),
    Done(String, bool),
}

#[derive(Clone, Default)]
struct Recorder {
    said: Arc<std::sync::Mutex<Vec<Said>>>,
}

impl Recorder {
    fn said(&self) -> std::sync::MutexGuard<'_, Vec<Said>> {
        self.said.lock().expect("not poisoned")
    }
}

impl ProgressReporter for Recorder {
    fn on_entry_start(&mut self, name: &str, size: u64) {
        self.said().push(Said::Start(name.to_string(), size));
    }

    fn on_progress(&mut self, done: u64, total: u64) -> bool {
        self.said().push(Said::Progress(done, total));
        true
    }

    fn on_ratio(&mut self, input: u64, output: u64) {
        self.said().push(Said::Ratio(input, output));
    }

    fn on_entry_complete(&mut self, name: &str, ok: bool) {
        self.said().push(Said::Done(name.to_string(), ok));
    }
}

/// Every entry is announced before it is worked on and reported when it is in.
///
/// The order matters as much as the presence: a batch is announced whole, at
/// the point the work on it starts, because its entries are compressed at the
/// same time as each other and no one of them is the one being worked on.
/// Reporting them as they finish instead would tell a caller nothing until the
/// wait was already over.
#[test]
fn test_every_entry_is_announced_before_it_is_written() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"))
        .progress(recorder.clone());

    for i in 0..3 {
        writer
            .add_bytes(
                ArchivePath::new(&format!("{i}.bin")).expect("path"),
                &payload(64 << 10),
            )
            .expect("adds");
    }
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let starts: Vec<&Said> = said
        .iter()
        .filter(|s| matches!(s, Said::Start(..)))
        .collect();
    let dones: Vec<&Said> = said
        .iter()
        .filter(|s| matches!(s, Said::Done(..)))
        .collect();

    assert_eq!(
        starts,
        vec![
            &Said::Start("0.bin".into(), 65536),
            &Said::Start("1.bin".into(), 65536),
            &Said::Start("2.bin".into(), 65536),
        ],
    );
    assert_eq!(
        dones,
        vec![
            &Said::Done("0.bin".into(), true),
            &Said::Done("1.bin".into(), true),
            &Said::Done("2.bin".into(), true),
        ],
    );

    // Announced whole, before any of them was finished.
    let last_start = said
        .iter()
        .rposition(|s| matches!(s, Said::Start(..)))
        .expect("a start");
    let first_done = said
        .iter()
        .position(|s| matches!(s, Said::Done(..)))
        .expect("a done");
    assert!(
        last_start < first_done,
        "the batch was reported one entry at a time as each finished, which \
         tells a caller nothing while it waits: {said:?}",
    );
}

/// Data that compresses well, so what is produced is plainly not what was read.
fn compressible(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(
            b"a line of text that repeats often enough for the matcher to earn its keep ",
        );
        data.extend_from_slice(&state.to_le_bytes()[..2]);
    }
    data.truncate(len);
    data
}

/// A large entry reports what has been produced, not what has been read.
///
/// It is compressed across cores, which takes the entry in far faster than it
/// works through it and finishes the rest when it is closed. A caller told
/// about reads would watch the count reach the end within a second and then
/// wait out the compression in silence, which is worse than not being told.
#[test]
fn test_a_large_entry_reports_the_work_rather_than_the_reading() {
    let data = compressible(STREAMING_THRESHOLD + (32 << 20));
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(
            WriteOptions::new()
                .level(1)
                .expect("level")
                .threads(Threads::count_or_single(4)),
        )
        .progress(recorder.clone());

    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut Cursor::new(data.clone()),
            EntryMeta::file(data.len() as u64),
        )
        .expect("adds");
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let counts: Vec<u64> = said
        .iter()
        .filter_map(|s| match s {
            Said::Progress(done, _) => Some(*done),
            _ => None,
        })
        .collect();

    assert!(
        counts.len() > 1,
        "one entry the size of {} bytes produced {} progress reports: a caller \
         watching this has nothing to watch",
        data.len(),
        counts.len(),
    );
    assert!(
        counts.windows(2).all(|w| w[1] >= w[0]),
        "what has been written went backwards: {counts:?}",
    );

    // What is reported is the archive being produced, which for data that
    // compresses is less than what was read.
    let produced = *counts.last().expect("a count");
    assert!(
        produced < data.len() as u64 / 2,
        "reported {produced} written for {} bytes of input that compresses to \
         a fraction of that: the count is the reading rather than the work",
        data.len(),
    );
}

/// The ratio a caller is given measures the same span on both sides.
///
/// Both halves cover the archive so far: what has been accepted into it, and
/// what it has become. Reporting one entry's input against the whole archive's
/// output gives a number that falls with every entry written whatever the data
/// does, so a caller showing it would watch compression appear to improve as
/// the archive grew.
#[test]
fn test_the_ratio_covers_the_archive_on_both_sides() {
    let sizes = [64usize << 10, 96 << 10, 32 << 10];
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"))
        .progress(recorder.clone());

    for (i, size) in sizes.iter().enumerate() {
        writer
            .add_bytes(
                ArchivePath::new(&format!("{i}.bin")).expect("path"),
                &compressible(*size),
            )
            .expect("adds");
    }
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let ratios: Vec<(u64, u64)> = said
        .iter()
        .filter_map(|s| match s {
            Said::Ratio(input, output) => Some((*input, *output)),
            _ => None,
        })
        .collect();

    assert_eq!(
        ratios.len(),
        sizes.len(),
        "one ratio was expected per entry written: {said:?}",
    );

    let mut running = 0u64;
    let accumulated: Vec<u64> = sizes
        .iter()
        .map(|size| {
            running += *size as u64;
            running
        })
        .collect();
    assert_eq!(
        ratios.iter().map(|(input, _)| *input).collect::<Vec<_>>(),
        accumulated,
        "the input side reported this entry rather than the archive so far, \
         which is not what the output side counts: {ratios:?}",
    );
    assert!(
        ratios
            .iter()
            .all(|(input, output)| *output > 0 && output < input),
        "data that compresses produced an archive at least as large as itself, \
         so the two sides are not measuring the same thing: {ratios:?}",
    );
}

/// A refusal from `on_progress` stops an entry that is being written.
///
/// It is the documented way to stop, and for a reporter with no state of its
/// own - one built from a closure cannot be given a `should_cancel` - it is the
/// only way. It is also the only way to stop at all once an entry large enough
/// to be written straight through has started, since nothing else is asked
/// until it is over.
///
/// What is left behind is not an archive that ends early: the entry's bytes are
/// already in the sink and belong to no folder, so the writer is finished with.
#[test]
fn test_a_refusal_from_on_progress_stops_the_entry() {
    struct Refuser {
        seen: Arc<AtomicUsize>,
    }

    impl ProgressReporter for Refuser {
        fn on_progress(&mut self, _done: u64, _total: u64) -> bool {
            self.seen.fetch_add(1, Ordering::Relaxed) < 1
        }
    }

    let data = compressible(STREAMING_THRESHOLD + (32 << 20));
    let seen = Arc::new(AtomicUsize::new(0));
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(
            WriteOptions::new()
                .level(1)
                .expect("level")
                .threads(Threads::count_or_single(4)),
        )
        .progress(Refuser {
            seen: Arc::clone(&seen),
        });

    let outcome = writer.add_stream(
        ArchivePath::new("big.bin").expect("path"),
        &mut Cursor::new(data.clone()),
        EntryMeta::file(data.len() as u64),
    );

    match outcome {
        Err(Error::Cancelled) => {}
        other => panic!("expected the entry to be called off, got {other:?}"),
    }
    assert!(
        seen.load(Ordering::Relaxed) >= 2,
        "the run stopped before the reporter had refused anything, so whatever \
         stopped it was not the refusal",
    );
}

/// A reporter that asks to stop is obeyed, between entries.
///
/// Between rather than during: an entry already being compressed is bytes on
/// their way to the sink, and stopping partway through leaves an archive that
/// has to be thrown away rather than one that simply ends early. Asking here
/// costs a caller nothing and leaves them with an archive they can finish.
#[test]
fn test_a_reporter_can_call_the_writing_off() {
    struct Stopper {
        seen: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
    }

    impl ProgressReporter for Stopper {
        fn on_entry_start(&mut self, _name: &str, _size: u64) {
            if self.seen.fetch_add(1, Ordering::Relaxed) >= 1 {
                self.stop.store(true, Ordering::Relaxed);
            }
        }

        fn should_cancel(&self) -> bool {
            self.stop.load(Ordering::Relaxed)
        }
    }

    let seen = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"))
        .progress(Stopper {
            seen: Arc::clone(&seen),
            stop: Arc::clone(&stop),
        });

    // The first entries are accepted; once the reporter has seen work start it
    // asks to stop, and the next entry offered is refused.
    let mut refused = None;
    for i in 0..40 {
        let outcome = writer.add_bytes(
            ArchivePath::new(&format!("{i:02}.bin")).expect("path"),
            &payload(1 << 20),
        );
        if let Err(e) = outcome {
            refused = Some(e);
            break;
        }
    }

    match refused {
        Some(Error::Cancelled) => {}
        other => panic!("expected the writing to be called off, got {other:?}"),
    }
}

/// Every entry an archive holds is announced and reported, whichever way in it
/// took.
///
/// Reporting was attached to the two paths that were being worked on when it
/// was written, and entries reach an archive by six. A solid archive went
/// through none of the two, so a caller watching one was told nothing at all
/// from the first entry to the last.
fn names_reported(said: &[Said]) -> (Vec<String>, Vec<String>) {
    let started = said
        .iter()
        .filter_map(|s| match s {
            Said::Start(name, _) => Some(name.clone()),
            _ => None,
        })
        .collect();
    let finished = said
        .iter()
        .filter_map(|s| match s {
            Said::Done(name, true) => Some(name.clone()),
            _ => None,
        })
        .collect();
    (started, finished)
}

/// Every entry is begun once and ended once, whichever paths an archive mixes.
///
/// The counts are the invariant that survives the paths being rearranged: an
/// entry announced twice leaves a caller counting two, and one ended twice
/// takes a bar past its own end. Both are what a second announcement or a
/// second close-out would look like from outside.
#[test]
fn test_each_entry_begins_once_and_ends_once() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(
            WriteOptions::new()
                .level(1)
                .expect("level")
                .threads(Threads::count_or_single(4)),
        )
        .progress(recorder.clone());

    writer
        .add_directory(
            ArchivePath::new("dir").expect("path"),
            EntryMeta::directory(),
        )
        .expect("adds a directory");
    for name in ["dir/one.bin", "dir/two.bin"] {
        writer
            .add_bytes(
                ArchivePath::new(name).expect("path"),
                &compressible(1 << 20),
            )
            .expect("adds");
    }
    writer
        .add_bytes(ArchivePath::new("empty.txt").expect("path"), &[])
        .expect("adds an empty one");

    let data = compressible(STREAMING_THRESHOLD + (4 << 20));
    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut Cursor::new(data.clone()),
            EntryMeta::file(data.len() as u64),
        )
        .expect("adds the large one");
    writer
        .add_anti_item(ArchivePath::new("gone.txt").expect("path"))
        .expect("adds an anti-item");
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let (started, finished) = names_reported(&said);
    let expected = [
        "dir",
        "dir/one.bin",
        "dir/two.bin",
        "empty.txt",
        "big.bin",
        "gone.txt",
    ];

    for name in expected {
        assert_eq!(
            started.iter().filter(|said| *said == name).count(),
            1,
            "{name} was not announced exactly once: {said:?}",
        );
        assert_eq!(
            finished.iter().filter(|said| *said == name).count(),
            1,
            "{name} did not end exactly once: {said:?}",
        );
    }
    // Nothing beyond them either, which is what a second announcement of an
    // entry already reported would look like from here.
    assert_eq!(
        (started.len(), finished.len()),
        (expected.len(), expected.len()),
        "the archive holds {} entries and {} were announced, {} reported: \
         {said:?}",
        expected.len(),
        started.len(),
        finished.len(),
    );
}

#[test]
fn test_a_solid_archive_reports_every_entry_in_the_block() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level").solid())
        .progress(recorder.clone());

    let names = ["one.bin", "two.bin", "three.bin"];
    for name in names {
        writer
            .add_bytes(
                ArchivePath::new(name).expect("path"),
                &compressible(48 << 10),
            )
            .expect("adds");
    }
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let (started, finished) = names_reported(&said);
    assert_eq!(started, names, "a solid block said nothing: {said:?}");
    assert_eq!(finished, names, "a solid block said nothing: {said:?}");

    // Announced whole before any of them finished, for the reason a batch is:
    // the block is one stream and no entry in it is the one being worked on.
    let last_start = said
        .iter()
        .rposition(|s| matches!(s, Said::Start(..)))
        .expect("a start");
    let first_done = said
        .iter()
        .position(|s| matches!(s, Said::Done(..)))
        .expect("a done");
    assert!(
        last_start < first_done,
        "the block was announced one entry at a time as each finished: {said:?}",
    );
}

/// A solid block of nothing but empty entries is still an archive of entries.
///
/// It has no data to compress, so it takes a path of its own that writes no
/// folder at all. A caller is told about those entries the same as any other.
#[test]
fn test_a_solid_block_of_empty_entries_still_reports_them() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level").solid())
        .progress(recorder.clone());

    let names = ["empty-a.txt", "empty-b.txt"];
    for name in names {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), &[])
            .expect("adds");
    }
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let (started, finished) = names_reported(&said);
    assert_eq!(started, names, "{said:?}");
    assert_eq!(finished, names, "{said:?}");
}

/// Directories and anti-items are entries, and the caller counts them.
///
/// They are written directly rather than compressed, which is why they were
/// missed. A CLI sizes its bar by everything it found on disk, directories
/// included, so entries that never report leave the bar short of its own end
/// on any archive of a tree.
#[test]
fn test_directories_and_anti_items_are_reported() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"))
        .progress(recorder.clone());

    writer
        .add_directory(
            ArchivePath::new("dir").expect("path"),
            EntryMeta::directory(),
        )
        .expect("adds a directory");
    writer
        .add_bytes(
            ArchivePath::new("dir/file.bin").expect("path"),
            &compressible(16 << 10),
        )
        .expect("adds a file");
    writer
        .add_anti_item(ArchivePath::new("gone.txt").expect("path"))
        .expect("adds an anti-item");
    writer
        .add_anti_directory(ArchivePath::new("gone-dir").expect("path"))
        .expect("adds an anti-directory");
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let (started, finished) = names_reported(&said);
    let expected = ["dir", "dir/file.bin", "gone.txt", "gone-dir"];
    assert_eq!(
        finished.len(),
        expected.len(),
        "an archive of four entries reported {} of them: {said:?}",
        finished.len(),
    );
    for name in expected {
        assert!(
            started.iter().any(|said| said == name),
            "{name} was never announced: {said:?}",
        );
        assert!(
            finished.iter().any(|said| said == name),
            "{name} was never reported finished: {said:?}",
        );
    }
}

/// An entry filtered through BCJ2 takes a path of its own, and reports.
#[test]
fn test_a_bcj2_entry_is_reported() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level").bcj2())
        .progress(recorder.clone());

    writer
        .add_bytes(
            ArchivePath::new("code.exe").expect("path"),
            &compressible(64 << 10),
        )
        .expect("adds");
    writer
        .add_bytes(ArchivePath::new("empty.exe").expect("path"), &[])
        .expect("adds an empty one");
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let (started, finished) = names_reported(&said);
    assert_eq!(started, ["code.exe", "empty.exe"], "{said:?}");
    assert_eq!(finished, ["code.exe", "empty.exe"], "{said:?}");
}

/// A batch compressed alongside a large entry is announced before it runs.
///
/// That batch takes a path of its own: it is handed to a thread rather than
/// compressed in order, and it is outstanding for as long as the entry beside
/// it takes to write. It is the longest wait a batch is ever part of, so a
/// caller hearing about its entries only as they finish would watch the whole
/// of it with nothing but the large entry to show.
#[test]
fn test_a_batch_sent_alongside_a_large_entry_is_announced_first() {
    let recorder = Recorder::default();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(
            WriteOptions::new()
                .level(1)
                .expect("level")
                .threads(Threads::count_or_single(4)),
        )
        .progress(recorder.clone());

    let batched = ["a.bin", "b.bin", "c.bin"];
    for name in batched {
        writer
            .add_bytes(
                ArchivePath::new(name).expect("path"),
                &compressible(1 << 20),
            )
            .expect("adds");
    }

    let data = compressible(STREAMING_THRESHOLD + (8 << 20));
    writer
        .add_stream(
            ArchivePath::new("big.bin").expect("path"),
            &mut Cursor::new(data.clone()),
            EntryMeta::file(data.len() as u64),
        )
        .expect("adds the large one");
    let _ = writer.finish_into_inner().expect("finishes");

    let said = recorder.said();
    let last_batched_start = said
        .iter()
        .rposition(|s| match s {
            Said::Start(name, _) => batched.contains(&name.as_str()),
            _ => false,
        })
        .expect("the batch was announced at all");
    let first_done = said
        .iter()
        .position(|s| matches!(s, Said::Done(..)))
        .expect("something finished");

    assert!(
        last_batched_start < first_done,
        "the batch running alongside the large entry was reported one entry at \
         a time as each finished, which is the end of the wait rather than the \
         start of it: {said:?}",
    );
}

/// A sink that takes a fixed number of writes and then refuses everything.
///
/// Counted in writes rather than bytes so that the failure lands between two
/// entries rather than wherever a compressed size happens to fall.
struct FailingSink {
    inner: Cursor<Vec<u8>>,
    allowed: usize,
}

impl std::io::Write for FailingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.allowed == 0 {
            return Err(std::io::Error::other("the sink is full"));
        }
        self.allowed -= 1;
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl std::io::Seek for FailingSink {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(to)
    }
}

/// A batch that fails after it was announced closes the entries it announced.
///
/// The batch is announced whole before it is compressed, so between that and
/// the entries reaching the sink there is a set of names a caller is showing as
/// running. If the writer gives up in between and says nothing, they are shown
/// as running for as long as the caller keeps the reporter: a bar that never
/// comes back, on a run that is already over.
///
/// Only the ones that did not make it. An entry already in the archive has been
/// reported finished, and reporting it again as failed would take back
/// something that is true.
#[test]
fn test_entries_announced_before_a_failure_are_closed() {
    let recorder = Recorder::default();
    // The signature header, then one entry. The second entry is what the sink
    // refuses.
    let mut writer = Writer::create(FailingSink {
        inner: Cursor::new(Vec::new()),
        allowed: 2,
    })
    .expect("writer")
    .options(WriteOptions::new().level(1).expect("level"))
    .progress(recorder.clone());

    let names = ["a.bin", "b.bin"];
    for name in names {
        writer
            .add_bytes(
                ArchivePath::new(name).expect("path"),
                &compressible(32 << 10),
            )
            .expect("accepted, since nothing is written yet");
    }
    writer
        .finish_into_inner()
        .err()
        .expect("the sink refuses the data");

    let said = recorder.said();
    let announced: Vec<&Said> = said
        .iter()
        .filter(|s| matches!(s, Said::Start(..)))
        .collect();
    let outcomes: Vec<(String, bool)> = said
        .iter()
        .filter_map(|s| match s {
            Said::Done(name, ok) => Some((name.clone(), *ok)),
            _ => None,
        })
        .collect();

    assert_eq!(
        announced.len(),
        names.len(),
        "the batch was not announced, so there is nothing to close: {said:?}",
    );
    assert_eq!(
        outcomes,
        vec![("a.bin".to_string(), true), ("b.bin".to_string(), false)],
        "every announced entry should end exactly once, the one that reached \
         the archive as a success and the one that did not as a failure: \
         {said:?}",
    );
}
