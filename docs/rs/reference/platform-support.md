---
title: Platform Support
description: Supported operating systems and architectures
---

# Platform Support

zesven supports multiple platforms with varying feature availability.

## Supported Platforms

| Platform    | Architecture            | Status             |
| ----------- | ----------------------- | ------------------ |
| Linux       | x86_64                  | Full support       |
| Linux       | aarch64                 | Full support       |
| macOS       | x86_64                  | Full support       |
| macOS       | aarch64 (Apple Silicon) | Full support       |
| Windows     | x86_64                  | Full support       |
| WebAssembly | wasm32                  | Via `wasm` feature |

## Platform-Specific Features

### Linux

Full support including:

- Hard links
- Symbolic links
- Unix permissions
- Extended attributes

### macOS

Full support including:

- Hard links
- Symbolic links
- Unix permissions
- Resource forks (limited)

### Windows

Full support including:

- Hard links (NTFS)
- Symbolic links (requires privileges)
- File attributes
- Alternate data streams

### WebAssembly

Limited support:

- No file system access
- Single-threaded only
- Memory constraints

## Feature Availability by Platform

| Feature    | Linux | macOS | Windows    | WASM |
| ---------- | ----- | ----- | ---------- | ---- |
| File I/O   | Yes   | Yes   | Yes        | No   |
| Parallel   | Yes   | Yes   | Yes        | No   |
| Hard links | Yes   | Yes   | Yes (NTFS) | No   |
| Symlinks   | Yes   | Yes   | Limited    | No   |
| Async      | Yes   | Yes   | Yes        | Yes  |
| Timestamps | Yes   | Yes   | Yes        | No   |

## Minimum Requirements

### Rust Version

**MSRV: Rust 1.85**

```toml
rust-version = "1.85"
```

### Operating System Versions

| Platform | Minimum Version |
| -------- | --------------- |
| Linux    | glibc 2.17+     |
| macOS    | 10.15+          |
| Windows  | Windows 10+     |

## Cross-Compilation

### Linux to Windows

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --target x86_64-pc-windows-gnu
```

### Linux to macOS

```bash
rustup target add x86_64-apple-darwin
cargo build --target x86_64-apple-darwin
```

### Any to WASM

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --features wasm-default
```

## Platform-Specific Code

Handle platform differences:

```rust
use zesven::{Archive, ExtractOptions, Result};
use zesven::read::PreserveMetadata;
use std::io::{Read, Seek};

fn extract_with_options<R: Read + Seek>(archive: &mut Archive<R>) -> Result<()> {
    // Preserve all metadata (timestamps, permissions, attributes)
    let options = ExtractOptions::default()
        .preserve_metadata(PreserveMetadata::all());

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Testing on Multiple Platforms

CI configuration tests all platforms:

```yaml
# .github/workflows/ci.yml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
```

## See Also

- [Feature Flags](./feature-flags) - Platform-specific features
- [WASM](../advanced/wasm) - WebAssembly details
