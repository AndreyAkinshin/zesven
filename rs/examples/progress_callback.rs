//! Monitor extraction progress using callbacks.
//!
//! This example demonstrates how to implement and use progress callbacks
//! to monitor archive extraction in real-time:
//! - Implementing the `ProgressReporter` trait
//! - Thread-safe progress tracking with atomic counters
//! - Displaying progress bars and status updates
//!
//! # Usage
//!
//! ```bash
//! cargo run --example progress_callback -- archive.7z ./output
//! ```

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use zesven::progress::ProgressReporter;
use zesven::{Archive, ExtractOptions, Result};

/// A progress reporter that tracks extraction progress.
///
/// This struct implements thread-safe progress tracking using atomic counters,
/// making it safe to use with parallel extraction.
struct ProgressBarReporter {
    /// Total bytes extracted so far
    bytes_extracted: AtomicU64,
    /// Total bytes expected (sum of all entry sizes)
    total_bytes: u64,
    /// Number of entries processed
    entries_processed: AtomicU64,
    /// Total number of entries
    total_entries: u64,
    /// Current entry name (for display)
    current_entry: std::sync::Mutex<String>,
}

impl ProgressBarReporter {
    /// Creates a new progress reporter.
    fn new(total_bytes: u64, total_entries: u64) -> Self {
        Self {
            bytes_extracted: AtomicU64::new(0),
            total_bytes,
            entries_processed: AtomicU64::new(0),
            total_entries,
            current_entry: std::sync::Mutex::new(String::new()),
        }
    }

    /// Returns the current progress as a percentage.
    fn percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            100.0
        } else {
            let extracted = self.bytes_extracted.load(Ordering::Relaxed);
            (extracted as f64 / self.total_bytes as f64) * 100.0
        }
    }

    /// Prints a progress bar to the terminal.
    fn print_progress(&self) {
        let percentage = self.percentage();
        let bar_width = 40;
        let filled = ((percentage / 100.0) * bar_width as f64) as usize;
        let empty = bar_width - filled;

        let entries = self.entries_processed.load(Ordering::Relaxed);
        let current = self.current_entry.lock().unwrap();

        print!(
            "\r[{}{}] {:.1}% ({}/{} entries) {}",
            "=".repeat(filled),
            " ".repeat(empty),
            percentage,
            entries,
            self.total_entries,
            current.chars().take(30).collect::<String>()
        );

        // Pad with spaces to clear previous longer names
        print!("{}", " ".repeat(20));
        use std::io::Write;
        std::io::stdout().flush().ok();
    }
}

impl ProgressReporter for ProgressBarReporter {
    fn on_entry_start(&mut self, entry_name: &str, _size: u64) {
        if let Ok(mut current) = self.current_entry.lock() {
            *current = entry_name.to_string();
        }
        self.print_progress();
    }

    fn on_progress(&mut self, bytes_processed: u64, _total_bytes: u64) -> bool {
        self.bytes_extracted
            .fetch_add(bytes_processed, Ordering::Relaxed);
        self.print_progress();
        true // Continue extraction
    }

    fn on_entry_complete(&mut self, _entry_name: &str, success: bool) {
        if success {
            self.entries_processed.fetch_add(1, Ordering::Relaxed);
        }
        self.print_progress();
    }
}

/// An alternative simpler progress callback using a closure-like approach.
struct SimpleProgress {
    count: usize,
}

impl SimpleProgress {
    fn new() -> Self {
        Self { count: 0 }
    }
}

impl ProgressReporter for SimpleProgress {
    fn on_entry_start(&mut self, entry_name: &str, size: u64) {
        println!("Starting: {} ({} bytes)", entry_name, size);
    }

    fn on_progress(&mut self, bytes_processed: u64, total_bytes: u64) -> bool {
        if total_bytes > 0 {
            let percent = (bytes_processed as f64 / total_bytes as f64) * 100.0;
            print!("\r  Progress: {:.0}%", percent);
            use std::io::Write;
            std::io::stdout().flush().ok();
        }
        true // Continue extraction
    }

    fn on_entry_complete(&mut self, entry_name: &str, success: bool) {
        self.count += 1;
        let status = if success { "OK" } else { "FAILED" };
        println!("\r  [{}] {} - {}", self.count, entry_name, status);
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <archive.7z> <output_dir> [--simple]", args[0]);
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --simple    Use simple progress output instead of progress bar");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} archive.7z ./output", args[0]);
        eprintln!("  {} archive.7z ./output --simple", args[0]);
        std::process::exit(1);
    }

    let archive_path = &args[1];
    let output_dir = &args[2];
    let use_simple = args.get(3).is_some_and(|a| a == "--simple");

    // Open the archive
    println!("Opening archive: {}", archive_path);
    let mut archive = Archive::open_path(archive_path)?;

    // Calculate totals for progress tracking
    let total_bytes: u64 = archive.entries().iter().map(|e| e.size).sum();
    let total_entries = archive.len() as u64;

    println!("Archive info:");
    println!("  Entries: {}", total_entries);
    println!("  Total size: {} bytes", total_bytes);
    println!();

    if use_simple {
        // Use simple progress callback
        println!("Extracting with simple progress...");
        println!();

        let progress = SimpleProgress::new();
        let options = ExtractOptions::new().progress(progress);
        let result = archive.extract(output_dir, (), &options)?;

        println!();
        println!("Extraction complete!");
        println!("  Extracted: {} entries", result.entries_extracted);
        println!("  Failed: {} entries", result.entries_failed);
    } else {
        // Use progress bar
        println!("Extracting with progress bar...");
        println!();

        let progress = ProgressBarReporter::new(total_bytes, total_entries);
        let options = ExtractOptions::new().progress(progress);
        let result = archive.extract(output_dir, (), &options)?;

        // Clear line and print final status
        println!();
        println!();
        println!("Extraction complete!");
        println!("  Extracted: {} entries", result.entries_extracted);
        println!("  Bytes: {} bytes", result.bytes_extracted);
        println!("  Failed: {} entries", result.entries_failed);
    }

    Ok(())
}
