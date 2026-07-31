//! Combinations and failures the writer must refuse rather than survive.
//!
//! Each case here produced an archive that looked fine and was not: contents
//! paired with the wrong names, data written unencrypted after encryption was
//! requested, a folder shape the reader rejects, or bytes left behind by a
//! failed entry that move every folder after them. An archive that cannot be
//! written correctly must not be written at all.

#![cfg(feature = "lzma2")]

use std::io::{Cursor, Read};

use zesven::codec::CodecMethod;
use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, Writer};
use zesven::{ArchivePath, WriteFilter};

/// Yields data for a while, then fails, as a truncated file or a dropped mount
/// would.
struct FailsPartway {
    remaining: usize,
}

impl Read for FailsPartway {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("source went away"));
        }
        let n = buf.len().min(self.remaining);
        buf[..n].fill(b'A');
        self.remaining -= n;
        Ok(n)
    }
}

/// After a failure partway through an entry, the archive cannot be completed.
///
/// An entry past the streaming threshold is written to the sink as it is
/// compressed, so a read error partway through leaves bytes belonging to no
/// folder. The writer used to carry on: `finish` succeeded, the archive opened,
/// and the *next* entry came back corrupt - a failure the caller had no way to
/// notice.
///
/// The source has to give up more than the threshold before failing, or nothing
/// has reached the sink and there is nothing to poison; that case is below.
#[test]
fn test_a_failed_entry_poisons_the_writer() {
    for method in [CodecMethod::Copy, CodecMethod::Lzma2] {
        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(WriteOptions::new().level(1).unwrap().method(method));

        let mut source = FailsPartway {
            remaining: 70 << 20,
        };
        let failed = writer.add_stream(
            ArchivePath::new("broken.bin").unwrap(),
            &mut source,
            EntryMeta::file(80 << 20),
        );
        assert!(failed.is_err(), "{method:?}: the read error must surface");

        assert!(
            writer
                .add_bytes(ArchivePath::new("after.bin").unwrap(), b"AFTER")
                .is_err(),
            "{method:?}: the writer kept accepting entries after a partial write",
        );
        assert!(
            writer.finish_into_inner().is_err(),
            "{method:?}: finish produced an archive from a failed write",
        );
    }
}

/// A source that fails before anything is written costs only that entry.
///
/// Nothing has reached the sink, so the archive is still exactly what the
/// entries before it made it. Failing the whole archive here would mean one
/// unreadable file among ten thousand costing the run - and the caller cannot
/// even retry, since the writer would be poisoned.
#[test]
fn test_a_failure_before_anything_is_written_leaves_the_writer_usable() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());

    writer
        .add_bytes(ArchivePath::new("before.bin").unwrap(), b"BEFORE")
        .unwrap();

    let mut source = FailsPartway { remaining: 4 << 20 };
    assert!(
        writer
            .add_stream(
                ArchivePath::new("broken.bin").unwrap(),
                &mut source,
                EntryMeta::file(80 << 20),
            )
            .is_err(),
        "the read error must surface",
    );

    writer
        .add_bytes(ArchivePath::new("cafter.bin").unwrap(), b"AFTER")
        .expect("the writer is still usable");

    let (_result, sink) = writer.finish_into_inner().expect("finishes");
    let mut archive = Archive::open(Cursor::new(sink.into_inner())).expect("opens");
    let names: Vec<String> = archive
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    assert_eq!(names, vec!["before.bin", "cafter.bin"]);
    assert_eq!(archive.extract_to_vec("cafter.bin").unwrap(), b"AFTER");
}

/// Deterministic mode rejects entries that arrive out of order.
///
/// It used to sort the file list at the end, which rearranged names over
/// streams that had already been written: `a.txt` came back holding what was
/// written for `z.txt`, with no error and a matching checksum.
#[test]
fn test_deterministic_mode_requires_sorted_entries() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().deterministic(true));

    writer
        .add_bytes(ArchivePath::new("z.txt").unwrap(), b"CONTENT-Z")
        .unwrap();
    let out_of_order = writer.add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A");

    let message = out_of_order
        .expect_err("adding an earlier-sorting path must fail")
        .to_string();
    assert!(
        message.contains("sorted order"),
        "unhelpful error: {message}",
    );
}

/// In sorted order, deterministic mode pairs every name with its own contents.
#[test]
fn test_deterministic_mode_keeps_contents_with_their_names() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().deterministic(true));

    writer
        .add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A")
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("z.txt").unwrap(), b"CONTENT-Z")
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut archive = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.extract_to_vec("a.txt").unwrap(), b"CONTENT-A");
    assert_eq!(archive.extract_to_vec("z.txt").unwrap(), b"CONTENT-Z");
}

/// Encryption through BCJ2 must be refused, not silently skipped.
///
/// BCJ2 builds its own four-stream coder chain with nowhere to put an AES
/// coder, so the data went out in the clear: the archive opened and extracted
/// with no password at all, while `password()` and `encrypt_data(true)` had
/// both been set.
#[cfg(feature = "aes")]
#[test]
fn test_encryption_through_bcj2_is_refused() {
    let mut writer = Writer::create(Cursor::new(Vec::new())).unwrap().options(
        WriteOptions::new()
            .password("correct horse battery staple")
            .encrypt_data(true)
            .filter(WriteFilter::Bcj2),
    );

    let error = writer
        .add_bytes(ArchivePath::new("code.bin").unwrap(), &b"MARKER".repeat(64))
        .expect_err("encrypted BCJ2 must be refused");
    assert!(
        error.to_string().contains("BCJ2"),
        "unhelpful error: {error}",
    );
}

