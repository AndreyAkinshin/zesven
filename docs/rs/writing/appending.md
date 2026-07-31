---
title: Appending to Archives
description: Add files to existing archives
---

# Appending to Archives

zesven adds files to an existing archive by rebuilding it beside the old one
and moving the result into place.

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

1. A new archive is built beside the old one
2. Every existing entry is decompressed and compressed again into it
3. The new files are compressed into it as well
4. The finished archive is moved onto the old one, keeping its permissions

Existing entries **are** recompressed, and each one is held in memory whole
while that happens, so appending to an archive containing a multi-gigabyte
entry needs room for that entry. A failure at any point leaves the original
archive exactly as it was.

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
