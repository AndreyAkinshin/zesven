---
title: Async API
description: Tokio-based async/await operations
---

# Async API

zesven provides a full async/await API for non-blocking archive operations.

## Feature Flag

Enable the `async` feature:

```toml
[dependencies]
zesven = { version = "1.0", features = ["async"] }
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
| `AsyncDecoder`        | Async decompression stream |

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
