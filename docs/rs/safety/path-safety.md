---
title: Path Safety
description: Protection against path traversal attacks
---

# Path Safety

Path traversal attacks attempt to write files outside the intended extraction directory. zesven provides configurable protection.

## Path Safety Levels

```rust
use zesven::{ExtractOptions, read::PathSafety};

// Strict (default): Maximum protection
let strict = ExtractOptions::new().path_safety(PathSafety::Strict);

// Relaxed: Allow some edge cases
let relaxed = ExtractOptions::new().path_safety(PathSafety::Relaxed);

// Disabled: No protection (dangerous)
let disabled = ExtractOptions::new().path_safety(PathSafety::Disabled);
```

## Safety Level Comparison

| Check                          | Strict | Relaxed | Disabled |
| ------------------------------ | ------ | ------- | -------- |
| Reject `..` components         | Yes    | Yes     | No       |
| Reject absolute paths          | Yes    | Yes     | No       |
| Reject paths starting with `/` | Yes    | Yes     | No       |
| Reject paths starting with `\` | Yes    | Yes     | No       |
| Canonical path containment     | Yes    | No      | No       |

The key difference between Strict and Relaxed is that Strict performs canonical path resolution to verify the final path stays within the destination directory, preventing symlink-based escapes.

## Strict Mode (Default)

Rejects any potentially dangerous path:

```rust
use zesven::{Archive, ExtractOptions, read::PathSafety, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let options = ExtractOptions::new()
        .path_safety(PathSafety::Strict);

    // These paths would be rejected:
    // - ../escape.txt
    // - /etc/passwd
    // - C:\Windows\System32\file.dll

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Relaxed Mode

Allows some edge cases while still preventing obvious attacks:

```rust
use zesven::{Archive, ExtractOptions, read::PathSafety, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    let options = ExtractOptions::new()
        .path_safety(PathSafety::Relaxed);

    // Still rejects:
    // - ../escape.txt
    // - /etc/passwd

    // May allow:
    // - Unusual but non-exploitable paths

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Disabled (Dangerous)

::: danger
Only use with fully trusted archives. Malicious archives can overwrite arbitrary files.
:::

```rust
use zesven::{Archive, ExtractOptions, read::PathSafety, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("trusted.7z")?;

    let options = ExtractOptions::new()
        .path_safety(PathSafety::Disabled);

    // WARNING: No path validation!
    // Archive could write to any location

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## Symlink Handling

Symlinks can be used for path traversal:

```rust
use zesven::{ExtractOptions, read::LinkPolicy};

// Forbid symlinks (default, safest)
let safe = ExtractOptions::new()
    .link_policy(LinkPolicy::Forbid);

// Allow symlinks but validate targets stay within extraction directory
let validated = ExtractOptions::new()
    .link_policy(LinkPolicy::ValidateTargets);

// Allow all symlinks (use with caution)
let allow_all = ExtractOptions::new()
    .link_policy(LinkPolicy::Allow);
```

## Custom Validation

Add your own path validation:

```rust
use zesven::{Archive, ExtractOptions, Entry, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("archive.7z")?;

    archive.extract("./output", |entry: &Entry| {
        // Custom validation
        let path = entry.path.as_str();
        !path.contains("..") && !path.starts_with('/')
    }, &ExtractOptions::default())?;

    Ok(())
}
```

## See Also

- [Safety Overview](./) - Security features
- [Resource Limits](./resource-limits) - Size and memory limits
- [7z Spec: Security](/7z/17-security) - Security specification
