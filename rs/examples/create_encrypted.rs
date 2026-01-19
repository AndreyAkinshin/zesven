//! Create an encrypted 7z archive with AES-256 encryption.
//!
//! This example demonstrates how to create password-protected archives:
//! - Setting up encryption with AES-256
//! - Configuring compression options
//! - Adding files and directories
//! - Getting compression statistics
//!
//! # Usage
//!
//! ```bash
//! cargo run --example create_encrypted --features aes -- output.7z secret_password file1.txt file2.txt
//! ```
//!
//! # Note
//!
//! This example requires the `aes` feature to be enabled.

#[cfg(feature = "aes")]
use zesven::{ArchivePath, Password, Result, WriteOptions, Writer};

#[cfg(feature = "aes")]
use std::env;

#[cfg(feature = "aes")]
fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 4 {
        eprintln!(
            "Usage: {} <output.7z> <password> <file1> [file2...]",
            args[0]
        );
        eprintln!();
        eprintln!("Creates an encrypted 7z archive with the specified files.");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} secret.7z mypassword document.pdf", args[0]);
        eprintln!("  {} backup.7z strong_pass123 file1.txt file2.txt", args[0]);
        std::process::exit(1);
    }

    let output_path = &args[1];
    let password = &args[2];
    let input_files: Vec<&String> = args[3..].iter().collect();

    println!("Creating encrypted archive: {}", output_path);
    println!("Files to add: {}", input_files.len());

    // Configure write options with encryption
    let options = WriteOptions::new()
        .password(Password::new(password))
        .level(7)?; // Higher compression level (0-9)

    // Create the archive writer
    let mut writer = Writer::create_path(output_path)?.options(options);

    // Add each file to the archive
    for file_path in &input_files {
        let path = std::path::Path::new(file_path);

        if !path.exists() {
            eprintln!("Warning: File not found, skipping: {}", file_path);
            continue;
        }

        // Use the file name as the archive path
        let archive_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        let archive_path = ArchivePath::new(archive_name)?;
        println!("  Adding: {} -> {}", file_path, archive_name);

        writer.add_path(path, archive_path)?;
    }

    // Finish writing the archive
    let result = writer.finish()?;

    println!();
    println!("Archive created successfully!");
    println!("  Entries: {}", result.entries_written);
    println!("  Directories: {}", result.directories_written);
    println!("  Original size: {} bytes", result.total_size);
    println!("  Compressed size: {} bytes", result.compressed_size);
    println!(
        "  Compression ratio: {:.1}%",
        result.compression_ratio() * 100.0
    );
    println!("  Space saved: {:.1}%", result.space_savings() * 100.0);

    Ok(())
}

#[cfg(not(feature = "aes"))]
fn main() {
    eprintln!("This example requires the 'aes' feature.");
    eprintln!("Run with: cargo run --example create_encrypted --features aes -- <args>");
    std::process::exit(1);
}
