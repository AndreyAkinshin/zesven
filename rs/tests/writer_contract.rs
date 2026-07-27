//! What every writer must do, whatever it is writing into.
//!
//! Each round of review here found the same shape of defect twice: something
//! true of one sink but not another, or of one writer but not its twin. A
//! buffered sink was left unflushed while a `Cursor` was fine. A size was
//! measured from a position that only meant what it looked like when the
//! archive began at nought. The async writer reported figures the blocking one
//! did not, for the same entries.
//!
//! So rather than a case per defect, this is a small set of properties checked
//! across every sink and both writers: the archive is readable *from where it
//! actually lives*, and the result describes what was really written. A new
//! sink or a new field is then covered by construction rather than by someone
//! remembering to add a case.

#![cfg(feature = "lzma2")]

use std::fs;
use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};

use tempfile::TempDir;
use zesven::ArchivePath;
use zesven::codec::CodecMethod;
use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, WriteResult, Writer};

/// The entries every scenario writes, and expects back.
fn entries() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("text.txt", b"the same bytes out as went in\n".repeat(40)),
        ("empty.bin", Vec::new()),
        (
            "binary.bin",
            (0u8..=255).cycle().take(300_000).collect::<Vec<u8>>(),
        ),
    ]
}

/// Writes the standard entries, and returns what the writer reported.
fn write_entries<W: Write + Seek + Send>(sink: W) -> (WriteResult, W) {
    let mut writer = Writer::create(sink)
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    for (path, data) in entries() {
        writer
            .add_bytes(ArchivePath::new(path).unwrap(), &data)
            .unwrap();
    }
    writer
        .add_directory(ArchivePath::new("dir").unwrap(), EntryMeta::directory())
        .unwrap();
    writer.finish_into_inner().unwrap()
}

/// Asserts an archive at `bytes` holds exactly what was written into it.
fn assert_holds_the_entries(bytes: Vec<u8>, context: &str) {
    let mut archive = Archive::open(Cursor::new(bytes))
        .unwrap_or_else(|e| panic!("{context}: the archive does not open: {e}"));
    for (path, data) in entries() {
        let got = archive
            .extract_to_vec(path)
            .unwrap_or_else(|e| panic!("{context}: {path} did not extract: {e}"));
        assert_eq!(got, data, "{context}: {path} came back different");
    }
}

/// The reported size must be the length of the archive that was written.
fn assert_reports_its_length(result: &WriteResult, written: u64, context: &str) {
    assert_eq!(result.volume_count, 1, "{context}: wrong volume count");
    assert_eq!(
        result.volume_sizes,
        vec![written],
        "{context}: the result claims a length the archive does not have",
    );
}

/// An in-memory sink: the easy case, and the one every other test used.
#[test]
fn test_writing_to_a_cursor() {
    let (result, sink) = write_entries(Cursor::new(Vec::new()));
    let bytes = sink.into_inner();

    assert_reports_its_length(&result, bytes.len() as u64, "cursor");
    assert_holds_the_entries(bytes, "cursor");
}

/// A buffered file: the sink `create_path` uses, and the one that hid a defect.
///
/// The signature header is written last, by seeking back over the start of the
/// archive, so a writer that hands the sink back without flushing leaves a file
/// whose first 32 bytes are still in the buffer. `Drop` would flush it and
/// discard any error, so the file was correct only if nothing went wrong and
/// only after the caller let go of it.
#[test]
fn test_writing_to_a_buffered_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("archive.7z");

    let file = fs::File::create(&path).unwrap();
    let (result, _sink) = write_entries(BufWriter::new(file));

    // Read before the sink is dropped: everything must already be on disk.
    let bytes = fs::read(&path).unwrap();
    assert_reports_its_length(&result, bytes.len() as u64, "buffered file");
    assert_holds_the_entries(bytes, "buffered file");
}

