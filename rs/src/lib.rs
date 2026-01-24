//! # zesven
//!
//! A pure-Rust library for reading and writing 7z archives.
//!
//! This crate provides a safe, efficient, and fully-featured implementation of
//! the 7z archive format with support for multiple compression methods, AES-256
//! encryption, solid archives, and streaming decompression.
//!
//! ## Quick Start
//!
//! ### Extracting an Archive
//!
//! ```rust,no_run
//! use zesven::{Archive, ExtractOptions, Result};
//! use std::io::Cursor;
//!
//! fn main() -> Result<()> {
//!     // Open from a file path
//!     let mut archive = Archive::open_path("archive.7z")?;
//!
//!     // List entries
//!     for entry in archive.entries() {
//!         println!("{}: {} bytes", entry.path.as_str(), entry.size);
//!     }
//!
//!     // Extract all entries to a directory
//!     archive.extract("./output", (), &ExtractOptions::default())?;
//!     Ok(())
//! }
//! ```
//!
//! ### Creating an Archive
//!
//! ```rust,no_run
//! use zesven::{Writer, WriteOptions, ArchivePath, Result};
//!
//! fn main() -> Result<()> {
//!     // Create a new archive
//!     let mut writer = Writer::create_path("new.7z")?;
//!
//!     // Add files from disk
//!     writer.add_path("file.txt", ArchivePath::new("file.txt")?)?;
//!
//!     // Add data from memory
//!     writer.add_bytes(ArchivePath::new("hello.txt")?, b"Hello, World!")?;
//!
//!     // Finish and get statistics
//!     let result = writer.finish()?;
//!     println!("Wrote {} entries ({:.1}% compression)",
//!         result.entries_written,
//!         result.space_savings() * 100.0);
//!     Ok(())
//! }
//! ```
//!
//! ### Extracting Password-Protected Archives
//!
//! ```rust,ignore
//! # #[cfg(feature = "aes")]
//! use zesven::{Archive, ExtractOptions, Password, Result};
//!
//! # #[cfg(feature = "aes")]
//! fn main() -> Result<()> {
//!     let mut archive = Archive::open_path_with_password(
//!         "encrypted.7z",
//!         Password::new("secret"),
//!     )?;
//!     archive.extract("./output", (), &ExtractOptions::default())?;
//!     Ok(())
//! }
//! # #[cfg(not(feature = "aes"))]
//! # fn main() {}
//! ```
//!
//! ### Creating an Encrypted Archive
//!
//! ```rust,ignore
//! # #[cfg(feature = "aes")]
//! use zesven::{Writer, WriteOptions, ArchivePath, Password, Result};
//!
//! # #[cfg(feature = "aes")]
//! fn main() -> Result<()> {
//!     let options = WriteOptions::new()
//!         .password(Password::new("secret"))
//!         .level(7)?;
//!
//!     let mut writer = Writer::create_path("encrypted.7z")?
//!         .options(options);
//!
//!     writer.add_bytes(ArchivePath::new("secret.txt")?, b"Secret data")?;
//!     writer.finish()?;
//!     Ok(())
//! }
//! # #[cfg(not(feature = "aes"))]
//! # fn main() {}
//! ```
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `lzma` | Yes | LZMA compression support |
//! | `lzma2` | Yes | LZMA2 compression support (includes `lzma`) |
//! | `deflate` | Yes | Deflate/zlib compression |
//! | `bzip2` | Yes | BZip2 compression |
//! | `ppmd` | Yes | PPMd compression |
//! | `aes` | Yes | AES-256 encryption for data and headers |
//! | `parallel` | Yes | Multi-threaded compression with Rayon |
//! | `lz4` | No | LZ4 compression support |
//! | `zstd` | No | Zstandard compression support |
//! | `brotli` | No | Brotli compression support |
//! | `fast-lzma2` | No | Fast LZMA2 encoder with radix match-finder |
//! | `regex` | No | Regex-based file filtering |
//! | `sysinfo` | No | System info for adaptive memory limits |
//! | `async` | No | Async/await API with Tokio integration |
//! | `wasm` | No | WebAssembly/browser support |
//! | `cli` | No | Command-line interface tool |
//!
//! ### Disabling Default Features
//!
//! To create a minimal build, disable default features:
//!
//! ```toml
//! [dependencies]
//! zesven = { version = "1.0", default-features = false, features = ["lzma2"] }
//! ```
//!
//! ## Async API
//!
//! Enable the `async` feature for Tokio-based async operations:
//!
//! ```rust,ignore
//! # #[cfg(feature = "async")]
//! use zesven::{AsyncArchive, AsyncExtractOptions, Result};
//!
//! # #[cfg(feature = "async")]
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let mut archive = AsyncArchive::open_path("archive.7z").await?;
//!     archive.extract("./output", (), &AsyncExtractOptions::default()).await?;
//!     Ok(())
//! }
//! # #[cfg(not(feature = "async"))]
//! # fn main() {}
//! ```
//!
//! ## Streaming API
//!
//! For memory-efficient processing of large archives, use the streaming API:
//!
//! ```rust,ignore
//! use zesven::{StreamingArchive, StreamingConfig, Result};
//!
//! fn main() -> Result<()> {
//!     let config = StreamingConfig::default()
//!         .max_memory_buffer(64 * 1024 * 1024); // 64 MB limit
//!
//!     // With default features (aes enabled), pass empty string for unencrypted archives
//!     let archive = StreamingArchive::open_path_with_config("large.7z", "", config)?;
//!
//!     for entry in archive.entries()? {
//!         let entry = entry?;
//!         println!("Processing: {}", entry.path().as_str());
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Error Handling
//!
//! All operations return [`Result<T>`], which is an alias for
//! `std::result::Result<T, Error>`. The [`Error`] enum covers all possible
//! failure modes:
//!
//! ```rust,no_run
//! use zesven::{Archive, Error};
//!
//! fn open_archive(path: &str) -> zesven::Result<()> {
//!     match Archive::open_path(path) {
//!         Ok(archive) => {
//!             println!("Opened archive with {} entries", archive.len());
//!             Ok(())
//!         }
//!         Err(Error::Io(e)) => {
//!             eprintln!("I/O error: {}", e);
//!             Err(Error::Io(e))
//!         }
//!         Err(Error::InvalidFormat(msg)) => {
//!             eprintln!("Not a valid 7z file: {}", msg);
//!             Err(Error::InvalidFormat(msg))
//!         }
//!         Err(e @ Error::WrongPassword { .. }) => {
//!             eprintln!("Incorrect password");
//!             Err(e)
//!         }
//!         Err(e) => Err(e),
//!     }
//! }
//! # fn main() {}
//! ```
//!
//! ## Safety and Resource Limits
//!
//! The library includes built-in protections against malicious archives:
//!
//! - **Path traversal protection**: Prevents extraction outside the destination
//! - **Resource limits**: Guards against zip bombs and excessive memory usage
//! - **CRC verification**: Validates data integrity during extraction
//!
//! ```rust,no_run
//! use zesven::{ExtractOptions, read::PathSafety};
//!
//! // Enable strict path validation (default)
//! let options = ExtractOptions::new()
//!     .path_safety(PathSafety::Strict);
//! ```
//!
//! ## Platform Support
//!
//! | Platform | Status |
//! |----------|--------|
//! | Linux (x86_64, aarch64) | Full support |
//! | macOS (x86_64, aarch64) | Full support |
//! | Windows (x86_64) | Full support |
//! | WebAssembly | Via `wasm` feature |
//!
//! ## Minimum Supported Rust Version (MSRV)
//!
//! This crate requires **Rust 1.85** or later.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]

