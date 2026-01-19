//! Async archive writing API for 7z archives.
//!
//! This module provides the async API for creating 7z archives, including
//! adding files, directories, and streams with various compression options.
//!
//! # Example
//!
//! ```rust,ignore
//! use zesven::async_write::{AsyncWriter, WriteOptions};
//!
//! #[tokio::main]
//! async fn main() -> zesven::Result<()> {
//!     // Create an archive
//!     let mut writer = AsyncWriter::create_path("archive.7z").await?;
//!
//!     // Add files
//!     writer.add_bytes("test.txt".try_into()?, b"Hello, World!").await?;
//!
//!     // Finish writing
//!     let result = writer.finish().await?;
//!     println!("Wrote {} entries", result.entries_written);
//!     Ok(())
//! }
//! ```

use std::io::SeekFrom;
use std::path::Path;

use tokio::fs::File;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufWriter,
};

use crate::codec::CodecMethod;
use crate::format::{SIGNATURE, SIGNATURE_HEADER_SIZE, property_id};
use crate::write::{EntryMeta, WriteOptions, WriteResult};
use crate::{ArchivePath, Error, Result};

/// State of the async writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AsyncWriterState {
    /// Accepting new entries.
    AcceptingEntries,
    /// Building the archive (flushing, writing headers).
    Building,
    /// Archive is finished.
    Finished,
}

/// Entry data stored for header writing.
#[derive(Debug)]
struct PendingEntry {
    /// Archive path.
    path: ArchivePath,
    /// Entry metadata.
    meta: EntryMeta,
    /// CRC32 of uncompressed data.
    crc: u32,
    /// Uncompressed size.
    uncompressed_size: u64,
}

/// Stream info for pack/unpack info.
#[derive(Debug, Default)]
struct StreamInfo {
    /// Packed sizes for each stream.
    pack_sizes: Vec<u64>,
    /// Unpacked sizes for each stream.
    unpack_sizes: Vec<u64>,
    /// CRCs for each stream.
    crcs: Vec<u32>,
}

/// An async 7z archive writer.
///
/// This provides the same functionality as the sync `Writer` but with
/// async/await support for non-blocking I/O operations.
pub struct AsyncWriter<W> {
    sink: W,
    options: WriteOptions,
    state: AsyncWriterState,
    entries: Vec<PendingEntry>,
    stream_info: StreamInfo,
    /// Total compressed bytes written.
    compressed_bytes: u64,
}

impl AsyncWriter<BufWriter<File>> {
    /// Creates a new archive file at the given path asynchronously.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file to create
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut writer = AsyncWriter::create_path("archive.7z").await?;
    /// ```
    pub async fn create_path(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::create(path.as_ref()).await.map_err(Error::Io)?;
        let writer = BufWriter::new(file);
        Self::create(writer).await
    }
}

impl<W: AsyncWrite + AsyncSeek + Unpin + Send> AsyncWriter<W> {
    /// Creates a new archive writer asynchronously.
    ///
    /// # Arguments
    ///
    /// * `sink` - The async writer to output archive data to
    ///
    /// # Errors
    ///
    /// Returns an error if the initial seek fails.
    pub async fn create(mut sink: W) -> Result<Self> {
        // Reserve space for signature header (32 bytes)
        sink.seek(SeekFrom::Start(SIGNATURE_HEADER_SIZE))
            .await
            .map_err(Error::Io)?;

        Ok(Self {
            sink,
            options: WriteOptions::default(),
            state: AsyncWriterState::AcceptingEntries,
            entries: Vec::new(),
            stream_info: StreamInfo::default(),
            compressed_bytes: 0,
        })
    }

    /// Sets the write options.
    pub fn options(mut self, options: WriteOptions) -> Self {
        self.options = options;
        self
    }

