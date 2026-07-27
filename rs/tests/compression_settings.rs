//! The settings an archive declares must be the settings it was written with.
//!
//! A 7z reader allocates its dictionary from what the header declares, so a
//! header that names a different dictionary than the encoder used is both a
//! lie and a memory cost. These settings used to be derived twice, once when
//! compressing and once when writing the header, from formulas that did not
//! agree.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::ArchivePath;
use zesven::codec::CodecMethod;
use zesven::codec::lzma::{decode_lzma2_dict_size, dict_size_covering, preset_dict_size};
use zesven::format::parser::read_archive_header;
use zesven::write::{WriteOptions, Writer};

/// Enough data that the smaller levels are capped by their level rather than
/// by the payload, and the larger ones the other way round. Both directions of
/// the cap are therefore exercised by one payload.
const PAYLOAD: usize = 2 * 1024 * 1024;

/// Builds an archive of one entry and returns the dictionary its header declares.
fn declared_dictionary(level: u32, method: CodecMethod, data: &[u8]) -> u32 {
    let options = WriteOptions::new().level(level).unwrap().method(method);
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(options);
    writer
        .add_bytes(ArchivePath::new("entry.bin").unwrap(), data)
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().unwrap();

    let mut archive = Cursor::new(cursor.into_inner());
    let (_start, header) = read_archive_header(&mut archive, None).unwrap();
    let folders = header.folders();
    assert_eq!(folders.len(), 1, "expected exactly one folder");

    let properties = folders[0].coders[0]
        .properties
        .as_ref()
        .expect("the codec declares a dictionary");

    match method {
        CodecMethod::Lzma2 => decode_lzma2_dict_size(properties[0]).unwrap(),
        CodecMethod::Lzma => u32::from_le_bytes(properties[1..5].try_into().unwrap()),
        other => panic!("unexpected method {other:?}"),
    }
}

/// Data that is worth compressing, so the writer keeps a real coder chain.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x1234_5678_9abc_def0u64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"the quick brown fox jumps over the lazy dog ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// The declared dictionary is the level's, capped by the size of the data.
///
/// Both caps used to be wrong at once. The encoder clamped itself to 8 MiB
/// while the header declared up to 32 MiB from an unclamped copy of the same
/// formula, and neither side looked at how much data there actually was, so a
/// 2 MiB entry at level 9 told every reader to reserve 32 MiB.
#[test]
fn test_declared_dictionary_is_the_level_capped_by_the_data() {
    let data = payload(PAYLOAD);

    for level in 0..=9u32 {
        let expected = preset_dict_size(level).min(dict_size_covering(data.len() as u64));
        assert_eq!(
            declared_dictionary(level, CodecMethod::Lzma2, &data),
            expected,
            "level {level} declares a dictionary it did not compress with",
        );
    }
}

/// The same invariant holds for LZMA1, which declares its dictionary outright.
#[cfg(feature = "lzma")]
#[test]
fn test_lzma1_declares_the_dictionary_it_used() {
    let data = payload(PAYLOAD);

    for level in 0..=9u32 {
        let expected = preset_dict_size(level).min(dict_size_covering(data.len() as u64));
        assert_eq!(
            declared_dictionary(level, CodecMethod::Lzma, &data),
            expected,
            "level {level} declares a dictionary it did not compress with",
        );
    }
}

/// The compression level has to reach the encoder, not just the header.
///
/// The level used to pick a dictionary and nothing else: the match finder and
/// encoder mode were pinned to preset 6 whatever the caller asked for, so
/// requesting the fastest level bought no speed at all.
#[test]
fn test_level_changes_the_encoded_output() {
    let data = payload(PAYLOAD);

    let sizes: Vec<usize> = [1u32, 9]
        .iter()
        .map(|&level| {
            let options = WriteOptions::new().level(level).unwrap();
            let mut writer = Writer::create(Cursor::new(Vec::new()))
                .unwrap()
                .options(options);
            writer
                .add_bytes(ArchivePath::new("entry.bin").unwrap(), &data)
                .unwrap();
            let (_result, cursor) = writer.finish_into_inner().unwrap();
            cursor.into_inner().len()
        })
        .collect();

    assert!(
        sizes[1] < sizes[0],
        "level 9 produced {} bytes and level 1 produced {}; the level is not reaching the encoder",
        sizes[1],
        sizes[0],
    );
}

/// Data large enough to be split across chunks must still round-trip.
///
/// Level 1 uses a 1 MiB dictionary and therefore 4 MiB chunks, so 12 MiB is
/// several chunks and reaches the multi-threaded encoder.
#[cfg(feature = "parallel")]
#[test]
fn test_chunked_stream_round_trips() {
    let data = payload(12 * 1024 * 1024);

    let options = WriteOptions::new().level(1).unwrap();
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(options);
    writer
        .add_bytes(ArchivePath::new("big.bin").unwrap(), &data)
        .unwrap();
    let (_result, cursor) = writer.finish_into_inner().unwrap();

    let mut archive = zesven::read::Archive::open(Cursor::new(cursor.into_inner())).unwrap();
    assert_eq!(archive.extract_to_vec("big.bin").unwrap(), data);
}

/// The same input must produce the same archive every time.
///
/// Work is handed to whichever worker is free, so a run that assembled chunks
/// in completion order rather than input order would still decode - into the
/// wrong bytes - and would do so only sometimes.
#[cfg(feature = "parallel")]
#[test]
fn test_chunked_output_is_deterministic() {
    let data = payload(12 * 1024 * 1024);

    let archive_bytes = || {
        let options = WriteOptions::new().level(1).unwrap();
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(options);
        writer
            .add_bytes(ArchivePath::new("big.bin").unwrap(), &data)
            .unwrap();
        writer.finish_into_inner().unwrap().1.into_inner()
    };

    assert_eq!(
        archive_bytes(),
        archive_bytes(),
        "the same input produced two different archives",
    );
}

/// Whatever the header declares has to be enough to decode the archive.
#[test]
fn test_every_level_round_trips() {
    let data = payload(256 * 1024);

    for level in 0..=9u32 {
        let options = WriteOptions::new().level(level).unwrap();
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(options);
        writer
            .add_bytes(ArchivePath::new("entry.bin").unwrap(), &data)
            .unwrap();
        let (_result, cursor) = writer.finish_into_inner().unwrap();

        let mut archive = zesven::read::Archive::open(Cursor::new(cursor.into_inner())).unwrap();
        let extracted = archive.extract_to_vec("entry.bin").unwrap();
        assert_eq!(extracted, data, "level {level} did not round-trip");
    }
}
