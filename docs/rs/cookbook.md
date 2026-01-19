---
title: Cookbook
description: Practical examples for common zesven tasks
---

# Cookbook

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zesven = "1.0"
```

For a minimal build:

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["lzma2"] }
```

---

## Reading Archives

### List Archive Contents

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;

    for entry in archive.entries() {
        println!("{}: {} bytes", entry.path.as_str(), entry.size);
    }
    Ok(())
}
```

### Extract All Files

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Selective Extraction by Pattern

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract only .txt files
    let filter = |entry: &zesven::Entry| entry.path.as_str().ends_with(".txt");
    archive.extract("./output", filter, &ExtractOptions::default())?;
    Ok(())
}
```

### Selective Extraction with Regex

```rust
use zesven::{Archive, ExtractOptions, Result};
use regex::Regex;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract files matching pattern
    let pattern = Regex::new(r"^src/.*\.rs$").unwrap();
    let filter = |entry: &zesven::Entry| pattern.is_match(entry.path.as_str());
    archive.extract("./output", filter, &ExtractOptions::default())?;
    Ok(())
}
```

### Extract to Memory

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    // Extract specific file to bytes
    let data = archive.extract_to_vec("config.json")?;
    let config: serde_json::Value = serde_json::from_slice(&data)?;
    Ok(())
}
```

### Test Archive Integrity

```rust
use zesven::{Archive, Result};
use zesven::read::TestOptions;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let result = archive.test((), &TestOptions::default())?;
    if result.entries_failed == 0 {
        println!("Archive integrity verified: {} entries passed", result.entries_passed);
    } else {
        eprintln!("Archive corrupted: {} entries failed", result.entries_failed);
        for (path, error) in &result.failures {
            eprintln!("  {}: {}", path, error);
        }
    }
    Ok(())
}
```

### Open from Bytes

```rust
use zesven::{Archive, Result};
use std::io::Cursor;

fn main() -> Result<()> {
    let data = std::fs::read("archive.7z")?;
    let archive = Archive::open(Cursor::new(data))?;

    for entry in archive.entries() {
        println!("{}", entry.path.as_str());
    }
    Ok(())
}
```

### Multi-Volume Archives

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    // Automatically detects and opens all volumes
    let mut archive = Archive::open_path("archive.7z.001")?;
    archive.extract("./output", (), &Default::default())?;
    Ok(())
}
```

### Self-Extracting Archives

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    // SFX archives are handled transparently
    let mut archive = Archive::open_path("installer.exe")?;
    archive.extract("./output", (), &Default::default())?;
    Ok(())
}
```

---

## Creating Archives

### Basic Archive Creation

```rust
use zesven::{Writer, ArchivePath, Result};

fn main() -> Result<()> {
    let mut writer = Writer::create_path("new.7z")?;

    // Add file from disk
    writer.add_path("document.pdf", ArchivePath::new("document.pdf")?)?;

    // Add bytes directly
    writer.add_bytes(ArchivePath::new("hello.txt")?, b"Hello, World!")?;

    let result = writer.finish()?;
    println!("Compressed {} bytes to {} bytes",
        result.total_size, result.compressed_size);
    Ok(())
}
```

### Add Directory Recursively

```rust
use zesven::{Writer, ArchivePath, Result};
use walkdir::WalkDir;

fn main() -> Result<()> {
    let mut writer = Writer::create_path("project.7z")?;

    for entry in WalkDir::new("src").into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let path = entry.path();
            let archive_path = ArchivePath::new(path.to_str().unwrap())?;
            writer.add_path(path, archive_path)?;
        }
    }

    writer.finish()?;
    Ok(())
}
```

### Configure Compression

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;
use zesven::codec::CodecMethod;

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .method(CodecMethod::Lzma2)
        .level(9)?  // 0-9, where 9 is maximum compression
        .solid();

    let mut writer = Writer::create_path("compressed.7z")?
        .options(options);
    writer.add_path("large_file.bin", ArchivePath::new("large_file.bin")?)?;
    writer.finish()?;
    Ok(())
}
```

