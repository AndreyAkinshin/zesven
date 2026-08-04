//! Command implementations for the CLI tool.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use zesven::{
    Archive, ArchivePath, ExtractOptions, MemoryLimit, TestOptions, WriteOptions, Writer,
    read::{OverwritePolicy, PreserveMetadata, Threads},
};

use crate::exit_codes::{ExitCode, error_to_exit_code};
use crate::file_selector::FileSelector;
use crate::output::create_formatter;
use crate::password::{get_or_prompt_password, get_password};
use crate::progress::{CliProgress, SimpleProgress, WriteProgress};
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
    pub memory_limit: Option<u64>,
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

/// Create command implementation.
///
/// The archive is built beside its destination and moved onto it once it is
/// finished. Writing in place would truncate the destination the moment the
/// run started, so a failure anywhere - a file that has gone away, a limit the
/// writer cannot work within, a full disk - would leave neither the archive
/// that was asked for nor whatever used to be there. Which is worse than
/// failing: a script that checks the exit code still finds a file at that path,
/// and so does whoever looks next week.
pub fn create(config: &CreateConfig<'_>) -> ExitCode {
    let destination = match resolve_destination(config.archive_path) {
        Ok(destination) => destination,
        Err(e) => {
            eprintln!("Error: {}: {}", config.archive_path.display(), e);
            return ExitCode::IoError;
        }
    };

    // The scratch file belongs to this run: it is created rather than opened,
    // and removed only by the value that created it. Deciding to remove it
    // from whether the path exists is not a check but the defect itself - a
    // run that failed before opening anything would delete a file it never
    // touched, which is the thing being fixed one path over.
    // The files are collected before anything is written, so that the archive
    // being built is not one of them. A scratch file created first sits in the
    // directory while the walk runs, and `zesven create out.7z -r .` then
    // archives it: the writer reads its own output while writing to it, and
    // with a stored entry that pump runs until the disk is full.
    let files = match collect_files(config, &destination) {
        Ok(files) => files,
        Err(code) => return code,
    };

    let mut scratch = match ScratchArchive::create(&destination) {
        Ok(scratch) => scratch,
        Err(e) => {
            eprintln!(
                "Error: could not start writing beside {}: {}",
                destination.display(),
                e
            );
            return ExitCode::IoError;
        }
    };

    let code = create_inner(config, scratch.take_file(), &files);
    if !matches!(code, ExitCode::Success) {
        // Dropping it removes this run's own file and nothing else.
        return code;
    }

    match scratch.commit(&destination) {
        Ok(()) => code,
        Err(e) => {
            eprintln!(
                "Error: could not move the finished archive to {}: {}",
                destination.display(),
                e,
            );
            ExitCode::IoError
        }
    }
}

/// Where the archive really goes.
///
/// A symlink names a file somewhere else, and that is what the caller means by
/// it: building beside the link and renaming onto it would replace the link
/// and leave the file it named alone. `canonicalize` resolves that, but only
/// when the target exists - a link to an unmounted disk resolves to nothing,
/// and the archive silently landed on the link instead. So the links are
/// followed one at a time, and a target that is not there yet is still where
/// the archive belongs.
fn resolve_destination(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();

    // Bounded, because a link can point at itself. Running out is not a
    // destination to fall back on: writing to the link would replace the loop
    // with a regular file and report success, where every other tool says
    // ELOOP.
    for _ in 0..40 {
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => break,
        };
        if !metadata.file_type().is_symlink() {
            break;
        }
        let target = match std::fs::read_link(&current) {
            Ok(target) => target,
            Err(_) => break,
        };
        current = if target.is_absolute() {
            target
        } else {
            match current.parent() {
                Some(parent) => parent.join(target),
                None => target,
            }
        };
    }

    if std::fs::symlink_metadata(&current)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        // `ErrorKind::FilesystemLoop` is still unstable; the message is what
        // the caller reads either way.
        return Err(std::io::Error::other(format!(
            "too many levels of symbolic links: {}",
            path.display()
        )));
    }

    Ok(current)
}

/// The archive being written, beside where it belongs.
///
/// Removes its own file when dropped, so every way out of `create` that is not
/// a commit leaves the destination and everything around it as it was.
struct ScratchArchive {
    path: PathBuf,
    file: Option<std::fs::File>,
    committed: bool,
}

impl ScratchArchive {
    fn create(destination: &Path) -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let path = destination.with_extension(format!(
            "{}part-{}-{}",
            destination
                .extension()
                .map(|e| format!("{}.", e.to_string_lossy()))
                .unwrap_or_default(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));

        // Created, never opened: a file already there is somebody's, and
        // truncating it is what this whole arrangement exists to avoid. The
        // handle is kept rather than the path reopened, so nothing can be
        // swapped in between creating it and writing to it.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;

        Ok(Self {
            path,
            file: Some(file),
            committed: false,
        })
    }

    fn take_file(&mut self) -> std::fs::File {
        self.file
            .take()
            .expect("the handle is taken once, by the writer that fills it")
    }