    /// Adds a file from a filesystem path asynchronously.
    ///
    /// # Arguments
    ///
    /// * `disk_path` - Path to the file on disk
    /// * `archive_path` - Path within the archive
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the writer is in an invalid state.
    pub async fn add_path(
        &mut self,
        disk_path: impl AsRef<Path>,
        archive_path: ArchivePath,
    ) -> Result<()> {
        self.ensure_accepting_entries()?;

        let disk_path = disk_path.as_ref();
        let meta = EntryMeta::from_path_async(disk_path).await?;

        if meta.is_directory {
            self.add_directory(archive_path, meta).await
        } else {
            let mut file = File::open(disk_path).await.map_err(Error::Io)?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).await.map_err(Error::Io)?;
            self.add_bytes_internal(archive_path, &data, meta).await
        }
    }

    /// Adds a directory entry asynchronously.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `meta` - Entry metadata
    ///
    /// # Errors
    ///
    /// Returns an error if the writer is in an invalid state.
    pub async fn add_directory(
        &mut self,
        archive_path: ArchivePath,
        meta: EntryMeta,
    ) -> Result<()> {
        self.ensure_accepting_entries()?;

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta {
                is_directory: true,
                ..meta
            },
            crc: 0,
            uncompressed_size: 0,
        };

        self.entries.push(entry);
        Ok(())
    }

    /// Adds data from an async stream.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `source` - Async reader providing the data
    /// * `meta` - Entry metadata
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails or if the writer is in an invalid state.
    pub async fn add_stream<R: AsyncRead + Unpin>(
        &mut self,
        archive_path: ArchivePath,
        mut source: R,
        meta: EntryMeta,
    ) -> Result<()> {
        self.ensure_accepting_entries()?;

        // Read all data
        let mut data = Vec::new();
        source.read_to_end(&mut data).await.map_err(Error::Io)?;

        self.add_bytes_internal(archive_path, &data, meta).await
    }

    /// Adds data from a byte slice asynchronously.
    ///
    /// # Arguments
    ///
    /// * `archive_path` - Path within the archive
    /// * `data` - The data to add
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails or if the writer is in an invalid state.
    pub async fn add_bytes(&mut self, archive_path: ArchivePath, data: &[u8]) -> Result<()> {
        let meta = EntryMeta::file(data.len() as u64);
        self.add_bytes_internal(archive_path, data, meta).await
    }

    /// Internal method to add bytes with metadata.
    async fn add_bytes_internal(
        &mut self,
        archive_path: ArchivePath,
        data: &[u8],
        meta: EntryMeta,
    ) -> Result<()> {
        let crc = crc32fast::hash(data);
        let uncompressed_size = data.len() as u64;

        // Compress data using spawn_blocking for CPU-bound work
        let method = self.options.method;
        let level = self.options.level;
        let data_owned = data.to_vec();

        let compressed =
            tokio::task::spawn_blocking(move || compress_data_sync(&data_owned, method, level))
                .await
                .map_err(|e| Error::Io(std::io::Error::other(e)))??;

        let compressed_size = compressed.len() as u64;

        // Write compressed data asynchronously
        self.sink.write_all(&compressed).await.map_err(Error::Io)?;
        self.compressed_bytes += compressed_size;

        // Track stream info
        self.stream_info.pack_sizes.push(compressed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.crcs.push(crc);

        // Add entry
        let entry = PendingEntry {
            path: archive_path,
            meta,
            crc,
            uncompressed_size,
        };
        self.entries.push(entry);

        Ok(())
    }

    /// Finishes writing the archive asynchronously.
    ///
    /// # Returns
    ///
    /// A WriteResult with statistics about the written archive.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing fails.
    pub async fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner().await?;
        Ok(result)
    }

    /// Finishes writing the archive and returns the underlying sink.
    ///
    /// This is useful when you need access to the written data, such as
    /// when writing to a `Cursor<Vec<u8>>` and need to retrieve the buffer.
    ///
    /// # Returns
    ///
    /// A tuple of (WriteResult, W) where W is the underlying sink.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::async_write::AsyncWriter;
    /// use std::io::Cursor;
    ///
    /// let mut writer = AsyncWriter::create(Cursor::new(Vec::new())).await?;
    /// writer.add_bytes("test.txt".try_into()?, b"Hello").await?;
    /// let (result, cursor) = writer.finish_into_inner().await?;
    /// let archive_bytes = cursor.into_inner();
    /// ```
    pub async fn finish_into_inner(mut self) -> Result<(WriteResult, W)> {
        self.ensure_accepting_entries()?;
        self.state = AsyncWriterState::Building;

        // Sort entries if deterministic mode
        if self.options.deterministic {
            self.entries
                .sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        }

        // Record header position
        let header_pos = self.sink.stream_position().await.map_err(Error::Io)?;

        // Encode header using sync code in spawn_blocking
        let method = self.options.method;
        let level = self.options.level;
        let entries_data: Vec<_> = self
            .entries
            .iter()
            .map(|e| {
                (
                    e.path.as_str().to_string(),
                    e.meta.clone(),
                    e.crc,
                    e.uncompressed_size,
                )
            })
            .collect();
        let stream_info_data = (
            self.stream_info.pack_sizes.clone(),
            self.stream_info.unpack_sizes.clone(),
            self.stream_info.crcs.clone(),
        );

        let header_data = tokio::task::spawn_blocking(move || {
            encode_header_sync(&entries_data, &stream_info_data, method, level)
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))??;

        self.sink.write_all(&header_data).await.map_err(Error::Io)?;

        // Write signature header at start
        self.write_signature_header_async(header_pos, &header_data)
            .await?;

        self.state = AsyncWriterState::Finished;

        // Build result
        let result = WriteResult {
            entries_written: self.entries.iter().filter(|e| !e.meta.is_directory).count(),
            directories_written: self.entries.iter().filter(|e| e.meta.is_directory).count(),
            total_size: self.entries.iter().map(|e| e.uncompressed_size).sum(),
            compressed_size: self.compressed_bytes,
            volume_count: 1,
            volume_sizes: vec![],
        };

        Ok((result, self.sink))
    }

    /// Writes the signature header at the start of the file asynchronously.
    async fn write_signature_header_async(
        &mut self,
        header_pos: u64,
        header_data: &[u8],
    ) -> Result<()> {
        let next_header_offset = header_pos - SIGNATURE_HEADER_SIZE;
        let next_header_size = header_data.len() as u64;
        let next_header_crc = crc32fast::hash(header_data);

        // Build start header (20 bytes)
        let mut start_header = Vec::with_capacity(20);
        start_header.extend_from_slice(&next_header_offset.to_le_bytes());
        start_header.extend_from_slice(&next_header_size.to_le_bytes());
        start_header.extend_from_slice(&next_header_crc.to_le_bytes());

        let start_header_crc = crc32fast::hash(&start_header);

        // Seek to start and write signature header
        self.sink
            .seek(SeekFrom::Start(0))
            .await
            .map_err(Error::Io)?;

        // Write signature (6 bytes)
        self.sink.write_all(SIGNATURE).await.map_err(Error::Io)?;

        // Write version (2 bytes)
        self.sink
            .write_all(&[0x00, 0x04])
            .await
            .map_err(Error::Io)?;

        // Write start header CRC (4 bytes)
        self.sink
            .write_all(&start_header_crc.to_le_bytes())
            .await
            .map_err(Error::Io)?;

        // Write start header (20 bytes)
        self.sink
            .write_all(&start_header)
            .await
            .map_err(Error::Io)?;

        Ok(())
    }

    /// Ensures the writer is in the AcceptingEntries state.
    fn ensure_accepting_entries(&self) -> Result<()> {
        if self.state != AsyncWriterState::AcceptingEntries {
            return Err(Error::InvalidFormat(
                "Writer is not accepting entries".into(),
            ));
        }
        Ok(())
    }
}

