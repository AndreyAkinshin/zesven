# Rust Development Instructions

Instructions for LLM agents working with the Rust codebase.

## Running Cargo Commands

**All cargo commands must be run from the `rs/` directory.**

**Never set `RUSTFLAGS` or `RUSTDOCFLAGS` environment variables.** These flags are already configured in `.cargo/config.toml`:

- `rustflags = ["-D", "warnings"]`
- `rustdocflags = ["-D", "warnings"]`

Setting them again via environment variables will override the config and may cause issues.

### Correct Commands

```bash
cd rs

# Check formatting
cargo fmt --all --check

# Run Clippy linter
cargo clippy --all-features --all-targets -- -D warnings

# Build documentation
cargo doc --all-features --no-deps

# Check compilation
cargo check --all-features

# Run tests
cargo test --all-features

# Build the project
cargo build --all-features
```

### Incorrect Commands (Do NOT Use)

```bash
# WRONG - do not set RUSTFLAGS
RUSTFLAGS="-D warnings" cargo clippy ...

# WRONG - do not set RUSTDOCFLAGS
RUSTDOCFLAGS="-D warnings" cargo doc ...
```

## CI Checks

The CI pipeline runs these checks (in order of typical execution):

1. **Formatting**: `cargo fmt --all --check`
2. **Clippy**: `cargo clippy --all-features --all-targets -- -D warnings`
3. **Documentation**: `cargo doc --all-features --no-deps`
4. **Build**: `cargo build --all-features`
5. **Tests**: `cargo test --all-features`

## Feature Flags

The project supports multiple feature combinations.

### Default Features

- `lzma` - LZMA compression
- `lzma2` - LZMA2 compression (includes `lzma`)
- `deflate` - Deflate/zlib compression
- `bzip2` - BZip2 compression
- `ppmd` - PPMd compression
- `aes` - AES-256 encryption
- `parallel` - Multi-threaded processing via Rayon

### Optional Features

- `lz4` - LZ4 compression
- `zstd` - Zstandard compression
- `brotli` - Brotli compression
- `fast-lzma2` - Fast LZMA2 encoder with radix match-finder (experimental)
- `regex` - Regex-based file filtering
- `sysinfo` - System info for adaptive memory limits
- `async` - Async API with Tokio
- `wasm` - WebAssembly/browser support (mutually exclusive with `parallel`)
- `cli` - Command-line interface binary

### Built-in Codecs (no feature flag required)

- LZ5 - Pure Rust implementation
- Lizard - Pure Rust implementation

## Project Structure

```
rs/
├── src/
│   ├── lib.rs          # Library entry point
│   ├── bin/            # CLI binary
│   ├── codec/          # Compression codecs and BCJ filters
│   ├── crypto/         # AES encryption
│   ├── read/           # Archive reading
│   ├── write/          # Archive writing
│   ├── streaming/      # Streaming API
│   └── async_*.rs      # Async implementations
├── tests/              # Integration tests
├── examples/           # Usage examples
├── fuzz/               # Fuzz testing targets
└── pkg/                # WASM package output
```

## Testing

```bash
# Run all tests
cargo test --all-features

# Run specific test
cargo test test_name --all-features

# Run tests with output
cargo test --all-features -- --nocapture

# Run ignored tests (may require test fixtures)
cargo test --all-features -- --ignored
```

## Examples

```bash
# List available examples
ls examples/

# Run an example
cargo run --example extract_selective -- archive.7z ./output
cargo run --example create_archive -- output.7z file1.txt file2.txt
```

## CLI Development

```bash
# Build CLI
cargo build --features cli

# Run CLI
cargo run --features cli -- list archive.7z
cargo run --features cli -- extract archive.7z -o ./output
cargo run --features cli -- create new.7z files/
```

## WASM Development

```bash
# Build for WASM (requires wasm-pack)
wasm-pack build --target web --features wasm-default

# Output goes to pkg/
```

Note: `wasm` and `parallel` features are mutually exclusive.
