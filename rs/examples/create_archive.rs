//! Create a simple 7z archive from files.
//!
//! This example demonstrates basic archive creation:
//! - Creating an archive from files on disk
//! - Adding data from memory
//! - Configuring compression options
//! - Getting compression statistics
//!
//! # Usage
//!
//! ```bash
//! cargo run --example create_archive -- output.7z file1.txt file2.txt
//! ```

use std::env;
use zesven::codec::CodecMethod;
use zesven::{ArchivePath, Result, WriteOptions, Writer};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <output.7z> [file1] [file2...]", args[0]);
        eprintln!();
        eprintln!("Creates a 7z archive from the specified files.");
        eprintln!("If no files are specified, creates a demo archive with sample data.");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} archive.7z file1.txt file2.txt", args[0]);
        eprintln!("  {} demo.7z  # Creates demo archive", args[0]);
        std::process::exit(1);
    }

    let output_path = &args[1];
    let input_files: Vec<&String> = args[2..].iter().collect();

    // Configure compression options
    let options = WriteOptions::new()
        .method(CodecMethod::Lzma2) // Best compression
        .level(5)? // Medium compression level (0-9)
        .deterministic(true); // Reproducible output

    println!("Creating archive: {}", output_path);
    println!("Compression: LZMA2 level 5");
    println!();

    // Create the archive writer
    let mut writer = Writer::create_path(output_path)?.options(options);

    if input_files.is_empty() {
        // Create a demo archive with sample data
        println!("No files specified, creating demo archive...");
        println!();

        // Add a text file from memory
        let readme_content = b"Welcome to zesven!\n\nThis is a demo archive.";
        writer.add_bytes(ArchivePath::new("readme.txt")?, readme_content)?;
        println!("  Added: readme.txt ({} bytes)", readme_content.len());

        // Add a JSON config file
        let config_content = br#"{
    "name": "zesven",
    "version": "0.1.0",
    "features": ["lzma2", "aes", "parallel"]
}"#;
        writer.add_bytes(ArchivePath::new("config.json")?, config_content)?;
        println!("  Added: config.json ({} bytes)", config_content.len());

        // Add some sample data
        let data = vec![0u8; 1000]; // 1KB of zeros (compresses well)
        writer.add_bytes(ArchivePath::new("data/sample.bin")?, &data)?;
        println!("  Added: data/sample.bin ({} bytes)", data.len());

        // Add a larger file to demonstrate compression
        let large_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        writer.add_bytes(ArchivePath::new("data/large.bin")?, &large_data)?;
        println!("  Added: data/large.bin ({} bytes)", large_data.len());
    } else {
        // Add files from disk
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
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

            writer.add_path(path, archive_path)?;
            println!("  Added: {} ({} bytes)", archive_name, size);
        }
    }

    // Finish writing the archive
    let result = writer.finish()?;

    println!();
    println!("Archive created successfully!");
    println!("Statistics:");
    println!("  Files written: {}", result.entries_written);
    println!("  Directories: {}", result.directories_written);
    println!("  Original size: {} bytes", result.total_size);
    println!("  Compressed size: {} bytes", result.compressed_size);

    if result.total_size > 0 {
        println!(
            "  Compression ratio: {:.1}%",
            result.compression_ratio() * 100.0
        );
        println!("  Space saved: {:.1}%", result.space_savings() * 100.0);
    }

    Ok(())
}
