---
title: Self-Extracting Archives
description: Create self-extracting executables
---

# Self-Extracting Archives

Create archives that extract themselves without needing external tools.

## Creating SFX Archives

```rust
use zesven::sfx::{SfxBuilder, SfxConfig, SfxStub};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    // Load a stub executable (e.g., 7zS.sfx from 7-Zip)
    let stub = SfxStub::from_file("7zS.sfx")?;

    let config = SfxConfig::new()
        .title("My Application Installer")
        .extract_path("./MyApp");

    // Read the archive data
    let archive_data = std::fs::read("archive.7z")?;

    // Build the SFX
    let mut output = File::create("installer.exe")?;
    SfxBuilder::new()
        .stub(stub)
        .config(config)
        .build(&mut output, &archive_data)?;

    Ok(())
}
```

## SFX Configuration

Customize the self-extractor:

```rust
use zesven::sfx::SfxConfig;

let config = SfxConfig::new()
    .title("My Installer")
    .extract_path("$TEMP\\MyApp")
    .run_program("setup.exe")
    .run_parameters("/silent")
    .progress(true)
    .begin_prompt("Install My Application?");
```

### Configuration Options

| Option           | Description                     |
| ---------------- | ------------------------------- |
| `title`          | Window title                    |
| `extract_path`   | Default extraction directory    |
| `run_program`    | Program to run after extraction |
| `run_parameters` | Parameters for the program      |
| `progress`       | Show progress dialog            |
| `begin_prompt`   | Prompt shown before extraction  |
| `icon`           | Custom icon (Windows only)      |

## SFX Stub Formats

zesven supports multiple executable formats:

```rust
use zesven::sfx::{SfxStub, SfxFormat};

// Auto-detect format from file
let stub = SfxStub::from_file("7zS.sfx")?;
println!("Detected format: {:?}", stub.format);

// Or create with explicit format
let data = std::fs::read("custom_stub.exe")?;
let stub = SfxStub::with_format(data, SfxFormat::WindowsPe);
```

### Supported Formats

| Format       | Description                   |
| ------------ | ----------------------------- |
| `WindowsPe`  | Windows PE executable         |
| `LinuxElf`   | Linux ELF binary              |
| `MacOsMachO` | macOS Mach-O binary           |
| `Generic`    | Custom format (no validation) |

## Reading SFX Archives

zesven can read SFX archives:

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    // Open SFX as regular archive (auto-detects SFX offset)
    let mut archive = Archive::open_path("installer.exe")?;

    // List contents
    for entry in archive.entries() {
        println!("{}", entry.path);
    }

    // Extract normally
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Extracting Archive from SFX

Get just the embedded 7z archive:

```rust
use zesven::sfx::extract_archive_from_sfx;
use zesven::Result;

fn main() -> Result<()> {
    let sfx_data = std::fs::read("installer.exe")?;
    let (archive_data, info) = extract_archive_from_sfx(&sfx_data)?;

    println!("Archive starts at offset: {}", info.archive_offset);
    println!("Stub format: {:?}", info.format);

    // Save just the archive
    std::fs::write("extracted.7z", archive_data)?;
    Ok(())
}
```

## Platform Support

| Platform | SFX Creation | SFX Execution |
| -------- | ------------ | ------------- |
| Windows  | Full         | Full          |
| Linux    | Full         | Via Wine      |
| macOS    | Full         | Via Wine      |

## Low-Level API

For direct SFX creation without the builder:

```rust
use zesven::sfx::{create_sfx, SfxConfig};
use zesven::Result;
use std::fs::File;

fn main() -> Result<()> {
    let stub = std::fs::read("7zS.sfx")?;
    let archive = std::fs::read("archive.7z")?;
    let config = SfxConfig::new().title("My App");

    let mut output = File::create("installer.exe")?;
    let total_bytes = create_sfx(&mut output, &stub, Some(&config), &archive)?;

    println!("Created SFX: {} bytes", total_bytes);
    Ok(())
}
```

## See Also

- [Creating Archives](../writing/creating-archives) - Basic archive creation
- [7z Spec: SFX Archives](/7z/15-sfx-archives) - Format specification
