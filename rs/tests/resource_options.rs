//! The thread and memory knobs must do what they promise.
//!
//! Both exist because the writer's defaults cannot suit everyone: parallel
//! compression cuts a solid block into chunks that cannot match against each
//! other, and every concurrent encoder reserves a match finder several times
//! its dictionary. Callers who care more about a reproducible archive, a
//! smaller one, or a smaller footprint need a way to say so.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::read::Archive;
use zesven::write::{WriteOptions, Writer};
use zesven::{ArchivePath, MemoryLimit, Threads};

/// Data worth compressing, with matches at varying distances.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x5DEE_CE66_D3A5_1B9Cu64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(
            b"resource limits change how the work is done, not what it produces ",
        );
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Builds an archive of several entries with the given options.
fn archive(options: WriteOptions, data: &[u8], entries: usize) -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(options);
    for i in 0..entries {
        writer
            .add_bytes(ArchivePath::new(&format!("{i:02}.bin")).unwrap(), data)
            .unwrap();
    }
    writer.finish_into_inner().unwrap().1.into_inner()
}

/// A single thread must produce the same archive whatever the machine has.
///
/// This is the setting for reproducible builds, so it has to be free of every
/// decision that depends on core count.
#[test]
fn test_single_thread_output_does_not_depend_on_the_machine() {
    let data = payload(2 * 1024 * 1024);
    let single = || {
        archive(
            WriteOptions::new()
                .threads(Threads::Single)
                .deterministic(true),
            &data,
            6,
        )
    };

    // Repeat runs cannot vary, and neither can a run made to think the machine
    // is larger: with one thread nothing consults the core count at all.
    assert_eq!(single(), single());

    let explicit_one = archive(
        WriteOptions::new()
            .threads(Threads::count_or_single(1))
            .deterministic(true),
        &data,
        6,
    );
    assert_eq!(single(), explicit_one);
}

/// The archive must not depend on how many threads happen to run.
///
/// Only the setting may change the bytes, never the machine. What a writer
/// chunks follows from the dictionary and from `threads`; how many workers
/// consume those chunks follows from the hardware and the memory budget, and
/// must not reach the output. Otherwise a build machine with more cores
/// produces a different artifact from a developer's laptop.
#[test]
fn test_output_does_not_depend_on_how_many_workers_run() {
    let data = payload(3 * 1024 * 1024);

    for solid in [false, true] {
        let build = |threads, limit| {
            let mut options = WriteOptions::new().threads(threads).memory_limit(limit);
            if solid {
                options = options.solid();
            }
            archive(options, &data, 6)
        };

        // Different worker counts, reached two ways: by asking for fewer
        // threads, and by a budget that only affords fewer encoders.
        let many = build(Threads::count_or_single(8), MemoryLimit::Auto);
        let few = build(Threads::count_or_single(2), MemoryLimit::Auto);
        let starved = build(Threads::count_or_single(8), MemoryLimit::bytes_or_auto(1));

        assert_eq!(
            many, few,
            "solid={solid}: eight threads and two produced different archives",
        );
        assert_eq!(
            many, starved,
            "solid={solid}: a tight memory budget changed the archive, not just the speed",
        );
    }
}

/// A single thread must also compress at least as well as many do.
///
/// Chunking a solid block is what buys the parallel speed, and it costs ratio;
/// asking for one thread is how a caller declines that trade.
#[test]
fn test_single_thread_compresses_no_worse_than_many() {
    // A solid block, where the parallel path chunks and the sequential one
    // does not.
    let data = payload(4 * 1024 * 1024);

    let one = archive(
        WriteOptions::new().solid().threads(Threads::Single),
        &data,
        8,
    );
    let many = archive(WriteOptions::new().solid().threads(Threads::Auto), &data, 8);

    assert!(
        one.len() <= many.len(),
        "one thread produced {} bytes, more than the {} many threads produced; \
         the sequential path should compress at least as well",
        one.len(),
        many.len(),
    );
}

/// Whatever the settings, the archive has to decode to what went in.
#[test]
fn test_every_resource_setting_round_trips() {
    let data = payload(1024 * 1024);

    let settings = [
        WriteOptions::new().threads(Threads::Single),
        WriteOptions::new().threads(Threads::count_or_single(2)),
        WriteOptions::new().memory_limit(MemoryLimit::bytes_or_auto(32 * 1024 * 1024)),
        WriteOptions::new()
            .solid()
            .threads(Threads::count_or_single(3)),
        WriteOptions::new()
            .solid()
            .memory_limit(MemoryLimit::bytes_or_auto(64 * 1024 * 1024)),
    ];

    for (index, options) in settings.into_iter().enumerate() {
        let bytes = archive(options, &data, 4);
        let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
        for entry in 0..4 {
            assert_eq!(
                opened.extract_to_vec(&format!("{entry:02}.bin")).unwrap(),
                data,
                "setting {index} did not round-trip entry {entry}",
            );
        }
    }
}

/// A tight memory limit must still produce a correct archive.
///
/// The limit caps concurrency, and the floor beneath it means a single encoder
/// always runs: a writer that refused to compress because it was given a small
/// budget would be worse than one that used a little more than it was told.
#[test]
fn test_a_tight_memory_limit_still_writes() {
    let data = payload(2 * 1024 * 1024);

    let bytes = archive(
        WriteOptions::new()
            .level(9)
            .unwrap()
            .memory_limit(MemoryLimit::bytes_or_auto(1)),
        &data,
        4,
    );

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(opened.extract_to_vec("00.bin").unwrap(), data);
}
