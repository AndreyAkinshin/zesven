---
title: Quick Start
description: Get started with zesven in minutes
---

# Quick Start

zesven is a pure-Rust library for reading and writing 7z archives.

## Installation

```sh
cargo add zesven
```

## Extract an Archive

```rust
use zesven::{Archive, ExtractOptions};

fn main() -> zesven::Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Create an Archive

```rust
use zesven::{Writer, ArchivePath};

fn main() -> zesven::Result<()> {
    let mut writer = Writer::create_path("backup.7z")?;
    writer.add_path("document.pdf", ArchivePath::new("document.pdf")?)?;
    writer.finish()?;
    Ok(())
}
```

## What's Next?

**[Cookbook](./cookbook)** — practical examples for common tasks

### By Task

| I want to...                       | Go to                                                  |
| ---------------------------------- | ------------------------------------------------------ |
| Open and list archive contents     | [Reading Archives](./reading/)                         |
| Extract specific files             | [Selective Extraction](./reading/selective-extraction) |
| Create compressed archives         | [Writing Archives](./writing/)                         |
| Work with encrypted archives       | [Encryption](./encryption/)                            |
| Process large archives efficiently | [Streaming API](./streaming/)                          |
| Use async/await with Tokio         | [Async API](./async/)                                  |

### Reference

- [Feature Flags](./reference/feature-flags) — customize your build
- [Error Handling](./reference/error-handling) — error types and patterns
- [API Docs](https://docs.rs/zesven) — full API reference on docs.rs
