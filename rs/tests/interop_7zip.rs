//! Interoperability tests against the reference 7-Zip implementation.
//!
//! Every other test in this suite verifies that zesven can read what zesven
//! wrote. That is blind to the most damaging class of format bug: a writer that
//! diverges from the 7z specification together with a reader that diverges the
//! same way. Round-trips stay green while no other implementation can open the
//! archives (and we cannot open theirs).
//!
//! These tests close that hole by driving the official 7-Zip binary in both
//! directions: archives we produce must pass `7zz t` and extract byte-for-byte,
//! and archives 7-Zip produces must open and extract through our reader.
//!
//! The binary is located via `ZESVEN_7Z`, then `rs/target/tools/7zz`, then the
//! `PATH`. `mise run tools:7zip` downloads a pinned build into the first
//! location. When no binary is found the tests skip loudly, unless
//! `ZESVEN_REQUIRE_7Z=1` is set, which turns a missing binary into a failure so
//! CI cannot silently lose this coverage.

#![cfg(all(feature = "lzma2", feature = "aes"))]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

use tempfile::TempDir;
use zesven::codec::CodecMethod;
use zesven::crypto::NoncePolicy;
use zesven::read::Archive;
use zesven::write::SolidOptions;
use zesven::{ArchivePath, WriteFilter, WriteOptions, Writer};

const PASSWORD: &str = "correct horse battery staple";

/// Key derivation strength used by the tests.
///
/// The 7z default is 2^19 SHA-256 iterations, which costs a noticeable fraction
/// of a second per archive in an unoptimized test build and would be paid twice
/// per archive here. 2^4 exercises the identical code path at a thousandth of
/// the cost; `test_default_kdf_strength_is_interoperable` covers the default.
const FAST_KDF: u8 = 4;

// =============================================================================
// Reference binary discovery
// =============================================================================

/// Locates the reference 7-Zip binary, or explains why the test is skipped.
fn reference_7z() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZESVEN_7Z") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "ZESVEN_7Z points at {}, which is not a file",
            path.display()
        );
        return Some(path);
    }

    // Deliberately not searching PATH: whichever 7z a machine happens to carry
    // is a different implementation and version from the one these expectations
    // were derived against, and testing conformance against an unknown reference
    // proves nothing in either direction.
    let vendored = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/tools/7zz");
    if vendored.is_file() {
        return Some(vendored);
    }

    assert!(
        env::var_os("ZESVEN_REQUIRE_7Z").is_none(),
        "ZESVEN_REQUIRE_7Z is set but no 7-Zip binary was found; \
         run `mise run tools:7zip` to fetch the pinned reference build"
    );
    eprintln!(
        "SKIP: no reference 7-Zip binary found (set ZESVEN_7Z or run `mise run tools:7zip`); \
         7z interoperability is NOT covered by this run"
    );
    None
}

/// Skips the test body when no reference binary is available.
macro_rules! reference_7z_or_skip {
    () => {
        match reference_7z() {
            Some(bin) => bin,
            None => return,
        }
    };
}

// =============================================================================
// Reference binary invocation
// =============================================================================

