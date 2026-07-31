//! Writing an extracted file beside its destination until it is whole.
//!
//! Extracting over an existing file used to truncate it first and decompress
//! into it afterwards, so anything that went wrong in between - a damaged
//! archive, a checksum that did not match, a disk that filled - left the
//! caller with neither the extracted file nor the one they had. The cleanup
//! made it worse rather than better: it deleted what was left, so a failed
//! extraction over a directory of files removed the ones it failed on.
//!
//! Here the bytes go to a file beside the destination and are moved onto it
//! only once they are all there and correct. A failure leaves the destination
//! exactly as it was, which is what a caller re-extracting an archive over
//! their working copy is entitled to expect, and the scratch file goes with
//! the value that owns it rather than being cleaned up by hand at each of the
//! places that can fail.
//!
//! Replacing a file this way replaces the inode, so what belonged to the file
//! rather than to its contents does not survive: the mode is carried over
//! deliberately, but the owner becomes whoever ran the extraction, and access
//! control lists and extended attributes are not copied. Writing through the
//! destination kept all of it, at the cost of destroying the file whenever
//! extraction failed.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// A file being extracted, written beside where it belongs.
///
/// Dropped without [`Self::commit`], it removes the partial file it was
/// writing and leaves the destination alone.
pub(crate) struct StagedFile {
    destination: PathBuf,
    scratch: PathBuf,
    file: Option<File>,
}

impl StagedFile {
    /// Creates the file to extract into, beside `destination`.
    ///
    /// The scratch name is hidden, names the destination and carries the
    /// process id, so two extractions running at once do not write into one
    /// another's file. It is created rather than opened: something already at
    /// that path belongs to somebody else.
    pub(crate) fn create(destination: &Path) -> Result<Self> {
        let scratch = scratch_path(destination);

        // Created, never opened. Falling back to opening it would truncate a
        // file this run did not make - which is the defect this type exists to
        // prevent, moved to the scratch path. The name carries a counter as
        // well as the process id, so the only way to collide is a leftover
        // from a killed run of a process with this id, and failing on that is
        // better than writing through it.
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&scratch)
            .map_err(Error::Io)?;

        // A new file is created with whatever the umask allows, which for a
        // replacement is wrong in the direction that matters: extracting over
        // a file the owner had restricted to 0600 would publish it at 0644.
        // Writing through the destination used to keep its mode by keeping its
        // inode; staging has to carry it across deliberately. An entry that
        // declares a mode of its own still overrides this afterwards.
        if let Ok(existing) = fs::metadata(destination) {
            if let Err(e) = file.set_permissions(existing.permissions()) {
                log::warn!(
                    "Failed to carry the mode of '{}' onto the file replacing it: {}",
                    destination.display(),
                    e
                );
            }
        }

        Ok(Self {
            destination: destination.to_path_buf(),
            scratch,
            file: Some(file),
        })
    }

    /// The file to write the entry's bytes into.
    pub(crate) fn file(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("the file is only taken when committing")
    }

    /// Hands the file over to a caller that writes through it themselves.
    ///
    /// The staged file keeps the scratch path, so committing and cleaning up
    /// still belong to it; only the writing is somebody else's.
    pub(crate) fn take_file(&mut self) -> File {
        self.file
            .take()
            .expect("the file is only taken once, by the caller that writes it")
    }

    /// Where the bytes are while they are being written.
    ///
    /// What a checksum is computed over: reading the destination would mean
    /// putting a file there before knowing whether it is the right one.
    pub(crate) fn path(&self) -> &Path {
        &self.scratch
    }

    /// Closes the file, keeping it staged.
    ///
    /// For a caller that has to read what it wrote - a checksum - before
    /// deciding whether to commit.
    pub(crate) fn close(&mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            // Dropping a `File` discards a write error; this does not.
            file.sync_data().map_err(Error::Io)?;
        }
        Ok(())
    }

    /// Moves the finished file onto its destination.
    pub(crate) fn commit(mut self) -> Result<PathBuf> {
        self.close()?;
        fs::rename(&self.scratch, &self.destination).map_err(Error::Io)?;
        let destination = std::mem::take(&mut self.destination);
        // Nothing left to clean up, and the scratch path no longer exists.
        self.scratch = PathBuf::new();
        Ok(destination)
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        self.file.take();
        if self.scratch.as_os_str().is_empty() {
            return;
        }
        if let Err(e) = fs::remove_file(&self.scratch) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Failed to clean up partial file '{}': {}",
                    self.scratch.display(),
                    e
                );
            }
        }
    }
}

/// Returns where to write an extracted file before it is moved into place.
///
/// Beside the destination rather than in a temporary directory, so that moving
/// it is a rename within one filesystem rather than a copy across two.
fn scratch_path(destination: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let name = destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "entry".to_string());

    // The decoration is about twenty bytes, and a filename may not exceed 255
    // on the filesystems this runs on - so an entry named close to that limit
    // would fail to extract on its name alone. The name is only there to make
    // the file recognisable while it exists; the process id and counter are
    // what make it unique, so it is the name that gives way.
    let suffix = format!(
        ".zesven-{}-{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    );
    const MAX_NAME: usize = 255;
    let room = MAX_NAME.saturating_sub(suffix.len() + 1);
    let mut trimmed = name;
    while trimmed.len() > room {
        trimmed.pop();
    }
    let scratch = format!(".{trimmed}{suffix}");

    match destination.parent() {
        Some(parent) => parent.join(scratch),
        None => PathBuf::from(scratch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_a_committed_file_reaches_its_destination() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let destination = dir.path().join("out.bin");

        let mut staged = StagedFile::create(&destination).expect("creates");
        staged.file().write_all(b"CONTENTS").expect("writes");
        staged.commit().expect("commits");

        assert_eq!(fs::read(&destination).expect("reads"), b"CONTENTS");
        assert_eq!(strays(dir.path()), Vec::<String>::new());
    }

    /// The case the whole type exists for.
    #[test]
    fn test_an_abandoned_file_leaves_the_destination_untouched() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let destination = dir.path().join("out.bin");
        fs::write(&destination, b"WHAT WAS THERE BEFORE").expect("writes");

        {
            let mut staged = StagedFile::create(&destination).expect("creates");
            staged
                .file()
                .write_all(b"HALF OF SOMETHING")
                .expect("writes");
            // Dropped without committing, as a failed extraction drops it.
        }

        assert_eq!(
            fs::read(&destination).expect("reads"),
            b"WHAT WAS THERE BEFORE",
            "a failed extraction destroyed the file it was replacing",
        );
        assert_eq!(strays(dir.path()), Vec::<String>::new());
    }

    /// Two entries extracting at once must not share a file.
    #[test]
    fn test_two_destinations_do_not_share_a_scratch_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let first = dir.path().join("one.bin");
        let second = dir.path().join("two.bin");

        let mut a = StagedFile::create(&first).expect("creates");
        let mut b = StagedFile::create(&second).expect("creates");
        assert_ne!(a.path(), b.path());

        a.file().write_all(b"FIRST").expect("writes");
        b.file().write_all(b"SECOND").expect("writes");
        a.commit().expect("commits");
        b.commit().expect("commits");

        assert_eq!(fs::read(&first).expect("reads"), b"FIRST");
        assert_eq!(fs::read(&second).expect("reads"), b"SECOND");
    }

    fn strays(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .expect("reads")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("part"))
            .collect();
        found.sort();
        found
    }
}
