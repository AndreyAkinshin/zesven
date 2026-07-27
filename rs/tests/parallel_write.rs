//! Entries are compressed in batches so that independent folders can be built
//! at the same time. Batching is only sound if what comes out the far end is
//! indistinguishable from having compressed them one at a time: same entries,
//! same order, same bytes.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::ArchivePath;
use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, Writer};

/// Data worth compressing, so entries take a real trip through the codec.
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"batched entry payload, repeated for the matcher ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Entries must appear in the order they were added, whatever kind they are.
///
/// Directories and anti-items are recorded the moment they are added, while
/// file entries wait in the batch, so adding one kind after the other is what
/// lets a batched file overtake the entry that was added before it.
#[test]
fn test_entry_order_survives_batching() {
    let data = payload(1, 4096);

    let mut writer = Writer::create(Cursor::new(Vec::new())).unwrap();
    writer
        .add_bytes(ArchivePath::new("01-file.bin").unwrap(), &data)
        .unwrap();
    writer
        .add_directory(ArchivePath::new("02-dir").unwrap(), EntryMeta::directory())
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("03-file.bin").unwrap(), &data)
        .unwrap();
    writer
        .add_anti_item(ArchivePath::new("04-anti.bin").unwrap())
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("05-file.bin").unwrap(), &data)
        .unwrap();
    writer
        .add_anti_directory(ArchivePath::new("06-anti-dir").unwrap())
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("07-file.bin").unwrap(), &data)
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().unwrap();

    let archive = Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    let paths: Vec<&str> = archive.entries().iter().map(|e| e.path.as_str()).collect();

    assert_eq!(
        paths,
        vec![
            "01-file.bin",
            "02-dir",
            "03-file.bin",
            "04-anti.bin",
            "05-file.bin",
            "06-anti-dir",
            "07-file.bin",
        ],
    );
}

/// Every entry must still decode to what was put in, across a batch boundary.
///
/// The batch is flushed once it holds enough data, so this writes more than one
/// batch worth and checks the entries either side of the boundary.
#[test]
fn test_entries_round_trip_across_batch_boundaries() {
    // The batch flushes at 64 MiB; 10 MiB apiece crosses that twice.
    const ENTRY: usize = 10 * 1024 * 1024;
    const COUNT: usize = 14;

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    for i in 0..COUNT {
        writer
            .add_bytes(
                ArchivePath::new(&format!("{i:02}.bin")).unwrap(),
                &payload(i as u64, ENTRY),
            )
            .unwrap();
    }
    let (_result, cursor) = writer.finish_into_inner().unwrap();

    let mut archive = Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    assert_eq!(archive.entries().len(), COUNT);
    for i in 0..COUNT {
        let extracted = archive.extract_to_vec(&format!("{i:02}.bin")).unwrap();
        assert_eq!(
            extracted,
            payload(i as u64, ENTRY),
            "entry {i} did not round-trip",
        );
    }
}

/// Empty entries carry no stream, and must not shift the ones that do.
///
/// An empty entry is recorded in the file list but never becomes a folder, so
/// mixing them in is what exposes a folder index counted against the wrong list.
#[test]
fn test_empty_entries_keep_folders_aligned() {
    let first = payload(1, 8192);
    let second = payload(2, 8192);

    let mut writer = Writer::create(Cursor::new(Vec::new())).unwrap();
    writer
        .add_bytes(ArchivePath::new("empty-a.bin").unwrap(), b"")
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("first.bin").unwrap(), &first)
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("empty-b.bin").unwrap(), b"")
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("second.bin").unwrap(), &second)
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("empty-c.bin").unwrap(), b"")
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().unwrap();

    let mut archive = Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    let paths: Vec<String> = archive
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    assert_eq!(
        paths,
        vec![
            "empty-a.bin",
            "first.bin",
            "empty-b.bin",
            "second.bin",
            "empty-c.bin",
        ],
    );

    assert_eq!(archive.extract_to_vec("first.bin").unwrap(), first);
    assert_eq!(archive.extract_to_vec("second.bin").unwrap(), second);
    assert!(archive.extract_to_vec("empty-b.bin").unwrap().is_empty());
}

/// The same input must produce the same archive every time.
///
/// Entries are handed to whichever worker is free, so a writer that recorded
/// results in completion order rather than input order would still produce a
/// readable archive - a different one on each run.
#[test]
fn test_batched_output_is_deterministic() {
    let build = || {
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(WriteOptions::new().level(1).unwrap().deterministic(true));
        for i in 0..24u64 {
            writer
                .add_bytes(
                    ArchivePath::new(&format!("{i:02}.bin")).unwrap(),
                    &payload(i, 256 * 1024),
                )
                .unwrap();
        }
        writer.finish_into_inner().unwrap().1.into_inner()
    };

    assert_eq!(
        build(),
        build(),
        "the same entries produced two different archives",
    );
}
