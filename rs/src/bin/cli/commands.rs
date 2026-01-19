//! Command implementations for the CLI tool.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use zesven::{
    Archive, ArchivePath, ExtractOptions, TestOptions, WriteOptions, Writer,
    read::{OverwritePolicy, PreserveMetadata, Threads},
};

use crate::exit_codes::{ExitCode, error_to_exit_code};
use crate::file_selector::FileSelector;
use crate::output::create_formatter;
use crate::password::{get_or_prompt_password, get_password};
use crate::progress::{CliProgress, SimpleProgress};
use crate::{CompressionMethod, OutputFormat, OverwriteMode};

/// Configuration for the extract command.
pub struct ExtractConfig<'a> {
    pub archive_path: &'a Path,
    pub output_dir: &'a Path,
    pub include: &'a [String],
    pub exclude: &'a [String],
    pub overwrite: OverwriteMode,
    pub password: Option<String>,
    pub preserve_metadata: bool,
    pub format: OutputFormat,
    pub quiet: bool,
    pub thread_count: usize,
}

/// Configuration for the create command.
#[allow(dead_code)] // Some fields reserved for future features
pub struct CreateConfig<'a> {
    pub archive_path: &'a Path,
    pub files: &'a [PathBuf],
    pub method: CompressionMethod,
    pub level: u8,
    pub solid: bool,
    pub password: Option<String>,
    pub encrypt_headers: bool,
    pub deterministic: bool,
    pub exclude: &'a [String],
    pub recursive: bool,
    pub format: OutputFormat,
    pub quiet: bool,
    pub thread_count: usize,
}

/// Extract command implementation
pub fn extract(config: &ExtractConfig<'_>) -> ExitCode {
    let formatter = create_formatter(config.format);

    // Open the archive
    let archive = match open_archive(config.archive_path, config.password.clone()) {
        Ok(a) => a,
        Err(code) => return code,
    };

    let info = archive.info();

    // Check if we need a password
    if info.has_encrypted_entries || info.has_encrypted_header {
        let _pwd = get_password(config.password.clone(), true);
        // Password handling would be integrated with extraction
    }

    // Build selector
    let selector = match FileSelector::new(config.include, config.exclude) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::BadArgs;
        }
    };

    // Build options
    let overwrite_policy = match config.overwrite {
        OverwriteMode::Always => OverwritePolicy::Overwrite,
        OverwriteMode::Never => OverwritePolicy::Skip,
        OverwriteMode::Prompt => {
            // Interactive prompting is handled separately in the extraction loop
            OverwritePolicy::Skip
        }
    };

    // Track "all" choices for session during prompt mode
    let mut prompt_all_yes = false;
    let mut prompt_all_no = false;

    let threads = match config.thread_count {
        0 => Threads::Auto,
        n => Threads::count_or_single(n),
    };

    let metadata = if config.preserve_metadata {
        PreserveMetadata::all()
    } else {
        PreserveMetadata::none()
    };

    let options = ExtractOptions::new()
        .overwrite(overwrite_policy)
        .threads(threads)
        .preserve_metadata(metadata);

    // Create output directory if needed
    if let Err(e) = std::fs::create_dir_all(config.output_dir) {
        eprintln!("Error creating output directory: {}", e);
        return ExitCode::IoError;
    }

    // Create progress display
    let progress = CliProgress::new(info.entry_count as u64, config.quiet);
    if !config.quiet {
        progress.set_message("Extracting...");
    }

    // Perform extraction
    let mut archive = archive;
    let result = if matches!(config.overwrite, OverwriteMode::Prompt) {
        // Interactive extraction with prompting
        extract_with_prompts(
            &mut archive,
            config.output_dir,
            &selector,
            &options,
            &mut prompt_all_yes,
            &mut prompt_all_no,
            &progress,
        )
    } else {
        // Standard extraction
        archive.extract(config.output_dir, &selector, &options)
    };

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            progress.finish_with_message("Failed");
            eprintln!("Error: {}", e);
            return error_to_exit_code(&e);
        }
    };

    progress.finish();

    // Output results
    print!("{}", formatter.format_extract_result(&result));

    if result.is_ok() {
        ExitCode::Success
    } else {
        ExitCode::Warning
    }
}

