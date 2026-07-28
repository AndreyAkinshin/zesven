---
title: Compression Options
description: Configure compression methods and levels
---

# Compression Options

zesven supports multiple compression methods with configurable levels.

## Setting Options

Use `WriteOptions` to configure compression:

```rust
use zesven::{Writer, WriteOptions, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .level(7)?;  // 0 = fastest setting of the codec, 9 = maximum compression

    let writer = Writer::create_path("archive.7z")?
        .options(options);

    Ok(())
}
```

## Compression Levels

| Level | Description            | Speed   | Ratio  |
| ----- | ---------------------- | ------- | ------ |
| 0     | Codec's fastest setting | Fastest | Low   |
| 1-3   | Fast compression       | Fast    | Low    |
| 4-6   | Normal compression     | Medium  | Medium |
| 7-9   | Maximum compression    | Slow    | High   |

```rust
use zesven::{WriteOptions, Result};

fn main() -> Result<()> {
    // Fast compression for large files
    let fast = WriteOptions::new().level(1)?;

    // Maximum compression for final archives
    let maximum = WriteOptions::new().level(9)?;

    // Balanced (default)
    let balanced = WriteOptions::new().level(5)?;

    Ok(())
}
```

A level configures the codec; it does not switch it off. `level(0)` with BZip2
is BZip2 at its lowest setting, and still compresses. To store an entry
unchanged, use `CodecMethod::Copy`.

## Compression Methods

### LZMA2 (Default)

The default and most commonly used method:

```rust
use zesven::{Writer, WriteOptions, codec::CodecMethod, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::Lzma2)
        .level(7)?;

    let writer = Writer::create_path("archive.7z")?
        .options(options);

    Ok(())
}
```

### LZMA

Original LZMA algorithm:

```rust
use zesven::{WriteOptions, codec::CodecMethod, Result};

fn example() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::Lzma)
        .level(7)?;
    Ok(())
}
```

### Deflate

Compatible with ZIP, faster but lower ratio:

```rust
use zesven::{WriteOptions, codec::CodecMethod, Result};

fn example() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::Deflate)
        .level(6)?;
    Ok(())
}
```

### BZip2

Good for text files:

```rust
use zesven::{WriteOptions, codec::CodecMethod, Result};

fn example() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::BZip2)
        .level(9)?;
    Ok(())
}
```

### PPMd

Excellent for text, high memory usage:

```rust
use zesven::{WriteOptions, codec::CodecMethod, Result};

fn example() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::PPMd)
        .level(8)?;
    Ok(())
}
```

### Optional Methods

Enable additional methods via feature flags:

```toml
[dependencies]
zesven = { version = "2.0", features = ["zstd", "lz4", "brotli"] }
```

```rust
use zesven::{WriteOptions, codec::CodecMethod, Result};

fn example() -> Result<()> {
    // Zstandard - fast with good ratio (level range differs from LZMA)
    let options = WriteOptions::new()
        .method(CodecMethod::Zstd)
        .level_clamped(9);

    // LZ4 - extremely fast
    let options = WriteOptions::new()
        .method(CodecMethod::Lz4);

    // Brotli - excellent ratio for web content
    let options = WriteOptions::new()
        .method(CodecMethod::Brotli)
        .level_clamped(9);

    Ok(())
}
```

## Dictionary Size

The dictionary size is automatically determined based on compression level. Higher levels use larger dictionaries:

| Level | Approximate Dictionary | Memory Usage | Best For     |
| ----- | ---------------------- | ------------ | ------------ |
| 1-3   | 1-4 MB                 | ~12-48 MB    | Small files  |
| 4-6   | 4-16 MB                | ~48-192 MB   | Medium files |
| 7-9   | 16-64 MB               | ~192-768 MB  | Large files  |

## Multi-threading

Parallel compression is enabled by default with the `parallel` feature, and works two ways. The entries of a non-solid archive are compressed alongside each other, since each is its own folder; a solid block is instead cut into chunks that are compressed in parallel.

Use `threads` to bound it:

```rust
use zesven::{Threads, WriteOptions};

// Use the machine (the default).
let options = WriteOptions::new().threads(Threads::Auto);

// Use at most four threads.
let options = WriteOptions::new().threads(Threads::count_or_single(4));

// One thread: smallest archive, and identical on every machine.
let options = WriteOptions::new().threads(Threads::Single);
```

Two things follow from this setting:

- **Compression ratio.** Chunks of a solid block cannot match against each other, so parallel writing costs a little ratio. `Threads::Single` writes one stream, which is the smallest a level can produce.
- **Reproducibility.** Any explicit count writes the same bytes on every machine. `Threads::Auto` is the exception: on a single-core machine it has one thread to work with and writes what one thread writes. Ask for a number where byte-for-byte reproducibility matters.

## Memory

Each concurrent encoder holds a match finder several times the size of its dictionary, so writing on many cores can reserve a lot. `memory_limit` bounds it:

```rust
use zesven::{MemoryLimit, WriteOptions};

let options = WriteOptions::new()
    .memory_limit(MemoryLimit::bytes_or_auto(128 * 1024 * 1024));
```

The limit caps how many encoders run at once and how much data waits between them. It changes how fast an archive is written, never its contents.

It is not a cap on the writer's total footprint. An entry small enough to be compressed in memory still occupies what it occupies, so a single entry larger than the limit exceeds it, and the async writer buffers every entry regardless.

## Method Comparison

| Method  | Speed | Ratio | Memory    | Notes               |
| ------- | ----- | ----- | --------- | ------------------- |
| Store   | ★★★★★ | N/A   | Low       | No compression      |
| LZ4     | ★★★★★ | ★★    | Low       | Extremely fast      |
| Deflate | ★★★★  | ★★★   | Low       | Good compatibility  |
| Zstd    | ★★★★  | ★★★★  | Medium    | Best speed/ratio    |
| BZip2   | ★★    | ★★★★  | Medium    | Good for text       |
| LZMA    | ★★    | ★★★★★ | High      | Excellent ratio     |
| LZMA2   | ★★★   | ★★★★★ | High      | Multi-threaded LZMA |
| Brotli  | ★★    | ★★★★★ | High      | Best for web        |
| PPMd    | ★     | ★★★★★ | Very High | Best for text       |

## See Also

- [Creating Archives](./creating-archives) - Basic archive creation
- [Solid Archives](./solid-archives) - Inter-file compression
- [Feature Flags](../reference/feature-flags) - Enable compression methods
