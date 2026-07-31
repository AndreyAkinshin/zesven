---
title: Async API
description: Tokio-based async/await operations
---

# Async API

zesven provides an async/await API for non-blocking archive operations. Both halves of it cover a subset of the blocking API: see [Limitations](#limitations) before choosing between them.

## Feature Flag

Enable the `async` feature:

```toml
[dependencies]
zesven = { version = "2.0", features = ["async"] }
```

## Quick Example

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut archive = AsyncArchive::open_path("archive.7z").await?;

    for entry in archive.entries() {
        println!("{}: {} bytes", entry.path.as_str(), entry.size);
    }

    archive.extract("./output", (), &AsyncExtractOptions::default()).await?;
    Ok(())
}
```

## Key Types

| Type                  | Description                |
| --------------------- | -------------------------- |
| `AsyncArchive`        | Async archive reader       |
| `AsyncWriter`         | Async archive writer       |
| `AsyncExtractOptions` | Extraction configuration   |

## Limitations

`AsyncWriter` writes the same archives as the blocking `Writer`, from the same folder model and header encoder, and awaits its I/O rather than blocking on it. It does not implement everything the blocking writer does, and returns an error rather than quietly ignoring what it cannot apply:

- Encryption, including header encryption
- Pre-compression filters, such as delta and the branch converters
- Solid mode
- Archive comments

It also has no multi-volume output: there is no async counterpart to `Writer::create_multivolume`. And it buffers each entry whole before compressing it, so `memory_limit` bounds the concurrency of compression but not what the writer holds.

`AsyncArchive` reads plain single-file archives. The blocking `Archive` also handles two shapes it does not:

- **Multi-volume sets.** `Archive::open_path` recognises one from its name and reads through the volumes; `AsyncArchive::open_path` opens the one file it was given, so `archive.7z.001` fails once reading passes the end of that volume.
- **Self-extracting archives.** The blocking reader finds the 7z data after the executable stub; the async one looks at offset zero and reports an invalid signature.
- **BCJ2 folders.** A BCJ2 chain takes four inputs, and the async decoder builds single-input chains only, so such an entry is reported as failed rather than decoded. Other folders in the same archive still read.

Use the blocking `Archive` for either.

For any of the above, use the blocking API.

## When to Use Async

Use the async API when:

- Building async applications (web servers, CLI tools with async runtimes)
- Processing multiple archives concurrently
- Need non-blocking I/O
- Integrating with Tokio ecosystem

## Topics

- [Tokio Integration](./tokio-integration) - Working with Tokio
- [Cancellation](./cancellation) - Cancelling operations

## Basic Operations

### Reading Archives

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut archive = AsyncArchive::open_path("archive.7z").await?;

    // List entries
    for entry in archive.entries() {
        println!("{}", entry.path.as_str());
    }

    // Extract all
    archive.extract("./output", (), &AsyncExtractOptions::default()).await?;
    Ok(())
}
```

### Creating Archives

```rust
use zesven::{AsyncWriter, ArchivePath, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut writer = AsyncWriter::create_path("new.7z").await?;

    writer.add_bytes(ArchivePath::new("hello.txt")?, b"Hello").await?;
    writer.finish().await?;
    Ok(())
}
```

## Concurrent Processing

Process multiple archives concurrently:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use futures::future::join_all;

#[tokio::main]
async fn main() -> Result<()> {
    let archives = vec!["a.7z", "b.7z", "c.7z"];

    let tasks: Vec<_> = archives.iter().map(|path| {
        async move {
            let mut archive = AsyncArchive::open_path(path).await?;
            archive.extract(
                format!("./output/{}", path),
                (),
                &AsyncExtractOptions::default()
            ).await
        }
    }).collect();

    join_all(tasks).await;
    Ok(())
}
```

## See Also

- [Tokio Integration](./tokio-integration) - Tokio-specific features
- [Cancellation](./cancellation) - Graceful cancellation
