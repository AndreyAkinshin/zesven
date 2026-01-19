---
title: Memory Management
description: Understanding memory usage in streaming operations
---

# Memory Management

Understanding how zesven manages memory during streaming operations.

## Memory Components

### Decompression Buffers

Memory required to decompress data:

- **LZMA/LZMA2**: Dictionary size (typically 16-64 MB)
- **Deflate**: ~32 KB
- **BZip2**: ~900 KB per block
- **PPMd**: Model size (typically 16-256 MB)

### Solid Block Buffers

For solid archives, the entire block must be decompressed to access any file within it.

### Decoder Pool

Cached decoders for solid archives to avoid re-decompressing from the start.

## Estimating Memory

Estimate memory before processing using `StreamingConfig`:

```rust
use zesven::streaming::{CompressionMethod, MemoryEstimate};

fn main() {
    // Estimate for LZMA2 with 64 MB dictionary
    let estimate = CompressionMethod::Lzma2.estimate_decoder_memory(Some(64 * 1024 * 1024));
    println!("Minimum memory: {} MB", estimate.minimum / 1024 / 1024);
    println!("Typical memory: {} MB", estimate.typical / 1024 / 1024);
    println!("Maximum memory: {} MB", estimate.maximum / 1024 / 1024);
}
```

## Memory Tracking

Track memory usage during extraction via the archive's internal tracker:

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(64 * 1024 * 1024);  // 64 MB limit

    // With default features (aes enabled), pass empty string for unencrypted archives
    let mut archive = StreamingArchive::open_path_with_config("archive.7z", "", config)?;

    // Access the internal memory tracker
    let tracker = archive.memory_tracker();
    println!("Memory limit: {} MB", tracker.limit() / 1024 / 1024);

    for entry in archive.entries()? {
        let entry = entry?;
        let tracker = archive.memory_tracker();
        println!("Current memory: {} MB", tracker.current_usage() / 1024 / 1024);
        println!("Peak memory: {} MB", tracker.peak_usage() / 1024 / 1024);
    }

    Ok(())
}
```

## Memory Limits

Prevent excessive memory allocation by configuring limits:

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(64 * 1024 * 1024);  // Limit to 64 MB

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("archive.7z", "", config)?;
    Ok(())
}
```

When the memory limit is exceeded during decompression, an error is returned.

## Solid Archive Strategies

For solid archives with large blocks:

### Sequential Processing

Process files in archive order to minimize memory:

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .decoder_pool_capacity(Some(1));  // Minimal caching

    // With default features (aes enabled), pass empty string for unencrypted archives
    let mut archive = StreamingArchive::open_path_with_config("solid.7z", "", config)?;

    // Process in order - each file decompresses from where previous stopped
    for entry in archive.entries()? {
        let mut entry = entry?;
        // Process entry...
    }
    Ok(())
}
```

### Cached Decoders

For random access patterns, cache more decoders:

```rust
use zesven::{StreamingArchive, StreamingConfig, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .decoder_pool_capacity(Some(4))  // Cache 4 block states
        .max_memory_buffer(256 * 1024 * 1024);

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("solid.7z", "", config)?;
    Ok(())
}
```

## See Also

- [Streaming Overview](./) - Streaming API concepts
- [Configuration](./config) - Configure memory limits
- [Resource Limits](../safety/resource-limits) - Protect against zip bombs
