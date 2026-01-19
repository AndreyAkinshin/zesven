---
title: Reading Archives
description: Opening and extracting 7z archives
---

# Reading Archives

zesven provides a flexible API for reading 7z archives, from simple file extraction to advanced streaming operations.

## Overview

The main type for reading archives is `Archive<R>`, which is generic over any type that implements `Read + Seek`. This allows reading from files, memory buffers, or any other seekable source.

```rust
use zesven::{Archive, Result};
use std::fs::File;
use std::io::Cursor;

fn main() -> Result<()> {
    // From a file path (most common)
    let archive = Archive::open_path("archive.7z")?;

    // From an open file
    let file = File::open("archive.7z")?;
    let archive = Archive::open(file)?;

    // From memory
    let data = std::fs::read("archive.7z")?;
    let archive = Archive::open(Cursor::new(data))?;

    Ok(())
}
```

## Key Types

| Type             | Description                                   |
| ---------------- | --------------------------------------------- |
| `Archive<R>`     | Main archive reader, generic over reader type |
| `Entry`          | Metadata for a single file or directory       |
| `ExtractOptions` | Configuration for extraction operations       |
| `ExtractResult`  | Statistics from an extraction operation       |

## Basic Workflow

1. **Open** the archive with `Archive::open_path()` or `Archive::open()`
2. **Inspect** entries using `archive.entries()` or `archive.len()`
3. **Extract** files using `archive.extract()` or `archive.extract_entry()`

## Topics

- [Opening Archives](./opening-archives) - Different ways to open archives
- [Extracting Files](./extracting) - Extract to disk or memory
- [Selective Extraction](./selective-extraction) - Filter which files to extract
- [Progress Callbacks](./progress-callbacks) - Monitor extraction progress

## Quick Example

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Print archive info
    println!("Archive: {} entries, {} total bytes",
        archive.len(),
        archive.entries().map(|e| e.size).sum::<u64>());

    // Extract with default options
    let result = archive.extract("./output", (), &ExtractOptions::default())?;

    println!("Extracted {} files ({} bytes)",
        result.entries_extracted, result.bytes_extracted);
    Ok(())
}
```
