---
title: Error Handling
description: Error types and handling patterns
---

# Error Handling

zesven uses a comprehensive `Error` enum for all failure cases.

## Result Type

All operations return `zesven::Result<T>`:

```rust
use zesven::{Archive, Result};
use std::io::BufReader;
use std::fs::File;

fn open_archive(path: &str) -> Result<Archive<BufReader<File>>> {
    Archive::open_path(path)
}
```

## Error Variants

```rust
use zesven::Error;

match result {
    Ok(value) => { /* success */ }
    Err(Error::Io(e)) => { /* I/O error */ }
    Err(Error::InvalidFormat(msg)) => { /* Invalid 7z format */ }
    Err(Error::WrongPassword { .. }) => { /* Incorrect password */ }
    Err(Error::PasswordRequired) => { /* No password provided */ }
    Err(Error::UnsupportedMethod { method_id }) => { /* Unknown compression */ }
    Err(Error::ResourceLimitExceeded(msg)) => { /* Limit hit */ }
    Err(Error::PathTraversal { entry_index, path }) => { /* Security violation */ }
    Err(Error::Cancelled) => { /* Operation cancelled */ }
    Err(Error::CrcMismatch { entry_index, expected, actual, .. }) => { /* Data corruption */ }
    Err(e) => { /* Other error */ }
}
```

## Common Error Types

### I/O Errors

File system and reader/writer errors:

```rust
use zesven::{Archive, Error};

match Archive::open_path("missing.7z") {
    Ok(_) => {}
    Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
        eprintln!("File not found");
    }
    Err(Error::Io(e)) => {
        eprintln!("I/O error: {}", e);
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

### Format Errors

Invalid or corrupted archive:

```rust
use zesven::{Archive, Error};

match Archive::open_path("not_a_7z.txt") {
    Err(Error::InvalidFormat(msg)) => {
        eprintln!("Not a valid 7z file: {}", msg);
    }
    _ => {}
}
```

### Password Errors

Encryption-related failures:

```rust
use zesven::{Archive, Error, Password};

match Archive::open_path_with_password("encrypted.7z", Password::new("wrong")) {
    Err(Error::WrongPassword { entry_index, entry_name, .. }) => {
        if let Some(name) = entry_name {
            eprintln!("Wrong password for entry: {}", name);
        } else if let Some(idx) = entry_index {
            eprintln!("Wrong password for entry at index {}", idx);
        } else {
            eprintln!("Wrong password");
        }
    }
    Err(Error::PasswordRequired) => {
        eprintln!("This archive requires a password");
    }
    _ => {}
}
```

### Resource Limits

Protection against malicious archives:

```rust
use zesven::Error;

match result {
    Err(Error::ResourceLimitExceeded(msg)) => {
        eprintln!("Limit exceeded: {}", msg);
    }
    _ => {}
}
```

## Error Helper Methods

```rust
use zesven::Error;

fn handle_error(error: &Error) {
    // Check error categories
    if error.is_security_error() { /* Path traversal or symlink security issue */ }
    if error.is_recoverable() { /* Operation can be retried with different input */ }
    if error.is_corruption() { /* CRC mismatch or corrupt header */ }
    if error.is_encryption_error() { /* Wrong password or crypto failure */ }
    if error.is_unsupported() { /* Unsupported method or feature */ }

    // Get context about which entry caused the error
    if let Some(idx) = error.entry_index() {
        eprintln!("Error in entry {}", idx);
    }
    if let Some(name) = error.entry_name() {
        eprintln!("Error for: {}", name);
    }
    if let Some(method) = error.method_id() {
        eprintln!("Unsupported method ID: {:#x}", method);
    }
}
```

## Converting Errors

The `Error` type implements `From` for common types:

```rust
use zesven::{Error, Result};

fn example() -> Result<()> {
    // std::io::Error automatically converts
    let file = std::fs::File::open("file.txt")?;

    Ok(())
}
```

## Error Display

Errors implement `Display` for user-friendly messages:

```rust
use zesven::Archive;

if let Err(e) = Archive::open_path("archive.7z") {
    eprintln!("Failed to open archive: {}", e);
}
```

## Recoverable vs. Fatal Errors

| Error                        | Recoverable | Action                         |
| ---------------------------- | ----------- | ------------------------------ |
| `WrongPassword`              | Yes         | Prompt for password            |
| `PasswordRequired`           | Yes         | Provide password               |
| `VolumeMissing`              | Yes         | Provide missing volume file    |
| `Cancelled`                  | Yes         | User requested                 |
| `Io(WouldBlock/Interrupted)` | Yes         | Retry operation                |
| `Io(NotFound)`               | No          | File does not exist            |
| `UnsupportedMethod`          | No          | Recompile with feature enabled |
| `InvalidFormat`              | No          | File is corrupted or not 7z    |

## See Also

- [Quick Start](../) - Basic error handling
- [Resource Limits](../safety/resource-limits) - Limit errors
