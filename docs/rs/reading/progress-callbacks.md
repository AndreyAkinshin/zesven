---
title: Progress Callbacks
description: Monitor extraction progress with callbacks
---

# Progress Callbacks

zesven supports progress callbacks to monitor long-running extraction operations.

## Basic Progress Callback

Implement progress tracking using the `ProgressReporter` trait or the `progress_fn` helper:

```rust
use zesven::{Archive, ExtractOptions, Result, progress_fn};
use std::sync::atomic::{AtomicU64, Ordering};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("large_archive.7z")?;

    let total_size: u64 = archive.entries().map(|e| e.size).sum();

    let options = ExtractOptions::new()
        .progress(progress_fn(|bytes_processed, total_bytes| {
            let percent = (bytes_processed as f64 / total_bytes.max(1) as f64) * 100.0;
            println!("\r{:.1}% complete", percent);
            true // Return true to continue, false to cancel
        }));

    archive.extract("./output", (), &options)?;
    println!("\nDone!");
    Ok(())
}
```

## Progress Information

The `progress_fn` closure receives:

| Parameter         | Type   | Description                           |
| ----------------- | ------ | ------------------------------------- |
| `bytes_processed` | `u64`  | Total bytes processed so far          |
| `total_bytes`     | `u64`  | Total bytes to process                |
| **Returns**       | `bool` | `true` to continue, `false` to cancel |

## Throttling Updates

For large archives, use `ThrottledProgress` to reduce callback overhead:

```rust
use zesven::{Archive, ExtractOptions, Result};
use zesven::progress::{ThrottledProgress, StatisticsProgress};
use std::time::Duration;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("large_archive.7z")?;

    // Wrap a progress reporter with throttling (min 100ms between updates)
    let inner = StatisticsProgress::new();
    let throttled = ThrottledProgress::new(inner, Duration::from_millis(100));

    let options = ExtractOptions::new()
        .progress(throttled);

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Cancellation

Return `false` from the progress callback to cancel extraction:

```rust
use zesven::{Archive, ExtractOptions, Result, progress_fn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();

    // Set up signal handler (simplified)
    ctrlc::set_handler(move || {
        cancelled_clone.store(true, Ordering::SeqCst);
    }).ok();

    let options = ExtractOptions::new()
        .progress(progress_fn(move |_bytes, _total| {
            if cancelled.load(Ordering::SeqCst) {
                println!("\nCancellation requested");
                return false; // Returning false cancels extraction
            }
            true
        }));

    match archive.extract("./output", (), &options) {
        Ok(_) => println!("Extraction complete"),
        Err(zesven::Error::Cancelled) => println!("Extraction cancelled"),
        Err(e) => return Err(e),
    }
    Ok(())
}
```

## Progress with indicatif

For terminal progress bars, use the `indicatif` crate with `progress_fn`:

```rust
use zesven::{Archive, ExtractOptions, Result, progress_fn};
use indicatif::{ProgressBar, ProgressStyle};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let total_size: u64 = archive.entries().map(|e| e.size).sum();
    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    let options = ExtractOptions::new()
        .progress(progress_fn(move |bytes_processed, _total| {
            pb.set_position(bytes_processed);
            true
        }));

    archive.extract("./output", (), &options)?;
    println!("Extraction complete");
    Ok(())
}
```

## See Also

- [Extracting Files](./extracting) - Basic extraction operations
- [Async API](../async/) - Async progress with channels
