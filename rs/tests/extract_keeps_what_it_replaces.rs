//! Extracting over files that are already there must not destroy them.
//!
//! Re-extracting an archive over a working copy is ordinary: a build output, a
//! dataset, a checkout somebody keeps up to date. Extraction used to truncate
//! each destination before it had a single byte to put there, so an archive
//! that turned out to be damaged left the caller with neither the extracted
//! file nor the one they had - and the cleanup then deleted what was left of
//! it, which is the worst of the three outcomes.

#![cfg(feature = "lzma2")]

use std::io::Cursor;
use std::path::Path;

use zesven::read::{Archive, ExtractOptions, OverwritePolicy, SelectAll};

/// The same property, for every API that writes extracted files.
///
/// The blocking reader was fixed first; the async reader and both streaming
/// readers wrote into the destination the same way, so an archive that failed
/// partway took the caller's file with it there too.
mod common;

/// Returns an archive of one entry, and the same archive with its packed
/// stream corrupted so that extracting it fails.
fn good_and_damaged(name: &str, contents: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let good = common::create_archive(&[(name, contents)]).expect("archive");

    // The packed stream begins after the 32 byte signature header. Flipping
    // bits there leaves the header intact - so the archive opens, the entry is
    // listed, and the failure happens partway through decompressing it, which
    // is exactly when a destination has already been truncated.
    let mut damaged = good.clone();
    for byte in damaged.iter_mut().skip(40).take(64) {
        *byte ^= 0xFF;
    }

    (good, damaged)
}

/// An archive whose entry decodes cleanly but fails its checksum.
///
/// Corrupting a compressed stream usually fails inside the decoder, before a
/// single byte reaches the file - which tests the wrong thing. Stored entries
/// decode whatever is there, so the failure lands where it matters: after the
/// file has been written, at the checksum.
#[cfg(feature = "async")]
fn stored_but_wrong(name: &str, contents: &[u8]) -> Vec<u8> {
    use zesven::codec::CodecMethod;
    use zesven::write::WriteOptions;

    let good = common::create_archive_with_options(
        WriteOptions::new().method(CodecMethod::Copy),
        &[(name, contents)],
    )
    .expect("archive");

    let mut damaged = good;
    // Past the 32 byte signature header, inside the stored data.
    for byte in damaged.iter_mut().skip(64).take(32) {
        *byte ^= 0xFF;
    }
    damaged
}

fn extract_into(archive: &[u8], out: &Path) -> zesven::Result<zesven::read::ExtractResult> {
    let mut opened = Archive::open(Cursor::new(archive.to_vec())).expect("opens");
    opened.extract(
        out,
        SelectAll,
        &ExtractOptions::new().overwrite(OverwritePolicy::Overwrite),
    )
}

/// A damaged archive must leave the file it was extracting over alone.
#[test]
fn test_a_failed_extraction_keeps_the_file_it_was_replacing() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    let existing = out.join("payload.bin");
    std::fs::write(&existing, b"WHAT THE CALLER ALREADY HAD").expect("writes");

    let (_good, damaged) = good_and_damaged("payload.bin", &b"NEW CONTENTS ".repeat(4096));

    let result = extract_into(&damaged, &out);
    let failed = match result {
        Err(_) => true,
        Ok(outcome) => !outcome.is_ok(),
    };
    assert!(failed, "the damaged archive extracted successfully");

    assert_eq!(
        std::fs::read(&existing).expect("the file is still there"),
        b"WHAT THE CALLER ALREADY HAD",
        "a failed extraction destroyed the file it was replacing",
    );
    assert_eq!(strays(&out), Vec::<String>::new());
}

/// And a successful extraction still replaces it.
#[test]
fn test_a_successful_extraction_replaces_the_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    let existing = out.join("payload.bin");
    std::fs::write(&existing, b"OLD").expect("writes");

    let contents = b"NEW CONTENTS ".repeat(4096);
    let (good, _damaged) = good_and_damaged("payload.bin", &contents);

    let outcome = extract_into(&good, &out).expect("extracts");
    assert!(outcome.is_ok(), "{outcome:?}");

    assert_eq!(std::fs::read(&existing).expect("reads"), contents);
    assert_eq!(strays(&out), Vec::<String>::new());
}

