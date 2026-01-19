---
title: Cancellation
description: Cancel async operations gracefully
---

# Cancellation

zesven supports graceful cancellation of async operations using `CancellationToken`.

## Basic Cancellation

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    // Spawn extraction task
    let extract_task = tokio::spawn(async move {
        let mut archive = AsyncArchive::open_path("large.7z").await?;
        let options = AsyncExtractOptions::new()
            .cancel_token(token_clone);
        archive.extract("./output", (), &options).await
    });

    // Cancel after 5 seconds
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    token.cancel();

    match extract_task.await? {
        Ok(_) => println!("Extraction completed"),
        Err(zesven::Error::Cancelled) => println!("Extraction cancelled"),
        Err(e) => return Err(e),
    }

    Ok(())
}
```

## Ctrl+C Handling

Cancel on Ctrl+C:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    // Handle Ctrl+C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\nCancelling...");
        token.cancel();
    });

    let mut archive = AsyncArchive::open_path("archive.7z").await?;
    let options = AsyncExtractOptions::new()
        .cancel_token(token_clone);

    match archive.extract("./output", (), &options).await {
        Ok(_) => println!("Done"),
        Err(zesven::Error::Cancelled) => println!("Cancelled by user"),
        Err(e) => return Err(e),
    }

    Ok(())
}
```

## Checking Cancellation State

Check if an operation was cancelled:

```rust
use zesven::Error;

fn handle_result(result: zesven::Result<()>) {
    match result {
        Ok(_) => println!("Success"),
        Err(Error::Cancelled) => println!("Operation was cancelled"),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

## Cancellation Points

Cancellation is checked:

- Between file extractions
- During decompression at regular intervals
- Before starting each major operation

This ensures responsive cancellation while maintaining data integrity.

## Cleanup on Cancellation

When an operation is cancelled, partial files may remain. You can handle cleanup in your application:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let token = CancellationToken::new();

    let options = AsyncExtractOptions::new()
        .cancel_token(token.clone());

    let mut archive = AsyncArchive::open_path("archive.7z").await?;

    match archive.extract("./output", (), &options).await {
        Ok(_) => println!("Extraction complete"),
        Err(zesven::Error::Cancelled) => {
            println!("Cancelled - cleaning up partial files");
            // Handle cleanup as needed
        }
        Err(e) => return Err(e),
    }

    Ok(())
}
```

## Timeout as Cancellation

Implement timeout using cancellation:

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};
use tokio_util::sync::CancellationToken;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    // Cancel after timeout
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        token.cancel();
    });

    let mut archive = AsyncArchive::open_path("archive.7z").await?;
    let options = AsyncExtractOptions::new()
        .cancel_token(token_clone);

    archive.extract("./output", (), &options).await?;
    Ok(())
}
```

## See Also

- [Async Overview](./) - Async API concepts
- [Tokio Integration](./tokio-integration) - Tokio-specific features
