//! Extract only specific files from a 7z archive.
//!
//! This example demonstrates how to selectively extract files from an archive
//! using different filtering strategies:
//! - Extract files by extension (e.g., only .txt files)
//! - Extract files by name pattern
//! - Extract files matching custom predicates
//!
//! # Usage
//!
//! ```bash
//! cargo run --example extract_selective -- archive.7z ./output
//! ```

use std::env;
use std::path::Path;
use zesven::{Archive, Entry, ExtractOptions, Result};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <archive.7z> <output_dir>", args[0]);
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} archive.7z ./output", args[0]);
        std::process::exit(1);
    }

    let archive_path = &args[1];
    let output_dir = &args[2];

    // Open the archive
    println!("Opening archive: {}", archive_path);
    let mut archive = Archive::open_path(archive_path)?;

    println!("Archive contains {} entries:", archive.len());
    for entry in archive.entries() {
        let type_indicator = if entry.is_directory { "DIR " } else { "FILE" };
        println!(
            "  [{}] {} ({} bytes)",
            type_indicator,
            entry.path.as_str(),
            entry.size
        );
    }
    println!();

    // Example 1: Extract only .txt files using a closure
    println!("Extracting only .txt files...");
    let txt_selector = |entry: &Entry| !entry.is_directory && entry.path.as_str().ends_with(".txt");

    let txt_output = Path::new(output_dir).join("txt_only");
    let result = archive.extract(&txt_output, txt_selector, &ExtractOptions::default())?;
    println!(
        "  Extracted {} .txt files to {}",
        result.entries_extracted,
        txt_output.display()
    );
    println!();

    // Example 2: Extract files larger than a certain size
    println!("Extracting files larger than 100 bytes...");
    let large_file_selector = |entry: &Entry| !entry.is_directory && entry.size > 100;

    let large_output = Path::new(output_dir).join("large_files");
    let result = archive.extract(
        &large_output,
        large_file_selector,
        &ExtractOptions::default(),
    )?;
    println!(
        "  Extracted {} large files to {}",
        result.entries_extracted,
        large_output.display()
    );
    println!();

    // Example 3: Extract specific files by name
    println!("Extracting specific files by name...");
    let specific_names: &[&str] = &["readme.txt", "config.json", "data.xml"];
    let name_selector = |entry: &Entry| {
        let name = entry.name().to_lowercase();
        specific_names.iter().any(|&n| name == n)
    };

    let specific_output = Path::new(output_dir).join("specific");
    let result = archive.extract(&specific_output, name_selector, &ExtractOptions::default())?;
    println!(
        "  Extracted {} specific files to {}",
        result.entries_extracted,
        specific_output.display()
    );
    println!();

    // Example 4: Extract all files except certain patterns
    println!("Extracting all files except temporary files...");
    let exclude_temp_selector = |entry: &Entry| {
        let name = entry.name();
        !name.ends_with(".tmp")
            && !name.ends_with(".bak")
            && !name.starts_with("~")
            && !name.starts_with(".")
    };

    let filtered_output = Path::new(output_dir).join("filtered");
    let result = archive.extract(
        &filtered_output,
        exclude_temp_selector,
        &ExtractOptions::default(),
    )?;
    println!(
        "  Extracted {} files (excluding temp files) to {}",
        result.entries_extracted,
        filtered_output.display()
    );

    println!();
    println!("Selective extraction complete!");

    Ok(())
}
