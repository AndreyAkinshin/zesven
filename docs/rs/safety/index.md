---
title: Safety & Security
description: Security features and best practices
---

# Safety & Security

zesven includes built-in protections against common archive-based attacks.

## Overview

The library provides:

- **Path traversal protection** - Prevents extraction outside destination
- **Resource limits** - Guards against zip bombs
- **CRC verification** - Validates data integrity
- **Secure defaults** - Safe configuration out of the box

## Topics

- [Path Safety](./path-safety) - Path validation and traversal protection
- [Resource Limits](./resource-limits) - Memory and size limits

## Quick Examples

### Safe Extraction (Default)

```rust
use zesven::{Archive, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("untrusted.7z")?;

    // Default options include safety features
    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Custom Safety Configuration

```rust
use zesven::{Archive, ExtractOptions, read::PathSafety, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let options = ExtractOptions::new()
        .path_safety(PathSafety::Strict);

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Default Protections

| Protection       | Default  | Description            |
| ---------------- | -------- | ---------------------- |
| Path safety      | Strict   | Reject paths with `..` |
| CRC verification | Enabled  | Verify checksums       |
| Symlink creation | Disabled | Don't create symlinks  |
| Overwrite        | Disabled | Don't overwrite files  |

## See Also

- [Path Safety](./path-safety) - Path traversal protection
- [Resource Limits](./resource-limits) - Zip bomb protection
- [7z Spec: Security](/7z/17-security) - Security specification