/// Create command implementation
pub fn create(config: &CreateConfig<'_>) -> ExitCode {
    let _formatter = create_formatter(config.format);

    // Get password if encryption requested
    let pwd = if config.password.is_some() {
        get_or_prompt_password(config.password.clone(), true)
    } else {
        None
    };

    // Build write options
    let mut options = match WriteOptions::new()
        .method(config.method.into())
        .level(config.level as u32)
    {
        Ok(opts) => opts.deterministic(config.deterministic),
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::BadArgs;
        }
    };

    if config.solid {
        options = options.solid();
    }

    #[cfg(feature = "aes")]
    if let Some(ref p) = pwd {
        options = options.password(p.as_str());
    }

    // Create the writer
    let mut writer = match Writer::create_path(config.archive_path) {
        Ok(w) => w.options(options),
        Err(e) => {
            eprintln!("Error creating archive: {}", e);
            return error_to_exit_code(&e);
        }
    };

    // Build exclude selector
    let exclude_selector = match FileSelector::new(&[], config.exclude) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::BadArgs;
        }
    };

    // Collect all files to add
    let mut all_files: Vec<(std::path::PathBuf, String)> = Vec::new();

    for path in config.files {
        if path.is_dir() {
            if config.recursive {
                for entry in WalkDir::new(path).follow_links(false) {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!("Warning: {}", e);
                            continue;
                        }
                    };

                    let rel_path = entry
                        .path()
                        .strip_prefix(path)
                        .unwrap_or(entry.path())
                        .to_string_lossy()
                        .to_string();

                    if rel_path.is_empty() {
                        continue;
                    }

                    if !exclude_selector.matches(&rel_path) {
                        continue;
                    }

                    all_files.push((entry.path().to_path_buf(), rel_path));
                }
            } else {
                eprintln!(
                    "Warning: {} is a directory, use -r for recursive",
                    path.display()
                );
            }
        } else if path.is_file() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if exclude_selector.matches(&name) {
                all_files.push((path.clone(), name));
            }
        } else {
            eprintln!("Warning: {} does not exist", path.display());
        }
    }

    if all_files.is_empty() {
        eprintln!("Error: No files to add to archive");
        return ExitCode::BadArgs;
    }

    // Progress
    let progress = SimpleProgress::new(all_files.len() as u64, config.quiet);
    if !config.quiet {
        progress.set_message("Creating archive...");
    }

    // Add files
    for (disk_path, archive_name) in &all_files {
        let archive_path = match ArchivePath::new(archive_name) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Warning: Invalid path {}: {}", archive_name, e);
                progress.inc(1);
                continue;
            }
        };

        if let Err(e) = writer.add_path(disk_path, archive_path) {
            eprintln!("Warning: Failed to add {}: {}", disk_path.display(), e);
        }
        progress.inc(1);
    }

    // Finish writing
    let result = match writer.finish() {
        Ok(r) => r,
        Err(e) => {
            progress.finish_with_message("Failed");
            eprintln!("Error finalizing archive: {}", e);
            return error_to_exit_code(&e);
        }
    };

    progress.finish();

    if !config.quiet {
        println!(
            "Created archive with {} files ({} -> {})",
            result.entries_written,
            crate::output::humanize_bytes(result.total_size),
            crate::output::humanize_bytes(result.compressed_size)
        );
        println!(
            "Compression ratio: {:.1}% (saved {:.1}%)",
            result.compression_ratio() * 100.0,
            result.space_savings() * 100.0
        );
    }

    ExitCode::Success
}

/// List command implementation
pub fn list(
    archive_path: &Path,
    technical: bool,
    password: Option<String>,
    format: OutputFormat,
    _quiet: bool,
) -> ExitCode {
    let formatter = create_formatter(format);

    // Open the archive
    let archive = match open_archive(archive_path, password) {
        Ok(a) => a,
        Err(code) => return code,
    };

    // Get entries
    let entries = archive.entries();

    // Output
    print!("{}", formatter.format_list(entries, technical));

    ExitCode::Success
}

/// Test command implementation
pub fn test(
    archive_path: &Path,
    password: Option<String>,
    include: &[String],
    format: OutputFormat,
    quiet: bool,
    thread_count: usize,
) -> ExitCode {
    let formatter = create_formatter(format);

    // Open the archive
    let mut archive = match open_archive(archive_path, password) {
        Ok(a) => a,
        Err(code) => return code,
    };

    // Build selector
    let selector = match FileSelector::new(include, &[]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::BadArgs;
        }
    };

    let threads = match thread_count {
        0 => Threads::Auto,
        n => Threads::count_or_single(n),
    };

    let options = TestOptions::new().threads(threads);

    // Progress
    let info = archive.info();
    let progress = SimpleProgress::new(info.entry_count as u64, quiet);
    if !quiet {
        progress.set_message("Testing...");
    }

    // Perform test
    let result = match archive.test(&selector, &options) {
        Ok(r) => r,
        Err(e) => {
            progress.finish_with_message("Failed");
            eprintln!("Error: {}", e);
            return error_to_exit_code(&e);
        }
    };

    progress.finish();

    // Output results
    print!("{}", formatter.format_test_result(&result));

    if result.is_ok() {
        ExitCode::Success
    } else {
        ExitCode::BadArchive
    }
}

