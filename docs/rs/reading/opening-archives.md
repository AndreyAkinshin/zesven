---
title: Opening Archives
description: Different ways to open 7z archives
---

# Opening Archives

zesven provides several ways to open archives depending on your source and requirements.

## From File Path

The simplest way to open an archive:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    println!("Entries: {}", archive.len());
    Ok(())
}
```

This automatically handles multi-volume archives (`.7z.001`, `.7z.002`, etc.).

## From Any Reader

For more control, open from any type implementing `Read + Seek`:

```rust
use zesven::{Archive, Result};
use std::fs::File;
use std::io::{BufReader, Cursor};

fn main() -> Result<()> {
    // From an open file
    let file = File::open("archive.7z")?;
    let archive = Archive::open(file)?;

    // With buffering
    let file = File::open("archive.7z")?;
    let archive = Archive::open(BufReader::new(file))?;

    // From memory
    let data = std::fs::read("archive.7z")?;
    let archive = Archive::open(Cursor::new(data))?;

    Ok(())
}
```

## Password-Protected Archives

For encrypted archives, provide a password:

```rust
use zesven::{Archive, Password, Result};

fn main() -> Result<()> {
    let archive = Archive::open_path_with_password(
        "encrypted.7z",
        Password::new("secret"),
    )?;
    Ok(())
}
```

The password is required if either the data or headers are encrypted.

## Archive Information

Once opened, you can inspect the archive:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;

    // Number of entries
    println!("Entries: {}", archive.len());

    // Check if empty
    if archive.is_empty() {
        println!("Archive is empty");
    }

    // Iterate entries
    for entry in archive.entries() {
        println!("{}: {} bytes", entry.path.as_str(), entry.size);
    }

    Ok(())
}
```

## Entry Properties

Each `Entry` contains metadata about a file:

| Property            | Type                    | Description                          |
| ------------------- | ----------------------- | ------------------------------------ |
| `path`              | `ArchivePath`           | Path within the archive              |
| `size`              | `u64`                   | Uncompressed size in bytes           |
| `is_directory`      | `bool`                  | Whether entry is a directory         |
| `crc32`             | `Option<u32>`           | CRC-32 checksum                      |
| `modification_time` | `Option<u64>`           | Modification time (Windows FILETIME) |
| `creation_time`     | `Option<u64>`           | Creation time (Windows FILETIME)     |
| `access_time`       | `Option<u64>`           | Access time (Windows FILETIME)       |
| `attributes`        | `Option<u32>`           | Windows file attributes              |
| `is_encrypted`      | `bool`                  | Whether entry is encrypted           |
| `is_symlink`        | `bool`                  | Whether entry is a symbolic link     |
| `is_anti`           | `bool`                  | Whether entry is an anti-item        |
| `ownership`         | `Option<UnixOwnership>` | Unix file ownership (UID, GID)       |

**Helper methods** for timestamp conversion:

- `modified()` / `created()` / `accessed()` → `Option<SystemTime>`
- `modification_timestamp()` / `creation_timestamp()` / `access_timestamp()` → `Option<Timestamp>`

## Error Handling

Common errors when opening archives:

```rust
use zesven::{Archive, Error, Result};

fn open_safe(path: &str) -> Result<()> {
    match Archive::open_path(path) {
        Ok(_archive) => {
            println!("Opened successfully");
            Ok(())
        }
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("File not found: {}", path);
            Err(Error::Io(e))
        }
        Err(Error::InvalidFormat(msg)) => {
            eprintln!("Invalid 7z format: {}", msg);
            Err(Error::InvalidFormat(msg))
        }
        Err(e @ Error::WrongPassword { .. }) => {
            eprintln!("Incorrect password or archive is encrypted");
            Err(e)
        }
        Err(e) => Err(e),
    }
}
```

## See Also

- [Extracting Files](./extracting) - Extract files from opened archives
- [Selective Extraction](./selective-extraction) - Filter which files to extract
- [Encryption](../encryption/) - Working with encrypted archives