### Create Solid Archive

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .solid();  // Compress files together for better ratio

    let mut writer = Writer::create_path("solid.7z")?
        .options(options);

    // Similar files compress better together in solid mode
    writer.add_path("file1.txt", ArchivePath::new("file1.txt")?)?;
    writer.add_path("file2.txt", ArchivePath::new("file2.txt")?)?;
    writer.add_path("file3.txt", ArchivePath::new("file3.txt")?)?;

    writer.finish()?;
    Ok(())
}
```

### Use Different Codecs

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;
use zesven::codec::CodecMethod;

fn main() -> Result<()> {
    // Fast compression with Zstd
    let options = WriteOptions::new()
        .method(CodecMethod::Zstd);

    let mut writer = Writer::create_path("fast.7z")?
        .options(options);
    writer.add_path("data.bin", ArchivePath::new("data.bin")?)?;
    writer.finish()?;
    Ok(())
}
```

---

## Encryption

### Read Encrypted Archive

```rust
use zesven::{Archive, Password, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path_with_password(
        "secret.7z",
        Password::new("my_password"),
    )?;

    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Create Encrypted Archive

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;
use zesven::crypto::Password;

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret123"));

    let mut writer = Writer::create_path("encrypted.7z")?
        .options(options);
    writer.add_path("sensitive.doc", ArchivePath::new("sensitive.doc")?)?;
    writer.finish()?;
    Ok(())
}
```

### Header Encryption (Hide Filenames)

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;
use zesven::crypto::Password;

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret123"))
        .encrypt_header(true);  // Encrypt filenames and metadata

    let mut writer = Writer::create_path("private.7z")?
        .options(options);
    writer.add_path("confidential.pdf", ArchivePath::new("confidential.pdf")?)?;
    writer.finish()?;
    Ok(())
}
```

### Prompt for Password

```rust
use zesven::{Archive, Password, Result};

fn main() -> Result<()> {
    let password = rpassword::prompt_password("Enter password: ")?;
    let mut archive = Archive::open_path_with_password(
        "secret.7z",
        Password::new(&password),
    )?;

    for entry in archive.entries() {
        println!("{}", entry.path.as_str());
    }
    Ok(())
}
```

---

## Archive Editing

### Append Files to Existing Archive

```rust
use zesven::{ArchiveAppender, ArchivePath, Result};

fn main() -> Result<()> {
    let mut appender = ArchiveAppender::open("existing.7z")?;

    appender.add_path("new_file.txt", ArchivePath::new("new_file.txt")?)?;
    appender.add_bytes(ArchivePath::new("readme.md")?, b"# New content")?;

    appender.finish()?;
    Ok(())
}
```

### Update Files in Archive

```rust
use zesven::{Archive, ArchivePath, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    // Replace existing file
    editor.update("config.json", b"{\"version\": 2}")?;

    let mut output = File::create("modified.7z")?;
    editor.apply(&mut output)?;
    Ok(())
}
```

### Delete Files from Archive

```rust
use zesven::{Archive, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    // Delete specific files
    editor.delete("old_file.txt")?;
    editor.delete("temp/")?;  // Delete directory

    let mut output = File::create("modified.7z")?;
    editor.apply(&mut output)?;
    Ok(())
}
```

### Rename Files in Archive

```rust
use zesven::{Archive, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    editor.rename("old_name.txt", "new_name.txt")?;

    let mut output = File::create("modified.7z")?;
    editor.apply(&mut output)?;
    Ok(())
}
```

---

## Advanced Features

### Progress Callbacks

```rust
use zesven::{Archive, ExtractOptions, ProgressReporter, Result};

struct SimpleProgress;

impl ProgressReporter for SimpleProgress {
    fn on_entry_start(&mut self, name: &str, size: u64) {
        println!("Extracting: {} ({} bytes)", name, size);
    }
    fn on_entry_complete(&mut self, name: &str, success: bool) {
        println!("  {} {}", if success { "OK" } else { "FAIL" }, name);
    }
}

