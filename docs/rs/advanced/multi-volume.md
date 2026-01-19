---
title: Multi-Volume Archives
description: Work with split archives
---

# Multi-Volume Archives

Multi-volume archives split data across multiple files, useful for large backups or distribution on size-limited media.

## Reading Multi-Volume Archives

zesven automatically handles multi-volume archives:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    // Just open the first volume
    let mut archive = Archive::open_path("backup.7z.001")?;

    // Extraction spans all volumes automatically
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Volume Naming

Volumes follow the pattern:

- `archive.7z.001`
- `archive.7z.002`
- `archive.7z.003`
- ...

## Creating Multi-Volume Archives

Split archives by size using `VolumeConfig`:

```rust
use zesven::{Writer, VolumeConfig, ArchivePath, Result};

fn main() -> Result<()> {
    // Create config for 700 MB volumes (CD size)
    let config = VolumeConfig::new("backup.7z", 700 * 1024 * 1024);

    let mut writer = Writer::create_multivolume(config)?;

    // Add files as normal
    writer.add_path("large_file.iso", ArchivePath::new("large_file.iso")?)?;
    let result = writer.finish()?;

    // Creates: backup.7z.001, backup.7z.002, etc.
    println!("Created {} volumes", result.volume_count);
    Ok(())
}
```

## Volume Size Options

Common volume sizes:

| Size            | Use Case       |
| --------------- | -------------- |
| 1,457,664 bytes | 1.44 MB floppy |
| 700 MB          | CD-R           |
| 4.7 GB          | DVD            |
| 25 GB           | Blu-ray        |
| Custom          | Any size       |

```rust
use zesven::VolumeConfig;

// CD-sized volumes (convenience method)
let cd = VolumeConfig::cd("archive.7z");

// DVD-sized volumes (convenience method)
let dvd = VolumeConfig::dvd("archive.7z");

// Custom size
let custom = VolumeConfig::new("archive.7z", 100 * 1024 * 1024);

// Default 100 MB volumes
let default = VolumeConfig::with_default_size("archive.7z");
```

## Handling Missing Volumes

When a volume is missing:

```rust
use zesven::{Archive, Error, Result};

fn main() -> Result<()> {
    match Archive::open_path("backup.7z.001") {
        Ok(archive) => {
            println!("Archive opened successfully");
        }
        Err(Error::VolumeMissing { volume, path, .. }) => {
            eprintln!("Missing volume {}: {}", volume, path);
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
```

## See Also

- [Creating Archives](../writing/creating-archives) - Basic archive creation
- [7z Spec: Multi-Volume](/7z/14-multi-volume) - Format specification
