//! Extracting used to carry a reporter and tell it nothing.
//!
//! `ExtractOptions` has taken one since it was written, and the only method
//! ever called on it was `should_cancel`: a caller could stop a run but never
//! see one. These are the calls that were missing, and they matter most where
//! extraction takes long enough to watch.

#![cfg(feature = "lzma2")]

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zesven::progress::ProgressReporter;
use zesven::read::{Archive, ExtractOptions};
use zesven::write::{WriteOptions, Writer};
use zesven::{ArchivePath, Error};

/// Data worth compressing, so entries have a size worth reporting.
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = seed | 1;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"payload that repeats for the matcher ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

fn archive_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"));
    for (name, data) in entries {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), data)
            .expect("adds");
    }
    writer.finish_into_inner().expect("finishes").1.into_inner()
}

#[derive(Debug, PartialEq, Eq)]
enum Said {
    Total(u64),
    Start(String, u64),
    Progress(u64, u64),
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
    fn on_total(&mut self, total: u64) {
        self.said().push(Said::Total(total));
    }

    fn on_entry_start(&mut self, name: &str, size: u64) {
        self.said().push(Said::Start(name.to_string(), size));
    }

    fn on_progress(&mut self, done: u64, total: u64) -> bool {
        self.said().push(Said::Progress(done, total));
        true
    }

    fn on_entry_complete(&mut self, name: &str, ok: bool) {
        self.said().push(Said::Done(name.to_string(), ok));
    }
}

/// Extraction says what it is about to do, does it, and says how it went.
///
/// Unlike writing, the total is known before anything starts: the archive
/// declares what it holds. So a caller can be given a fraction rather than a
/// running count, and it is given one.
#[test]
fn test_extraction_reports_each_entry_and_the_whole() {
    let entries = vec![
        ("one.bin", payload(1, 40 << 10)),
        ("two.bin", payload(2, 60 << 10)),
    ];
    let archive = archive_of(&entries);
    let out = tempfile::tempdir().expect("tempdir");

    let recorder = Recorder::default();
    let options = ExtractOptions::new().progress(recorder.clone());
    let mut opened = Archive::open(Cursor::new(archive)).expect("opens");
    let result = opened.extract(out.path(), (), &options).expect("extracts");
    assert_eq!(result.entries_extracted, entries.len());

    let said = recorder.said();
    let declared: u64 = entries.iter().map(|(_, d)| d.len() as u64).sum();

    assert_eq!(
        said.first(),
        Some(&Said::Total(declared)),
        "the run began without saying how much there was to do: {said:?}",
    );
    assert!(
        said.contains(&Said::Start("one.bin".into(), 40 << 10)),
        "no entry was announced: {said:?}",
    );
    assert!(
        said.contains(&Said::Done("two.bin".into(), true)),
        "no entry was reported finished: {said:?}",
    );

    let last_progress = said
        .iter()
        .rev()
        .find_map(|s| match s {
            Said::Progress(done, total) => Some((*done, *total)),
            _ => None,
        })
        .expect("progress was reported");
    assert_eq!(
        last_progress,
        (declared, declared),
        "the run ended without having reported all of it done: {said:?}",
    );
}

/// Every entry is announced before it is extracted, not after.
#[test]
fn test_an_entry_is_announced_before_it_is_extracted() {
    let entries = vec![
        ("a.bin", payload(3, 8 << 10)),
        ("b.bin", payload(4, 8 << 10)),
    ];
    let archive = archive_of(&entries);
    let out = tempfile::tempdir().expect("tempdir");

    let recorder = Recorder::default();
    let options = ExtractOptions::new().progress(recorder.clone());
    let mut opened = Archive::open(Cursor::new(archive)).expect("opens");
    let result = opened.extract(out.path(), (), &options).expect("extracts");
    assert_eq!(result.entries_extracted, entries.len());

    let said = recorder.said();
    let order: Vec<&Said> = said
        .iter()
        .filter(|s| matches!(s, Said::Start(..) | Said::Done(..)))
        .collect();

    assert_eq!(
        order,
        vec![
            &Said::Start("a.bin".into(), 8 << 10),
            &Said::Done("a.bin".into(), true),
            &Said::Start("b.bin".into(), 8 << 10),
            &Said::Done("b.bin".into(), true),
        ],
        "entries were not bracketed by their own start and finish",
    );
}

/// A refusal from `on_progress` stops the run.
///
/// It is the documented way to stop, and for a reporter with no state of its
/// own - one built from a closure cannot be given a `should_cancel` - it is the
/// only way. The run stops where it was rather than before it began: what has
/// already been extracted stays.
#[test]
fn test_a_refusal_from_on_progress_stops_the_run() {
    struct Refuser;

    impl ProgressReporter for Refuser {
        fn on_progress(&mut self, _done: u64, _total: u64) -> bool {
            false
        }
    }

    let entries = vec![
        ("a.bin", payload(7, 8 << 10)),
        ("b.bin", payload(8, 8 << 10)),
    ];
    let archive = archive_of(&entries);
    let out = tempfile::tempdir().expect("tempdir");

    let options = ExtractOptions::new().progress(Refuser);
    let mut opened = Archive::open(Cursor::new(archive)).expect("opens");

    match opened.extract(out.path(), (), &options) {
        Err(Error::Cancelled) => {}
        other => panic!("expected the run to be called off, got {other:?}"),
    }
    assert!(
        out.path().join("a.bin").exists(),
        "the run stopped before extracting anything, so what stopped it was \
         not the refusal that follows the first entry",
    );
    assert!(
        !out.path().join("b.bin").exists(),
        "the refusal was noted and the run carried on anyway",
    );
}

/// A reporter that asks to stop is still obeyed.
///
/// This was the one thing the reader did with a reporter, and it has to keep
/// working now that the reporter is reached through a lock.
#[test]
fn test_cancelling_still_stops_the_run() {
    struct Stopper {
        stop: Arc<AtomicBool>,
    }

    impl ProgressReporter for Stopper {
        fn on_entry_start(&mut self, _name: &str, _size: u64) {
            self.stop.store(true, Ordering::Relaxed);
        }

        fn should_cancel(&self) -> bool {
            self.stop.load(Ordering::Relaxed)
        }
    }

    let entries = vec![
        ("a.bin", payload(5, 8 << 10)),
        ("b.bin", payload(6, 8 << 10)),
    ];
    let archive = archive_of(&entries);
    let out = tempfile::tempdir().expect("tempdir");

    let options = ExtractOptions::new().progress(Stopper {
        stop: Arc::new(AtomicBool::new(false)),
    });
    let mut opened = Archive::open(Cursor::new(archive)).expect("opens");

    match opened.extract(out.path(), (), &options) {
        Err(Error::Cancelled) => {}
        other => panic!("expected the run to be called off, got {other:?}"),
    }
}