fn main() -> Result<()> {
    let mut archive = Archive::open_path("large.7z")?;
    let options = ExtractOptions::new().progress(SimpleProgress);
    archive.extract("./output", (), &options)?;
    Ok(())
}
```

### Cancellation

```rust
use zesven::{Archive, ExtractOptions, ProgressReporter, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct CancellableProgress {
    cancelled: Arc<AtomicBool>,
}

impl ProgressReporter for CancellableProgress {
    fn on_entry_start(&mut self, _entry_name: &str, _entry_size: u64) {}
    fn on_entry_complete(&mut self, _entry_name: &str, _success: bool) {}
    fn should_cancel(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

fn main() -> Result<()> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();

    // Set up Ctrl+C handler
    ctrlc::set_handler(move || {
        cancelled_clone.store(true, Ordering::SeqCst);
    })?;

    let mut archive = Archive::open_path("large.7z")?;

    let options = ExtractOptions::new()
        .progress(CancellableProgress { cancelled });

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

### Streaming Decompression

```rust
use zesven::streaming::StreamingArchive;
use zesven::Result;

fn main() -> Result<()> {
    // Open archive with streaming API for memory-efficient extraction
    // With default features (aes enabled), pass empty string for unencrypted archives
    let mut archive = StreamingArchive::open_path("archive.7z", "")?;

    // Process entries via iterator
    for entry_result in archive.entries() {
        let entry = entry_result?;
        println!("Processing: {}", entry.entry().path.as_str());

        if !entry.entry().is_directory {
            // Extract or skip based on your needs
        }
    }
    Ok(())
}
```

### Archive Recovery

```rust
use zesven::recovery::{recover_archive, RecoveryOptions, RecoveryStatus};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let file = File::open("damaged.7z")?;
    let result = recover_archive(file, RecoveryOptions::default())?;

    match result.status {
        RecoveryStatus::FullRecovery => {
            println!("All {} entries recovered!", result.recovered_count());
        }
        RecoveryStatus::PartialRecovery => {
            println!("Recovered {} entries, {} failed",
                result.recovered_count(),
                result.failed_count());
        }
        RecoveryStatus::Failed => println!("Could not recover archive"),
        _ => {}
    }
    Ok(())
}
```

### Parallel Compression

```rust
use zesven::{Writer, ArchivePath, Result};
use zesven::write::WriteOptions;

fn main() -> Result<()> {
    // Parallel compression is enabled by default with the `parallel` feature
    let options = WriteOptions::new()
        .level(9)?;

    let mut writer = Writer::create_path("parallel.7z")?
        .options(options);

    // Files are compressed in parallel when using solid mode
    writer.add_path("file1.bin", ArchivePath::new("file1.bin")?)?;
    writer.add_path("file2.bin", ArchivePath::new("file2.bin")?)?;
    writer.add_path("file3.bin", ArchivePath::new("file3.bin")?)?;

    writer.finish()?;
    Ok(())
}
```

### Resource Limits

```rust
use zesven::{Archive, Result};
use zesven::read::ExtractOptions;
use zesven::format::streams::ResourceLimits;

fn main() -> Result<()> {
    let mut archive = Archive::open_path("untrusted.7z")?;

    let limits = ResourceLimits::new()
        .max_total_unpacked(100 * 1024 * 1024)  // 100 MB max
        .max_entries(10_000);                    // Limit entry count

    let options = ExtractOptions::new()
        .limits(limits);

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

---

## Async API

Enable the `async` feature:

```toml
[dependencies]
zesven = { version = "1.0", features = ["async"] }
```

### Async Extraction

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut archive = AsyncArchive::open_path("archive.7z").await?;
    archive.extract("./output", (), &AsyncExtractOptions::default()).await?;
    Ok(())
}
```

### Async with Progress

```rust
use zesven::{AsyncArchive, AsyncExtractOptions, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut archive = AsyncArchive::open_path("archive.7z").await?;

    let options = AsyncExtractOptions::new();

    archive.extract("./output", (), &options).await?;
    Ok(())
}
```

### Async Archive Creation

```rust
use zesven::{AsyncWriter, ArchivePath, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut writer = AsyncWriter::create_path("new.7z").await?;

    writer.add_path("file.txt", ArchivePath::new("file.txt")?).await?;
    writer.add_bytes(ArchivePath::new("data.bin")?, &[1, 2, 3, 4]).await?;

    writer.finish().await?;
    Ok(())
}
```

---

## WebAssembly

Enable the `wasm` feature:

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["wasm-default"] }
```

### JavaScript Usage

```javascript
import init, { WasmArchive } from "zesven";

async function extractArchive(arrayBuffer) {
  await init();

  const archive = new WasmArchive(new Uint8Array(arrayBuffer));

  // List entries
  const entries = archive.getEntries();
  for (const entry of entries) {
    console.log(`${entry.path}: ${entry.size} bytes`);
  }

  // Extract specific file
  const data = archive.extractFile("config.json");
  const config = JSON.parse(new TextDecoder().decode(data));

  archive.free();
  return config;
}
```

### With Password

```javascript
import init, { WasmArchive } from "zesven";

async function extractEncrypted(arrayBuffer, password) {
  await init();

  const archive = WasmArchive.openWithPassword(
    new Uint8Array(arrayBuffer),
    password,
  );

  const data = archive.extractAll();
  archive.free();
  return data;
}
```

---

## CLI Tool

Install:

```bash
cargo install zesven --features cli
```

### List Contents

```bash
zesven list archive.7z
zesven list --technical archive.7z  # Show technical details
```

### Extract Files

```bash
zesven extract archive.7z -o ./output
zesven extract archive.7z -o ./output "*.txt"  # Only .txt files
zesven extract archive.7z -o ./output -p secret  # With password
```

### Create Archive

```bash
zesven create new.7z file1.txt file2.txt dir/
zesven create -m lzma2 -l 9 compressed.7z large_file.bin
zesven create -p secret --encrypt-headers encrypted.7z sensitive/  # With encryption
```

### Test Integrity

```bash
zesven test archive.7z
```

### Archive Info

```bash
zesven info archive.7z  # Show archive metadata
```

---

## Safety Features

### Path Traversal Protection

```rust
use zesven::{Archive, Result};
use zesven::read::{ExtractOptions, PathSafety};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("untrusted.7z")?;

    // Default: blocks ../ and absolute paths
    let options = ExtractOptions::new()
        .path_safety(PathSafety::Strict);  // Most restrictive (default)

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

### Symlink Policy

```rust
use zesven::{Archive, Result};
use zesven::read::{ExtractOptions, LinkPolicy};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let options = ExtractOptions::new()
        .link_policy(LinkPolicy::Forbid);  // Don't create symlinks (default)

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

---

## Feature Flags

| Feature      | Default | Description                                      |
| ------------ | ------- | ------------------------------------------------ |
| `lzma`       | Yes     | LZMA compression                                 |
| `lzma2`      | Yes     | LZMA2 compression (includes `lzma`)              |
| `deflate`    | Yes     | Deflate/zlib compression                         |
| `bzip2`      | Yes     | BZip2 compression                                |
| `ppmd`       | Yes     | PPMd compression                                 |
| `aes`        | Yes     | AES-256 encryption                               |
| `parallel`   | Yes     | Multi-threaded operations with Rayon             |
| `lz4`        | No      | LZ4 compression support                          |
| `zstd`       | No      | Zstandard compression support                    |
| `brotli`     | No      | Brotli compression support                       |
| `lz5`        | Builtin | LZ5 compression (pure Rust, always available)    |
| `lizard`     | Builtin | Lizard compression (pure Rust, always available) |
| `fast-lzma2` | No      | Fast LZMA2 encoder with radix match-finder       |
| `regex`      | No      | Regex-based file filtering                       |
| `sysinfo`    | No      | System info for adaptive memory limits           |
| `async`      | No      | Async API with Tokio                             |
| `wasm`       | No      | WebAssembly/browser support                      |
| `cli`        | No      | Command-line interface                           |
