---
title: Tokio Integration
description: Working with Tokio async runtime
---

# Tokio Integration

zesven's async API is built on Tokio for seamless integration with Tokio-based applications.

## Runtime Requirements

The async API requires a Tokio runtime:

```rust
use zesven::{AsyncArchive, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let archive = AsyncArchive::open_path("archive.7z").await?;
    println!("Entries: {}", archive.len());
    Ok(())
}
```

Or with manual runtime:

```rust
use zesven::{AsyncArchive, Result};
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Runtime::new()?;
    rt.block_on(async {
        let archive = AsyncArchive::open_path("archive.7z").await?;
        println!("Entries: {}", archive.len());
        Ok(())
    })
}
```

## Async File I/O

The async API uses Tokio's file I/O:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio::fs::File;
use tokio::io::BufReader;

#[tokio::main]
async fn main() -> Result<()> {
    // Open from Tokio file
    let file = File::open("archive.7z").await?;
    let reader = BufReader::new(file);
    let archive = AsyncArchive::open(reader).await?;

    println!("Found {} entries", archive.len());
    Ok(())
}
```

## Progress with Channels

Use channels for async progress reporting:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, ChannelProgressReporter, ProgressEvent, Result};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let (reporter, mut rx) = ChannelProgressReporter::new(100);

    let extract_task = tokio::spawn(async move {
        let mut archive = AsyncArchive::open_path("archive.7z").await?;
        let options = AsyncExtractOptions::new()
            .progress(Arc::new(reporter));
        archive.extract("./output", (), &options).await
    });

    // Receive progress updates
    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::EntryStart { name, size } => {
                println!("Extracting: {} ({} bytes)", name, size);
            }
            ProgressEvent::Progress { bytes_extracted, total_bytes } => {
                println!("Progress: {}/{}", bytes_extracted, total_bytes);
            }
            ProgressEvent::EntryComplete { name, success } => {
                println!("{}: {}", name, if success { "OK" } else { "FAILED" });
            }
        }
    }

    extract_task.await??;
    Ok(())
}
```

## Timeouts

Add timeouts to operations:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio::time::{timeout, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let mut archive = AsyncArchive::open_path("archive.7z").await?;

    // 60 second timeout for extraction
    match timeout(
        Duration::from_secs(60),
        archive.extract("./output", (), &AsyncExtractOptions::default())
    ).await {
        Ok(result) => result?,
        Err(_) => {
            eprintln!("Extraction timed out");
            return Ok(());
        }
    };

    Ok(())
}
```

## Spawning Tasks

Spawn extraction as a background task:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let handle = tokio::spawn(async {
        let mut archive = AsyncArchive::open_path("archive.7z").await?;
        archive.extract("./output", (), &AsyncExtractOptions::default()).await
    });

    // Do other work...
    println!("Extraction started in background");

    // Wait for extraction to complete
    handle.await??;
    println!("Extraction complete");
    Ok(())
}
```

## Cancellation

Use cancellation tokens for graceful cancellation:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let cancel_token = CancellationToken::new();

    let options = AsyncExtractOptions::new()
        .cancel_token(cancel_token.clone());

    let extract_task = {
        let token = cancel_token.clone();
        tokio::spawn(async move {
            let mut archive = AsyncArchive::open_path("archive.7z").await?;
            archive.extract("./output", (), &options).await
        })
    };

    // Cancel after 5 seconds
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        cancel_token.cancel();
    });

    match extract_task.await? {
        Ok(_) => println!("Extraction complete"),
        Err(zesven::Error::Cancelled) => println!("Extraction cancelled"),
        Err(e) => return Err(e),
    }

    Ok(())
}
```

## Listing Entries

List archive contents asynchronously:

```rust
use zesven::{AsyncArchive, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let archive = AsyncArchive::open_path("archive.7z").await?;

    for entry in archive.entries() {
        if entry.is_directory {
            println!("{}/", entry.path.as_str());
        } else {
            println!("{} ({} bytes)", entry.path.as_str(), entry.size);
        }
    }

    Ok(())
}
```

## See Also

- [Async Overview](./) - Async API concepts
- [Cancellation](./cancellation) - Cancel async operations
