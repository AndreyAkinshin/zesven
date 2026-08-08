//! An entry large enough to be written straight through is left finishing
//! while the entry after it is read, so that the last blocks of one stream -
//! the largest a stream cuts, and the ones with nothing behind them - are
//! compressed against the first blocks of the next rather than against an idle
//! machine.
//!
//! That overlap is only allowed to change how long writing takes. What it must
//! not touch is the archive: which bytes an entry compresses to, where its
//! folder sits, and where its name appears in the file list are all settled by
//! the input and the options, and none of them may depend on what happened to
//! be running alongside.

#![cfg(all(feature = "lzma2", feature = "parallel"))]

use std::io::Cursor;

use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, Writer};
use zesven::{ArchivePath, MemoryLimit, Threads};

/// Entries at or above this go through the write-through path.
///
/// Mirrors `write::streaming_entry::STREAMING_THRESHOLD`, which is internal. A
/// test that quietly stopped exercising that path would still pass, so the
/// sizes below are stated relative to this rather than as bare numbers.
const STREAMING_THRESHOLD: usize = 64 * 1024 * 1024;

/// Where the data area ends, read from the signature header.
///
/// Everything before it is entry data and nothing else, so it is exactly the
/// span two archives have to agree on for their entries to have been written
/// identically - the header after it differs as soon as the archives hold
/// different entries, which says nothing about the streams.
const SIGNATURE_HEADER_SIZE: usize = 32;

/// Data that compresses a little, as a video container does.
///
/// Neither incompressible nor repetitive: the first would hide a matcher that
/// silently produced nothing, and the second compresses so far that a block
/// boundary stops being visible in the output at all.
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = seed | 1;
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

fn options(threads: usize) -> WriteOptions {
    WriteOptions::new()
        .level(1)
        .expect("level")
        // Stated rather than detected, so that what the entries are allowed to
        // hold is the same on every machine this runs on.
        .memory_limit(MemoryLimit::bytes_or_auto(1 << 30))
        .threads(Threads::count_or_single(threads))
}

/// Writes the named entries in order and returns the archive.
fn write_entries(entries: &[(&str, &[u8])], threads: usize) -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options(threads));
    for (name, data) in entries {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), data)
            .expect("adds");
    }
    let (_result, cursor) = writer.finish_into_inner().expect("finishes");
    cursor.into_inner()
}

/// How long the data area is, from the archive's own signature header.
fn data_area(archive: &[u8]) -> usize {
    let offset = u64::from_le_bytes(archive[12..20].try_into().expect("eight bytes"));
    SIGNATURE_HEADER_SIZE + offset as usize
}

/// What an entry compresses to must not depend on what is written after it.
///
/// The entry is left finishing while the next one is read, so the two are
/// genuinely in the encoder at the same time. If anything about that reached
/// the first entry's stream - a block cut differently because the window had
/// been narrowed, output interleaved with the entry behind it - then adding a
/// second entry would change the first, and an archive would stop being a
/// function of what went into it.
#[test]
fn test_an_entry_is_written_the_same_whatever_follows_it() {
    let first = payload(1, STREAMING_THRESHOLD + 3 * 1024 * 1024);
    let second = payload(2, STREAMING_THRESHOLD + 5 * 1024 * 1024);

    let alone = write_entries(&[("a.bin", &first)], 8);
    let followed = write_entries(&[("a.bin", &first), ("b.bin", &second)], 8);

    // From the end of the signature header, which is not part of the data: it
    // carries the position and the checksum of the header that follows the
    // streams, and both differ as soon as the archive holds another entry.
    let span = data_area(&alone);
    assert_eq!(
        &followed[SIGNATURE_HEADER_SIZE..span],
        &alone[SIGNATURE_HEADER_SIZE..span],
        "the first entry's stream changed when a second entry was written after it",
    );
    assert!(
        followed.len() > alone.len(),
        "the second entry produced no data at all",
    );
}

/// Entries compressed alongside each other must still give one archive.
///
/// How many of them are in flight at once follows from the thread count and the
/// memory budget, so if any of that reached the output the same directory would
/// give one archive on a laptop and another on a build server.
#[test]
fn test_overlapping_entries_do_not_depend_on_the_worker_count() {
    let first = payload(3, STREAMING_THRESHOLD + 1024 * 1024);
    let second = payload(4, STREAMING_THRESHOLD + 2 * 1024 * 1024);
    let third = payload(5, STREAMING_THRESHOLD + 3 * 1024 * 1024);
    let entries: &[(&str, &[u8])] = &[("a.bin", &first), ("b.bin", &second), ("c.bin", &third)];

    let reference = write_entries(entries, 2);
    for threads in [4, 8, 16] {
        assert_eq!(
            write_entries(entries, threads),
            reference,
            "{threads} workers changed the archive",
        );
    }

    let mut archive = Archive::open(Cursor::new(reference)).expect("opens");
    assert_eq!(archive.extract_to_vec("a.bin").expect("extracts"), first);
    assert_eq!(archive.extract_to_vec("b.bin").expect("extracts"), second);
    assert_eq!(archive.extract_to_vec("c.bin").expect("extracts"), third);
}

