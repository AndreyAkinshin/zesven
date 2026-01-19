---
title: Reference
description: API reference and technical details
---

# Reference

Technical reference documentation for zesven.

## Topics

- [Feature Flags](./feature-flags) - Available compilation features
- [Error Handling](./error-handling) - Error types and handling
- [Platform Support](./platform-support) - Supported platforms

## API Documentation

For complete API documentation, see [docs.rs/zesven](https://docs.rs/zesven).

## Quick Reference

### Common Types

| Type             | Description              |
| ---------------- | ------------------------ |
| `Archive<R>`     | Archive reader           |
| `Writer<W>`      | Archive writer           |
| `Entry`          | Archive entry metadata   |
| `ArchivePath`    | Validated path           |
| `ExtractOptions` | Extraction configuration |
| `WriteOptions`   | Write configuration      |
| `Error`          | Error type               |
| `Result<T>`      | Result alias             |

### Common Operations

```rust
use zesven::{Archive, Writer, ArchivePath, ExtractOptions, WriteOptions, Result};

// Open archive
let archive = Archive::open_path("archive.7z")?;

// Extract
let mut archive = Archive::open_path("archive.7z")?;
archive.extract("./output", (), &ExtractOptions::default())?;

// Create archive
let mut writer = Writer::create_path("new.7z")?;
writer.add_bytes(ArchivePath::new("file.txt")?, b"data")?;
writer.finish()?;
```

### Feature Matrix

| Feature    | Included By |
| ---------- | ----------- |
| `lzma2`    | default     |
| `aes`      | default     |
| `parallel` | default     |
| `async`    | opt-in      |
| `wasm`     | opt-in      |

## See Also

- [Quick Start](../) - Quick start guide
- [Cookbook](../cookbook) - Practical examples
