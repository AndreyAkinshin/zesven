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
zesven = { version = "3.0", features = ["zstd", "lz4", "brotli"] }
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

Parallel compression is enabled by default with the `parallel` feature, and works two ways.

Entries that are compressed together go one per core: each is its own folder in a non-solid archive, so they are independent. That covers an archive of many ordinary files.

A stream that has nothing to run alongside it goes the other way: it is cut into blocks that are compressed at the same time. This applies to a large entry, which is written on its own, and to a solid block, which is one folder however many files went into it. Without it, archiving a single large file would take one core no matter how many the machine has.

Use `threads` to bound both:

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

- **Compression ratio.** A block is handed the data immediately before it as its dictionary, so it matches across its own start as an unbroken stream would. On text this costs a fraction of a percent rather than the tenth it would cost otherwise, and on data whose matches lie far apart it is the difference between compressing and not. `Threads::Single` writes one unbroken stream, which is the smallest a level can produce.
- **Reproducibility.** Any count above one writes the same bytes on every machine: where a stream is cut follows from the level and the data, never from the core count, the memory limit, or how the entry was read. Settings that resolve to a *single* thread are the exception - `Threads::Single`, `Threads::count_or_single(1)`, and `Threads::Auto` on a single-core machine - since one thread writes an unbroken stream. Ask for a number above one where byte-for-byte reproducibility matters, or ask for one thread everywhere.

## Memory

Each concurrent encoder holds a match finder several times the size of its dictionary, so writing on many cores can reserve a lot. `memory_limit` bounds it:

```rust
use zesven::{MemoryLimit, WriteOptions};

let options = WriteOptions::new()
    .memory_limit(MemoryLimit::bytes_or_auto(128 * 1024 * 1024));
```

The limit caps how many encoders run at once and how much data waits between them. It changes how fast an archive is written, never its contents.

`MemoryLimit::Auto` is what this machine's cores can put to use, bounded by half of what is free. Asking the machine needs the `sysinfo` feature; it is on by default. Without it, `Auto` has the cores and any cgroup cap to go on, and falls back to a fixed 512 MiB on a machine with no cap.

Under a cgroup cap the cap is what counts, whether it comes from a container runtime or from a systemd unit with `MemoryMax`, and whether it sits on the process's own group or on one above it. Under cgroup v2, cached files inside the cap count as free where a reclaim would free them, while anonymous pages, unreclaimable kernel memory, socket buffers, huge pages, pinned pages and shared memory count as occupied. Cache that a `memory.min` floor somewhere beneath the cap has been promised counts as occupied too, since the kernel will not reclaim below such a floor; those floors are found by walking the groups under the cap, not only the ones this process is in, and each binds only as much as its group is actually holding. Anything the breakdown cannot account for is counted as occupied as well, so a line a newer kernel reports does not read as free memory. Where that walk cannot be completed, or where a cap's usage cannot be read at all, everything the group holds counts as occupied: overstating what is in use costs a little speed, and understating it costs the process. Under v1 the whole usage counts, cache included: that version reports no figure that separates tmpfs from ordinary cache, and overstating what is occupied costs speed where understating it costs the process.

Sizing it by the cores is what makes a large machine finish sooner: the budget buys blocks in flight, and a budget that keeps eight of them busy leaves a twenty-four core machine three quarters idle. Lowering it is always safe and always costs speed rather than correctness.

It is not a cap on the writer's total footprint. An entry compressed in memory still occupies what it occupies, so a single entry larger than the limit exceeds it.

A large entry compressed straight into the sink is the exception, and deliberately: it is compressed as it is read, in blocks, with only a few blocks in flight at a time. Archiving a file larger than memory costs what the blocks cost and not what the file does.

Whether an entry qualifies is settled by reading it and not by the size its metadata declares, so an entry that says it is small and is not costs nothing extra and produces the same archive. The price of that is a fixed one: up to 64 MiB of any entry is read before it can be told apart from a small one. It does not grow with the entry.

That path needs the compressed bytes to go straight out, which these options prevent:

- **`solid`** - a solid block is one folder built from every entry in it, so the entries are held until the block is closed, and the block itself is held while it is compressed.
- **A filter** - a filter transforms the entry before the codec sees it, and holds it to do so. BCJ2 additionally splits it into four streams.
- **Encryption of the data** - the ciphertext is produced from the compressed entry in hand.
- **Any codec but LZMA, LZMA2 and `Copy`** - the others hand back a buffer rather than writing through.
- **The async writer** - it buffers every entry it is given, whatever the options say.

Under any of those, a 4 GB entry costs 4 GB and more. The blocking writer with plain LZMA2 and no filter is where the bound holds.

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