/// Default buffer size for read operations (8 KiB).
pub(crate) const READ_BUFFER_SIZE: usize = 8192;

pub mod archive_path;
pub mod checksum;
pub mod codec;
pub mod edit;
pub mod error;
pub mod format;
pub mod fs;
pub mod hardlink;
pub mod ntfs;
pub mod ownership;
pub mod recovery;
pub mod sfx;

#[cfg(feature = "aes")]
#[cfg_attr(docsrs, doc(cfg(feature = "aes")))]
pub mod crypto;

pub mod progress;
pub mod read;
pub mod safety;
pub mod stats;
pub mod streaming;
pub mod timestamp;
pub mod volume;
pub mod write;

// Async modules (requires "async" feature)
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod async_codec;

#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod async_options;

#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod async_password;

#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod async_read;

#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod async_write;

pub use archive_path::ArchivePath;
pub use error::{Error, PasswordDetectionMethod, Result};
pub use timestamp::Timestamp;

#[cfg(feature = "aes")]
pub use crypto::Password;

// Re-export reading API at crate root for convenience
pub use read::{Archive, Entry, ExtractOptions, ExtractResult, TestOptions, TestResult};

// Re-export writing API at crate root for convenience
pub use write::{AppendResult, ArchiveAppender, WriteFilter, WriteOptions, WriteResult, Writer};

