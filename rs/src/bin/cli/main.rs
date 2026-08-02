//! CLI tool for zesven archive operations.

mod commands;
mod exit_codes;
mod file_selector;
mod output;
mod password;
mod progress;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use std::path::PathBuf;

use exit_codes::ExitCode;

/// Pure Rust 7z archive tool
#[derive(Parser)]
#[command(name = "zesven")]
#[command(author, version, about = "Pure Rust 7z archive tool", long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format
    #[arg(long, short = 'f', value_enum, default_value = "human", global = true)]
    format: OutputFormat,

    /// Suppress progress output
    #[arg(long, short = 'q', global = true)]
    quiet: bool,

    /// Number of threads (0 = auto)
    #[arg(long, short = 't', default_value = "0", global = true)]
    threads: usize,

    /// Memory concurrent compression may reserve, e.g. 512M or 2G (default:
    /// what this machine's cores can use, up to half of what is free)
    #[arg(long, short = 'M', value_parser = parse_size, global = true)]
    memory_limit: Option<u64>,
}

/// Parses a byte count written as a plain number or with a K, M or G suffix.
///
/// Sizes here are megabytes and up, and asking for them in bytes is how a
/// caller means to write 512M and reserves half a kilobyte instead.
fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (digits, scale) = match text.as_bytes().last() {
        Some(b'k' | b'K') => (&text[..text.len() - 1], 1024),
        Some(b'm' | b'M') => (&text[..text.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        _ => (text, 1),
    };

    digits
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("'{text}' is not a size; write a number, or one like 512M or 2G"))?
        .checked_mul(scale)
        .ok_or_else(|| format!("'{text}' is larger than this machine can address"))
}

#[derive(Subcommand)]
enum Commands {
    /// Extract files from archive (alias: x)
    #[command(alias = "x")]
    Extract {
        /// Archive file to extract
        archive: PathBuf,

        /// Output directory
        #[arg(short = 'o', long, default_value = ".")]
        output: PathBuf,

        /// File patterns to extract (glob patterns supported)
        #[arg(short = 'i', long)]
        include: Vec<String>,

        /// File patterns to exclude
        #[arg(short = 'e', long)]
        exclude: Vec<String>,

        /// Overwrite mode
        #[arg(long, value_enum, default_value = "prompt")]
        overwrite: OverwriteMode,

        /// Password (will prompt if needed and not provided)
        #[arg(short = 'p', long)]
        password: Option<String>,

        /// Preserve file permissions and timestamps
        #[arg(long)]
        preserve_metadata: bool,
    },

    /// Create archive (alias: a)
    #[command(alias = "a")]
    Create {
        /// Archive file to create
        archive: PathBuf,

        /// Files and directories to add (glob patterns supported)
        files: Vec<PathBuf>,

        /// Compression method
        #[arg(short = 'm', long, value_enum, default_value = "lzma2")]
        method: CompressionMethod,

        /// Compression level (0-9)
        #[arg(short = 'l', long, default_value = "5")]
        level: u8,

        /// Create solid archive
        #[arg(long)]
        solid: bool,

        /// Encrypt archive with password
        #[arg(short = 'p', long)]
        password: Option<String>,

        /// Encrypt file headers
        #[arg(long)]
        encrypt_headers: bool,

        /// Enable deterministic output
        #[arg(long)]
        deterministic: bool,

        /// Exclude patterns
        #[arg(short = 'x', long)]
        exclude: Vec<String>,

        /// Recurse into directories (the default)
        ///
        /// Accepted so that `-r` keeps working; it asks for what already
        /// happens. Use `--no-recursive` to turn it off.
        #[arg(short = 'r', long)]
        recursive: bool,

        /// Do not recurse into directories
        ///
        /// A directory named on the command line is then an error rather than
        /// something quietly left out of the archive.
        #[arg(long, conflicts_with = "recursive")]
        no_recursive: bool,
    },

    /// List archive contents (alias: l)
    #[command(alias = "l")]
    List {
        /// Archive file to list
        archive: PathBuf,

        /// Show technical details
        #[arg(long)]
        technical: bool,

        /// Password (will prompt if needed)
        #[arg(short = 'p', long)]
        password: Option<String>,
    },

    /// Test archive integrity (alias: t)
    #[command(alias = "t")]
    Test {
        /// Archive file to test
        archive: PathBuf,

        /// Password (will prompt if needed)
        #[arg(short = 'p', long)]
        password: Option<String>,

        /// Include patterns
        #[arg(short = 'i', long)]
        include: Vec<String>,
    },

    /// Show archive information (alias: i)
    #[command(alias = "i")]
    Info {
        /// Archive file to inspect
        archive: PathBuf,

        /// Password (will prompt if needed)
        #[arg(short = 'p', long)]
        password: Option<String>,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum OverwriteMode {
    Always,
    Never,
    Prompt,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum CompressionMethod {
    Copy,
    Lzma,
    Lzma2,
    Deflate,
    Bzip2,
}

impl From<CompressionMethod> for zesven::codec::CodecMethod {
    fn from(method: CompressionMethod) -> Self {
        match method {
            CompressionMethod::Copy => zesven::codec::CodecMethod::Copy,
            CompressionMethod::Lzma => zesven::codec::CodecMethod::Lzma,
            CompressionMethod::Lzma2 => zesven::codec::CodecMethod::Lzma2,
            CompressionMethod::Deflate => zesven::codec::CodecMethod::Deflate,
            CompressionMethod::Bzip2 => zesven::codec::CodecMethod::BZip2,
        }
    }
}

fn main() {
    // Set up Ctrl+C handler
    ctrlc::set_handler(move || {
        eprintln!("\nInterrupted");
        std::process::exit(exit_codes::USER_INTERRUPT);
    })
    .ok();

    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Extract {
            archive,
            output,
            include,
            exclude,
            overwrite,
            password,
            preserve_metadata,
        } => commands::extract(&commands::ExtractConfig {
            archive_path: &archive,
            output_dir: &output,
            include: &include,
            exclude: &exclude,
            overwrite,
            password,
            preserve_metadata,
            format: cli.format,
            quiet: cli.quiet,
            thread_count: cli.threads,
        }),

        Commands::Create {
            archive,
            files,
            method,
            level,
            solid,
            password,
            encrypt_headers,
            deterministic,
            exclude,
            recursive: _,
            no_recursive,
        } => commands::create(&commands::CreateConfig {
            archive_path: &archive,
            files: &files,
            method,
            level,
            solid,
            password,
            encrypt_headers,
            deterministic,
            exclude: &exclude,
            recursive: !no_recursive,
            format: cli.format,
            quiet: cli.quiet,
            thread_count: cli.threads,
            memory_limit: cli.memory_limit,
        }),

        Commands::List {
            archive,
            technical,
            password,
        } => commands::list(&archive, technical, password, cli.format, cli.quiet),

        Commands::Test {
            archive,
            password,
            include,
        } => commands::test(
            &archive,
            password,
            &include,
            cli.format,
            cli.quiet,
            cli.threads,
        ),

        Commands::Info { archive, password } => {
            commands::info(&archive, password, cli.format, cli.quiet)
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, name, &mut std::io::stdout());
            ExitCode::Success
        }
    };

    std::process::exit(exit_code.code());
}
