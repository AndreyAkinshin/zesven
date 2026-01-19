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

## See Also

- [Compression Options](./compression-options) - Configure compression
- [Solid Archives](./solid-archives) - Better compression ratios
- [Encryption](../encryption/creating-encrypted) - Password protection
