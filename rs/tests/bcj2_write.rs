//! BCJ2 separates an executable into four streams so that branch targets,
//! which compress badly interleaved with instructions, end up together where
//! they compress well. The filter itself compresses nothing: three of those
//! four streams still have to go through the codec, and the folder has to say
//! so, or the archive carries them raw and comes out larger than the input.
//!
//! That is exactly what it used to do, and no test noticed, because the tests
//! for BCJ2 all read archives written by 7-Zip. These write ours.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::codec::CodecMethod;
use zesven::read::Archive;
use zesven::write::{WriteOptions, Writer};
use zesven::{ArchivePath, WriteFilter};

/// Something shaped like x86: instruction bytes with frequent calls and jumps
/// to a small set of absolute targets.
///
/// The targets are what BCJ2 exists for. Left in place they differ in every
/// occurrence, because an absolute address depends on where the instruction
/// is; moved into a stream of their own and converted to relative form they
/// repeat, and the codec finds them.
fn executable(len: usize) -> Vec<u8> {
    const TARGETS: [u32; 4] = [0x0040_1000, 0x0040_2000, 0x0040_3000, 0x0040_5000];

    let mut code = Vec::with_capacity(len + 16);
    let mut state = 0x1234_5678u32;
    while code.len() < len {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);

        // A few ordinary instruction bytes.
        for shift in 0..3 {
            code.push((state >> (shift * 8)) as u8);
        }

        // Then a call or a jump to one of the targets, which is the pattern the
        // filter is built around.
        let opcode = if state % 3 == 0 { 0xE8 } else { 0xE9 };
        let target = TARGETS[(state >> 16) as usize % TARGETS.len()];
        let here = code.len() as u32;
        let relative = target.wrapping_sub(here.wrapping_add(5));

        code.push(opcode);
        code.extend_from_slice(&relative.to_le_bytes());
    }
    code.truncate(len);
    code
}

fn write(data: &[u8], filter: WriteFilter) -> Vec<u8> {
    let options = WriteOptions::new().level(5).expect("level").filter(filter);
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options);
    writer
        .add_bytes(ArchivePath::new("code.bin").expect("path"), data)
        .expect("adds");
    let (_result, sink) = writer.finish_into_inner().expect("finishes");
    sink.into_inner()
}

fn read_back(archive: &[u8]) -> Vec<u8> {
    let mut opened = Archive::open(Cursor::new(archive.to_vec())).expect("opens");
    opened.extract_to_vec("code.bin").expect("extracts")
}

/// The filter has to compress. This is the whole point of it.
///
/// Stated against the input rather than against a fixed number, because what
/// it comes to depends on the level. An archive that is not comfortably
/// smaller than what went into it means the streams are being stored, which is
/// what this filter did for as long as it existed.
#[test]
fn test_bcj2_compresses() {
    let data = executable(4 * 1024 * 1024);
    let archive = write(&data, WriteFilter::Bcj2);

    assert!(
        archive.len() < data.len() / 2,
        "BCJ2 produced {} bytes from {}: the streams are not being compressed",
        archive.len(),
        data.len(),
    );
    assert_eq!(read_back(&archive), data);
}

/// And it has to compress better than not filtering at all.
///
/// Otherwise there is no reason for a caller to ask for it. The margin is
/// small on synthetic code and large on real executables; the assertion is
/// only that the filter is not a loss.
#[test]
fn test_bcj2_beats_no_filter_on_code() {
    let data = executable(4 * 1024 * 1024);

    let filtered = write(&data, WriteFilter::Bcj2);
    let plain = write(&data, WriteFilter::None);

    assert!(
        filtered.len() < plain.len(),
        "BCJ2 gave {} bytes where no filter gave {}",
        filtered.len(),
        plain.len(),
    );
}

/// The folder has to name the codec, not just the filter.
///
/// The defect this guards was invisible from the outside: the archive opened,
/// listed and extracted correctly, and was simply enormous. What was missing
/// was in the coder chain, so that is what is checked.
#[test]
fn test_the_bcj2_chain_names_a_codec() {
    let data = executable(1024 * 1024);
    let archive = write(&data, WriteFilter::Bcj2);

    let opened = Archive::open(Cursor::new(archive)).expect("opens");
    let methods = opened.info().compression_methods.clone();

    assert!(
        methods
            .iter()
            .any(|m| matches!(m, CodecMethod::Lzma2 | CodecMethod::Lzma)),
        "the chain names no codec, so the filter's streams are stored raw: {methods:?}",
    );
}

/// Sizes around the edges have to survive, including ones with no branches at
/// all - where two of the four streams are empty.
#[test]
fn test_bcj2_round_trips_at_the_edges() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("one byte", vec![0x90]),
        ("no branches", vec![0x90; 100_000]),
        ("branch at the very end", {
            let mut data = vec![0x90; 1000];
            data.extend_from_slice(&[0xE8, 0x00, 0x10, 0x40, 0x00]);
            data
        }),
        ("truncated branch", {
            let mut data = vec![0x90; 1000];
            data.extend_from_slice(&[0xE8, 0x00]);
            data
        }),
        ("all branches", executable(200_000)),
    ];

    for (name, data) in cases {
        let archive = write(&data, WriteFilter::Bcj2);
        assert_eq!(read_back(&archive), data, "case: {name}");
    }
}

/// An empty entry keeps working: it has no folder at all.
#[test]
fn test_bcj2_accepts_an_empty_entry() {
    let archive = write(&[], WriteFilter::Bcj2);
    let mut opened = Archive::open(Cursor::new(archive)).expect("opens");
    assert_eq!(opened.entries().len(), 1);
    assert_eq!(
        opened.extract_to_vec("code.bin").expect("extracts"),
        Vec::<u8>::new()
    );
}

/// Several BCJ2 entries in one archive have to stay separable.
///
/// Each is its own folder with four packed streams, so the packed streams of
/// one must not be read as belonging to another - which is what the indices
/// written into the header decide.
#[test]
fn test_several_bcj2_entries_keep_their_streams_apart() {
    let first = executable(300_000);
    let second = executable(500_000);
    let third = vec![0x90u8; 50_000];

    let options = WriteOptions::new()
        .level(5)
        .expect("level")
        .filter(WriteFilter::Bcj2);
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .expect("writer")
        .options(options);
    for (name, data) in [("a.bin", &first), ("b.bin", &second), ("c.bin", &third)] {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), data)
            .expect("adds");
    }
    let (_result, sink) = writer.finish_into_inner().expect("finishes");

    let mut opened = Archive::open(Cursor::new(sink.into_inner())).expect("opens");
    assert_eq!(opened.extract_to_vec("a.bin").expect("extracts"), first);
    assert_eq!(opened.extract_to_vec("b.bin").expect("extracts"), second);
    assert_eq!(opened.extract_to_vec("c.bin").expect("extracts"), third);
}