/// A sink that already holds something, positioned at the end of it.
///
/// The archive then does not begin at nought, which every offset in the header
/// is relative to - and which the signature header is written back over.
#[test]
fn test_writing_after_a_prefix() {
    let prefix = b"a stub, a manifest, anything at all".repeat(10);

    let mut sink = Cursor::new(prefix.clone());
    sink.seek(SeekFrom::End(0)).unwrap();
    let (result, sink) = write_entries(sink);

    let whole = sink.into_inner();
    let archive = whole[prefix.len()..].to_vec();

    assert_eq!(
        &whole[..prefix.len()],
        &prefix[..],
        "the prefix was overwritten",
    );
    assert_reports_its_length(&result, archive.len() as u64, "after a prefix");
    assert_holds_the_entries(archive, "after a prefix");
}

/// The reported size is the archive's, not the sink's.
///
/// Writing into a sink that already had bytes past the end of the archive left
/// them there; the result describes what was written, which is what a caller
/// splitting or uploading the archive needs.
#[test]
fn test_the_reported_length_is_the_archives() {
    let (result, sink) = write_entries(Cursor::new(vec![0xaa; 4096]));
    let whole = sink.into_inner();
    let claimed = result.volume_sizes[0] as usize;

    assert!(
        claimed <= whole.len(),
        "the archive cannot be longer than the sink it went into",
    );
    assert_holds_the_entries(whole[..claimed].to_vec(), "reported length");
}

/// Multi-volume output must report the volumes it actually wrote.
#[test]
fn test_writing_across_volumes() {
    use zesven::VolumeConfig;

    let dir = TempDir::new().unwrap();
    let config = VolumeConfig::new(dir.path().join("archive.7z"), 64 * 1024);

    // Incompressible, so the archive really does outgrow a volume: the shared
    // entries compress to well under one.
    let mut payload = vec![0u8; 300_000];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for byte in payload.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }

    let mut writer = Writer::create_multivolume(config)
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("payload.bin").unwrap(), &payload)
        .unwrap();
    let result = writer.finish().unwrap();

    assert!(result.volume_count > 1, "the entries fitted in one volume");
    assert_eq!(
        result.volume_sizes.len(),
        result.volume_count as usize,
        "a size for every volume",
    );

    for (i, size) in result.volume_sizes.iter().enumerate() {
        let path = dir.path().join(format!("archive.7z.{:03}", i + 1));
        let actual = fs::metadata(&path)
            .unwrap_or_else(|e| panic!("volume {} is missing: {e}", i + 1))
            .len();
        assert_eq!(
            *size,
            actual,
            "volume {} is {actual} bytes, reported as {size}",
            i + 1,
        );
    }

    let mut archive = Archive::open_path(dir.path().join("archive.7z.001")).unwrap();
    assert_eq!(archive.extract_to_vec("payload.bin").unwrap(), payload);
}

/// An entry is written with the options it was accepted under.
///
/// Small entries are buffered and compressed later, and the options can be
/// replaced in between. Reading the live options at that point applied
/// settings the entry was never offered - most seriously, a file added with
/// encryption requested went into the archive in the clear once the options
/// were switched to an unencrypted method.
#[cfg(feature = "aes")]
#[test]
fn test_an_entry_is_written_with_the_options_it_was_accepted_under() {
    use zesven::crypto::Password;

    const PASSWORD: &str = "correct horse battery staple";

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().password(PASSWORD).encrypt_data(true));
    writer
        .add_bytes(ArchivePath::new("secret.txt").unwrap(), b"TOP SECRET")
        .unwrap();

    // Small enough to still be waiting when the options change.
    writer = writer.options(WriteOptions::new().method(CodecMethod::Copy));
    writer
        .add_bytes(ArchivePath::new("plain.txt").unwrap(), b"PLAIN")
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut without = Archive::open(Cursor::new(bytes.clone())).unwrap();
    assert!(
        without.extract_to_vec("secret.txt").is_err(),
        "the entry was encrypted when accepted and came back without a password",
    );
    assert_eq!(
        without.extract_to_vec("plain.txt").unwrap(),
        b"PLAIN",
        "the entry accepted without encryption should need no password",
    );

    let mut with =
        Archive::open_with_password(Cursor::new(bytes), Password::new(PASSWORD)).unwrap();
    assert_eq!(with.extract_to_vec("secret.txt").unwrap(), b"TOP SECRET");
}