/// Runs the reference binary and returns its output plus a printable transcript.
fn run_7z(bin: &Path, args: &[&str]) -> (Output, String) {
    let output = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to execute {}: {e}", bin.display()));

    let transcript = format!(
        "$ {} {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        bin.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    (output, transcript)
}

/// Runs the reference binary and asserts it succeeded.
fn run_7z_ok(bin: &Path, args: &[&str], context: &str) -> String {
    let (output, transcript) = run_7z(bin, args);
    assert!(
        output.status.success(),
        "reference 7-Zip rejected the archive ({context})\n{transcript}"
    );
    transcript
}

/// Asks the reference binary to verify an archive.
fn assert_7z_accepts(bin: &Path, archive: &Path, password: Option<&str>, context: &str) {
    let pw = password.map(|p| format!("-p{p}"));
    let mut args = vec!["t", "-bso0", "-bsp0"];
    if let Some(pw) = &pw {
        args.push(pw);
    } else {
        // Prevent the binary from blocking on a password prompt if it decides
        // the archive is encrypted after all.
        args.push("-p");
    }
    let archive = archive.to_string_lossy().to_string();
    args.push(&archive);

    run_7z_ok(bin, &args, context);
}

/// Extracts an archive with the reference binary and returns the output directory.
fn extract_with_7z(bin: &Path, archive: &Path, password: Option<&str>, context: &str) -> TempDir {
    let out_dir = TempDir::new().expect("create temp dir");
    let out_flag = format!("-o{}", out_dir.path().display());
    let pw = password
        .map(|p| format!("-p{p}"))
        .unwrap_or("-p".to_string());
    let archive = archive.to_string_lossy().to_string();

    run_7z_ok(
        bin,
        &["x", "-y", "-bso0", "-bsp0", &pw, &out_flag, &archive],
        context,
    );

    out_dir
}

/// Returns the entry sizes the reference binary reports, keyed by path.
///
/// This catches size metadata that is wrong even when extraction happens to
/// work, which is exactly how a mis-ordered coder unpack-size list hides.
fn list_sizes_with_7z(bin: &Path, archive: &Path, password: Option<&str>) -> Vec<(String, u64)> {
    let pw = password
        .map(|p| format!("-p{p}"))
        .unwrap_or("-p".to_string());
    let archive_arg = archive.to_string_lossy().to_string();
    let transcript = run_7z_ok(bin, &["l", "-slt", &pw, &archive_arg], "listing");

    let mut sizes = Vec::new();
    let mut current_path: Option<String> = None;
    for line in transcript.lines() {
        if let Some(value) = line.strip_prefix("Path = ") {
            current_path = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Size = ") {
            if let Some(path) = current_path.take() {
                if let Ok(size) = value.trim().parse::<u64>() {
                    sizes.push((path, size));
                }
            }
        }
    }

    // The first "Path =" is the archive itself and carries no "Size =" line in
    // the header block, so nothing extra needs filtering here.
    sizes
}

// =============================================================================
// Archive construction
// =============================================================================

/// Test options with a cheap KDF, so encrypted cases stay fast.
fn encrypted_options() -> WriteOptions {
    WriteOptions::new()
        .password(PASSWORD)
        .nonce_policy(NoncePolicy::random_with_params(FAST_KDF, 8))
}

/// Writes an archive to a temp directory and returns its path.
fn write_archive(
    dir: &TempDir,
    name: &str,
    options: WriteOptions,
    entries: &[(&str, &[u8])],
) -> PathBuf {
    let path = dir.path().join(name);
    let mut writer = Writer::create_path(&path)
        .expect("create writer")
        .options(options);

    for (entry_path, data) in entries {
        writer
            .add_bytes(
                ArchivePath::new(entry_path).expect("valid archive path"),
                data,
            )
            .expect("add entry");
    }
    let _ = writer.finish().expect("finish archive");

    path
}

/// Asserts the reference binary accepts, lists and extracts what we wrote.
fn assert_interoperable(
    bin: &Path,
    archive: &Path,
    password: Option<&str>,
    entries: &[(&str, &[u8])],
    context: &str,
) {
    assert_7z_accepts(bin, archive, password, context);

    let sizes = list_sizes_with_7z(bin, archive, password);
    for (entry_path, data) in entries {
        let reported = sizes
            .iter()
            .find(|(path, _)| path.replace('\\', "/") == *entry_path)
            .unwrap_or_else(|| panic!("{context}: 7-Zip listing has no entry {entry_path}"));
        assert_eq!(
            reported.1,
            data.len() as u64,
            "{context}: 7-Zip reports the wrong size for {entry_path}"
        );
    }

    let extracted = extract_with_7z(bin, archive, password, context);
    for (entry_path, data) in entries {
        let path = extracted.path().join(entry_path);
        let actual = fs::read(&path)
            .unwrap_or_else(|e| panic!("{context}: 7-Zip did not extract {entry_path}: {e}"));
        assert_eq!(
            actual, *data,
            "{context}: 7-Zip extracted different bytes for {entry_path}"
        );
    }

    if password.is_some() {
        assert_encrypted(bin, archive, password, context);
    }
}

/// Asserts the reference binary agrees that the entries really are encrypted.
///
/// Reading back what we wrote proves nothing about confidentiality: an archive
/// with a password that was never applied round-trips perfectly.
fn assert_encrypted(bin: &Path, archive: &Path, password: Option<&str>, context: &str) {
    let sizes = run_7z_ok(
        bin,
        &[
            "l",
            "-slt",
            &password
                .map(|p| format!("-p{p}"))
                .unwrap_or("-p".to_string()),
            &archive.to_string_lossy(),
        ],
        "listing",
    );

    // Empty entries have no stream to encrypt, so only entries with content
    // carry the flag.
    let mut size = 0u64;
    let mut checked = 0usize;
    for line in sizes.lines() {
        if let Some(value) = line.strip_prefix("Size = ") {
            size = value.trim().parse().unwrap_or(0);
        } else if let Some(flag) = line.strip_prefix("Encrypted = ")
            && size > 0
        {
            assert_eq!(
                flag.trim(),
                "+",
                "{context}: 7-Zip reports an entry as unencrypted"
            );
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "{context}: no entry with content was checked for encryption"
    );
}

/// Asserts our reader agrees with the given entries.
fn assert_readable_by_us(
    archive: &Path,
    password: Option<&str>,
    entries: &[(&str, &[u8])],
    context: &str,
) {
    // Run our structural validator over the reference implementation's own
    // output. Anything it complains about here is a rule we got wrong, not an
    // archive that is malformed.
    let raw = fs::read(archive).expect("read archive");
    common::format_check::assert_archive_well_formed(&raw);

    let file = fs::File::open(archive).expect("open archive");
    let mut opened = match password {
        Some(pw) => Archive::open_with_password(file, pw)
            .unwrap_or_else(|e| panic!("{context}: our reader could not open the archive: {e}")),
        None => Archive::open(file)
            .unwrap_or_else(|e| panic!("{context}: our reader could not open the archive: {e}")),
    };

    for (entry_path, data) in entries {
        let entry = opened
            .entries()
            .iter()
            .find(|e| e.path.as_str() == *entry_path)
            .unwrap_or_else(|| panic!("{context}: our reader is missing entry {entry_path}"));
        assert_eq!(
            entry.size,
            data.len() as u64,
            "{context}: our reader reports the wrong size for {entry_path}"
        );

        let actual = opened.extract_to_vec(entry_path).unwrap_or_else(|e| {
            panic!("{context}: our reader could not extract {entry_path}: {e}")
        });
        assert_eq!(
            actual, *data,
            "{context}: our reader produced different bytes for {entry_path}"
        );
    }
}

/// Builds an archive with the reference binary from the given entries.
fn create_with_7z(
    bin: &Path,
    dir: &TempDir,
    name: &str,
    entries: &[(&str, &[u8])],
    extra_args: &[&str],
) -> PathBuf {
    let source = dir.path().join(format!("{name}.src"));
    fs::create_dir_all(&source).expect("create source dir");
    for (entry_path, data) in entries {
        let path = source.join(entry_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&path, data).expect("write source file");
    }

    let archive = dir.path().join(name);
    let archive_arg = archive.to_string_lossy().to_string();
    let source_arg = format!("{}/.", source.display());

    let mut args = vec!["a", "-bso0", "-bsp0"];
    args.extend_from_slice(extra_args);
    args.push(&archive_arg);
    args.push(&source_arg);
    run_7z_ok(bin, &args, "creating reference archive");

    archive
}

// =============================================================================
// zesven -> 7-Zip
// =============================================================================

/// Baseline: an unencrypted archive must be readable by the reference binary.
///
/// If this fails, every other failure in this file is suspect.
#[test]
fn test_7zip_reads_plain_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"hello from zesven\n")];

    let archive = write_archive(&dir, "plain.7z", WriteOptions::new(), entries);
    assert_interoperable(&bin, &archive, None, entries, "plain archive");
}

