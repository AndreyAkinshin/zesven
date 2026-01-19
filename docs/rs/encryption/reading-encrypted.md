---
title: Reading Encrypted Archives
description: Open and extract password-protected archives
---

# Reading Encrypted Archives

Learn how to open and extract archives protected with AES-256 encryption.

## Basic Usage

Provide the password when opening:

```rust
use zesven::{Archive, Password, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path_with_password(
        "encrypted.7z",
        Password::new("my_password"),
    )?;

    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Detecting Encrypted Archives

Check if an archive is encrypted before opening:

```rust
use zesven::{Archive, Password, Result};

fn main() -> Result<()> {
    // Try opening without password first
    match Archive::open_path("archive.7z") {
        Ok(archive) => {
            println!("Archive is not encrypted");
        }
        Err(zesven::Error::WrongPassword { .. }) => {
            println!("Archive requires a password");
            // Prompt user for password and retry
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
```

## Password Prompting

Interactive password prompt:

```rust
use zesven::{Archive, Password, ExtractOptions, Result};
use std::io::{self, Write};

fn main() -> Result<()> {
    let path = "encrypted.7z";

    // Try without password
    let mut archive = match Archive::open_path(path) {
        Ok(archive) => archive,
        Err(zesven::Error::WrongPassword { .. }) => {
            // Prompt for password
            print!("Password: ");
            io::stdout().flush()?;
            let mut password = String::new();
            io::stdin().read_line(&mut password)?;
            let password = password.trim();

            Archive::open_path_with_password(path, Password::new(password))?
        }
        Err(e) => return Err(e),
    };

    archive.extract("./output", (), &ExtractOptions::default())?;
    Ok(())
}
```

## Handling Wrong Passwords

```rust
use zesven::{Archive, Password, Error, Result};

fn try_passwords(path: &str, passwords: &[&str]) -> Result<Archive<std::io::BufReader<std::fs::File>>> {
    for password in passwords {
        match Archive::open_path_with_password(path, Password::new(*password)) {
            Ok(archive) => return Ok(archive),
            Err(Error::WrongPassword { .. }) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::PasswordRequired)
}

fn main() -> Result<()> {
    let archive = try_passwords("archive.7z", &["password1", "password2", "secret"])?;
    println!("Found correct password!");
    Ok(())
}
```

## Header Encryption

When headers are encrypted, file names are hidden until the password is provided:

```rust
use zesven::{Archive, Password, Result};

fn main() -> Result<()> {
    // Without password, can't even see file names
    let archive = Archive::open_path_with_password(
        "header_encrypted.7z",
        Password::new("secret"),
    )?;

    // Now we can see the file names
    for entry in archive.entries() {
        println!("{}", entry.path.as_str());
    }
    Ok(())
}
```

## Mixed Encryption

Some archives have only certain files encrypted:

```rust
use zesven::{Archive, Password, ExtractOptions, Result};

fn main() -> Result<()> {
    let mut archive = Archive::open_path("mixed.7z")?;

    for entry in archive.entries() {
        if entry.is_encrypted {
            println!("{} (encrypted)", entry.path.as_str());
        } else {
            println!("{}", entry.path.as_str());
        }
    }

    // Need password to extract encrypted entries
    let options = ExtractOptions::new()
        .password(Password::new("secret"));

    archive.extract("./output", (), &options)?;
    Ok(())
}
```

## See Also

- [Creating Encrypted](./creating-encrypted) - Create encrypted archives
- [Encryption Overview](./) - Encryption concepts
