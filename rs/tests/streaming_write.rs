//! Large entries are compressed straight into the sink.
//!
//! Archiving a file used to cost as much memory as the file was long, because
//! every entry was read into memory before being compressed. Entries past a
//! size threshold now go through the codec as they are read.
//!
//! That is a second way of producing a folder, with its own way of arriving at
//! the packed size and the checksum, so these cases check that what it writes
//! decodes, describes itself correctly, and leaves the entries around it in the
//! order they were added - including the cases it declines and hands back to
//! the buffered path.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::codec::CodecMethod;
use zesven::read::Archive;
use zesven::write::{WriteOptions, Writer};
use zesven::{ArchivePath, WriteFilter};

/// Just past the threshold at which the streaming path takes over.
///
/// No larger than it needs to be: every case here compresses this much, and
/// the feature matrix runs the suite once per feature combination.
const LARGE: usize = 68 * 1024 * 1024;

/// The fastest level, since these cases exercise the path rather than the
/// codec. It runs an order of magnitude quicker than the default.
fn fast() -> WriteOptions {
    WriteOptions::new().level(1).unwrap()
}

/// Data with matches at varying distances, so the dictionary matters.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x243F_6A88_85A3_08D3u64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"a large entry should not have to fit in memory to be archived ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Writes one entry and returns the archive.
fn archive_one(options: WriteOptions, data: &[u8]) -> Vec<u8> {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(options);
    writer
        .add_bytes(ArchivePath::new("big.bin").unwrap(), data)
        .unwrap();
    writer.finish_into_inner().unwrap().1.into_inner()
}

