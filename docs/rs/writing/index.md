---
title: Writing Archives
description: Creating and modifying 7z archives
---

# Writing Archives

zesven provides a flexible API for creating 7z archives with various compression options.

## Overview

The main type for creating archives is `Writer<W>`, which writes to any type implementing `Write + Seek`.

```rust
use zesven::{Writer, ArchivePath, Result};
use std::fs::File;
use std::io::Cursor;

fn main() -> Result<()> {
    // Create at a file path (most common)
    let writer = Writer::create_path("archive.7z")?;

    // Write to an existing file
    let file = File::create("archive.7z")?;
    let writer = Writer::create(file)?;

    // Write to memory
    let buffer = Cursor::new(Vec::new());
    let writer = Writer::create(buffer)?;

    Ok(())
}
```

## Key Types

| Type           | Description                                     |
| -------------- | ----------------------------------------------- |
| `Writer<W>`    | Main archive writer, generic over writer type   |
| `WriteOptions` | Configuration for compression, encryption, etc. |
| `WriteResult`  | Statistics from a completed write operation     |
| `ArchivePath`  | Validated path for archive entries              |

## Basic Workflow

1. **Create** a writer with `Writer::create_path()` or `Writer::create()`
2. **Configure** options with `.options()` if needed
3. **Add** files with `add_path()`, `add_bytes()`, or `add_directory()`
4. **Finish** with `.finish()` to write the archive

## Topics

- [Creating Archives](./creating-archives) - Basic archive creation
- [Compression Options](./compression-options) - Configure compression methods
- [Solid Archives](./solid-archives) - Maximize compression ratio
- [Appending](./appending) - Add files to existing archives

## Quick Example

```rust
use zesven::{Writer, WriteOptions, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .level(7)?          // Compression level 0-9
        .solid();           // Enable solid compression

    let mut writer = Writer::create_path("backup.7z")?
        .options(options);

    // Add files from disk
    writer.add_path("document.pdf", ArchivePath::new("docs/document.pdf")?)?;
    writer.add_path("image.png", ArchivePath::new("images/photo.png")?)?;

    // Add data from memory
    writer.add_bytes(ArchivePath::new("readme.txt")?, b"Project readme")?;

    // Finalize
    let result = writer.finish()?;
    println!("Compressed {} bytes to {} bytes ({:.1}% savings)",
        result.total_size,
        result.compressed_size,
        result.space_savings() * 100.0);
    Ok(())
}
```
