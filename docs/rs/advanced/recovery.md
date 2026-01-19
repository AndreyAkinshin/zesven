---
title: Archive Recovery
description: Recover data from damaged archives
---

# Archive Recovery

zesven provides tools to recover data from damaged or corrupted archives.

## Basic Recovery

```rust
use zesven::recovery::{recover_archive, RecoveryOptions, RecoveryStatus};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let file = File::open("damaged.7z")?;
    let result = recover_archive(file, RecoveryOptions::default())?;

    match result.status {
        RecoveryStatus::FullRecovery => {
            println!("All {} entries recovered!", result.recovered_count());
        }
        RecoveryStatus::PartialRecovery => {
            println!("Recovered {} entries, {} failed",
                result.recovered_count(),
                result.failed_count());
        }
        RecoveryStatus::HeaderRecovered => {
            println!("Header recovered but some data is corrupt");
        }
        RecoveryStatus::Failed => {
            println!("Could not recover archive");
        }
    }
    Ok(())
}
```

## Recovery Options

Configure recovery behavior:

```rust
use zesven::recovery::RecoveryOptions;

let options = RecoveryOptions::new()
    .search_limit(2 * 1024 * 1024)   // Search 2 MiB for signatures
    .validate_crcs(false)             // Skip CRC validation
    .skip_corrupt_entries(true)       // Continue past corrupt entries
    .try_multiple_headers(true);      // Try alternative header locations
```

### Recovery Options

| Option                 | Default | Description                        |
| ---------------------- | ------- | ---------------------------------- |
| `search_limit`         | 1 MiB   | Max bytes to search for signatures |
| `validate_crcs`        | true    | Validate CRC checksums             |
| `skip_corrupt_entries` | false   | Continue past corrupt entries      |
| `try_multiple_headers` | false   | Try multiple header locations      |

## Signature Scanner

Find archives embedded in other files:

```rust
use zesven::recovery::{find_all_signatures, SignatureScanner};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let mut file = File::open("disk_image.bin")?;
    let offsets = find_all_signatures(&mut file, Some(10 * 1024 * 1024))?;

    for offset in offsets {
        println!("Found 7z signature at offset {}", offset);
    }
    Ok(())
}
```

## Recovery Result

The `RecoveryResult` provides detailed information about the recovery:

```rust
use zesven::recovery::{recover_archive, RecoveryOptions};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let file = File::open("damaged.7z")?;
    let result = recover_archive(file, RecoveryOptions::default())?;

    // Check recovered entries
    for entry in &result.recovered_entries {
        println!("Recovered: {} ({} bytes, CRC valid: {})",
            entry.path, entry.size, entry.crc_valid);
    }

    // Check failed entries
    for entry in &result.failed_entries {
        let path = entry.path.as_deref().unwrap_or("unknown");
        println!("Failed: {} - {}", path, entry.reason);
    }

    // Check warnings
    for warning in &result.warnings {
        println!("Warning: {}", warning);
    }

    // Recovery statistics
    println!("Recovery rate: {:.1}%", result.recovery_rate() * 100.0);
    Ok(())
}
```

## Recovery Status

| Status            | Description                           |
| ----------------- | ------------------------------------- |
| `FullRecovery`    | All entries recovered successfully    |
| `PartialRecovery` | Some entries recovered, others failed |
| `HeaderRecovered` | Header parsed but data is corrupt     |
| `Failed`          | Could not recover archive             |

## Quick Validation

Check if a file is a valid 7z archive:

```rust
use zesven::recovery::is_valid_archive;
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let mut file = File::open("archive.7z")?;
    if is_valid_archive(&mut file)? {
        println!("Valid 7z archive");
    } else {
        println!("Not a valid 7z archive");
    }
    Ok(())
}
```

## Common Corruption Types

| Corruption      | Recoverability                   |
| --------------- | -------------------------------- |
| Truncated file  | Often partial recovery           |
| Header damage   | May recover data blocks          |
| CRC errors      | Data recoverable, may be corrupt |
| Missing volumes | Other volumes recoverable        |

## See Also

- [Error Handling](../reference/error-handling) - Handle recovery errors
- [7z Spec: Error Conditions](/7z/18-error-conditions) - Error types