/// A stream compressed on several threads must still be an ordinary LZMA2 stream.
///
/// The multi-threaded encoder emits one independently coded chunk per worker
/// and concatenates them. That is legal LZMA2 and any decoder handles it, but
/// only a foreign decoder proves it: our own reader would happily accept a
/// chunk framing no one else recognises.
#[cfg(feature = "parallel")]
#[test]
fn test_7zip_reads_a_multi_threaded_stream() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");

    // Only a solid block is chunked: the entries of a non-solid archive are
    // compressed alongside each other instead, so the codec never splits one.
    // Writing this case non-solid tested the ordinary single-stream path while
    // claiming to cover the multi-threaded one.
    let payload = compressible_payload(12 * 1024 * 1024);
    let entries: &[(&str, &[u8])] = &[
        ("first.bin", &payload[..6 * 1024 * 1024]),
        ("second.bin", &payload[6 * 1024 * 1024..]),
    ];

    let archive = write_archive(
        &dir,
        "threaded.7z",
        WriteOptions::new()
            .level(1)
            .expect("valid level")
            .solid()
            .threads(zesven::Threads::count_or_single(4)),
        entries,
    );
    assert_interoperable(&bin, &archive, None, entries, "multi-threaded stream");
}

/// Builds data large enough to be split into chunks and worth compressing.
#[cfg(feature = "parallel")]
fn compressible_payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x0123_4567_89ab_cdefu64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(b"7z interop payload, repeated to give the matcher something ");
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// An entry compressed straight into the sink must be an ordinary entry.
///
/// Large entries take a different path through the writer: the codec writes to
/// the sink as the entry is read, and the packed size is counted rather than
/// measured afterwards. Our own reader would accept a folder described from
/// either path, so the check that matters is a foreign one.
#[test]
fn test_7zip_reads_a_streamed_entry() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");

    // Just past the threshold at which the writer stops buffering entries.
    let mut payload = Vec::with_capacity(68 << 20);
    let mut state = 0x243F_6A88_85A3_08D3u64;
    while payload.len() < (68 << 20) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        payload.extend_from_slice(b"streamed straight into the sink, and still an ordinary entry ");
        payload.extend_from_slice(&state.to_le_bytes());
    }
    let entries: &[(&str, &[u8])] = &[("big.bin", &payload)];

    // Level 1: this checks the shape of what the streaming path writes, not how
    // hard it compresses, and the default level costs several seconds here.
    let archive = write_archive(
        &dir,
        "streamed.7z",
        WriteOptions::new().level(1).expect("valid level"),
        entries,
    );
    assert_interoperable(&bin, &archive, None, entries, "streamed entry");
}

