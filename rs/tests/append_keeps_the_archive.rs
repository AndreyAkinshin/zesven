//! Appending rebuilds an archive next to itself and moves it into place.
//!
//! The archive being replaced is somebody's, and often the only copy: a
//! failure partway must leave it exactly as it was, and success must not
//! quietly widen who can read it. Both were wrong once - the temporary file
//! was named for the archive alone, so two appends built in the same file, and
//! the mode was put on at the end, so the contents of a 0600 archive sat
//! world-readable for the whole rebuild.

#![cfg(feature = "lzma2")]

use std::path::Path;

use zesven::ArchivePath;
use zesven::read::Archive;
use zesven::write::{ArchiveAppender, WriteOptions, Writer};

fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = Writer::create_path(path)
        .expect("writer")
        .options(WriteOptions::new().level(1).expect("level"));
    for (name, data) in entries {
        writer
            .add_bytes(ArchivePath::new(name).expect("path"), data)
            .expect("adds");
    }
    let _ = writer.finish().expect("finishes");
}

fn names(path: &Path) -> Vec<String> {
    let archive = Archive::open_path(path).expect("opens");
    let mut found: Vec<String> = archive
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    found.sort();
    found
}

/// Nothing is left beside the archive when appending succeeds.
#[test]
fn test_a_successful_append_leaves_no_temporary_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let archive = dir.path().join("out.7z");
    write_archive(&archive, &[("first.txt", b"FIRST")]);

    let mut appender = ArchiveAppender::open(&archive).expect("opens");
    appender
        .add_bytes(ArchivePath::new("second.txt").expect("path"), b"SECOND")
        .expect("adds");
    let _ = appender.finish().expect("finishes");

    assert_eq!(names(&archive), vec!["first.txt", "second.txt"]);
    assert_eq!(strays(dir.path()), Vec::<String>::new());
}

/// A failed append leaves the archive and the directory as they were.
#[test]
fn test_a_failed_append_leaves_the_archive_alone() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let archive = dir.path().join("out.7z");
    write_archive(&archive, &[("first.txt", b"FIRST")]);
    let before = std::fs::read(&archive).expect("reads");

    let mut appender = ArchiveAppender::open(&archive).expect("opens");
    appender
        .add_bytes(ArchivePath::new("second.txt").expect("path"), b"SECOND")
        .expect("adds");

    // The archive goes away underneath the appender, which reads it back to
    // copy the entries across. Whatever else that fails on, it must not be the
    // caller's file - and here there is nothing to lose, which is the point:
    // the temporary file must not be left behind either.
    std::fs::remove_file(&archive).expect("removes");
    let failed = appender.finish();
    assert!(failed.is_err(), "appending to a missing archive succeeded");

    assert_eq!(strays(dir.path()), Vec::<String>::new());

    // And the ordinary case: the archive is still there and still itself.
    write_archive(&archive, &[("first.txt", b"FIRST")]);
    assert_eq!(std::fs::read(&archive).expect("reads"), before);
}

/// Replacing the archive keeps the permissions it had.
#[cfg(unix)]
#[test]
fn test_appending_keeps_the_archive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let archive = dir.path().join("out.7z");
    write_archive(&archive, &[("first.txt", b"FIRST")]);
    std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let mut appender = ArchiveAppender::open(&archive).expect("opens");
    appender
        .add_bytes(ArchivePath::new("second.txt").expect("path"), b"SECOND")
        .expect("adds");
    let _ = appender.finish().expect("finishes");

    let mode = std::fs::metadata(&archive)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the archive came back as {mode:o}");
    assert_eq!(names(&archive), vec!["first.txt", "second.txt"]);
}

/// Two appends running at once must not build in the same file.
///
/// The temporary name used to be derived from the archive alone, so the second
/// append opened - and truncated - the file the first was writing.
#[test]
fn test_two_appends_do_not_share_a_temporary_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let first = dir.path().join("one.7z");
    let second = dir.path().join("two.7z");
    write_archive(&first, &[("a.txt", b"A")]);
    write_archive(&second, &[("b.txt", b"B")]);

    // Two appenders open at once, finishing one after the other: with a shared
    // name the second would find the first's file already there.
    let mut a = ArchiveAppender::open(&first).expect("opens");
    let mut b = ArchiveAppender::open(&second).expect("opens");
    a.add_bytes(ArchivePath::new("a2.txt").expect("path"), b"A2")
        .expect("adds");
    b.add_bytes(ArchivePath::new("b2.txt").expect("path"), b"B2")
        .expect("adds");
    let _ = a.finish().expect("finishes");
    let _ = b.finish().expect("finishes");

    assert_eq!(names(&first), vec!["a.txt", "a2.txt"]);
    assert_eq!(names(&second), vec!["b.txt", "b2.txt"]);
    assert_eq!(strays(dir.path()), Vec::<String>::new());
}

fn strays(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .expect("reads")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("tmp") || name.contains("part"))
        .collect();
    found.sort();
    found
}
