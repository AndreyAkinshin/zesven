# Fuzz Testing

This directory contains fuzz targets for security-critical parsing code.

## Requirements

- Rust nightly toolchain
- cargo-fuzz: `cargo install cargo-fuzz`
- Linux recommended (macOS may have C++ toolchain issues with libfuzzer-sys)

## Available Targets

### archive_open

Fuzzes `Archive::open()` with arbitrary byte input to find panics or hangs
in the archive parsing logic.

```bash
cargo +nightly fuzz run archive_open
```

### archive_path

Fuzzes `ArchivePath::new()` with arbitrary string input to find path validation
bypasses or panics.

```bash
cargo +nightly fuzz run archive_path
```

## Usage

List available targets:

```bash
cargo +nightly fuzz list
```

Run a target (will run indefinitely until stopped with Ctrl+C):

```bash
cargo +nightly fuzz run <target>
```

Run with a time limit:

```bash
cargo +nightly fuzz run <target> -- -max_total_time=60
```

## Corpus

Interesting inputs found by the fuzzer are saved in `corpus/<target>/`.
Inputs that cause crashes are saved in `artifacts/<target>/`.
