---
title: Creating Archives
description: Basic archive creation with zesven
---

# Creating Archives

Learn how to create 7z archives from files and data.

## Basic Creation

Create an archive and add files:

```rust
use zesven::{Writer, ArchivePath, Result};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;

    // Add a file from disk
    writer.add_path("source.txt", ArchivePath::new("source.txt")?)?;

    // Finalize the archive
    writer.finish()?;
    Ok(())
}
```

## Adding Files

### From Disk

Add files from the filesystem:

```rust
use zesven::{Writer, ArchivePath, Result};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;

    // Add with same name
    writer.add_path("file.txt", ArchivePath::new("file.txt")?)?;

    // Add with different archive path
    writer.add_path(
        "/home/user/documents/report.pdf",
        ArchivePath::new("reports/2024/report.pdf")?,
    )?;

    writer.finish()?;
    Ok(())
}
```

### From Memory

Add data directly from memory:

```rust
use zesven::{Writer, ArchivePath, Result};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;

    // Add string data
    let content = "Hello, World!";
    writer.add_bytes(ArchivePath::new("hello.txt")?, content.as_bytes())?;

    // Add binary data
    let data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03];
    writer.add_bytes(ArchivePath::new("data.bin")?, &data)?;

    writer.finish()?;
    Ok(())
}
```

### From Reader

Add data from any `Read` implementation:

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::EntryMeta;
use std::io::Cursor;

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;

    let mut data = Cursor::new(b"Stream content");
    let meta = EntryMeta::file(14);  // size in bytes
    writer.add_stream(ArchivePath::new("stream.txt")?, &mut data, meta)?;

    writer.finish()?;
    Ok(())
}
```

## Adding Directories

Create directory entries:

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::EntryMeta;

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;

    // Add an empty directory
    writer.add_directory(
        ArchivePath::new("empty_folder")?,
        EntryMeta::directory(),
    )?;

    // Add files within it
    writer.add_bytes(
        ArchivePath::new("empty_folder/readme.txt")?,
        b"Folder contents",
    )?;

    writer.finish()?;
    Ok(())
}
```

## Adding Directory Trees

Recursively add a directory:

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::EntryMeta;
use std::path::Path;
use walkdir::WalkDir;

fn main() -> Result<()> {
    let mut writer = Writer::create_path("project.7z")?;

    let base = Path::new("./my_project");
    for entry in WalkDir::new(base) {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(base)?;

        if path.is_dir() {
            writer.add_directory(
                ArchivePath::new(relative.to_str().unwrap())?,
                EntryMeta::directory(),
            )?;
        } else {
            writer.add_path(path, ArchivePath::new(relative.to_str().unwrap())?)?;
        }
    }

    writer.finish()?;
    Ok(())
}
```

## Archive Path Validation

`ArchivePath` validates paths for safety:

```rust
use zesven::ArchivePath;

// Valid paths
let path = ArchivePath::new("file.txt").unwrap();
let path = ArchivePath::new("folder/file.txt").unwrap();
let path = ArchivePath::new("a/b/c/deep.txt").unwrap();

// Invalid paths (will error)
assert!(ArchivePath::new("../escape.txt").is_err());  // Path traversal
assert!(ArchivePath::new("/absolute.txt").is_err());   // Absolute path
assert!(ArchivePath::new("").is_err());                 // Empty path
```

## Write Result

The `finish()` method returns statistics:

```rust
use zesven::{Writer, ArchivePath, Result};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?;
    writer.add_bytes(ArchivePath::new("data.txt")?, b"Some data to compress")?;

    let result = writer.finish()?;

    println!("Entries written: {}", result.entries_written);
    println!("Total size: {} bytes", result.total_size);
    println!("Compressed size: {} bytes", result.compressed_size);
    println!("Space savings: {:.1}%", result.space_savings() * 100.0);

    Ok(())
}
```

## Reproducible Archives

`deterministic(true)` makes the same input produce byte-identical output. It requires entries to arrive in sorted order and reports the entry that breaks it, rather than sorting the list for you:

```rust
use zesven::{ArchivePath, Result, WriteOptions, Writer};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?
        .options(WriteOptions::new().deterministic(true));

    // Sorted by archive path, or the second call returns an error.
    writer.add_bytes(ArchivePath::new("a.txt")?, b"first")?;
    writer.add_bytes(ArchivePath::new("z.txt")?, b"second")?;

    writer.finish()?;
    Ok(())
}
```

An entry's position in the file list is what binds it to its data, and the data has already been written by the time the last entry arrives - so sorting afterwards would pair names with the wrong contents. Sort your paths before adding them.

Reproducibility also needs an explicit thread count: see [Compression Options](./compression-options#multi-threading).

An encrypted archive is not reproducible whatever this is set to: every archive
draws a fresh nonce, which is what stops two archives of the same data under
the same password from being comparable. That property is worth more than
reproducibility, and this setting does not override it.

## Watching an Archive Being Written

Writing is quiet from the outside. Entries are gathered and compressed together, so the call that accepts an entry usually returns before anything has been compressed, and the work lands on whichever call fills the batch, or on `finish`. Timing the calls yourself shows that as one long pause among instant ones, which describes the batching rather than the work.

Ask to be told instead:

```rust
use zesven::progress::ProgressReporter;
use zesven::{ArchivePath, Result, write::Writer};

struct Watcher;

impl ProgressReporter for Watcher {
    fn on_entry_start(&mut self, name: &str, size: u64) {
        println!("working on {name} ({size} bytes)");
    }

    fn on_progress(&mut self, produced: u64, declared: u64) -> bool {
        println!("  {produced} of about {declared} bytes written");
        true // false asks the writer to stop
    }

    fn on_entry_complete(&mut self, name: &str, ok: bool) {
        println!("{name} is in (ok: {ok})");
    }
}

fn main() -> Result<()> {
    let mut writer = Writer::create_path("archive.7z")?.progress(Watcher);
    writer.add_bytes(ArchivePath::new("a.txt")?, b"hello")?;
    writer.finish()?;
    Ok(())
}
```

Each callback says what a writer can honestly say at that moment:

- **`on_entry_start`** comes before the work. For a batch it arrives for every entry in it at once, because they are compressed at the same time as each other and none of them is *the* one being worked on.
- **`on_progress`** covers a single entry large enough to be compressed on its own, and counts bytes of archive produced rather than bytes read. Such an entry is taken in far faster than it is compressed - reading can finish in a second and leave half a minute of compressing behind it - so a count of what had been read would reach the end immediately and then say nothing.
- **`on_ratio`** comes with each completed entry and covers the archive so far. There is no total to compare against: what the archive will hold depends on calls that have not been made yet.

Entries do not begin and end one at a time. A large entry is left finishing while the entry after it is read, and a batch is compressed while a large entry is, so an entry can be reported as started before the one before it is reported as finished. The order they finish in is the order they were added, because that is the order their bytes reach the archive.

Returning `false` from `on_progress`, or `true` from `should_cancel`, asks the writer to stop. It is honoured between entries rather than partway through one: the next entry offered is refused with `Error::Cancelled`, and what has been written stays a coherent archive rather than one that has to be thrown away.

## See Also

- [Compression Options](./compression-options) - Configure compression
- [Solid Archives](./solid-archives) - Better compression ratios
- [Encryption](../encryption/creating-encrypted) - Password protection
