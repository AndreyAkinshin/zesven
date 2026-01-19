---
title: Encryption
description: AES-256 encryption for 7z archives
---

# Encryption

zesven supports AES-256 encryption for protecting archive contents.

## Overview

The `aes` feature (enabled by default) provides:

- **Content encryption** (`encrypt_data`) - Encrypts file data with AES-256-CBC
- **Header encryption** (`encrypt_header`) - Hides file names and metadata
- **Key derivation** with SHA-256 and configurable iteration counts

Both encryption types can be used independently or together for maximum security.

## Quick Example

### Reading Encrypted Archives

```rust
use zesven::{Archive, Password, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path_with_password(
        "encrypted.7z",
        Password::new("secret"),
    )?;

    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

### Creating Encrypted Archives

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret"))
        .encrypt_data(true)    // Encrypt file contents
        .encrypt_header(true); // Hide file names

    let mut writer = Writer::create_path("encrypted.7z")?
        .options(options);

    writer.add_bytes(ArchivePath::new("secret.txt")?, b"Secret data")?;
    writer.finish()?;
    Ok(())
}
```

## Topics

- [Reading Encrypted](./reading-encrypted) - Open password-protected archives
- [Creating Encrypted](./creating-encrypted) - Create encrypted archives

## Encryption Details

### Algorithm

zesven uses the same encryption scheme as 7-Zip:

1. **Key Derivation**: SHA-256 with salt and iteration count
2. **Cipher**: AES-256-CBC
3. **IV**: Random 16-byte initialization vector per block

### Security Considerations

- Use strong passwords (12+ characters, mixed case, numbers, symbols)
- Enable header encryption to hide file names
- Higher iteration counts slow down brute-force attacks

## Feature Flag

Encryption requires the `aes` feature:

```toml
[dependencies]
zesven = { version = "1.0", features = ["aes"] }
```

This is enabled by default.

## See Also

- [7z Spec: Encryption](/7z/12-encryption) - Format specification
- [7z Spec: Security](/7z/17-security) - Security considerations
