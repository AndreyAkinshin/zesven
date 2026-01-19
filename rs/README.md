# zesven (Rust)

Rust implementation of the zesven 7z archive library.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zesven = "1.0"
```

## Quick Start

### Extract an archive

```rust
use zesven::{Archive, ExtractOptions};

fn main() -> zesven::Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Create an archive

```rust
use zesven::{Writer, ArchivePath};

fn main() -> zesven::Result<()> {
    let mut writer = Writer::create_path("new.7z")?;
    writer.add_path("file.txt", ArchivePath::new("file.txt")?)?;
    writer.finish()?;
    Ok(())
}
```

## Feature Flags

| Feature      | Default | Description                       |
| ------------ | ------- | --------------------------------- |
| `lzma`       | Yes     | LZMA compression                  |
| `lzma2`      | Yes     | LZMA2 compression                 |
| `deflate`    | Yes     | Deflate compression               |
| `bzip2`      | Yes     | BZip2 compression                 |
| `ppmd`       | Yes     | PPMd compression                  |
| `aes`        | Yes     | AES-256 encryption                |
| `parallel`   | Yes     | Multi-threaded processing         |
| `lz4`        | No      | LZ4 compression                   |
| `zstd`       | No      | Zstandard compression             |
| `brotli`     | No      | Brotli compression                |
| `fast-lzma2` | No      | Fast LZMA2 encoder (experimental) |
| `regex`      | No      | Regex-based file filtering        |
| `sysinfo`    | No      | System info for adaptive limits   |
| `async`      | No      | Async API with Tokio              |
| `wasm`       | No      | WebAssembly support               |
| `cli`        | No      | Command-line interface            |

**Note:** LZ5 and Lizard codecs are built-in (pure Rust implementations, always available).

## MSRV

Rust 1.85 or later.

## Documentation

- [API Documentation](https://docs.rs/zesven)
- [User Guide](https://zesven.akinshin.dev/rs/)

## License

MIT OR Apache-2.0