/// A BCJ2 folder must be one the reference binary can take apart.
///
/// It is the most involved chain this writer produces: four packed streams,
/// three of them through a codec of their own, bound to the filter's four
/// inputs, with the indices saying which stream feeds which. Our own reader
/// would accept a chain that named the streams in a different order, or that
/// named no codec at all - which is what it used to write - so the check that
/// matters is a foreign one.
#[cfg(feature = "lzma2")]
#[test]
fn test_7zip_reads_a_bcj2_folder() {
    use zesven::WriteFilter;

    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");

    // Shaped like x86: instruction bytes and frequent branches to a few
    // absolute targets, which is what the filter separates out.
    const TARGETS: [u32; 4] = [0x0040_1000, 0x0040_2000, 0x0040_3000, 0x0040_5000];
    let mut code = Vec::with_capacity(2 << 20);
    let mut state = 0x1234_5678u32;
    while code.len() < (2 << 20) {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        for shift in 0..3 {
            code.push((state >> (shift * 8)) as u8);
        }
        let here = code.len() as u32;
        let target = TARGETS[(state >> 16) as usize % TARGETS.len()];
        code.push(if state % 3 == 0 { 0xE8 } else { 0xE9 });
        code.extend_from_slice(&target.wrapping_sub(here.wrapping_add(5)).to_le_bytes());
    }

    let entries: &[(&str, &[u8])] = &[("code.bin", &code)];
    let archive = write_archive(
        &dir,
        "bcj2.7z",
        WriteOptions::new()
            .level(1)
            .expect("valid level")
            .filter(WriteFilter::Bcj2),
        entries,
    );
    assert_interoperable(&bin, &archive, None, entries, "BCJ2 folder");
}

/// Data encryption must produce archives the reference binary can decrypt.
#[test]
fn test_7zip_reads_data_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"hello from zesven\n")];

    let archive = write_archive(
        &dir,
        "data.7z",
        encrypted_options().encrypt_data(true),
        entries,
    );
    assert_interoperable(&bin, &archive, Some(PASSWORD), entries, "data encryption");
}