/// Entries have to be listed in the order they were added, not the order they
/// finished.
///
/// A large entry is still being compressed when the calls after it return, and
/// the entries those calls add reach the file list by four different routes: a
/// directory and an empty file are recorded without writing any data at all, a
/// small file waits in a batch, and a second large entry goes straight through.
/// Each is a way for something to overtake an entry that was accepted before
/// it, and the position of a name in that list is what binds it to its data.
#[test]
fn test_order_survives_an_entry_still_being_finished() {
    let first = payload(6, STREAMING_THRESHOLD + 2 * 1024 * 1024);
    let last = payload(7, STREAMING_THRESHOLD + 1024 * 1024);
    let small = payload(8, 64 * 1024);

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options(8));
    writer
        .add_bytes(ArchivePath::new("1_big.bin").expect("path"), &first)
        .expect("adds");
    writer
        .add_directory(
            ArchivePath::new("2_dir").expect("path"),
            EntryMeta::directory(),
        )
        .expect("adds");
    writer
        .add_bytes(ArchivePath::new("3_empty.bin").expect("path"), b"")
        .expect("adds");
    writer
        .add_bytes(ArchivePath::new("4_small.bin").expect("path"), &small)
        .expect("adds");
    writer
        .add_bytes(ArchivePath::new("5_big.bin").expect("path"), &last)
        .expect("adds");
    let (_result, cursor) = writer.finish_into_inner().expect("finishes");

    let mut archive = Archive::open(Cursor::new(cursor.into_inner())).expect("opens");
    let names: Vec<String> = archive
        .entries()
        .iter()
        .map(|entry| entry.path.as_str().to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "1_big.bin",
            "2_dir",
            "3_empty.bin",
            "4_small.bin",
            "5_big.bin",
        ],
    );

    assert_eq!(
        archive.extract_to_vec("1_big.bin").expect("extracts"),
        first
    );
    assert_eq!(
        archive.extract_to_vec("3_empty.bin").expect("extracts"),
        b""
    );
    assert_eq!(
        archive.extract_to_vec("4_small.bin").expect("extracts"),
        small
    );
    assert_eq!(archive.extract_to_vec("5_big.bin").expect("extracts"), last);
}

/// A reporter that refuses to let the write go on after so many blocks.
///
/// Counts blocks rather than bytes so that the refusal lands somewhere in the
/// middle of an entry whatever the data compresses to.
struct RefusesAfter {
    seen: std::sync::atomic::AtomicUsize,
    limit: usize,
    finished_badly: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl zesven::progress::ProgressReporter for RefusesAfter {
    fn on_progress(&mut self, _processed: u64, _total: u64) -> bool {
        self.seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < self.limit
    }

    fn on_entry_complete(&mut self, entry_name: &str, success: bool) {
        if !success {
            if let Ok(mut held) = self.finished_badly.lock() {
                held.push(entry_name.to_string());
            }
        }
    }
}

/// Calling the write off partway through an entry must reach the caller as a
/// cancellation, and must leave a writer that refuses to be finished.
///
/// The refusal travels out through the encoder as an ordinary write error,
/// because that is all an encoder can raise, and has to be turned back into
/// what the caller asked for. What it leaves behind is bytes in the sink that
/// belong to no folder, so the archive cannot be completed - and an archive
/// that opened and then failed on some later entry would give the caller no way
/// to tell.
///
/// A second entry is written so that the refusal lands with an earlier one
/// already accepted rather than on an empty writer. Whether that earlier entry
/// is still being finished at the moment of the refusal depends on timing and
/// is deliberately not asserted; winding one down is covered directly by
/// dropping a writer, below.
#[test]
fn test_calling_off_a_write_ends_as_a_cancellation() {
    let first = payload(9, STREAMING_THRESHOLD + 2 * 1024 * 1024);
    let second = payload(10, STREAMING_THRESHOLD + 2 * 1024 * 1024);
    let badly = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options(8))
        .progress(RefusesAfter {
            seen: std::sync::atomic::AtomicUsize::new(0),
            // Past the first entry, so that it is out on a thread when the
            // second one is called off.
            limit: 24,
            finished_badly: std::sync::Arc::clone(&badly),
        });

    writer
        .add_bytes(ArchivePath::new("a.bin").expect("path"), &first)
        .expect("adds");
    let stopped = writer.add_bytes(ArchivePath::new("b.bin").expect("path"), &second);

    let error = stopped.expect_err("the reporter called the write off");
    assert!(
        matches!(error, zesven::Error::Cancelled),
        "a refusal reached the caller as {error} rather than as a cancellation",
    );

    // An entry stopped partway has left bytes in the sink that belong to no
    // folder, so the archive cannot be completed - and refusing is the point:
    // an archive that opened and then failed on some later entry would give the
    // caller no way to tell it had happened.
    writer
        .finish()
        .expect_err("an archive stopped partway cannot be finished");

    let badly = badly.lock().expect("lock");
    assert!(
        !badly.is_empty(),
        "nothing was reported as having failed, so a caller watching would \
         still be showing entries as running",
    );
}

/// A writer dropped without being finished must not leave a thread behind.
///
/// An entry is left compressing on a thread that writes into a bounded area,
/// and a thread that has filled that area waits for someone to drain it. Where
/// nobody ever will, dropping has to say so rather than wait: this is the
/// difference between a writer that is dropped and a program that stops.
#[test]
fn test_dropping_a_writer_does_not_wait_for_an_entry_nobody_will_write() {
    let data = payload(11, STREAMING_THRESHOLD + 4 * 1024 * 1024);
    let (done, finished) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .expect("writer")
            .options(options(8));
        writer
            .add_bytes(ArchivePath::new("a.bin").expect("path"), &data)
            .expect("adds");
        // Dropped with the entry still being compressed, which is where the
        // call above is allowed to return.
        drop(writer);
        let _ = done.send(());
    });

    finished
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect(
            "dropping a writer with an entry still being compressed did not return: \
             the thread finishing it was left waiting on an area nobody drains",
        );
}
