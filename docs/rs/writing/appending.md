---
title: Appending to Archives
description: Add files to existing archives
---

# Appending to Archives

zesven allows adding files to existing archives without full recompression.

## Using ArchiveAppender

Open an existing archive for appending:

```rust
use zesven::{ArchiveAppender, ArchivePath, Result};

fn main() -> Result<()> {
    let mut appender = ArchiveAppender::open("existing.7z")?;

    // Add new files
    appender.add_bytes(ArchivePath::new("new_file.txt")?, b"New content")?;
    appender.add_path("document.pdf", ArchivePath::new("docs/document.pdf")?)?;

    // Finalize
    appender.finish()?;
    Ok(())
}
```

## How Appending Works

When you append to an archive:

1. The existing archive structure is read
2. New files are compressed as a new folder (block)
3. The archive header is rewritten to include both old and new entries

Original data is **not** recompressed, making appends efficient.

## Append Options

Configure compression for new files:

```rust
use zesven::{ArchiveAppender, WriteOptions, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .level(7)?
        .solid();

    let mut appender = ArchiveAppender::open("archive.7z")?
        .with_options(options);

    appender.add_bytes(ArchivePath::new("data.txt")?, b"Compressed data")?;
    appender.finish()?;
    Ok(())
}
```

## Appending with Custom Options

Configure compression options for appended files:

```rust
use zesven::{ArchiveAppender, WriteOptions, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .level(9)?;

    let mut appender = ArchiveAppender::open("archive.7z")?
        .with_options(options);

    appender.add_bytes(ArchivePath::new("new_file.txt")?, b"New data")?;
    appender.finish()?;
    Ok(())
}
```

## Limitations

- **No duplicate handling**: Files with the same path create duplicates
- **No solid merging**: New files form a separate solid block
- **Header size grows**: Each append adds header overhead

For archives that need frequent updates, consider using `ArchiveEditor` instead.

## When to Append vs. Recreate

**Use appending when:**

- Adding a few files to a large archive
- Speed is more important than optimal compression
- You don't need to remove or update existing files

**Recreate the archive when:**

- Many files need to be added
- Existing files need to be updated or removed
- Optimal compression ratio is important
- The archive has accumulated many appends

## Append Result

```rust
use zesven::{ArchiveAppender, ArchivePath, Result};

fn main() -> Result<()> {
    let mut appender = ArchiveAppender::open("archive.7z")?;
    appender.add_bytes(ArchivePath::new("data.txt")?, b"Data")?;

    let result = appender.finish()?;

    println!("Files added: {}", result.entries_added);
    println!("Total entries: {}", result.total_entries);
    println!("Total bytes: {}", result.total_bytes);

    Ok(())
}
```

## See Also

- [Creating Archives](./creating-archives) - Create new archives
- [Editing Archives](../advanced/editing) - Update or remove files
