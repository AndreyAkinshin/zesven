---
title: Streaming Configuration
description: Configure memory limits for streaming operations
---

# Streaming Configuration

Control memory usage with `StreamingConfig`.

## Basic Configuration

```rust
use zesven::{streaming::{StreamingArchive, StreamingConfig}, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(64 * 1024 * 1024);  // 64 MiB

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("archive.7z", "", config)?;
    Ok(())
}
```

## Configuration Options

### Memory Buffer Size

Maximum memory for decompression buffers:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::default()
    .max_memory_buffer(128 * 1024 * 1024);  // 128 MiB
```

### Decoder Pool Capacity

Number of decoders to keep in memory for solid archives:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::default()
    .decoder_pool_capacity(Some(4));  // Keep 4 decoders cached
```

### Read Buffer Size

Size of I/O read buffers:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::default()
    .read_buffer_size(64 * 1024);  // 64 KiB read buffer
```

### CRC Verification

Control CRC checksum verification:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::default()
    .verify_crc(true);  // Enabled by default
```

### Progress Tracking

Enable or disable progress information:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::default()
    .track_progress(true);  // Enabled by default
```

## Combined Configuration

```rust
use zesven::{streaming::{StreamingArchive, StreamingConfig}, Result};

fn main() -> Result<()> {
    let config = StreamingConfig::default()
        .max_memory_buffer(64 * 1024 * 1024)
        .decoder_pool_capacity(Some(2))
        .read_buffer_size(32 * 1024)
        .verify_crc(true);

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("archive.7z", "", config)?;
    Ok(())
}
```

## Automatic Configuration

Use system information to configure automatically (requires `sysinfo` feature):

```rust
use zesven::{streaming::{StreamingArchive, StreamingConfig}, Result};

fn main() -> Result<()> {
    // Auto-size based on available system RAM
    let config = StreamingConfig::auto_sized();

    // With default features (aes enabled), pass empty string for unencrypted archives
    let archive = StreamingArchive::open_path_with_config("archive.7z", "", config)?;
    Ok(())
}
```

## Configuration Presets

```rust
use zesven::streaming::StreamingConfig;

// Low memory (embedded systems)
let low = StreamingConfig::low_memory();

// Default balanced configuration
let balanced = StreamingConfig::default();

// High performance (more memory for faster processing)
let high = StreamingConfig::high_performance();
```

Or build custom configurations:

```rust
use zesven::streaming::StreamingConfig;

// Custom low memory
let low = StreamingConfig::default()
    .max_memory_buffer(16 * 1024 * 1024)
    .decoder_pool_capacity(Some(1));

// Custom high performance
let high = StreamingConfig::default()
    .max_memory_buffer(512 * 1024 * 1024)
    .decoder_pool_capacity(Some(8));
```

## Validation

Validate configuration before use:

```rust
use zesven::streaming::StreamingConfig;

let config = StreamingConfig::new()
    .max_memory_buffer(64 * 1024 * 1024)
    .read_buffer_size(32 * 1024);

config.validate()?;  // Returns error if invalid
# Ok::<(), zesven::Error>(())
```

## See Also

- [Streaming Overview](./) - Streaming API concepts
- [Memory Management](./memory-management) - How memory is used
