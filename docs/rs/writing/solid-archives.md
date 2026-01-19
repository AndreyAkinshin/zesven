---
title: Solid Archives
description: Maximize compression with solid blocks
---

# Solid Archives

Solid archives compress multiple files together, achieving better compression ratios by exploiting similarities between files.

## What is Solid Compression?

In a **non-solid** archive, each file is compressed independently:

```
file1.txt → [compressed data 1]
file2.txt → [compressed data 2]
file3.txt → [compressed data 3]
```

In a **solid** archive, files are concatenated and compressed together:

```
file1.txt + file2.txt + file3.txt → [single compressed block]
```

This allows the compressor to find patterns across files, significantly improving compression for similar files.

## Enabling Solid Mode

```rust
use zesven::{Writer, WriteOptions, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .solid()
        .level(7)?;

    let mut writer = Writer::create_path("solid.7z")?
        .options(options);

    // Files will be compressed together
    writer.add_bytes(ArchivePath::new("file1.txt")?, b"Hello World")?;
    writer.add_bytes(ArchivePath::new("file2.txt")?, b"Hello World")?;  // Similar content!
    writer.add_bytes(ArchivePath::new("file3.txt")?, b"Hello World")?;

    writer.finish()?;
    Ok(())
}
```

## Solid Block Size

Control how many files are grouped together:

```rust
use zesven::WriteOptions;
use zesven::write::SolidOptions;

let options = WriteOptions::new()
    .solid_options(SolidOptions::enabled().block_size(64 * 1024 * 1024));  // 64 MB blocks
```

| Block Size | Compression | Extraction Speed | Memory  |
| ---------- | ----------- | ---------------- | ------- |
| 1 MB       | Good        | Fast             | Low     |
| 16 MB      | Better      | Medium           | Medium  |
| 64 MB      | Best        | Slower           | High    |
| Unlimited  | Maximum     | Slowest          | Highest |

## Trade-offs

### Advantages

- **Better compression**: 10-50% smaller for similar files
- **Ideal for source code**: Many similar files benefit greatly
- **Ideal for backups**: Similar file versions compress well

### Disadvantages

- **Slower random access**: Must decompress from block start
- **Higher memory**: Entire block loaded for extraction
- **All-or-nothing**: Can't extract one file without processing the block

## When to Use Solid Mode

**Use solid mode for:**

- Source code archives
- Document collections
- Backup archives
- Distribution packages

**Avoid solid mode for:**

- Archives where individual files are frequently extracted
- Archives with unrelated file types
- Situations requiring low memory extraction

## Grouping Strategy

For best results, group similar files together:

```rust
use zesven::{Writer, WriteOptions, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new().solid();
    let mut writer = Writer::create_path("project.7z")?.options(options);

    // Group by file type for better compression
    // Source files together
    writer.add_path("src/main.rs", ArchivePath::new("src/main.rs")?)?;
    writer.add_path("src/lib.rs", ArchivePath::new("src/lib.rs")?)?;
    writer.add_path("src/utils.rs", ArchivePath::new("src/utils.rs")?)?;

    // Config files together
    writer.add_path("Cargo.toml", ArchivePath::new("Cargo.toml")?)?;
    writer.add_path("config.json", ArchivePath::new("config.json")?)?;

    // Binary files separately (don't compress well together with text)
    writer.add_path("icon.png", ArchivePath::new("icon.png")?)?;

    writer.finish()?;
    Ok(())
}
```

## Compression Comparison

Example with 100 similar source files:

| Mode      | Compressed Size | Ratio |
| --------- | --------------- | ----- |
| Non-solid | 2.5 MB          | 4:1   |
| Solid     | 800 KB          | 12:1  |

## See Also

- [Creating Archives](./creating-archives) - Basic archive creation
- [Compression Options](./compression-options) - Method and level settings
- [7z Spec: Solid Archives](/7z/13-solid-archives) - Format specification