/// A solid block of several entries and a single-entry folder in one archive.
///
/// Substream sizes are recorded only for folders that hold more than one entry,
/// so mixing the two shapes is what exposes an index that walks the wrong list.
#[test]
fn test_7zip_reads_mixed_folder_shapes() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[
        ("a.txt", b"aaaaaaaaaaaaaaaa"),
        ("b.txt", b"bbbbbbbbbbbbbbbbbbbbbbbb"),
        ("c.txt", b"cccc"),
    ];

    // Two entries per block, so the archive holds a two-entry folder and a
    // one-entry folder.
    let archive = write_archive(
        &dir,
        "mixed.7z",
        WriteOptions::new().solid_options(SolidOptions::enabled().files_per_block(2)),
        entries,
    );
    assert_interoperable(&bin, &archive, None, entries, "mixed folder shapes");
}

/// An archive whose entries are all empty has no streams at all.
#[test]
fn test_7zip_reads_archive_of_only_empty_entries() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("one.txt", b""), ("two.txt", b"")];

    let archive = write_archive(&dir, "all-empty.7z", WriteOptions::new().solid(), entries);
    assert_interoperable(&bin, &archive, None, entries, "only empty entries");
}

/// Non-ASCII entry names must survive the UTF-16 name encoding in both tools.
#[test]
fn test_7zip_reads_unicode_entry_names() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[
        ("файл-тест.txt", b"cyrillic name"),
        ("nested/\u{1F600}.txt", b"emoji name"),
    ];

    let archive = write_archive(&dir, "unicode.7z", WriteOptions::new(), entries);
    assert_interoperable(&bin, &archive, None, entries, "unicode entry names");
}

/// Header encryption must produce archives the reference binary can open.
///
/// This is the exact configuration reported in issue #7.
#[test]
fn test_7zip_reads_header_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"hello from zesven\n")];

    let archive = write_archive(
        &dir,
        "header.7z",
        encrypted_options().encrypt_header(true).encrypt_data(true),
        entries,
    );
    assert_interoperable(&bin, &archive, Some(PASSWORD), entries, "header encryption");
}

/// Header encryption without data encryption is a distinct code path.
#[test]
fn test_7zip_reads_header_encrypted_plain_data_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"hello from zesven\n")];

    let archive = write_archive(
        &dir,
        "header-only.7z",
        encrypted_options().encrypt_header(true),
        entries,
    );
    assert_interoperable(
        &bin,
        &archive,
        Some(PASSWORD),
        entries,
        "header encryption without data encryption",
    );
}

/// Several entries in one solid block, fully encrypted.
#[test]
fn test_7zip_reads_solid_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[
        ("a.txt", b"first entry payload"),
        ("b.txt", b"second entry payload, a little longer"),
        ("nested/c.txt", b"third entry payload"),
    ];

    let archive = write_archive(
        &dir,
        "solid.7z",
        encrypted_options()
            .solid()
            .encrypt_header(true)
            .encrypt_data(true),
        entries,
    );
    assert_interoperable(&bin, &archive, Some(PASSWORD), entries, "solid encrypted");
}

/// A filter in front of the codec adds a third coder to the chain.
#[test]
fn test_7zip_reads_filtered_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    // Bytes that look enough like x86 code for the BCJ filter to do work.
    let payload: Vec<u8> = (0..2048u32)
        .map(|i| if i % 16 == 0 { 0xE8 } else { (i % 251) as u8 })
        .collect();
    let entries: &[(&str, &[u8])] = &[("program.bin", &payload)];

    let archive = write_archive(
        &dir,
        "filtered.7z",
        encrypted_options()
            .filter(WriteFilter::BcjX86)
            .encrypt_data(true),
        entries,
    );
    assert_interoperable(
        &bin,
        &archive,
        Some(PASSWORD),
        entries,
        "BCJ filter with encryption",
    );
}

/// Empty entries have no stream at all and are described only by header bits.
#[test]
fn test_7zip_reads_encrypted_archive_with_empty_entry() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("empty.txt", b""), ("data.txt", b"not empty")];

    let archive = write_archive(
        &dir,
        "empty.7z",
        encrypted_options().encrypt_header(true).encrypt_data(true),
        entries,
    );
    assert_interoperable(&bin, &archive, Some(PASSWORD), entries, "empty entry");
}