// Re-export volume API at crate root for convenience
pub use volume::VolumeConfig;

// Re-export safety utilities
pub use format::streams::{LimitMode, RatioLimit, ResourceLimits};
pub use safety::{LimitedReader, validate_extract_path};

// Re-export streaming API
pub use streaming::{
    CompressionMethod, DecoderPool, EntryIterator, ExtractAllResult, MemoryEstimate, MemoryGuard,
    MemoryTracker, PoolStats, StreamingArchive, StreamingConfig, StreamingEntry,
};

// Re-export stats API
pub use stats::{ReadStats, StatsConfig, StatsReader, WithStats};

// Re-export progress API
pub use progress::{
    AtomicProgress, NoProgress, ProgressReporter, ProgressState, StatisticsProgress,
    ThrottledProgress, progress_fn,
};

// Re-export edit API
pub use edit::{ArchiveEditor, EditResult, EditableArchive, Operation, OperationBuilder};

#[allow(unused)]
mod s3fifo;

// Re-export SFX API
pub use sfx::{SfxBuilder, SfxConfig, SfxFormat, SfxInfo, SfxResult, SfxStub, create_sfx};

// Re-export recovery API
pub use recovery::{
    FailedEntry, RecoveredEntry, RecoveryOptions, RecoveryResult, RecoveryStatus, SignatureScanner,
    find_all_signatures, is_valid_archive, recover_archive,
};

// Re-export ownership API
pub use ownership::UnixOwnership;

// Re-export hard link API
pub use hardlink::{HardLinkEntry, HardLinkInfo, HardLinkTracker, create_hard_link};

// Re-export NTFS alternate data streams API
pub use ntfs::{
    ADS_SEPARATOR, AltStream, discover_alt_streams, is_ads_path, make_ads_path, parse_ads_path,
    read_alt_stream, write_alt_stream,
};

// Async API re-exports (requires "async" feature)
#[cfg(feature = "async")]
pub use async_options::{
    AsyncExtractOptions, AsyncProgressCallback, AsyncTestOptions, ChannelProgressReporter,
    ProgressEvent,
};

#[cfg(feature = "async")]
pub use async_read::AsyncArchive;

#[cfg(feature = "async")]
pub use async_write::AsyncWriter;

#[cfg(all(feature = "async", feature = "aes"))]
pub use async_password::{
    AsyncPassword, AsyncPasswordProvider, CallbackPasswordProvider, InteractivePasswordProvider,
};

#[cfg(feature = "async")]
pub use async_codec::{AsyncDecoder, AsyncEncoder, build_async_decoder, build_async_encoder};

// Re-export CancellationToken for convenience
#[cfg(feature = "async")]
pub use tokio_util::sync::CancellationToken;
// WASM/Browser support (requires "wasm" feature)

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
#[cfg_attr(docsrs, doc(cfg(feature = "wasm")))]
pub mod wasm;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub use wasm::{
    WasmArchive, WasmMemoryConfig, WasmWriteOptions, WasmWriter, extract_as_stream,
    extract_entry_async, open_archive_async, open_from_stream,
};
