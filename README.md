# zesven

[![Crates.io](https://img.shields.io/crates/v/zesven.svg)](https://crates.io/crates/zesven)
[![Documentation](https://docs.rs/zesven/badge.svg)](https://docs.rs/zesven)
[![Minimum Stable Rust Version](https://img.shields.io/badge/Rust-1.85.0-blue?color=fc8d62&logo=rust)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)

A comprehensive, pure Rust implementation of the 7z archive format with no FFI dependencies. zesven provides full support for reading, writing, and editing 7z archives with a wide range of compression codecs, AES-256 encryption with header encryption, streaming decompression, async APIs, and WebAssembly support.

For detailed documentation, examples, and API reference, visit **[zesven.akinshin.dev](https://zesven.akinshin.dev)**.

## Features

### Compression Codecs

- **LZMA/LZMA2** - Native support with parallel compression variant
- **Deflate** - Via `flate2` with zlib-rs backend
- **BZip2** - Full encode/decode support
- **PPMd** - Prediction by Partial Matching
- **Zstd** - Modern compression (optional feature)
- **Brotli** - Google's compression algorithm (optional feature)
- **LZ4** - Fast compression (optional feature)
- **LZ5/Lizard** - Pure Rust implementations

### Filters

- **Delta** - Byte-level delta encoding
- **BCJ** - x86, ARM, ARM64, ARM Thumb, PowerPC, SPARC, IA-64, RISC-V
- **BCJ2** - Complex 4-stream x86 filter

### Encryption

- **AES-256-CBC** with SHA-256 key derivation
- **Header encryption** - Encrypt filenames and metadata
- **Configurable iterations** - Tunable key derivation strength

### Archive Features

- **Solid archives** - Read and write
- **Multi-volume** - `.7z.001`, `.7z.002`, etc.
- **Self-extracting** - Windows PE, Linux ELF, macOS Mach-O detection
- **Recovery** - Signature scanning and partial recovery
- **Random access** - For non-solid archives

### APIs

- **Streaming** - Bounded memory with `StreamingArchive`
- **Async** - Tokio-based (optional `async` feature)
- **Parallel** - Rayon-based (optional `parallel` feature)
- **WASM** - Browser-compatible (optional `wasm` feature)

### Safety

- **Path traversal protection** - Blocks `../`, absolute paths, symlink escapes
- **Resource limits** - Header size, entry count, unpacked size limits
- **CRC verification** - All entries validated
- **Zip bomb prevention** - Compression ratio limits

## License

Copyright 2026 Andrey Akinshin. Licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