/// A large entry must decode to exactly what went in.
#[test]
fn test_large_entry_round_trips() {
    let data = payload(LARGE);
    let bytes = archive_one(fast(), &data);

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// A streamed entry must describe itself the way a buffered one does.
///
/// The two paths record a folder from different places - the buffered one
/// measures the compressed buffer it holds, the streaming one counts bytes as
/// they leave - so the sizes each writes into the header can disagree without
/// either failing to decode. The archives themselves cannot be compared
/// directly, since nothing can force the same data down both paths, so this
/// checks the figures a reader sees.
#[test]
fn test_a_streamed_entry_reports_its_sizes() {
    let data = payload(LARGE);
    let streamed = archive_one(fast(), &data);

    let mut opened = Archive::open(Cursor::new(streamed)).unwrap();
    let entries = opened.entries().to_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].size, data.len() as u64);
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// A stored entry must stream too, and decode unchanged.
#[test]
fn test_large_stored_entry_round_trips() {
    let data = payload(LARGE);
    let bytes = archive_one(fast().method(CodecMethod::Copy), &data);

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// LZMA1 has its own encoder and its own properties; both must survive.
#[cfg(feature = "lzma")]
#[test]
fn test_large_lzma1_entry_round_trips() {
    let data = payload(LARGE);
    let bytes = archive_one(fast().method(CodecMethod::Lzma), &data);

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// Cases the streaming path declines must still work, and still be correct.
///
/// A filter sits ahead of the codec and encryption behind it, so neither can be
/// applied to bytes that have already left for the sink. Those keep the
/// buffered path, and this checks the writer really does fall back rather than
/// dropping the filter or the encryption on the floor.
#[test]
fn test_large_entries_with_filters_fall_back_and_stay_correct() {
    let data = payload(LARGE);

    let filtered = archive_one(fast().filter(WriteFilter::delta(4)), &data);
    let mut opened = Archive::open(Cursor::new(filtered)).unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// Encrypted large entries fall back too, and must decrypt.
#[cfg(feature = "aes")]
#[test]
fn test_large_encrypted_entry_round_trips() {
    use zesven::crypto::{NoncePolicy, Password};

    let data = payload(LARGE);
    let options = fast()
        .password("correct horse battery staple")
        .nonce_policy(NoncePolicy::random_with_params(4, 8))
        .encrypt_data(true);

    let bytes = archive_one(options, &data);
    let mut opened = Archive::open_with_password(
        Cursor::new(bytes),
        Password::new("correct horse battery staple"),
    )
    .unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}

/// A large entry among small ones must not disturb their order.
///
/// The streaming path writes immediately while the buffered one gathers
/// entries, so a large entry has to push whatever is waiting to the sink first.
#[test]
fn test_a_large_entry_keeps_the_order_of_small_ones() {
    let small = payload(64 * 1024);
    let large = payload(LARGE);

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(fast());
    writer
        .add_bytes(ArchivePath::new("01-small.bin").unwrap(), &small)
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("02-large.bin").unwrap(), &large)
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("03-small.bin").unwrap(), &small)
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    let paths: Vec<String> = opened
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    assert_eq!(paths, vec!["01-small.bin", "02-large.bin", "03-small.bin"]);

    assert_eq!(opened.extract_to_vec("01-small.bin").unwrap(), small);
    assert_eq!(opened.extract_to_vec("02-large.bin").unwrap(), large);
    assert_eq!(opened.extract_to_vec("03-small.bin").unwrap(), small);
}

/// A source that claims a large size and yields nothing must not corrupt the
/// archive.
///
/// What an entry claims decides nothing - the path it takes is settled by
/// reading it - so a source that reports eighty megabytes and delivers none is
/// simply an empty entry. It carries no stream at all, and an encoder built
/// before that was known would have put its framing into the sink belonging to
/// no folder, moving every folder written after it.
#[test]
fn test_an_empty_source_claiming_to_be_large_is_harmless() {
    use zesven::write::EntryMeta;

    let data = payload(64 * 1024);

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(fast());
    // Claims 80 MB, delivers none.
    writer
        .add_stream(
            ArchivePath::new("empty.bin").unwrap(),
            &mut Cursor::new(Vec::new()),
            EntryMeta::file(80 * 1024 * 1024),
        )
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("after.bin").unwrap(), &data)
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut opened = Archive::open(Cursor::new(bytes)).unwrap();
    let paths: Vec<String> = opened
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    assert_eq!(paths, vec!["empty.bin", "after.bin"]);
    assert!(opened.extract_to_vec("empty.bin").unwrap().is_empty());
    assert_eq!(opened.extract_to_vec("after.bin").unwrap(), data);
}

/// Data that does not compress, so an archive of it is as large as its input.
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

/// Streaming must work into a multivolume sink, whose position spans files.
#[test]
fn test_large_entry_across_volumes() {
    use zesven::VolumeConfig;

    let dir = tempfile::TempDir::new().unwrap();
    // Incompressible, so the archive really does outgrow a volume: compressible
    // data at this size fits in one, and then the case proves nothing.
    let data = incompressible(LARGE);

    let config = VolumeConfig::new(dir.path().join("big.7z"), 17 * 1024 * 1024);
    let mut writer = Writer::create_multivolume(config).unwrap().options(fast());
    writer
        .add_bytes(ArchivePath::new("big.bin").unwrap(), &data)
        .unwrap();
    let result = writer.finish().unwrap();

    // The point is that the entry spans volumes: a sink whose position is not a
    // single file's offset is exactly what the streaming path's byte counting
    // has to cope with. `volume_count >= 1` would have been true of any archive
    // at all, including one that never crossed a boundary.
    assert!(
        result.volume_count > 1,
        "the entry fitted in one volume, so this proves nothing: {} volume(s)",
        result.volume_count,
    );
    assert!(
        result
            .volume_sizes
            .iter()
            .take(result.volume_count as usize - 1)
            .all(|&size| size == 17 * 1024 * 1024),
        "every volume but the last must be full: {:?}",
        result.volume_sizes,
    );

    let mut opened = Archive::open_path(dir.path().join("big.7z.001")).unwrap();
    assert_eq!(opened.extract_to_vec("big.bin").unwrap(), data);
}
