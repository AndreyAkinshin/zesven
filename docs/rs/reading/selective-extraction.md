---
title: Selective Extraction
description: Filter which files to extract from archives
---

# Selective Extraction

zesven allows you to extract only specific files using entry selectors.

## Using Entry Selectors

Pass a selector to the `extract()` method to filter entries:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract only .txt files
    archive.extract("./output", |entry: &_| {
        entry.path.as_str().ends_with(".txt")
    }, &ExtractOptions::default())?;

    Ok(())
}
```

## Selector Types

### Closure Selector

Filter with any logic:

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract files larger than 1KB
    archive.extract("./output", |entry: &Entry| {
        entry.size > 1024
    }, &ExtractOptions::default())?;

    // Extract files from a specific directory
    archive.extract("./output", |entry: &Entry| {
        entry.path.as_str().starts_with("src/")
    }, &ExtractOptions::default())?;

    Ok(())
}
```

### Extract Everything

Pass `()` to extract all entries:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Extract by Path List

Extract specific paths:

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};
use std::collections::HashSet;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let wanted: HashSet<&str> = [
        "readme.txt",
        "config.json",
        "src/main.rs",
    ].into_iter().collect();

    archive.extract("./output", |entry: &Entry| {
        wanted.contains(entry.path.as_str())
    }, &ExtractOptions::default())?;

    Ok(())
}
```

## Regex Filtering

Enable the `regex` feature for pattern-based filtering:

```toml
[dependencies]
zesven = { version = "1.0", features = ["regex"] }
```

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};
use regex::Regex;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Match all Rust source files
    let pattern = Regex::new(r"\.rs$").unwrap();

    archive.extract("./output", |entry: &Entry| {
        pattern.is_match(entry.path.as_str())
    }, &ExtractOptions::default())?;

    Ok(())
}
```

## Combining Conditions

Build complex filters by combining conditions:

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    archive.extract("./output", |entry: &Entry| {
        // Not a directory
        !entry.is_directory
        // In src/ directory
        && entry.path.as_str().starts_with("src/")
        // Less than 1MB
        && entry.size < 1024 * 1024
        // Not a backup file
        && !entry.path.as_str().ends_with(".bak")
    }, &ExtractOptions::default())?;

    Ok(())
}
```

## Iterating Manually

For full control, iterate entries and extract selectively using a filter:

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Use a filter closure to extract only matching files
    archive.extract("./output", |entry: &Entry| {
        should_extract(entry)
    }, &ExtractOptions::default())?;

    Ok(())
}

fn should_extract(entry: &Entry) -> bool {
    entry.path.as_str().ends_with(".rs")
}
```

## See Also

- [Extracting Files](./extracting) - Basic extraction operations
- [Progress Callbacks](./progress-callbacks) - Monitor extraction progress
