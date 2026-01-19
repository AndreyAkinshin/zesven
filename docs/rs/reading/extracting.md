---
title: Extracting Files
description: Extract files from 7z archives
---

# Extracting Files

zesven provides flexible options for extracting files from archives.

## Basic Extraction

Extract all files to a directory:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Extract Options

Customize extraction behavior with `ExtractOptions`:

```rust
use zesven::{Archive, ExtractOptions, Result};
use zesven::read::{PathSafety, OverwritePolicy, PreserveMetadata};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let options = ExtractOptions::new()
        .path_safety(PathSafety::Strict)           // Prevent path traversal
        .overwrite(OverwritePolicy::Overwrite)     // Overwrite existing files
        .preserve_metadata(PreserveMetadata::all()); // Keep original timestamps

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

### Available Options

| Option              | Default                     | Description                     |
| ------------------- | --------------------------- | ------------------------------- |
| `path_safety`       | `PathSafety::Strict`        | Path traversal protection level |
| `overwrite`         | `OverwritePolicy::Error`    | Policy for existing files       |
| `preserve_metadata` | `PreserveMetadata::none()`  | Metadata preservation options   |
| `link_policy`       | `LinkPolicy::Forbid`        | Symbolic link handling policy   |
| `limits`            | `ResourceLimits::default()` | Resource limits for extraction  |
| `password`          | `None`                      | Password for encrypted entries  |

## Extract Single Entry

Extract a specific file by name:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract specific file to bytes
    let data = archive.extract_to_vec("important.txt")?;
    std::fs::write("important.txt", &data)?;

    Ok(())
}
```

## Extract to Memory

Extract file contents to a byte vector:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract specific file to memory
    let data = archive.extract_to_vec("config.json")?;
    let config = String::from_utf8(data)?;
    println!("Config: {}", config);

    Ok(())
}
```

## Extraction Results

The `extract()` method returns statistics about the operation:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    let result = archive.extract("./output", (), &ExtractOptions::default())?;

    println!("Entries extracted: {}", result.entries_extracted);
    println!("Entries skipped: {}", result.entries_skipped);
    println!("Bytes extracted: {}", result.bytes_extracted);
    println!("Failures: {}", result.entries_failed);

    for (path, error) in &result.failures {
        eprintln!("Failed {}: {}", path, error);
    }
    Ok(())
}
```

## Path Safety

zesven protects against path traversal attacks by default:

```rust
use zesven::{ExtractOptions, read::PathSafety};

// Strict (default): Reject paths with "..", absolute paths, etc.
let options = ExtractOptions::new().path_safety(PathSafety::Strict);

// Relaxed: Allow some edge cases but still prevent obvious attacks
let options = ExtractOptions::new().path_safety(PathSafety::Relaxed);

// Disabled: No protection (dangerous - use only with trusted archives)
let options = ExtractOptions::new().path_safety(PathSafety::Disabled);
```

::: warning
Setting `PathSafety::Disabled` can allow malicious archives to overwrite arbitrary files. Only use with archives you completely trust.
:::

## See Also

- [Selective Extraction](./selective-extraction) - Filter which files to extract
- [Progress Callbacks](./progress-callbacks) - Monitor extraction progress
- [Path Safety](../safety/path-safety) - Security considerations