/// The shipped default key-derivation strength must interoperate too.
///
/// Every other encrypted case here weakens the KDF for speed; this test proves
/// the weakening is not what makes them pass.
#[test]
fn test_default_kdf_strength_is_interoperable() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"hello from zesven\n")];

    let archive = write_archive(
        &dir,
        "default-kdf.7z",
        WriteOptions::new()
            .password(PASSWORD)
            .encrypt_header(true)
            .encrypt_data(true),
        entries,
    );
    assert_interoperable(&bin, &archive, Some(PASSWORD), entries, "default KDF");
}

/// Codecs that 7-Zip also implements must produce archives it accepts.
#[test]
fn test_7zip_reads_all_shared_codecs() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[(
        "payload.txt",
        b"shared codec payload, repeated. ".repeat(16).leak(),
    )];

    let codecs: &[(&str, CodecMethod, bool)] = &[
        ("copy", CodecMethod::Copy, true),
        ("lzma", CodecMethod::Lzma, cfg!(feature = "lzma")),
        ("lzma2", CodecMethod::Lzma2, cfg!(feature = "lzma2")),
        ("deflate", CodecMethod::Deflate, cfg!(feature = "deflate")),
        ("bzip2", CodecMethod::BZip2, cfg!(feature = "bzip2")),
        ("ppmd", CodecMethod::PPMd, cfg!(feature = "ppmd")),
    ];

    for (name, method, enabled) in codecs {
        if !enabled {
            continue;
        }
        let archive = write_archive(
            &dir,
            &format!("codec-{name}.7z"),
            encrypted_options().method(*method).encrypt_data(true),
            entries,
        );
        assert_interoperable(
            &bin,
            &archive,
            Some(PASSWORD),
            entries,
            &format!("{name} with encryption"),
        );
    }
}

// =============================================================================
// 7-Zip -> zesven
// =============================================================================

/// Archives 7-Zip encrypts with `-p` must open through our reader.
#[test]
fn test_we_read_7zip_data_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"reference payload\n")];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-data.7z", entries, &[&pw]);
    assert_readable_by_us(&archive, Some(PASSWORD), entries, "7-Zip data encryption");
}

/// Archives 7-Zip encrypts with `-mhe=on` must open through our reader.
#[test]
fn test_we_read_7zip_header_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("manifest.txt", b"reference payload\n")];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-mhe.7z", entries, &[&pw, "-mhe=on"]);
    assert_readable_by_us(&archive, Some(PASSWORD), entries, "7-Zip header encryption");
}

/// A larger header-encrypted archive exercises a compressed encrypted header.
///
/// 7-Zip emits an AES-only coder chain for tiny headers and adds LZMA once the
/// header grows, so this covers the two-coder decode path.
#[test]
fn test_we_read_7zip_header_encrypted_multifile_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let owned: Vec<(String, Vec<u8>)> = (0..40)
        .map(|i| {
            (
                format!("dir{}/file-with-a-long-name-{i:03}.txt", i % 5),
                format!("payload number {i}\n").into_bytes(),
            )
        })
        .collect();
    let entries: Vec<(&str, &[u8])> = owned
        .iter()
        .map(|(path, data)| (path.as_str(), data.as_slice()))
        .collect();

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-mhe-multi.7z", &entries, &[&pw, "-mhe=on"]);
    assert_readable_by_us(
        &archive,
        Some(PASSWORD),
        &entries,
        "7-Zip header encryption, many entries",
    );
}

/// A reference archive that is solid and encrypted at once.
///
/// Several entries share one encrypted folder, so extraction has to decrypt and
/// then locate each entry inside the decoded block.
#[test]
fn test_we_read_7zip_solid_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[
        ("a.txt", b"first reference payload"),
        ("b.txt", b"second reference payload, longer than the first"),
        ("c.txt", b"third reference payload"),
    ];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-solid.7z", entries, &[&pw, "-ms=on"]);
    assert_readable_by_us(&archive, Some(PASSWORD), entries, "7-Zip solid encrypted");
}