// ============================================================================
// Helper Extensions
// ============================================================================

impl EntryMeta {
    /// Creates metadata from a filesystem path asynchronously.
    pub async fn from_path_async(path: impl AsRef<Path>) -> Result<Self> {
        let metadata = tokio::fs::metadata(path).await.map_err(Error::Io)?;
        Ok(Self::from_metadata(&metadata))
    }
}

// ============================================================================
// Sync Helper Functions (called via spawn_blocking)
// ============================================================================

/// Compresses data synchronously (called in spawn_blocking).
fn compress_data_sync(data: &[u8], method: CodecMethod, level: u32) -> Result<Vec<u8>> {
    match method {
        CodecMethod::Copy => Ok(data.to_vec()),
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => compress_lzma2_sync(data, level),
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => compress_lzma_sync(data, level),
        #[cfg(feature = "deflate")]
        CodecMethod::Deflate => compress_deflate_sync(data, level),
        #[cfg(feature = "bzip2")]
        CodecMethod::BZip2 => compress_bzip2_sync(data, level),
        _ => Err(Error::UnsupportedMethod {
            method_id: method.method_id(),
        }),
    }
}

#[cfg(feature = "lzma2")]
fn compress_lzma2_sync(data: &[u8], level: u32) -> Result<Vec<u8>> {
    use crate::codec::lzma::{Lzma2Encoder, Lzma2EncoderOptions};

    let opts = Lzma2EncoderOptions {
        dict_size: Some(1 << (16 + level.min(7))),
        ..Default::default()
    };
    let mut output = Vec::new();
    {
        let mut encoder = Lzma2Encoder::new(&mut output, &opts);
        std::io::Write::write_all(&mut encoder, data).map_err(Error::Io)?;
        encoder.try_finish().map_err(Error::Io)?;
    }
    Ok(output)
}

