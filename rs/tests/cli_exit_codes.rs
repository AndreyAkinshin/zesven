//! What the CLI reports when it does not do what it was asked.
//!
//! A command that writes an incomplete archive and exits zero is worse than one
//! that fails: the caller is a script, the exit code is what it reads, and what
//! happens next - deleting the originals, moving on to the next directory,
//! reporting a backup as taken - happens on the strength of that zero.
//!
//! These run the real binary, because the exit code is the thing under test and
//! calling the library cannot produce one. Cargo.toml names the features that
//! build it, so this file is skipped rather than compiled without them.

use std::path::Path;
use std::process::{Command, Output};

fn zesven(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zesven"))
        .args(args)
        .output()
        .expect("the CLI binary runs")
}

fn wrote_nothing(archive: &Path) -> bool {
    !archive.exists() || std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0) == 0
}

/// A file the caller named and that is not there fails the command.
#[test]
fn test_a_missing_file_is_an_error_not_a_warning() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");
    let archive = dir.path().join("out.7z");

    let output = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
        dir.path().join("absent.txt").to_str().expect("path"),
    ]);

    assert!(
        !output.status.success(),
        "a file that is not there was archived successfully: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        wrote_nothing(&archive),
        "an archive was left behind for a run that failed",
    );
}

/// A failed run must not cost the caller the file it was pointed at.
///
/// Creating an archive replaces whatever is at that path, so a run that fails
/// partway used to leave nothing there at all - the new archive was never
/// finished and the old one had already been truncated. Backups are exactly
/// where this is done twice to the same path.
#[test]
fn test_a_failed_run_leaves_an_existing_archive_alone() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");

    // A real archive at the destination, from a run that worked.
    let archive = dir.path().join("out.7z");
    let first = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);
    assert!(first.status.success(), "the first run has to succeed");
    let before = std::fs::read(&archive).expect("reads");

    // A second run that cannot do what it was asked.
    let output = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
        dir.path().join("absent.txt").to_str().expect("path"),
    ]);
    assert!(!output.status.success(), "the second run has to fail");

    assert_eq!(
        std::fs::read(&archive).expect("the archive is still there"),
        before,
        "a failed run replaced the archive that was already there",
    );

    // And nothing was left beside it either.
    let strays: Vec<_> = std::fs::read_dir(dir.path())
        .expect("reads")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("part") || name.contains("tmp"))
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

/// A directory named with recursion turned off fails rather than being skipped.
#[test]
fn test_a_directory_without_recursion_is_an_error() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let inner = dir.path().join("inner");
    std::fs::create_dir(&inner).expect("creates");
    std::fs::write(inner.join("a.txt"), b"contents").expect("writes");
    let archive = dir.path().join("out.7z");

    let output = zesven(&[
        "create",
        "--no-recursive",
        archive.to_str().expect("path"),
        inner.to_str().expect("path"),
    ]);

    // Not merely "did not succeed": a flag clap rejects also does not succeed,
    // and this test used to pass on `--recursive=false` doing exactly that.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is a directory"),
        "the run failed for some other reason: {stderr}",
    );
    assert!(
        wrote_nothing(&archive),
        "an archive was left behind for a run that failed",
    );
}

/// Nothing beside the destination is touched by a run that fails.
///
/// The scratch file is named for the process that makes it, so a test cannot
/// predict the name the binary will pick. What it can pin is the property that
/// matters: a failing run leaves the directory exactly as it found it, however
/// many files of the caller's are sitting in it. An earlier version of this
/// test used guessed names that could never collide with the real one and so
/// passed while the defect it was written for was live.
#[test]
fn test_a_failed_run_leaves_the_directory_as_it_found_it() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");
    let archive = dir.path().join("out.7z");

    // Files of the caller's, including ones shaped like a scratch name.
    for name in ["out.7z.part", "out.part", "out.7z.tmp", "notes.txt"] {
        std::fs::write(dir.path().join(name), b"PRECIOUS USER DATA").expect("writes");
    }
    let before = listing(dir.path());

    let output = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
        dir.path().join("absent.txt").to_str().expect("path"),
    ]);
    assert!(!output.status.success(), "the run has to fail");

    assert_eq!(
        listing(dir.path()),
        before,
        "a failed run changed the directory"
    );
}

/// A destination that is a symlink to a file that is not there yet.
///
/// Resolving it with `canonicalize` only works when the target exists, so a
/// link to an unmounted disk resolved to nothing and the archive landed on the
/// link - which destroyed the link and put the archive somewhere the caller
/// did not ask for.
#[cfg(unix)]
#[test]
fn test_a_dangling_symlink_destination_still_means_its_target() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");

    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("creates");
    let target = elsewhere.join("not-yet.7z");

    let link = dir.path().join("link.7z");
    std::os::unix::fs::symlink(&target, &link).expect("symlinks");

    let output = zesven(&[
        "create",
        link.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);
    assert!(
        output.status.success(),
        "the run failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat")
            .file_type()
            .is_symlink(),
        "the symlink was replaced by a regular file",
    );
    let written = std::fs::read(&target).expect("the archive is at the target");
    assert!(written.starts_with(b"7z"), "the target is not an archive");
}

/// Every file in a directory, with its contents, for comparing before to after.
fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("reads")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    found.sort();
    found
}