/// Options that cannot describe what is already buffered must not reach it.
///
/// BCJ2 writes a folder of four streams for one entry, so applying it to
/// entries accepted under an ordinary method produced an archive this crate's
/// own reader rejects - from a `finish` that reported success.
#[test]
fn test_changing_the_filter_does_not_reshape_buffered_entries() {
    use zesven::WriteFilter;

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new());
    writer
        .add_bytes(
            ArchivePath::new("small.bin").unwrap(),
            b"SMALL DATA".repeat(10).as_slice(),
        )
        .unwrap();

    writer = writer.options(WriteOptions::new().filter(WriteFilter::Bcj2));
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut archive = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(
        archive.extract_to_vec("small.bin").unwrap().len(),
        100,
        "the buffered entry was reshaped by a filter set after it was accepted",
    );
}

/// Entries come back in the order they were added, whatever path they took.
///
/// A solid archive buffers its files while directories and removals are
/// recorded immediately, so an entry that skipped the buffer used to overtake
/// the ones still in it.
#[test]
fn test_entries_keep_their_order_across_the_buffers() {
    for options in [WriteOptions::new(), WriteOptions::new().solid()] {
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(options);
        writer
            .add_bytes(ArchivePath::new("a.txt").unwrap(), b"A")
            .unwrap();
        writer
            .add_directory(ArchivePath::new("dir").unwrap(), EntryMeta::directory())
            .unwrap();
        writer
            .add_anti_item(ArchivePath::new("gone.txt").unwrap())
            .unwrap();
        writer
            .add_bytes(ArchivePath::new("z.txt").unwrap(), b"Z")
            .unwrap();
        let bytes = writer.finish_into_inner().unwrap().1.into_inner();

        let archive = Archive::open(Cursor::new(bytes)).unwrap();
        let paths: Vec<String> = archive
            .entries()
            .iter()
            .map(|e| e.path.as_str().to_string())
            .collect();
        assert_eq!(paths, vec!["a.txt", "dir", "gone.txt", "z.txt"]);
    }
}

/// A method this build cannot use is refused before an entry is taken.
///
/// It used to be discovered when the buffer was compressed, by which point the
/// caller had been told the entry was accepted - and the batch it was in was
/// dropped, leaving an archive that opened fine and was missing a file.
#[test]
fn test_an_unavailable_method_is_refused_up_front() {
    let unavailable = [
        CodecMethod::Zstd,
        CodecMethod::Brotli,
        CodecMethod::Lz4,
        CodecMethod::PPMd,
    ]
    .into_iter()
    .find(|m| !m.is_available());

    let Some(method) = unavailable else {
        return; // Every codec is compiled in; nothing to check here.
    };

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().method(method));
    assert!(
        writer
            .add_bytes(ArchivePath::new("a.bin").unwrap(), b"DATA")
            .is_err(),
        "{method:?} is not available in this build and the entry was accepted anyway",
    );
}

