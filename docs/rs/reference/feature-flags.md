---
title: Feature Flags
description: Available compilation features
---

# Feature Flags

zesven uses Cargo features to enable optional functionality.

## Default Features

These features are enabled by default:

```toml
[dependencies]
zesven = "1.0"  # Includes default features
```

| Feature    | Description                         |
| ---------- | ----------------------------------- |
| `lzma`     | LZMA compression                    |
| `lzma2`    | LZMA2 compression (includes `lzma`) |
| `deflate`  | Deflate/zlib compression            |
| `bzip2`    | BZip2 compression                   |
| `ppmd`     | PPMd compression                    |
| `aes`      | AES-256 encryption                  |
| `parallel` | Multi-threaded compression          |

## Minimal Build

Disable defaults for a smaller binary:

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["lzma2"] }
```

## All Features

### Compression

| Feature      | Default | Description                         |
| ------------ | ------- | ----------------------------------- |
| `lzma`       | Yes     | LZMA compression via lzma-rust2     |
| `lzma2`      | Yes     | LZMA2 compression (includes `lzma`) |
| `deflate`    | Yes     | Deflate via zlib-rs                 |
| `bzip2`      | Yes     | BZip2 compression                   |
| `ppmd`       | Yes     | PPMd compression                    |
| `lz4`        | No      | LZ4 fast compression                |
| `zstd`       | No      | Zstandard compression               |
| `brotli`     | No      | Brotli compression                  |
| `fast-lzma2` | No      | Fast LZMA2 encoder (experimental)   |

**Built-in codecs** (always available, no feature flag required):

- **LZ5** - Pure Rust implementation
- **Lizard** - Pure Rust implementation

### Security

| Feature | Default | Description        |
| ------- | ------- | ------------------ |
| `aes`   | Yes     | AES-256 encryption |

### Performance

| Feature    | Default | Description               |
| ---------- | ------- | ------------------------- |
| `parallel` | Yes     | Multi-threaded with Rayon |
| `sysinfo`  | No      | Auto-detect system RAM    |

### APIs

| Feature | Default | Description            |
| ------- | ------- | ---------------------- |
| `async` | No      | Tokio-based async API  |
| `regex` | No      | Regex-based filtering  |
| `cli`   | No      | Command-line interface |

### Platform

| Feature        | Default | Description              |
| -------------- | ------- | ------------------------ |
| `wasm`         | No      | WebAssembly support      |
| `wasm-default` | No      | WASM with default codecs |

## Feature Dependencies

Some features depend on others:

```
lzma2 → lzma
async → tokio, tokio-util, async-compression, pin-project-lite, futures
wasm → wasm-bindgen, wasm-bindgen-futures, js-sys, web-sys, getrandom/js
wasm-default → wasm, lzma, lzma2, deflate, bzip2, ppmd, aes
```

## Example Configurations

### Web Server

```toml
[dependencies]
zesven = { version = "1.0", features = ["async"] }
```

### CLI Tool

```toml
[dependencies]
zesven = { version = "1.0", features = ["cli", "zstd", "lz4"] }
```

### Embedded System

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["lzma2"] }
```

### Browser (WASM)

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["wasm-default"] }
```

### Maximum Compatibility

```toml
[dependencies]
zesven = { version = "1.0", features = ["lz4", "zstd", "brotli"] }
```

## Binary Size Impact

Approximate binary size impact (release build):

| Configuration          | Size    |
| ---------------------- | ------- |
| Minimal (`lzma2` only) | ~500 KB |
| Default features       | ~1.5 MB |
| All features           | ~3 MB   |

## See Also

- [Compression Options](../writing/compression-options) - Using compression methods
- [Async API](../async/) - Async feature usage
- [WASM](../advanced/wasm) - WebAssembly usage