/// Replacing an archive keeps the permissions the caller gave it.
///
/// Building beside the destination and renaming replaces the inode, and with
/// it the mode: an archive deliberately kept at 0600 came back 0644 and
/// readable by everyone on the machine.
#[cfg(unix)]
#[test]
fn test_replacing_an_archive_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");
    let archive = dir.path().join("out.7z");

    let first = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);
    assert!(first.status.success(), "the first run has to succeed");

    std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let second = zesven(&[
        "create",
        archive.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);
    assert!(second.status.success(), "the second run has to succeed");

    let mode = std::fs::metadata(&archive)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the archive came back as {mode:o}");
}

/// A symlink destination means the file it points at.
///
/// Renaming onto the link would replace the link itself with a regular file
/// and leave the file it named untouched - which is the opposite of what a
/// caller who pointed their archive at another disk asked for.
#[cfg(unix)]
#[test]
fn test_a_symlink_destination_is_followed() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");

    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("creates");
    let target = elsewhere.join("real.7z");
    std::fs::write(&target, b"placeholder").expect("writes");

    let link = dir.path().join("link.7z");
    std::os::unix::fs::symlink(&target, &link).expect("symlinks");

    let output = zesven(&[
        "create",
        link.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);
    assert!(
        output.status.success(),
        "the run failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat")
            .file_type()
            .is_symlink(),
        "the symlink was replaced by a regular file",
    );
    let written = std::fs::read(&target).expect("reads");
    assert_ne!(written, b"placeholder", "the archive went somewhere else");
    assert!(written.starts_with(b"7z"), "the target is not an archive");
}

/// A recursive walk feeds the writer in sorted order.
///
/// A directory walk returns entries in whatever order the filesystem gives
/// them, so without sorting `--deterministic` - which requires sorted order -
/// rejected entries for arriving in the order they were found in, and the run
/// that produced an archive at all produced a different one each time.
#[test]
fn test_a_recursive_deterministic_create_succeeds_and_repeats() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).expect("creates");
    // Written in an order that is not the sorted one.
    for name in ["z.txt", "a.txt", "m.txt", "b.txt"] {
        std::fs::write(tree.join(name), format!("contents of {name}\n")).expect("writes");
    }

    let first = dir.path().join("first.7z");
    let second = dir.path().join("second.7z");

    for archive in [&first, &second] {
        let output = zesven(&[
            "create",
            "-r",
            "--deterministic",
            archive.to_str().expect("path"),
            tree.to_str().expect("path"),
        ]);
        assert!(
            output.status.success(),
            "a recursive deterministic create failed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let first_bytes = std::fs::read(&first).expect("reads");
    assert_eq!(
        first_bytes,
        std::fs::read(&second).expect("reads"),
        "two deterministic runs over the same tree produced different archives",
    );

    // And every file is in there.
    let listed = zesven(&["list", first.to_str().expect("path")]);
    let listed = String::from_utf8_lossy(&listed.stdout);
    for name in ["z.txt", "a.txt", "m.txt", "b.txt"] {
        assert!(listed.contains(name), "{name} is missing from {listed}");
    }
}

/// The archive being written must not end up inside itself.
///
/// `zesven create out.7z -r .` names the directory the archive is being
/// written into. Both the archive and the file it is built in sit there, and
/// archiving either means reading a file while writing it - which for a stored
/// entry is a pump that stops when the disk does.
#[test]
fn test_the_archive_being_written_is_not_archived() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.path().join(name), format!("contents of {name}")).expect("writes");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_zesven"))
        .args(["create", "out.7z", "-r", "."])
        .current_dir(dir.path())
        .output()
        .expect("the CLI binary runs");
    assert!(
        output.status.success(),
        "the run failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let listed = Command::new(env!("CARGO_BIN_EXE_zesven"))
        .args(["list", "out.7z"])
        .current_dir(dir.path())
        .output()
        .expect("the CLI binary runs");
    let listed = String::from_utf8_lossy(&listed.stdout);

    assert!(
        listed.contains("a.txt") && listed.contains("b.txt"),
        "{listed}"
    );
    assert!(
        !listed.contains("out.7z"),
        "the archive archived itself: {listed}",
    );
    assert!(
        !listed.contains("part-"),
        "the scratch file was archived: {listed}"
    );
}

/// A destination that is a loop of symlinks is an error, not a silent write.
///
/// Following the links has to stop somewhere, and stopping used to mean
/// writing to the last link visited - which replaced the loop with a regular
/// file and reported success, where every other tool reports ELOOP.
#[cfg(unix)]
#[test]
fn test_a_symlink_loop_destination_is_refused() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let present = dir.path().join("present.txt");
    std::fs::write(&present, b"here").expect("writes");

    let first = dir.path().join("loop-a.7z");
    let second = dir.path().join("loop-b.7z");
    std::os::unix::fs::symlink(&second, &first).expect("symlinks");
    std::os::unix::fs::symlink(&first, &second).expect("symlinks");

    let output = zesven(&[
        "create",
        first.to_str().expect("path"),
        present.to_str().expect("path"),
    ]);

    assert!(!output.status.success(), "a symlink loop was written to");
    assert!(
        std::fs::symlink_metadata(&first)
            .expect("stat")
            .file_type()
            .is_symlink(),
        "the loop was replaced by a regular file",
    );
}
