---
title: Editing Archives
description: Modify existing archives
---

# Editing Archives

zesven allows modifying existing archives without full recompression.

## ArchiveEditor

The `ArchiveEditor` type provides archive modification:

```rust
use zesven::{Archive, ArchivePath, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    // Open the archive first
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    // Queue operations using direct methods
    editor.delete("unwanted.txt")?;
    editor.rename("old.txt", "new.txt")?;

    // Apply all operations to a new file
    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

## Operations

### Delete Files

Remove files from an archive:

```rust
use zesven::{Archive, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    editor.delete("temp.txt")?;
    editor.delete("logs/debug.log")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

### Rename Files

Change file paths within an archive:

```rust
use zesven::{Archive, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    editor.rename("old_name.txt", "new_name.txt")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

### Add Files

Add new files to an archive:

```rust
use zesven::{Archive, ArchivePath, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    // Add content from bytes
    editor.add(ArchivePath::new("readme.txt")?, b"New readme content")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

### Update Files

Replace existing files:

```rust
use zesven::{Archive, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    editor.update("config.json", b"{\"version\": 2}")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

## Batch Operations

Multiple operations are executed efficiently:

```rust
use zesven::{Archive, ArchivePath, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    // All operations queued before apply
    for i in 0..10 {
        editor.delete(&format!("temp_{}.txt", i))?;
    }

    editor.add(
        ArchivePath::new("summary.txt")?,
        b"Cleaned up temp files",
    )?;

    // Single apply pass
    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

## Encrypted Archives

For encrypted archives, open the archive with a password first:

```rust
use zesven::{Archive, Password, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path_with_password(
        "encrypted.7z",
        Password::new("secret"),
    )?;
    let mut editor = ArchiveEditor::new(archive);

    editor.delete("file.txt")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

## Limitations

- **Solid archives**: Modifying files in solid blocks may require recompression
- **Encrypted headers**: Must provide password for any operation
- **Large deletions**: Many small deletions may fragment the archive

## See Also

- [Appending](../writing/appending) - Add files without modifying existing content
- [Creating Archives](../writing/creating-archives) - Create new archives