/// A buffer that empties without writing a folder carries nothing forward.
///
/// A solid block of nothing but empty files has no stream to store, so it
/// leaves through a path of its own. While the options a buffer belonged to
/// were kept beside it rather than in it, that path skipped clearing them and
/// the *next* entry was written under settings belonging to entries already
/// gone - encryption among them.
#[cfg(feature = "aes")]
#[test]
fn test_an_emptied_buffer_carries_no_settings_forward() {
    use zesven::write::SolidOptions;

    let solid = || SolidOptions::enabled().files_per_block(1);

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().solid_options(solid()));
    writer
        .add_bytes(ArchivePath::new("empty1.txt").unwrap(), b"")
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("empty2.txt").unwrap(), b"")
        .unwrap();

    writer = writer.options(
        WriteOptions::new()
            .solid_options(solid())
            .password("correct horse battery staple")
            .encrypt_data(true),
    );
    writer
        .add_bytes(ArchivePath::new("secret.txt").unwrap(), b"TOP SECRET")
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut archive = Archive::open(Cursor::new(bytes)).unwrap();
    assert!(
        archive.extract_to_vec("secret.txt").is_err(),
        "the entry was accepted under encryption and came back without a password",
    );
}

/// The whole of an entry's options survives the wait, not merely the method.
///
/// Encryption draws a salt and an IV, and the policy that produces them is part
/// of what an entry was accepted under. Reading the live policy instead lost a
/// deterministic one set alongside the entry, so the same programme run twice
/// produced different archives.
///
/// The options are changed once more *after* the entry, and never used again:
/// that is what makes the two implementations differ. Written the earlier way,
/// the flush at `finish` would draw its nonce from the random policy set last
/// and the two runs would not match. Solid mode keeps the entry in the buffer
/// whatever the machine's core count, so the flush really does happen at
/// `finish`.
#[cfg(feature = "aes")]
#[test]
fn test_a_buffered_entry_keeps_the_whole_of_its_options() {
    use zesven::crypto::NoncePolicy;
    use zesven::write::SolidOptions;

    let build = || {
        let encrypted = |policy: NoncePolicy| {
            WriteOptions::new()
                .level(1)
                .unwrap()
                .solid_options(SolidOptions::enabled())
                .password("correct horse battery staple")
                .nonce_policy(policy)
                .encrypt_data(true)
        };

        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(encrypted(NoncePolicy::Deterministic {
                num_cycles_power: 4,
                seed: [42u8; 32],
            }));
        writer
            .add_bytes(ArchivePath::new("a.bin").unwrap(), b"BUFFERED")
            .unwrap();

        // Replaced after the entry was accepted, and never used to write
        // anything: what reaches the sink must still be the policy above.
        writer = writer.options(encrypted(NoncePolicy::random_with_params(4, 8)));
        writer.finish_into_inner().unwrap().1.into_inner()
    };

    assert_eq!(
        build(),
        build(),
        "the entry was written with a nonce policy set after it was accepted",
    );
}

/// One archive, one password.
///
/// Entry data is encrypted with the password in force when the entry was
/// accepted, and the header with whatever is set at `finish`. Changing it in
/// between produced an archive no single password opens: the first fails on
/// the header, the second on the entry - which reads as corruption rather than
/// as the mistake it is.
#[cfg(feature = "aes")]
#[test]
fn test_the_password_cannot_change_once_something_is_encrypted() {
    let encrypted = |password: &str| {
        WriteOptions::new()
            .password(password)
            .encrypt_data(true)
            .encrypt_header(true)
    };

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(encrypted("first password"));
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), b"SECRET")
        .unwrap();

    writer = writer.options(encrypted("second password"));
    let refused = writer.add_bytes(ArchivePath::new("b.bin").unwrap(), b"MORE");
    assert!(
        refused.is_err(),
        "an entry was accepted under a password the archive cannot be opened with",
    );
    assert!(
        writer.finish_into_inner().is_err(),
        "an archive was produced with two passwords in it",
    );
}

