---
title: Streaming API
description: Memory-efficient processing of large archives
---

# Streaming API

The streaming API enables memory-efficient processing of large archives by controlling how data flows through the system.

## Overview

For large archives, especially solid archives with big blocks, the standard API may require significant memory. The streaming API provides fine-grained control over memory usage.

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(64 * 1024 * 1024);  // 64 MB limit

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("large.7z", "", config)?;

    for entry in archive.entries()? {
        let entry = entry?;
        println!("Processing: {}", entry.name());
    }
    Ok(())
}
```

## When to Use Streaming

Use the streaming API when:

- Processing archives larger than available RAM
- Working with solid archives containing large blocks
- Need to control memory usage precisely
- Processing entries one at a time without random access

## Key Types

| Type               | Description                      |
| ------------------ | -------------------------------- |
| `StreamingArchive` | Memory-efficient archive reader  |
| `StreamingConfig`  | Memory and buffer configuration  |
| `StreamingEntry`   | Entry with streaming data access |
| `MemoryEstimate`   | Estimate memory requirements     |

## Topics

- [Configuration](./config) - Configure memory limits
- [Memory Management](./memory-management) - Understanding memory usage

## Basic Example

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};
use std::fs::File;

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(32 * 1024 * 1024);

    // With default features (aes enabled), pass empty string for unencrypted archives
    let mut archive = StreamingArchive::open_path_with_config("large.7z", "", config)?;
    let mut iter = archive.entries()?;

    while let Some(entry_result) = iter.next() {
        let entry = entry_result?;
        if !entry.is_directory() {
            let path = format!("./output/{}", entry.name());
            std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap())?;
            let mut file = File::create(&path)?;
            iter.extract_current_to(&mut file)?;
        }
    }
    Ok(())
}
```

## Memory vs. Standard API

| Feature        | Standard API | Streaming API   |
| -------------- | ------------ | --------------- |
| Random access  | Yes          | No              |
| Memory usage   | Higher       | Controlled      |
| Speed          | Faster       | Slightly slower |
| API complexity | Simple       | More complex    |

## See Also

- [Configuration](./config) - Memory configuration options
- [Memory Management](./memory-management) - How memory is managed