#[cfg(feature = "lzma")]
fn compress_lzma_sync(data: &[u8], level: u32) -> Result<Vec<u8>> {
    use crate::codec::lzma::{LzmaEncoder, LzmaEncoderOptions};

    let opts = LzmaEncoderOptions {
        dict_size: Some(1 << (16 + level.min(7))),
        ..Default::default()
    };
    let mut output = Vec::new();
    {
        let mut encoder = LzmaEncoder::new(&mut output, &opts)?;
        std::io::Write::write_all(&mut encoder, data).map_err(Error::Io)?;
        encoder.try_finish().map_err(Error::Io)?;
    }
    Ok(output)
}

#[cfg(feature = "deflate")]
fn compress_deflate_sync(data: &[u8], level: u32) -> Result<Vec<u8>> {
    use crate::codec::deflate::{DeflateEncoder, DeflateEncoderOptions};

    let opts = DeflateEncoderOptions { level };
    let mut output = Vec::new();
    {
        let mut encoder = DeflateEncoder::new(&mut output, &opts);
        std::io::Write::write_all(&mut encoder, data).map_err(Error::Io)?;
        let _ = encoder.try_finish().map_err(Error::Io)?;
    }
    Ok(output)
}

#[cfg(feature = "bzip2")]
fn compress_bzip2_sync(data: &[u8], level: u32) -> Result<Vec<u8>> {
    use crate::codec::bzip2::{Bzip2Encoder, Bzip2EncoderOptions};

    let opts = Bzip2EncoderOptions { level };
    let mut output = Vec::new();
    {
        let mut encoder = Bzip2Encoder::new(&mut output, &opts);
        std::io::Write::write_all(&mut encoder, data).map_err(Error::Io)?;
        let _ = encoder.try_finish().map_err(Error::Io)?;
    }
    Ok(output)
}

