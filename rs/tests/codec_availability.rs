//! What a build says it can compress with, it must be able to compress with.
//!
//! `CodecMethod::is_available` is what the writer checks before accepting the
//! first entry, so a wrong answer is not a cosmetic mismatch: too optimistic and
//! entries are accepted into a configuration that fails later, too pessimistic
//! and a working codec is refused. It was answered by hand, one arm per method,
//! and LZMA2 was reported available whenever LZMA was - while its encoder is
//! compiled under a feature of its own.
//!
//! Deliberately not gated on any feature: the point is to run in every
//! combination the matrix builds, since that is where the answers differ.

use std::io::Cursor;

use zesven::ArchivePath;
use zesven::codec::CodecMethod;
use zesven::write::{WriteOptions, Writer};

/// Every method, so a new one is covered by adding it to the enum.
const METHODS: &[CodecMethod] = &[
    CodecMethod::Copy,
    CodecMethod::Lzma,
    CodecMethod::Lzma2,
    CodecMethod::Deflate,
    CodecMethod::BZip2,
    CodecMethod::PPMd,
    CodecMethod::Lz4,
    CodecMethod::Zstd,
    CodecMethod::Brotli,
];

/// Tries to write an archive with the method, and reports whether it worked.
fn can_write_with(method: CodecMethod) -> Result<(), String> {
    let options = WriteOptions::new().method(method);
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .map_err(|e| e.to_string())?
        .options(options);

    writer
        .add_bytes(
            ArchivePath::new("payload.bin").map_err(|e| e.to_string())?,
            &b"something worth compressing, at least a little\n".repeat(64),
        )
        .map_err(|e| e.to_string())?;
    let (result, _sink) = writer.finish_into_inner().map_err(|e| e.to_string())?;
    if result.entries_written != 1 {
        return Err(format!(
            "wrote {} entries, expected 1",
            result.entries_written
        ));
    }
    Ok(())
}

/// The answer must match what the build can actually do, in both directions.
#[test]
fn test_availability_matches_what_the_build_can_write() {
    for &method in METHODS {
        let claimed = method.is_available();
        let actual = can_write_with(method);

        match (claimed, &actual) {
            (true, Err(e)) => panic!(
                "{method:?} reports itself available and writing failed: {e}\n\
                 the writer accepts entries on the strength of that claim",
            ),
            (false, Ok(())) => panic!(
                "{method:?} reports itself unavailable and wrote an archive; \
                 callers are being refused a codec this build has",
            ),
            _ => {}
        }
    }
}

/// An unavailable method is refused before an entry is accepted.
///
/// Discovering it when the buffer is compressed loses entries the caller was
/// told had been taken.
#[test]
fn test_an_unavailable_method_is_refused_before_the_first_entry() {
    for &method in METHODS {
        if method.is_available() {
            continue;
        }

        let mut writer = Writer::create(Cursor::new(Vec::new()))
            .unwrap()
            .options(WriteOptions::new().method(method));
        let refused = writer.add_bytes(ArchivePath::new("a.bin").unwrap(), b"DATA");

        assert!(
            refused.is_err(),
            "{method:?} is not in this build and the entry was accepted anyway",
        );
    }
}