/// A reference archive combining a BCJ filter with encryption: three coders.
#[test]
fn test_we_read_7zip_filtered_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let payload: Vec<u8> = (0..4096u32)
        .map(|i| if i % 16 == 0 { 0xE8 } else { (i % 251) as u8 })
        .collect();
    let entries: &[(&str, &[u8])] = &[("program.bin", &payload)];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(
        &bin,
        &dir,
        "ref-bcj.7z",
        entries,
        &[&pw, "-m0=BCJ", "-m1=LZMA"],
    );
    assert_readable_by_us(
        &archive,
        Some(PASSWORD),
        entries,
        "7-Zip BCJ with encryption",
    );
}

/// A reference archive encrypted without compression: an AES-only folder.
///
/// The decoded stream is padded up to the AES block size, so whatever reads it
/// has to stop at the recorded entry size rather than at end of stream.
#[test]
fn test_we_read_7zip_stored_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    // A size that is deliberately not a multiple of 16.
    let entries: &[(&str, &[u8])] = &[("stored.txt", b"exactly twenty-one!!!")];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-stored.7z", entries, &[&pw, "-m0=Copy"]);
    assert_readable_by_us(
        &archive,
        Some(PASSWORD),
        entries,
        "7-Zip stored and encrypted",
    );
}

/// A reference archive encrypted over LZMA1 rather than LZMA2.
#[test]
fn test_we_read_7zip_lzma1_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("payload.txt", b"lzma1 reference payload, repeated. ")];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-lzma1.7z", entries, &[&pw, "-m0=LZMA"]);
    assert_readable_by_us(
        &archive,
        Some(PASSWORD),
        entries,
        "7-Zip LZMA1 with encryption",
    );
}

/// Non-solid reference archives put every entry in its own folder.
#[test]
fn test_we_read_7zip_non_solid_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[
        ("a.txt", b"first"),
        ("b.txt", b"second"),
        ("c.txt", b"third"),
    ];

    let pw = format!("-p{PASSWORD}");
    let archive = create_with_7z(&bin, &dir, "ref-nonsolid.7z", entries, &[&pw, "-ms=off"]);
    assert_readable_by_us(&archive, Some(PASSWORD), entries, "7-Zip non-solid");
}

// =============================================================================
// Cross-implementation round trip
// =============================================================================

/// The full loop: we write, 7-Zip repacks, we read the result back.
///
/// This catches divergences that survive a one-directional check because both
/// sides happen to agree on a wrong value.
#[test]
fn test_round_trip_through_7zip() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("a.txt", b"first payload"), ("b.txt", b"second payload")];

    let ours = write_archive(
        &dir,
        "ours.7z",
        encrypted_options().encrypt_header(true).encrypt_data(true),
        entries,
    );

    let extracted = extract_with_7z(&bin, &ours, Some(PASSWORD), "round trip: extract");
    let repacked = dir.path().join("repacked.7z");
    let repacked_arg = repacked.to_string_lossy().to_string();
    let source_arg = format!("{}/.", extracted.path().display());
    let pw = format!("-p{PASSWORD}");
    run_7z_ok(
        &bin,
        &[
            "a",
            "-bso0",
            "-bsp0",
            &pw,
            "-mhe=on",
            &repacked_arg,
            &source_arg,
        ],
        "round trip: repack",
    );

    assert_readable_by_us(&repacked, Some(PASSWORD), entries, "round trip: read back");
}

// =============================================================================
// CLI
// =============================================================================

