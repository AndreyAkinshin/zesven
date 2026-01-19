---
title: Resource Limits
description: Protection against zip bombs and excessive resource usage
---

# Resource Limits

zesven protects against zip bombs and excessive resource usage with configurable limits.

## Default Limits

zesven applies sensible defaults:

| Limit                | Default   | Description                       |
| -------------------- | --------- | --------------------------------- |
| `max_entries`        | 1,000,000 | Maximum files in archive          |
| `max_header_bytes`   | 64 MiB    | Maximum header size               |
| `max_total_unpacked` | 1 TiB     | Total extracted size              |
| `max_entry_unpacked` | 64 GiB    | Maximum single entry size         |
| `ratio_limit`        | 1000:1    | Max decompression ratio (default) |

## Configuring Limits

```rust
use zesven::Result;
use zesven::read::ExtractOptions;
use zesven::format::streams::ResourceLimits;

fn main() -> Result<()> {
    let limits = ResourceLimits::new()
        .max_entries(100_000)
        .max_total_unpacked(1024 * 1024 * 1024);  // 1 GB

    let options = ExtractOptions::new()
        .limits(limits);

    Ok(())
}
```

## Zip Bomb Protection

Zip bombs exploit compression to create files that expand to enormous sizes:

```rust
use zesven::{Archive, Result};
use zesven::read::ExtractOptions;
use zesven::format::streams::ResourceLimits;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("untrusted.7z")?;

    let limits = ResourceLimits::new()
        .max_total_unpacked(100 * 1024 * 1024);  // Max 100 MB output

    let options = ExtractOptions::new()
        .limits(limits);

    match archive.extract("./output", (), &options) {
        Ok(_) => println!("Extraction complete"),
        Err(e) => {
            eprintln!("Extraction failed: {}", e);
            return Err(e);
        }
    }
    Ok(())
}
```

## Entry Count Limits

Limit the number of entries to prevent directory exhaustion:

```rust
use zesven::read::ExtractOptions;
use zesven::format::streams::ResourceLimits;

let limits = ResourceLimits::new()
    .max_entries(10_000);  // Max 10,000 files

let options = ExtractOptions::new()
    .limits(limits);
```

## Memory Limits

Control memory usage during decompression:

```rust
use zesven::StreamingConfig;

let config = StreamingConfig::default()
    .max_memory_buffer(64 * 1024 * 1024);  // 64 MB max
```

## Checking Limits Before Extraction

Pre-check without extracting:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;

    // Check without extracting
    if archive.len() > 1000 {
        eprintln!("Too many entries");
        return Ok(());
    }

    let total_size: u64 = archive.entries().map(|e| e.size).sum();
    if total_size > 100 * 1024 * 1024 {
        eprintln!("Total size too large: {} bytes", total_size);
        return Ok(());
    }

    println!("Archive is safe to extract");
    Ok(())
}
```

## Disabling Limits (Dangerous)

::: danger
Only disable limits for trusted archives and when you have sufficient resources.
:::

```rust
use zesven::read::ExtractOptions;
use zesven::format::streams::ResourceLimits;

let limits = ResourceLimits::unlimited();  // No limits

let options = ExtractOptions::new()
    .limits(limits);
```

## Error Handling

```rust
use zesven::{Archive, Error, Result};
use zesven::read::ExtractOptions;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    match archive.extract("./output", (), &ExtractOptions::default()) {
        Ok(_) => println!("Success"),
        Err(Error::ResourceLimitExceeded(msg)) => {
            eprintln!("Limit exceeded: {}", msg);
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
```

## See Also

- [Safety Overview](./) - Security features
- [Path Safety](./path-safety) - Path traversal protection
- [Memory Management](../streaming/memory-management) - Memory control
