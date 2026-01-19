---
title: Advanced Topics
description: Advanced features for specialized use cases
---

# Advanced Topics

This section covers advanced features for specialized use cases.

## Topics

- [Editing Archives](./editing) - Modify existing archives
- [Multi-Volume](./multi-volume) - Split archives across multiple files
- [Self-Extracting](./sfx) - Create self-extracting executables
- [Archive Recovery](./recovery) - Recover data from damaged archives
- [WASM/Browser](./wasm) - Run in web browsers

## Overview

### Archive Editing

Modify existing archives without full recompression:

```rust
use zesven::{Archive, ArchivePath, Result};
use zesven::edit::ArchiveEditor;
use std::fs::File;

fn main() -> Result<()> {
    let archive = Archive::open_path("archive.7z")?;
    let mut editor = ArchiveEditor::new(archive);

    editor.delete("old_file.txt")?;
    editor.rename("a.txt", "b.txt")?;

    let output = File::create("modified.7z")?;
    editor.apply(output)?;
    Ok(())
}
```

### Multi-Volume Archives

Handle split archives:

```rust
use zesven::{Archive, Result};

fn main() -> Result<()> {
    // Automatically handles .7z.001, .7z.002, etc.
    let archive = Archive::open_path("backup.7z.001")?;
    Ok(())
}
```

### Self-Extracting Archives

Create SFX archives:

```rust
use zesven::sfx::{SfxBuilder, SfxConfig, SfxStub, SfxFormat};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let stub = SfxStub::from_file("7zS.sfx")?;
    let config = SfxConfig::new()
        .title("My Installer")
        .progress(true);

    let archive_data = std::fs::read("archive.7z")?;
    let mut output = File::create("installer.exe")?;

    SfxBuilder::new()
        .stub(stub)
        .config(config)
        .build(&mut output, &archive_data)?;
    Ok(())
}
```

### Archive Recovery

Recover data from damaged archives:

```rust
use zesven::recovery::{recover_archive, RecoveryOptions, RecoveryStatus};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let file = File::open("damaged.7z")?;
    let result = recover_archive(file, RecoveryOptions::default())?;

    match result.status {
        RecoveryStatus::FullRecovery => {
            println!("Recovered {} entries", result.recovered_count());
        }
        RecoveryStatus::PartialRecovery => {
            println!("Recovered {} of {} entries",
                result.recovered_count(), result.total_entries());
        }
        _ => println!("Recovery failed"),
    }
    Ok(())
}
```

### WebAssembly

Run in browsers:

```rust
use zesven::wasm::WasmArchive;
use js_sys::Uint8Array;

// In WASM context
let archive = WasmArchive::new(data)?;  // data is Uint8Array
```

## Feature Flags

Some advanced features require specific feature flags:

| Feature      | Flag             |
| ------------ | ---------------- |
| WASM         | `wasm`           |
| Recovery     | Always available |
| SFX          | Always available |
| Multi-volume | Always available |