/// The CLI must encrypt when asked, and 7-Zip must agree that it did.
///
/// `zesven create -p` wired the password into the options and never turned
/// encryption on, so it produced an archive the user believed was protected.
/// Nothing caught it because no test had ever run the binary.
#[test]
fn test_cli_create_password_produces_encrypted_archive() {
    let bin = reference_7z_or_skip!();
    let Some(cli) = cli_binary() else {
        eprintln!("SKIP: CLI binary not built; run `cargo build --features cli`");
        return;
    };

    let dir = TempDir::new().expect("temp dir");
    let source = dir.path().join("secret.txt");
    fs::write(&source, b"TOP SECRET PLAINTEXT MARKER\n").expect("write source");
    let archive = dir.path().join("cli.7z");

    for extra in [Vec::new(), vec!["--encrypt-headers".to_string()]] {
        let _ = fs::remove_file(&archive);
        let mut args = vec![
            "create".to_string(),
            archive.to_string_lossy().to_string(),
            source.to_string_lossy().to_string(),
            "-p".to_string(),
            PASSWORD.to_string(),
        ];
        args.extend(extra.iter().cloned());

        let output = Command::new(&cli)
            .args(&args)
            .output()
            .expect("run zesven CLI");
        assert!(
            output.status.success(),
            "zesven create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let entries: &[(&str, &[u8])] = &[("secret.txt", b"TOP SECRET PLAINTEXT MARKER\n")];
        assert_interoperable(
            &bin,
            &archive,
            Some(PASSWORD),
            entries,
            &format!("CLI create with {extra:?}"),
        );

        // The plaintext must not be sitting in the file for anyone to grep.
        let raw = fs::read(&archive).expect("read archive");
        assert!(
            !raw.windows(11).any(|w| w == b"TOP SECRET "),
            "archive contains the plaintext it was supposed to encrypt"
        );
    }
}

/// Locates the CLI binary next to the test executable.
fn cli_binary() -> Option<PathBuf> {
    let mut dir = env::current_exe().ok()?;
    dir.pop(); // deps/
    dir.pop(); // debug/ or release/
    let candidate = dir.join(if cfg!(windows) {
        "zesven.exe"
    } else {
        "zesven"
    });
    candidate.is_file().then_some(candidate)
}

/// 7-Zip must be able to verify our archives, not just open them.
///
/// Per-entry checksums live in SubStreamsInfo, and without them `7zz t` reports
/// success on any data at all: an archive that carries no digest is one nobody
/// can tell has rotted.
#[test]
fn test_7zip_verifies_our_checksums() {
    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let entries: &[(&str, &[u8])] = &[("a.txt", b"first payload"), ("b.txt", b"second payload")];

    let archive = write_archive(&dir, "checksums.7z", WriteOptions::new(), entries);

    let listing = run_7z_ok(
        &bin,
        &["l", "-slt", "-p", &archive.to_string_lossy()],
        "listing",
    );
    let digests = listing.lines().filter(|l| l.starts_with("CRC = ")).count();
    assert_eq!(
        digests,
        entries.len(),
        "7-Zip should see one checksum per entry:\n{listing}"
    );

    // Flip a byte in the packed data and confirm the reference notices.
    let mut corrupted = fs::read(&archive).expect("read archive");
    corrupted[40] ^= 0xFF;
    let corrupted_path = dir.path().join("corrupted.7z");
    fs::write(&corrupted_path, &corrupted).expect("write corrupted archive");

    let (output, transcript) = run_7z(&bin, &["t", "-p", &corrupted_path.to_string_lossy()]);
    assert!(
        !output.status.success(),
        "7-Zip reported corrupted data as intact, so our archives carry no usable checksum\n{transcript}"
    );
}

/// The async writer's archives must interoperate too.
///
/// Its output had only ever been read back by this crate, and only ever out of
/// an in-memory cursor. Through `create_path` it produced a file whose first 32
/// bytes were zero - the signature was still sitting in the buffered sink when
/// it was dropped - which neither 7-Zip nor this crate could open.
#[cfg(feature = "async")]
#[test]
fn test_7zip_reads_an_async_archive() {
    use zesven::AsyncWriter;

    let bin = reference_7z_or_skip!();
    let dir = TempDir::new().expect("temp dir");
    let archive = dir.path().join("async.7z");

    let text = b"hello from the async writer\n".repeat(64);
    let binary: Vec<u8> = (0u8..=255).cycle().take(100_000).collect();
    let entries: &[(&str, &[u8])] = &[("text.txt", &text), ("data.bin", &binary)];

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let mut writer = AsyncWriter::create_path(&archive)
            .await
            .expect("create async archive")
            .options(WriteOptions::new());
        for (path, data) in entries {
            writer
                .add_bytes(ArchivePath::new(path).unwrap(), data)
                .await
                .expect("add entry");
        }
        let result = writer.finish().await.expect("finish async archive");
        assert_eq!(result.entries_written, entries.len());
    });

    assert_interoperable(&bin, &archive, None, entries, "async archive");
}