/// Replacing a file must not widen who can read it.
///
/// The file goes to a scratch path and is renamed onto the destination, and a
/// newly created file gets whatever the umask allows - so a file its owner had
/// restricted to 0600 would come back at 0644 unless the mode is carried over
/// deliberately. Writing through the destination kept it by keeping the inode.
#[cfg(unix)]
#[test]
fn test_replacing_a_file_keeps_the_mode_it_had() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    let existing = out.join("payload.bin");
    std::fs::write(&existing, b"OLD").expect("writes");
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let contents = b"NEW CONTENTS ".repeat(4096);
    let (good, _damaged) = good_and_damaged("payload.bin", &contents);
    let outcome = extract_into(&good, &out).expect("extracts");
    assert!(outcome.is_ok(), "{outcome:?}");

    let mode = std::fs::metadata(&existing)
        .expect("stats")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "extracting over a private file must not publish it"
    );
    assert_eq!(std::fs::read(&existing).expect("reads"), contents);
}

/// One damaged entry must not cost the entries around it.
#[test]
fn test_one_bad_entry_does_not_take_the_others_files() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    for name in ["a.bin", "b.bin", "c.bin"] {
        std::fs::write(out.join(name), format!("EXISTING {name}")).expect("writes");
    }

    let payload = b"contents that compress a little ".repeat(2048);
    let good = common::create_archive(&[
        ("a.bin", payload.as_slice()),
        ("b.bin", payload.as_slice()),
        ("c.bin", payload.as_slice()),
    ])
    .expect("archive");

    let mut damaged = good.clone();
    for byte in damaged.iter_mut().skip(40).take(64) {
        *byte ^= 0xFF;
    }

    let _ = extract_into(&damaged, &out);

    // Whatever survived is either the new contents or the old ones, never a
    // truncated husk of either.
    for name in ["a.bin", "b.bin", "c.bin"] {
        let path = out.join(name);
        let found = std::fs::read(&path).expect("the file is still there");
        let expected_old = format!("EXISTING {name}").into_bytes();
        assert!(
            found == expected_old || found == payload,
            "{name} is neither what was there ({} bytes) nor what was extracted \
             ({} bytes): it is {} bytes",
            expected_old.len(),
            payload.len(),
            found.len(),
        );
    }
    assert_eq!(strays(&out), Vec::<String>::new());
}

fn strays(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .expect("reads")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("part"))
        .collect();
    found.sort();
    found
}

/// The async reader must keep the file it was replacing, like the blocking one.
#[cfg(feature = "async")]
#[tokio::test]
async fn test_the_async_reader_keeps_the_file_it_was_replacing() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    let existing = out.join("payload.bin");
    std::fs::write(&existing, b"WHAT THE CALLER ALREADY HAD").expect("writes");

    let damaged = stored_but_wrong("payload.bin", &b"NEW CONTENTS ".repeat(4096));
    let archive_path = dir.path().join("damaged.7z");
    std::fs::write(&archive_path, &damaged).expect("writes");

    let mut opened = zesven::AsyncArchive::open_path(&archive_path)
        .await
        .expect("opens");
    // Explicitly overwriting: the default refuses a file that is already
    // there, which would never reach the write at all - and a test that stops
    // there proves nothing about what happens to that file.
    let result = opened
        .extract(
            &out,
            (),
            &zesven::AsyncExtractOptions::default().overwrite(OverwritePolicy::Overwrite),
        )
        .await;
    let failed = match result {
        Err(_) => true,
        Ok(outcome) => outcome.entries_failed > 0,
    };
    assert!(failed, "the damaged archive extracted successfully");

    assert_eq!(
        std::fs::read(&existing).expect("the file is still there"),
        b"WHAT THE CALLER ALREADY HAD",
        "a failed async extraction destroyed the file it was replacing",
    );
    assert_eq!(strays(&out), Vec::<String>::new());
}

/// And the streaming reader.
#[test]
fn test_the_streaming_reader_keeps_the_file_it_was_replacing() {
    use zesven::streaming::StreamingArchive;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).expect("creates");

    let existing = out.join("payload.bin");
    std::fs::write(&existing, b"WHAT THE CALLER ALREADY HAD").expect("writes");

    let (_good, damaged) = good_and_damaged("payload.bin", &b"NEW CONTENTS ".repeat(4096));

    #[cfg(feature = "aes")]
    let mut opened = StreamingArchive::open(Cursor::new(damaged), "").expect("opens");
    #[cfg(not(feature = "aes"))]
    let mut opened = StreamingArchive::open(Cursor::new(damaged)).expect("opens");
    let outcome = opened.extract_all(&out, &ExtractOptions::new());
    let failed = match outcome {
        Err(_) => true,
        Ok(result) => result.entries_failed > 0,
    };
    assert!(failed, "the damaged archive extracted successfully");

    assert_eq!(
        std::fs::read(&existing).expect("the file is still there"),
        b"WHAT THE CALLER ALREADY HAD",
        "a failed streaming extraction destroyed the file it was replacing",
    );
    assert_eq!(strays(&out), Vec::<String>::new());
}
