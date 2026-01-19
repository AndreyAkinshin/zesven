---
title: Creating Encrypted Archives
description: Create password-protected archives with AES-256
---

# Creating Encrypted Archives

Learn how to create archives protected with AES-256 encryption.

## Encryption Types

zesven supports two types of encryption that can be used independently or together:

| Type                   | Method                 | What it protects        |
| ---------------------- | ---------------------- | ----------------------- |
| **Content encryption** | `encrypt_data(true)`   | File contents           |
| **Header encryption**  | `encrypt_header(true)` | File names and metadata |

## Content Encryption

Encrypt file contents with `encrypt_data()`:

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret_password"))
        .encrypt_data(true);  // Encrypt file contents

    let mut writer = Writer::create_path("encrypted.7z")?
        .options(options);

    writer.add_bytes(ArchivePath::new("secret.txt")?, b"Secret data")?;
    writer.finish()?;
    Ok(())
}
```

With content encryption:

- File data is encrypted with AES-256
- File names remain visible (use header encryption to hide them)
- Password required to extract files

## Header Encryption

Hide file names by encrypting the header:

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret"))
        .encrypt_header(true);  // Hide file names

    let mut writer = Writer::create_path("fully_encrypted.7z")?
        .options(options);

    writer.add_bytes(ArchivePath::new("classified.txt")?, b"Top secret")?;
    writer.finish()?;
    Ok(())
}
```

With header encryption:

- File names are hidden until password is entered
- Archive structure is not visible
- Provides better privacy

## Full Encryption

For maximum security, enable both content and header encryption:

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret"))
        .encrypt_data(true)    // Encrypt file contents
        .encrypt_header(true); // Hide file names

    let mut writer = Writer::create_path("fully_encrypted.7z")?
        .options(options);

    writer.add_bytes(ArchivePath::new("classified.txt")?, b"Top secret")?;
    writer.finish()?;
    Ok(())
}
```

This provides:

- **Content protection**: File data encrypted with AES-256
- **Metadata protection**: File names and sizes hidden
- **Privacy**: No information visible without the password

## Key Derivation Strength

The 7z format uses SHA-256 with configurable iteration counts for key derivation. The default iteration count provides a good balance between security and performance. Higher iteration counts make brute-force attacks slower but also slow down legitimate access.

| Iterations (2^n) | Value      | Security | Speed     |
| ---------------- | ---------- | -------- | --------- |
| 16               | 65,536     | Minimum  | Fast      |
| 19               | 524,288    | Default  | Medium    |
| 22               | 4,194,304  | High     | Slow      |
| 24               | 16,777,216 | Maximum  | Very slow |

## Combining with Compression

Encryption works with all compression settings:

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};

fn main() -> Result<()> {
    let options = WriteOptions::new()
        .password(Password::new("secret"))
        .encrypt_header(true)
        .level(9)?
        .solid();

    let mut writer = Writer::create_path("secure_backup.7z")?
        .options(options);

    writer.add_path("documents", ArchivePath::new("documents")?)?;
    writer.finish()?;
    Ok(())
}
```

## Password Security

Best practices for passwords:

```rust
use zesven::{Writer, WriteOptions, Password, ArchivePath, Result};
use zeroize::Zeroizing;

fn main() -> Result<()> {
    // Use Zeroizing to clear password from memory
    let password = Zeroizing::new(String::from("my_secure_password_123!"));

    let options = WriteOptions::new()
        .password(Password::new(&*password))
        .encrypt_header(true);

    let mut writer = Writer::create_path("secure.7z")?
        .options(options);

    writer.add_bytes(ArchivePath::new("data.txt")?, b"Sensitive data")?;
    writer.finish()?;

    // password is automatically zeroed when dropped
    Ok(())
}
```

## Password Guidelines

For secure archives:

- Use 12+ characters
- Mix uppercase, lowercase, numbers, and symbols
- Avoid dictionary words
- Use a password manager to generate and store passwords
- Enable header encryption for maximum privacy
- Use high iteration counts for sensitive data

## See Also

- [Reading Encrypted](./reading-encrypted) - Open encrypted archives
- [Encryption Overview](./) - Encryption concepts
- [7z Spec: Encryption](/7z/12-encryption) - Format details
