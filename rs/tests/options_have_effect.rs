//! Every write option must reach the encoder.
//!
//! Options are plumbed from a public setter, through `WriteOptions`, to the
//! code that builds the coder chain. When a link in that chain is missing the
//! setter still exists, still stores the value, and still returns `Self`, so
//! nothing looks wrong: the archive is simply written as though the caller had
//! not asked for anything. That has happened twice in this crate - the
//! compression level was pinned to one preset whatever was requested, and an
//! encoder variant was selected by an option no code ever read.
//!
//! Round-trip tests cannot see it, because an archive written with the wrong
//! settings still decodes. So this asserts the one thing that must be true of
//! a working option: setting it changes what gets written.

#![cfg(feature = "lzma2")]

use std::io::Cursor;

use zesven::ArchivePath;
use zesven::write::{WriteOptions, Writer};

/// Data with both structure and entropy, so any coder has something to do.
fn payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"an option that does nothing is worse than no option ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Data shaped like x86 machine code, which is what a BCJ filter transforms.
///
/// The filter only rewrites a CALL or JMP whose displacement looks like a real
/// one, so data without those byte patterns passes through it unchanged - and
/// an untransformed filter proves nothing about whether the option is wired up.
fn x86_like_payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut target = 0u32;
    while data.len() < len {
        data.extend_from_slice(&[0x55, 0x48, 0x89, 0xe5]); // push rbp; mov rbp,rsp
        // call rel32, with a displacement small enough for the filter to accept
        data.push(0xe8);
        data.extend_from_slice(&target.to_le_bytes());
        data.extend_from_slice(&[0x48, 0x83, 0xec, 0x20]); // sub rsp,0x20
        target = target.wrapping_add(0x40);
    }
    data.truncate(len);
    data
}

/// Writes a two-entry archive, returning its bytes and its packed streams.
///
/// The packed streams start right after the 32-byte signature header and run
/// for as many bytes as the writer reports, so they can be compared without
/// the header, which changes for reasons of its own.
fn archive_with(options: WriteOptions) -> (Vec<u8>, Vec<u8>) {
    archive_of(options, &payload(96 * 1024))
}

/// Writes a two-entry archive holding the given data.
fn archive_of(options: WriteOptions, data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    const SIGNATURE_HEADER: usize = 32;

    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(options);
    writer
        .add_bytes(ArchivePath::new("first.bin").unwrap(), data)
        .unwrap();
    writer
        .add_bytes(ArchivePath::new("second.bin").unwrap(), data)
        .unwrap();
    let (result, cursor) = writer.finish_into_inner().unwrap();

    let bytes = cursor.into_inner();
    let packed =
        bytes[SIGNATURE_HEADER..SIGNATURE_HEADER + result.compressed_size as usize].to_vec();
    (bytes, packed)
}

/// Asserts that an option changes the compressed data, not just the header.
///
/// The weaker check - that the archive differs somewhere - is not enough for
/// anything that claims to change how data is encoded. An option can alter
/// what the header declares while the encoder ignores it entirely, which is
/// exactly the shape of the compression-level bug: the declared dictionary
/// moved with the level while the bytes it described never did.
fn assert_changes_compressed_data(what: &str, options: WriteOptions) {
    let (_, baseline) = archive_with(WriteOptions::new());
    let (_, changed) = archive_with(options);
    assert_ne!(
        baseline,
        changed,
        "{what} produced byte-identical packed streams ({} bytes), \
         so it is not reaching the encoder",
        baseline.len(),
    );
}

/// Asserts that an option changes the archive, for options that only affect
/// what the header records.
fn assert_changes_archive(what: &str, options: WriteOptions) {
    let (baseline, _) = archive_with(WriteOptions::new());
    let (changed, _) = archive_with(options);
    assert_ne!(
        baseline, changed,
        "{what} produced a byte-identical archive, so nothing reads it",
    );
}

#[test]
fn test_level_reaches_the_encoder() {
    assert_changes_compressed_data("level(1)", WriteOptions::new().level(1).unwrap());
    assert_changes_compressed_data("level(9)", WriteOptions::new().level(9).unwrap());
}

#[test]
fn test_method_reaches_the_encoder() {
    use zesven::codec::CodecMethod;

    let methods: &[(&str, CodecMethod)] = &[
        ("method(Copy)", CodecMethod::Copy),
        #[cfg(feature = "lzma")]
        ("method(Lzma)", CodecMethod::Lzma),
        #[cfg(feature = "deflate")]
        ("method(Deflate)", CodecMethod::Deflate),
        #[cfg(feature = "bzip2")]
        ("method(BZip2)", CodecMethod::BZip2),
        #[cfg(feature = "ppmd")]
        ("method(PPMd)", CodecMethod::PPMd),
    ];

    for (what, method) in methods {
        assert_changes_compressed_data(what, WriteOptions::new().method(*method));
    }
}

#[test]
fn test_filter_reaches_the_encoder() {
    use zesven::WriteFilter;

    assert_changes_compressed_data("filter(Delta)", WriteOptions::new().delta(4));

    // The BCJ filters need data they would actually transform.
    let code = x86_like_payload(96 * 1024);
    let (_, baseline) = archive_of(WriteOptions::new(), &code);
    let (_, filtered) = archive_of(WriteOptions::new().filter(WriteFilter::BcjX86), &code);
    assert_ne!(
        baseline, filtered,
        "filter(BcjX86) produced byte-identical packed streams on x86-shaped \
         data, so it is not reaching the encoder",
    );
}

#[test]
fn test_solid_reaches_the_encoder() {
    assert_changes_compressed_data("solid()", WriteOptions::new().solid());
}

#[test]
fn test_comment_reaches_the_header() {
    assert_changes_archive("comment()", WriteOptions::new().comment("a comment"));
}

#[cfg(feature = "aes")]
#[test]
fn test_encryption_options_reach_the_encoder() {
    use zesven::crypto::NoncePolicy;

    // A cheap KDF: this asserts that the option is wired up, not that the key
    // derivation is strong, and the default costs a fraction of a second.
    let encrypted = || {
        WriteOptions::new()
            .password("correct horse battery staple")
            .nonce_policy(NoncePolicy::random_with_params(4, 8))
    };

    assert_changes_compressed_data("encrypt_data(true)", encrypted().encrypt_data(true));
    assert_changes_archive("encrypt_header(true)", encrypted().encrypt_header(true));
}