/// Encodes the archive header synchronously (called in spawn_blocking).
fn encode_header_sync(
    entries: &[(String, EntryMeta, u32, u64)],
    stream_info: &(Vec<u64>, Vec<u64>, Vec<u32>),
    method: CodecMethod,
    level: u32,
) -> Result<Vec<u8>> {
    use crate::format::reader::write_variable_u64;

    let (pack_sizes, unpack_sizes, crcs) = stream_info;

    let mut header = Vec::new();

    // Header marker
    header.push(property_id::HEADER);

    // MainStreamsInfo (if we have data)
    if !pack_sizes.is_empty() {
        header.push(property_id::MAIN_STREAMS_INFO);

        // PackInfo
        header.push(property_id::PACK_INFO);
        write_variable_u64(&mut header, 0)?;
        write_variable_u64(&mut header, pack_sizes.len() as u64)?;

        // Pack sizes
        header.push(property_id::SIZE);
        for &size in pack_sizes {
            write_variable_u64(&mut header, size)?;
        }
        header.push(property_id::END);

        // UnpackInfo
        header.push(property_id::UNPACK_INFO);
        header.push(property_id::FOLDER);
        write_variable_u64(&mut header, unpack_sizes.len() as u64)?;

        // External = 0 (coders inline)
        header.push(0);

        // For each folder (one per file in non-solid mode)
        for _i in 0..unpack_sizes.len() {
            // Number of coders = 1
            header.push(0x01);

            // Coder: method ID
            let method_id = method.method_id();
            let method_bytes = encode_method_id(method_id);

            // Coder flags and ID size
            let id_size = method_bytes.len() as u8;
            let has_props = method_has_properties(method);
            let flags = id_size | if has_props { 0x20 } else { 0 };
            header.push(flags);
            header.extend_from_slice(&method_bytes);

            // Properties if needed
            if has_props {
                let props = encode_method_properties(method, level);
                write_variable_u64(&mut header, props.len() as u64)?;
                header.extend_from_slice(&props);
            }
        }

        // Unpack sizes
        header.push(property_id::CODERS_UNPACK_SIZE);
        for &size in unpack_sizes {
            write_variable_u64(&mut header, size)?;
        }

        // CRCs for folders
        header.push(property_id::CRC);
        header.push(1); // all defined
        for &crc in crcs {
            header.extend_from_slice(&crc.to_le_bytes());
        }

        header.push(property_id::END); // End UnpackInfo
        header.push(property_id::END); // End MainStreamsInfo
    }

    // FilesInfo
    if !entries.is_empty() {
        header.push(property_id::FILES_INFO);
        write_variable_u64(&mut header, entries.len() as u64)?;

        // EmptyStream (directories and empty files)
        let empty_entries: Vec<_> = entries
            .iter()
            .map(|(_, meta, _, size)| meta.is_directory || *size == 0)
            .collect();

        if empty_entries.iter().any(|&x| x) {
            header.push(property_id::EMPTY_STREAM);
            let bool_vec = encode_bool_vector(&empty_entries);
            write_variable_u64(&mut header, bool_vec.len() as u64)?;
            header.extend_from_slice(&bool_vec);

            // EmptyFile (empty files that are not directories)
            let empty_files: Vec<_> = entries
                .iter()
                .filter(|(_, meta, _, size)| meta.is_directory || *size == 0)
                .map(|(_, meta, _, _)| !meta.is_directory)
                .collect();

            if empty_files.iter().any(|&x| x) {
                header.push(property_id::EMPTY_FILE);
                let bool_vec = encode_bool_vector(&empty_files);
                write_variable_u64(&mut header, bool_vec.len() as u64)?;
                header.extend_from_slice(&bool_vec);
            }
        }

        // Names
        header.push(property_id::NAME);
        let names_data = encode_names(entries);
        write_variable_u64(&mut header, names_data.len() as u64 + 1)?;
        header.push(0); // external = 0
        header.extend_from_slice(&names_data);

        // MTime (if any entries have it)
        let has_mtime: Vec<_> = entries
            .iter()
            .map(|(_, meta, _, _)| meta.modification_time.is_some())
            .collect();
        if has_mtime.iter().any(|&x| x) {
            header.push(property_id::MTIME);
            let mtime_data = encode_times(entries, &has_mtime);
            write_variable_u64(&mut header, mtime_data.len() as u64)?;
            header.extend_from_slice(&mtime_data);
        }

        header.push(property_id::END); // End FilesInfo
    }

    header.push(property_id::END); // End Header

    Ok(header)
}

/// Encodes a method ID to bytes.
fn encode_method_id(id: u64) -> Vec<u8> {
    if id == 0 {
        return vec![0];
    }

    let mut bytes = Vec::new();
    let mut val = id;
    while val > 0 {
        bytes.push((val & 0xFF) as u8);
        val >>= 8;
    }
    bytes.reverse();
    bytes
}