/// BCJ2 in a solid archive must be refused before anything is written.
///
/// The combination produced an archive our own reader rejects with "BCJ2 coder
/// requires exactly 4 input streams, found 1" - written successfully, readable
/// by nobody.
#[test]
fn test_bcj2_in_a_solid_archive_is_refused() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().solid().filter(WriteFilter::Bcj2));

    let error = writer
        .add_bytes(ArchivePath::new("code.bin").unwrap(), &b"x".repeat(4096))
        .expect_err("solid BCJ2 must be refused");
    assert!(
        error.to_string().contains("BCJ2"),
        "unhelpful error: {error}",
    );
}

/// Changing the options mid-archive must not corrupt the entries already written.
///
/// The header described every folder with whichever method was set last, so an
/// entry compressed with LZMA2 and then followed by a switch to Copy was
/// declared as stored: it decoded to garbage and failed its checksum. The
/// method each folder was written with is now recorded alongside it.
#[test]
fn test_changing_the_method_does_not_corrupt_earlier_entries() {
    // Large enough to reach the sink before the options change.
    let first: Vec<u8> = std::iter::repeat_n(b'A', 68 << 20).collect();

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), &first)
        .unwrap();

    writer = writer.options(
        WriteOptions::new()
            .level(1)
            .unwrap()
            .method(CodecMethod::Copy),
    );
    writer
        .add_bytes(ArchivePath::new("b.bin").unwrap(), b"SMALL")
        .unwrap();
    let bytes = writer.finish_into_inner().unwrap().1.into_inner();

    let mut archive = Archive::open(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.extract_to_vec("a.bin").unwrap(), first);
    assert_eq!(archive.extract_to_vec("b.bin").unwrap(), b"SMALL");
}

/// A sink that fails mid-archive poisons the writer on the buffered path too.
///
/// Only the streaming path used to do this, so a failure while flushing a batch
/// left the writer usable and produced an archive with folders at the wrong
/// offsets.
#[test]
fn test_a_failing_sink_poisons_the_buffered_path() {
    use std::io::{Seek, SeekFrom, Write};

    /// Accepts a fixed number of bytes, then refuses.
    struct FailingSink {
        inner: Cursor<Vec<u8>>,
        budget: usize,
    }

    impl Write for FailingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.budget == 0 {
                return Err(std::io::Error::other("disk full"));
            }
            let n = buf.len().min(self.budget);
            self.budget -= n;
            self.inner.write(&buf[..n])
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    impl Seek for FailingSink {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    // Incompressible, so the entries cannot shrink inside the budget.
    let mut data = vec![0u8; 4 * 1024 * 1024];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for byte in data.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }

    let mut writer = Writer::create(FailingSink {
        inner: Cursor::new(Vec::new()),
        budget: 200,
    })
    .unwrap()
    .options(
        WriteOptions::new()
            .level(1)
            .unwrap()
            .memory_limit(zesven::MemoryLimit::bytes_or_auto(16 * 1024 * 1024)),
    );

    // Enough entries that a batch flushes during an add rather than at finish.
    let mut failed = false;
    for i in 0..8 {
        if writer
            .add_bytes(ArchivePath::new(&format!("{i:02}.bin")).unwrap(), &data)
            .is_err()
        {
            failed = true;
            break;
        }
    }

    if failed {
        assert!(
            writer
                .add_bytes(ArchivePath::new("99.bin").unwrap(), b"AFTER")
                .is_err(),
            "the writer kept accepting entries after the sink failed",
        );
    }
    assert!(
        writer.finish_into_inner().is_err(),
        "an archive was produced from a sink that failed",
    );
}

/// A rejected add must not move the deterministic ordering forward.
///
/// The position was recorded before the entry was accepted, so a failed
/// `add_path` for "z.txt" then rejected a perfectly good "a.txt".
#[test]
fn test_a_failed_add_does_not_advance_the_order() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().deterministic(true));

    // Fails: no such file.
    assert!(
        writer
            .add_path("/nonexistent/z.txt", ArchivePath::new("z.txt").unwrap())
            .is_err(),
    );

    // "a.txt" sorts before "z.txt", but "z.txt" was never added.
    writer
        .add_bytes(ArchivePath::new("a.txt").unwrap(), b"A")
        .expect("a rejected entry must not reserve its position in the order");
}

/// Turning deterministic mode on mid-archive enforces the order from there on.
///
/// The position was only recorded while the setting was on, so switching it on
/// left the check comparing against whatever name had been recorded before it
/// was switched off - usually none at all, which accepted anything.
#[test]
fn test_deterministic_mode_enabled_midway_still_enforces_order() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().deterministic(false));
    writer
        .add_bytes(ArchivePath::new("z.txt").unwrap(), b"CONTENT-Z")
        .unwrap();

    writer = writer.options(WriteOptions::new().deterministic(true));
    let out_of_order = writer.add_bytes(ArchivePath::new("a.txt").unwrap(), b"CONTENT-A");

    assert!(
        out_of_order.is_err(),
        "'a.txt' sorts before the entry already written, and the setting is on",
    );
}

/// The reported archive size must be the size of the archive.
///
/// It was taken from the sink after the signature header had been written,
/// which happens last and seeks back to the start: every single-file archive
/// reported itself as 32 bytes long, the length of that header.
#[test]
fn test_the_write_result_reports_the_archive_length() {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new());
    writer
        .add_bytes(ArchivePath::new("a.txt").unwrap(), b"HELLO")
        .unwrap();
    let (result, sink) = writer.finish_into_inner().unwrap();

    let written = sink.into_inner().len() as u64;
    assert_eq!(result.volume_count, 1);
    assert_eq!(
        result.volume_sizes,
        vec![written],
        "the result claims a size the archive does not have",
    );
}
