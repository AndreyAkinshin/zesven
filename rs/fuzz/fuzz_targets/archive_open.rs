//! Fuzz target for Archive::open with arbitrary byte input.
//!
//! This target exercises the archive parsing code with potentially malformed
//! or adversarial input. The goal is to find panics, hangs, or memory issues
//! in the parsing logic.
//!
//! Run with: cargo +nightly fuzz run archive_open
//!
//! The fuzzer will automatically discover and save interesting inputs that
//! trigger new code paths.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    // Attempt to open arbitrary bytes as a 7z archive
    let cursor = Cursor::new(data);

    // We don't care about the result - we're looking for panics or hangs
    let _ = zesven::Archive::open(cursor, "");

    // If we got an archive, try to iterate entries (exercises more parsing)
    let cursor = Cursor::new(data);
    if let Ok(archive) = zesven::Archive::open(cursor, "") {
        for entry in archive.entries() {
            // Access entry fields to exercise lazy parsing
            let _ = entry.path.as_str();
            let _ = entry.uncompressed_size;
            let _ = entry.is_directory;
            let _ = entry.crc;
        }
    }
});