/// Encodes a boolean vector to bytes.
fn encode_bool_vector(bits: &[bool]) -> Vec<u8> {
    let num_bytes = bits.len().div_ceil(8);
    let mut bytes = vec![0u8; num_bytes];

    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            bytes[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    bytes
}

/// Returns whether the method has properties to encode.
fn method_has_properties(method: CodecMethod) -> bool {
    matches!(method, CodecMethod::Lzma | CodecMethod::Lzma2)
}

/// Encodes method-specific properties.
fn encode_method_properties(method: CodecMethod, level: u32) -> Vec<u8> {
    match method {
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => {
            vec![crate::codec::lzma::encode_lzma2_dict_size(
                1 << (16 + level),
            )]
        }
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => {
            let dict_size: u32 = 1 << (16 + level);
            let mut props = vec![0x5D];
            props.extend_from_slice(&dict_size.to_le_bytes());
            props
        }
        _ => Vec::new(),
    }
}

/// Encodes file names as UTF-16LE.
fn encode_names(entries: &[(String, EntryMeta, u32, u64)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (path, _, _, _) in entries {
        for c in path.encode_utf16() {
            data.extend_from_slice(&c.to_le_bytes());
        }
        // Null terminator
        data.extend_from_slice(&[0, 0]);
    }
    data
}

/// Encodes timestamps.
fn encode_times(entries: &[(String, EntryMeta, u32, u64)], defined: &[bool]) -> Vec<u8> {
    let mut data = Vec::new();

    // AllDefined flag
    let all_defined = defined.iter().all(|&x| x);
    if all_defined {
        data.push(1);
    } else {
        data.push(0);
        data.extend_from_slice(&encode_bool_vector(defined));
    }

    // External = 0
    data.push(0);

    // Times
    for (_, meta, _, _) in entries {
        if let Some(time) = meta.modification_time {
            data.extend_from_slice(&time.to_le_bytes());
        }
    }

    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_async_writer_create() {
        let buffer = std::io::Cursor::new(Vec::new());
        let writer = AsyncWriter::create(buffer).await.unwrap();
        assert_eq!(writer.state, AsyncWriterState::AcceptingEntries);
    }

    #[tokio::test]
    async fn test_async_writer_options() {
        let buffer = std::io::Cursor::new(Vec::new());
        let writer = AsyncWriter::create(buffer)
            .await
            .unwrap()
            .options(WriteOptions::new().level(9).unwrap());
        assert_eq!(writer.options.level, 9);
    }

    #[tokio::test]
    async fn test_async_writer_add_bytes_and_finish() {
        let buffer = std::io::Cursor::new(Vec::new());
        let mut writer = AsyncWriter::create(buffer).await.unwrap();

        let path = ArchivePath::new("test.txt").unwrap();
        writer.add_bytes(path, b"Hello, World!").await.unwrap();

        let result = writer.finish().await.unwrap();
        assert_eq!(result.entries_written, 1);
        assert_eq!(result.total_size, 13);
    }

    #[tokio::test]
    async fn test_async_writer_empty_archive() {
        let buffer = std::io::Cursor::new(Vec::new());
        let writer = AsyncWriter::create(buffer).await.unwrap();
        let result = writer.finish().await.unwrap();
        assert_eq!(result.entries_written, 0);
    }

    #[tokio::test]
    async fn test_async_writer_with_directory() {
        let buffer = std::io::Cursor::new(Vec::new());
        let mut writer = AsyncWriter::create(buffer).await.unwrap();

        let dir_path = ArchivePath::new("mydir").unwrap();
        writer
            .add_directory(dir_path, EntryMeta::directory())
            .await
            .unwrap();

        let result = writer.finish().await.unwrap();
        assert_eq!(result.entries_written, 0);
        assert_eq!(result.directories_written, 1);
    }

    #[test]
    fn test_encode_method_id() {
        assert_eq!(encode_method_id(0), vec![0]);
        assert_eq!(encode_method_id(0x21), vec![0x21]);
        assert_eq!(encode_method_id(0x030101), vec![0x03, 0x01, 0x01]);
    }

    #[test]
    fn test_encode_bool_vector() {
        assert_eq!(encode_bool_vector(&[true, false, true]), vec![0b10100000]);
        assert_eq!(encode_bool_vector(&[true; 8]), vec![0b11111111]);
    }
}