/// Info command implementation
pub fn info(
    archive_path: &Path,
    password: Option<String>,
    format: OutputFormat,
    _quiet: bool,
) -> ExitCode {
    let formatter = create_formatter(format);

    // Open the archive
    let archive = match open_archive(archive_path, password) {
        Ok(a) => a,
        Err(code) => return code,
    };

    // Get info
    let info = archive.info();

    // Output
    print!("{}", formatter.format_info(info));

    ExitCode::Success
}

/// Helper to open an archive with optional password
fn open_archive(
    path: &Path,
    password: Option<String>,
) -> Result<Archive<std::io::BufReader<std::fs::File>>, ExitCode> {
    // First try to open without password to check if encrypted
    let archive = if let Some(pwd) = password {
        #[cfg(feature = "aes")]
        {
            Archive::open_path_with_password(path, pwd).map_err(|e| {
                eprintln!("Error opening archive: {}", e);
                error_to_exit_code(&e)
            })?
        }
        #[cfg(not(feature = "aes"))]
        {
            let _ = pwd;
            eprintln!("Error: AES encryption support not enabled");
            return Err(ExitCode::FatalError);
        }
    } else {
        Archive::open_path(path).map_err(|e| {
            eprintln!("Error opening archive: {}", e);
            error_to_exit_code(&e)
        })?
    };

    Ok(archive)
}

/// User response to overwrite prompt.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OverwriteResponse {
    Yes,
    No,
    YesAll,
    NoAll,
}

/// Prompts the user about overwriting an existing file.
fn prompt_overwrite(path: &Path) -> OverwriteResponse {
    use dialoguer::{Select, theme::ColorfulTheme};

    let items = &[
        "Yes - overwrite this file",
        "No - skip this file",
        "Yes to all - overwrite all existing files",
        "No to all - skip all existing files",
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("File exists: {}", path.display()))
        .items(items)
        .default(1) // Default to "No"
        .interact();

    match selection {
        Ok(0) => OverwriteResponse::Yes,
        Ok(1) => OverwriteResponse::No,
        Ok(2) => OverwriteResponse::YesAll,
        Ok(3) => OverwriteResponse::NoAll,
        _ => OverwriteResponse::No, // On error, default to skip
    }
}

/// Extracts with interactive prompts for existing files.
fn extract_with_prompts<R: std::io::Read + std::io::Seek>(
    archive: &mut Archive<R>,
    output_dir: &Path,
    selector: &crate::file_selector::FileSelector,
    _options: &ExtractOptions,
    all_yes: &mut bool,
    all_no: &mut bool,
    progress: &CliProgress,
) -> zesven::Result<zesven::read::ExtractResult> {
    use zesven::read::{EntrySelector, ExtractResult};

    let mut result = ExtractResult::default();

    // Get indices of entries to extract
    let entries_to_extract: Vec<usize> = archive
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| selector.select(e))
        .map(|(idx, _)| idx)
        .collect();

    for idx in entries_to_extract {
        let entry = &archive.entries()[idx];
        let entry_path = entry.path.as_str().to_string();
        let is_directory = entry.is_directory;

        if is_directory {
            // Create directory
            let dir_path = output_dir.join(&entry_path);
            if let Err(e) = std::fs::create_dir_all(&dir_path) {
                result.entries_failed += 1;
                result.failures.push((entry_path.clone(), e.to_string()));
            } else {
                result.entries_extracted += 1;
            }
            progress.inc(1);
            continue;
        }

        // Check if file exists
        let file_path = output_dir.join(&entry_path);

        if file_path.exists() {
            // Check "all" flags first
            if *all_no {
                result.entries_skipped += 1;
                progress.inc(1);
                continue;
            }

            if !*all_yes {
                // Prompt user
                let response = prompt_overwrite(&file_path);
                match response {
                    OverwriteResponse::Yes => {}
                    OverwriteResponse::No => {
                        result.entries_skipped += 1;
                        progress.inc(1);
                        continue;
                    }
                    OverwriteResponse::YesAll => {
                        *all_yes = true;
                    }
                    OverwriteResponse::NoAll => {
                        *all_no = true;
                        result.entries_skipped += 1;
                        progress.inc(1);
                        continue;
                    }
                }
            }
        }

        // Create parent directories
        if let Some(parent) = file_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                result.entries_failed += 1;
                result.failures.push((entry_path.clone(), e.to_string()));
                progress.inc(1);
                continue;
            }
        }

        // Extract the file
        match archive.extract_entry_to_vec_by_index(idx) {
            Ok(data) => match std::fs::write(&file_path, &data) {
                Ok(()) => {
                    result.entries_extracted += 1;
                    result.bytes_extracted += data.len() as u64;
                }
                Err(e) => {
                    result.entries_failed += 1;
                    result.failures.push((entry_path.clone(), e.to_string()));
                }
            },
            Err(e) => {
                result.entries_failed += 1;
                result.failures.push((entry_path.clone(), e.to_string()));
            }
        }

        progress.inc(1);
    }

    Ok(result)
}