/// The same password throughout is not affected.
#[cfg(feature = "aes")]
#[test]
fn test_the_same_password_may_be_set_again() {
    use zesven::crypto::Password;

    const PASSWORD: &str = "correct horse battery staple";
    let encrypted = || {
        WriteOptions::new()
            .password(PASSWORD)
            .encrypt_data(true)
            .encrypt_header(true)
    };

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(encrypted());
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), b"FIRST")
        .unwrap();

    // Same password, different level: the rest of the options stay free.
    writer = writer.options(encrypted().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("b.bin").unwrap(), b"SECOND")
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut archive =
        Archive::open_with_password(Cursor::new(bytes), Password::new(PASSWORD)).unwrap();
    assert_eq!(archive.extract_to_vec("a.bin").unwrap(), b"FIRST");
    assert_eq!(archive.extract_to_vec("b.bin").unwrap(), b"SECOND");
}

/// A rejected entry does not decide what the archive is keyed on.
///
/// The password is fixed when a key is derived from it, not when an entry is
/// offered: checking and fixing in one step meant an entry turned away for an
/// unrelated reason - arriving out of order under `deterministic` - still
/// claimed the archive for its password, and the next perfectly good entry was
/// refused for using a different one.
#[cfg(feature = "aes")]
#[test]
fn test_a_rejected_entry_does_not_fix_the_password() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().deterministic(true));
    writer
        .add_bytes(ArchivePath::new("m.bin").unwrap(), b"MIDDLE")
        .unwrap();

    // Refused: sorts before what is already written.
    writer = writer.options(
        WriteOptions::new()
            .deterministic(true)
            .password("first password")
            .encrypt_data(true),
    );
    assert!(
        writer
            .add_bytes(ArchivePath::new("a.bin").unwrap(), b"OUT OF ORDER")
            .is_err(),
        "the entry sorts before one already written",
    );

    // In order, and nothing has ever been encrypted under the other password.
    writer = writer.options(
        WriteOptions::new()
            .deterministic(true)
            .password("second password")
            .encrypt_data(true),
    );
    writer
        .add_bytes(ArchivePath::new("z.bin").unwrap(), b"FINE")
        .expect("a rejected entry must not claim the archive for its password");
    let (result, _sink) = writer.finish_into_inner().unwrap();
    assert_eq!(result.entries_written, 2);
}

/// A directory does not decide it either.
///
/// Nothing about a directory is encrypted, so accepting one while a password
/// happens to be set must leave the archive free to be keyed on another.
#[cfg(feature = "aes")]
#[test]
fn test_an_unencrypted_entry_does_not_fix_the_password() {
    let mut writer = Writer::create(Cursor::new(Vec::new())).unwrap().options(
        WriteOptions::new()
            .password("first password")
            .encrypt_data(true),
    );
    writer
        .add_directory(ArchivePath::new("dir").unwrap(), EntryMeta::directory())
        .unwrap();

    writer = writer.options(
        WriteOptions::new()
            .password("second password")
            .encrypt_data(true),
    );
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), b"SECRET")
        .expect("no key has been derived yet, so the password is still open");
    let (result, _sink) = writer.finish_into_inner().unwrap();
    assert_eq!((result.entries_written, result.directories_written), (1, 1));
}

/// The header is encrypted with the password the entries were.
///
/// Changing it immediately before `finish` reaches no check on the way in -
/// nothing else is added - so the buffered entry was encrypted under the old
/// password while the header took the new one, and the archive opened with
/// neither. The password is fixed where each key is derived, header included.
#[cfg(feature = "aes")]
#[test]
fn test_the_header_cannot_be_keyed_on_a_second_password() {
    use zesven::Threads;

    // Two threads, so a single small entry stays buffered rather than being
    // written out as soon as it arrives.
    let encrypted = |password: &str, header: bool| {
        WriteOptions::new()
            .threads(Threads::count_or_single(2))
            .password(password)
            .encrypt_data(true)
            .encrypt_header(header)
    };

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(encrypted("first password", false));
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), b"SECRET")
        .unwrap();

    // Changed with nothing added afterwards: `finish` is the next thing to run.
    writer = writer.options(encrypted("second password", true));
    assert!(
        writer.finish_into_inner().is_err(),
        "an archive was produced whose header and entry take different passwords",
    );
}
