//! Multi-volume archive support.
//!
//! This module provides support for reading and writing multi-volume 7z archives,
//! which are archives split across multiple files (e.g., `.7z.001`, `.7z.002`, etc.).
//!
//! # Overview
//!
//! Multi-volume archives are useful for:
//! - Storing large archives on media with size limits (USB drives, DVDs)
//! - Splitting archives for easier transfer or upload
//! - Working around file system limitations
//!
//! # Reading Multi-Volume Archives
//!
//! ```rust,ignore
//! use zesven::volume::MultiVolumeReader;
//! use zesven::Archive;
//!
//! // Open the first volume (other volumes are discovered automatically)
//! let reader = MultiVolumeReader::open("archive.7z.001")?;
//! println!("Archive spans {} volumes", reader.volume_count());
//!
//! // Use with Archive just like a regular file
//! let archive = Archive::open(reader)?;
//! for entry in archive.entries() {
//!     println!("{}", entry.path.as_str());
//! }
//! ```
//!
//! # Writing Multi-Volume Archives
//!
//! ```rust,ignore
//! use zesven::volume::{VolumeConfig, MultiVolumeWriter};
//! use zesven::Writer;
//!
//! // Configure volume size (e.g., 100 MB per volume)
//! let config = VolumeConfig::new("archive.7z", 100 * 1024 * 1024);
//! let writer = MultiVolumeWriter::create(config)?;
//!
//! // Use with Writer just like a regular file
//! let mut archive = Writer::new(writer);
//! // ... add files ...
//! let result = archive.finish()?;
//! ```
//!
//! # Volume Naming Convention
//!
//! Multi-volume archives use the following naming convention:
//! - `archive.7z.001` - First volume
//! - `archive.7z.002` - Second volume
//! - `archive.7z.003` - Third volume
//! - etc.
//!
//! The volume number is always 3 digits, padded with zeros.

mod config;
mod reader;
mod unified;
mod writer;

pub use config::VolumeConfig;
pub use reader::{MultiVolumeReader, VolumeReader};
pub use unified::UnifiedReader;
pub use writer::MultiVolumeWriter;