    /// Moves the finished archive onto its destination, keeping the mode of
    /// whatever it replaces.
    ///
    /// The mode is set before the rename rather than after: setting it
    /// afterwards leaves the archive world-readable for as long as the two
    /// calls take, and a failure there reports that the archive could not be
    /// moved when it already has been.
    fn commit(&mut self, destination: &Path) -> std::io::Result<()> {
        self.file.take();
        if let Ok(existing) = std::fs::metadata(destination) {
            std::fs::set_permissions(&self.path, existing.permissions())?;
        }
        std::fs::rename(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for ScratchArchive {
    fn drop(&mut self) {
        self.file.take();
        if self.committed {
            return;
        }
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("Warning: could not remove {}: {}", self.path.display(), e);
            }
        }
    }
}

/// Returns whether two paths name the same file.
///
/// By resolved path rather than by name, so that `./out.7z` and `out.7z` are
/// recognised as one file, and a link to the archive as the archive.
fn is_same_file(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    resolve(a) == resolve(b)
}

/// Gathers what goes into the archive, before anything is written.
///
/// The destination is skipped: `zesven create out.7z -r .` names the archive
/// among the files to archive, and reading a file while writing it is a pump
/// that stops when the disk is full. It is compared by resolved path rather
/// than by name, so `./out.7z` and `out.7z` are the same file.
fn collect_files(
    config: &CreateConfig<'_>,
    destination: &Path,
) -> Result<Vec<(PathBuf, String)>, ExitCode> {
    // Build exclude selector
    let exclude_selector = match FileSelector::new(&[], config.exclude) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return Err(ExitCode::BadArgs);
        }
    };

    // Collect all files to add
    let mut all_files: Vec<(std::path::PathBuf, String)> = Vec::new();

    // Nothing the caller named is skipped with a warning. An archive missing a
    // file the caller asked for, reported as a success, is worse than no
    // archive at all: whatever was going to happen to the originals next -
    // deletion, a move, a report - happens on the strength of that exit code.
    for path in config.files {
        if path.is_dir() {
            if config.recursive {
                for entry in WalkDir::new(path).follow_links(false) {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            return Err(ExitCode::IoError);
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

                    if is_same_file(entry.path(), destination) {
                        continue;
                    }

                    all_files.push((entry.path().to_path_buf(), rel_path));
                }
            } else {
                eprintln!(
                    "Error: {} is a directory, use -r for recursive",
                    path.display()
                );
                return Err(ExitCode::BadArgs);
            }
        } else if path.is_file() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if exclude_selector.matches(&name) && !is_same_file(path, destination) {
                all_files.push((path.clone(), name));
            }
        } else {
            eprintln!("Error: {} does not exist", path.display());
            return Err(ExitCode::BadArgs);
        }
    }

    if all_files.is_empty() {
        eprintln!("Error: No files to add to archive");
        return Err(ExitCode::BadArgs);
    }

    // Sorted by the name each file will have in the archive. A directory walk
    // returns entries in whatever order the filesystem hands them over, which
    // differs between filesystems and between two runs on the same one - so
    // without this the archive's entry order came from the disk, and
    // `--deterministic`, which requires sorted order, rejected the second file
    // it was given as often as not.
    all_files.sort_by(|(_, a), (_, b)| a.cmp(b));

    Ok(all_files)
}

fn create_inner(
    config: &CreateConfig<'_>,
    scratch_file: std::fs::File,
    all_files: &[(PathBuf, String)],
) -> ExitCode {
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

    options = options.threads(match config.thread_count {
        0 => Threads::Auto,
        n => Threads::count_or_single(n),
    });

    if let Some(bytes) = config.memory_limit {
        options = options.memory_limit(MemoryLimit::bytes_or_auto(bytes));
    }

    if config.solid {
        options = options.solid();
    }

    #[cfg(feature = "aes")]
    if let Some(ref p) = pwd {
        options = options
            .password(p.as_str())
            .encrypt_header(config.encrypt_headers);
    }

    // Only now is anything written, and even then beside the destination
    // rather than onto it: everything above can fail, and nothing above should
    // cost the caller the file they pointed at.
    //
    // The bar is handed to the writer rather than driven from this loop.
    // Adding an entry mostly means putting it in a batch, so a bar advanced
    // here would fill up in milliseconds and then sit at the end for the whole
    // of the compression - which is the shape that had a reporter timing his
    // own calls and finding one of ten taking ninety-four seconds.
    let progress = WriteProgress::new(all_files.len() as u64, config.quiet);
    let watcher = progress.clone();

    let mut writer = match Writer::create(std::io::BufWriter::new(scratch_file)) {
        Ok(w) => w.options(options).progress(watcher),
        Err(e) => {
            eprintln!("Error creating archive: {}", e);
            return error_to_exit_code(&e);
        }
    };

    // Add files
    // A file that cannot be added ends the run. Carrying on would leave an
    // archive missing entries the caller asked for, and reporting success for
    // it: a backup script that checks the exit code would be told its files
    // were archived when they were not.
    for (disk_path, archive_name) in all_files {
        let archive_path = match ArchivePath::new(archive_name) {
            Ok(p) => p,
            Err(e) => {
                progress.finish_with_message("Failed");
                eprintln!("Error: Invalid path {}: {}", archive_name, e);
                return error_to_exit_code(&e);
            }
        };

        if let Err(e) = writer.add_path(disk_path, archive_path) {
            progress.finish_with_message("Failed");
            eprintln!("Error: Failed to add {}: {}", disk_path.display(), e);
            return error_to_exit_code(&e);
        }
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
) -> Result<Archive<zesven::read::ArchiveSource>, ExitCode> {
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
