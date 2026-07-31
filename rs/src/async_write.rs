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

use crate::format::{SIGNATURE, SIGNATURE_HEADER_SIZE};
use crate::write::codecs::Compressed;
use crate::write::header_encode::HeaderModel;
use crate::write::{EntryMeta, PendingEntry, StreamInfo, WriteOptions, WriteResult};
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
    /// A write failed with an entry's bytes already in the sink.
    ///
    /// Whatever reached the sink belongs to no folder, so every folder after it
    /// would be found at the wrong offset. Finishing anyway produces an archive
    /// that opens and fails on extraction, which is worse than refusing.
    Failed,
}

/// An async 7z archive writer.
///
/// Writes the same archives as the blocking [`Writer`], from the same folder
/// model and header encoder, with the I/O awaited instead of blocking and
/// compression handed to the blocking pool.
///
/// It implements a subset of what the blocking writer does, and refuses the
/// rest rather than ignoring it: encryption, pre-compression filters, solid
/// mode and archive comments all return an error. It also buffers each entry
/// whole, so `memory_limit` does not bound what it holds.
///
/// [`Writer`]: crate::write::Writer
pub struct AsyncWriter<W> {
    sink: W,
    /// Where this archive begins in the sink.
    start_pos: u64,
    options: WriteOptions,
    state: AsyncWriterState,
    entries: Vec<PendingEntry>,
    stream_info: StreamInfo,
    /// Total compressed bytes written.
    compressed_bytes: u64,
    /// The path of the entry added last, when order is being enforced.
    last_path: Option<String>,
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
        // Where the archive begins, which is wherever the sink happens to be
        // rather than nought: everything measured or seeked to is relative to
        // this, as in the blocking writer.
        let start_pos = sink.stream_position().await.map_err(Error::Io)?;

        // Reserve space for the signature header (32 bytes)
        sink.seek(SeekFrom::Start(start_pos + SIGNATURE_HEADER_SIZE))
            .await
            .map_err(Error::Io)?;

        Ok(Self {
            sink,
            start_pos,
            options: WriteOptions::default(),
            state: AsyncWriterState::AcceptingEntries,
            entries: Vec::new(),
            stream_info: StreamInfo::default(),
            compressed_bytes: 0,
            last_path: None,
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
        self.checks_before_reading(&archive_path)?;

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
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        let entry = PendingEntry {
            path: archive_path,
            meta: EntryMeta {
                is_directory: true,
                ..meta
            },
            uncompressed_size: 0,
        };

        self.entries.push(entry);
        self.record_order(&recorded);
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
        self.checks_before_reading(&archive_path)?;

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

    /// Checks that entries arrive in sorted order, when that was asked for.
    ///
    /// Sorting them afterwards is not an option: an entry's position in the
    /// file list is what binds it to its stream, and the stream has already
    /// been written by then.
    fn check_order(&self, archive_path: &ArchivePath) -> Result<()> {
        if !self.options.deterministic {
            return Ok(());
        }

        let path = archive_path.as_str();
        if let Some(previous) = &self.last_path {
            if path < previous.as_str() {
                return Err(Error::InvalidArchivePath(format!(
                    "deterministic mode requires entries in sorted order, \
                     but '{path}' was added after '{previous}'"
                )));
            }
        }

        Ok(())
    }

    /// Records a path as added, once the entry is really in.
    ///
    /// Recorded whether or not the setting is on: the options can be replaced
    /// between entries, and a gap in this leaves the check comparing against a
    /// stale name.
    fn record_order(&mut self, archive_path: &ArchivePath) {
        match &mut self.last_path {
            Some(last) => {
                last.clear();
                last.push_str(archive_path.as_str());
            }
            None => self.last_path = Some(archive_path.as_str().to_string()),
        }
    }

    /// Internal method to add bytes with metadata.
    async fn add_bytes_internal(
        &mut self,
        archive_path: ArchivePath,
        data: &[u8],
        meta: EntryMeta,
    ) -> Result<()> {
        // Every way of adding data ends here, which is the only place a check
        // covers all of them: `add_bytes` reached this without one, so the
        // writer's state and its unsupported options went unexamined on the
        // path callers use most.
        self.ensure_accepting_entries()?;
        self.check_order(&archive_path)?;
        let recorded = archive_path.clone();

        let crc = crc32fast::hash(data);
        let uncompressed_size = data.len() as u64;

        // An empty entry carries no stream at all - the header records it as
        // kEmptyStream. Giving it a folder as well made the reader pair every
        // following entry with the stream before it, so the next file came back
        // empty and its own contents were lost.
        if data.is_empty() {
            self.entries.push(PendingEntry {
                path: archive_path,
                meta,
                uncompressed_size: 0,
            });
            self.record_order(&recorded);
            return Ok(());
        }

        // Compress data using spawn_blocking for CPU-bound work.
        // The whole options go across, not just the method and level: `threads`
        // and `memory_limit` decide how the codec may spread itself, and an
        // async caller who set them meant them.
        let options = self.options.clone();
        let data_owned = data.to_vec();

        let compressed =
            tokio::task::spawn_blocking(move || compress_data_sync(&data_owned, &options))
                .await
                .map_err(|e| Error::Io(std::io::Error::other(e)))??;

        let compressed_size = compressed.data.len() as u64;

        self.write_entry_bytes(&compressed.data).await?;
        self.compressed_bytes += compressed_size;

        // One folder holding one stream, described exactly as the blocking
        // writer describes the same thing.
        self.stream_info.pack_sizes.push(compressed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.coder_methods.push(self.options.method);
        self.stream_info
            .coder_properties
            .push(compressed.properties);
        // The checksum belongs in SubStreamsInfo, where every reader looks for
        // it; a folder CRC as well would declare more digests than follow.
        self.stream_info.crcs.push(None);
        self.stream_info.substream_sizes.push(uncompressed_size);
        self.stream_info.substream_crcs.push(crc);
        self.stream_info.num_unpack_streams_per_folder.push(1);
        self.stream_info.filter_info.push(None);
        self.stream_info.bcj2_folder_info.push(None);
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(None);

        self.entries.push(PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        });
        self.record_order(&recorded);

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

        // Record header position
        let header_pos = self.sink.stream_position().await.map_err(Error::Io)?;

        // The blocking writer's header encoder, from the same model. It runs on
        // the blocking pool because it is a synchronous loop over every entry:
        // an archive of a hundred thousand of them holds the thread for tens of
        // milliseconds, during which a current-thread runtime polls nothing
        // else. The model is moved across and handed back, since the write
        // result is computed from it.
        // A large plain header is written compressed, exactly as the blocking
        // writer writes it: the payload goes in the data area and the structure
        // pointing at it becomes the next header. It is most of a small-file
        // archive otherwise - a few hundred names compress to a seventh of what
        // they occupy - and compressing it in one writer but not the other
        // would mean the two APIs producing different archives from the same
        // entries.
        //
        // Both happen inside the one blocking task. Encoding the header is a
        // synchronous loop over every entry and compressing it is a synchronous
        // LZMA2 encode of the result; on an archive of fifty thousand entries
        // the second was 147ms during which a current-thread runtime polled
        // nothing else, having carefully moved the first off the runtime.
        let stream_info = std::mem::take(&mut self.stream_info);
        let entries = std::mem::take(&mut self.entries);
        let options = self.options.clone();
        // Only the compressed form needs it, and that is behind `lzma2`.
        #[cfg_attr(not(feature = "lzma2"), allow(unused_variables))]
        let pack_pos = header_pos - self.start_pos - SIGNATURE_HEADER_SIZE;
        let (built, stream_info, entries) = tokio::task::spawn_blocking(move || {
            let built = HeaderModel {
                stream_info: &stream_info,
                entries: &entries,
                options: &options,
            }
            .encode_header()
            .and_then(|header| {
                #[cfg(feature = "lzma2")]
                let compressed =
                    crate::write::header_compression::encode_compressed_header(&header, pack_pos)?;
                #[cfg(not(feature = "lzma2"))]
                let compressed: Option<(Vec<u8>, Vec<u8>)> = None;
                Ok((header, compressed))
            });
            (built, stream_info, entries)
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
        self.stream_info = stream_info;
        self.entries = entries;
        let (header_data, compressed) = built?;

        let (next_header_pos, next_header) = match compressed {
            Some((payload, structure)) => {
                self.write_entry_bytes(&payload).await?;
                let structure_pos = self.sink.stream_position().await.map_err(Error::Io)?;
                self.write_entry_bytes(&structure).await?;
                (structure_pos, structure)
            }
            None => {
                self.write_entry_bytes(&header_data).await?;
                (header_pos, header_data)
            }
        };
        let archive_len = self.sink.stream_position().await.map_err(Error::Io)?;

        // Write signature header at start
        self.write_signature_header_async(next_header_pos, &next_header)
            .await?;

        // A buffered async sink has no drop that can flush it - dropping it
        // simply discards whatever it still holds. `create_path` wraps the file
        // in one, so without this the archive was left with 32 zero bytes where
        // its signature belongs: unreadable by 7-Zip and by this crate.
        self.sink.flush().await.map_err(Error::Io)?;

        self.state = AsyncWriterState::Finished;

        // Build result
        let result = WriteResult {
            // Anti-items are removals, not files, and the blocking writer has
            // always counted them that way.
            entries_written: self
                .entries
                .iter()
                .filter(|e| !e.meta.is_directory && !e.meta.is_anti)
                .count(),
            directories_written: self.entries.iter().filter(|e| e.meta.is_directory).count(),
            total_size: self.entries.iter().map(|e| e.uncompressed_size).sum(),
            compressed_size: self.compressed_bytes,
            volume_count: 1,
            volume_sizes: vec![archive_len - self.start_pos],
        };

        Ok((result, self.sink))
    }

    /// Writes the signature header at the start of the file asynchronously.
    async fn write_signature_header_async(
        &mut self,
        header_pos: u64,
        header_data: &[u8],
    ) -> Result<()> {
        let next_header_offset = header_pos - self.start_pos - SIGNATURE_HEADER_SIZE;
        let next_header_size = header_data.len() as u64;
        let next_header_crc = crc32fast::hash(header_data);

        // Build start header (20 bytes)
        let mut start_header = Vec::with_capacity(20);
        start_header.extend_from_slice(&next_header_offset.to_le_bytes());
        start_header.extend_from_slice(&next_header_size.to_le_bytes());
        start_header.extend_from_slice(&next_header_crc.to_le_bytes());

        let start_header_crc = crc32fast::hash(&start_header);

        // Seek to start and write signature header
        let start = self.start_pos;
        self.sink
            .seek(SeekFrom::Start(start))
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

    /// Writes to the sink, marking the writer unusable unless it completes.
    ///
    /// Once an entry's bytes are in the sink, anything short of finishing the
    /// write leaves data belonging to no folder, and every folder after it
    /// would be found at the wrong offset.
    ///
    /// The state is set before the await rather than in the error path,
    /// because an error is not the only way this fails to complete: a write
    /// that is cancelled - `tokio::time::timeout`, a losing `select!` branch -
    /// simply never resumes, so nothing after the await runs. That left the
    /// writer accepting entries with a half-written one already in the sink,
    /// and `finish` produced an archive whose next entry failed its checksum.
    async fn write_entry_bytes(&mut self, data: &[u8]) -> Result<()> {
        let resume_as = self.state;
        self.state = AsyncWriterState::Failed;
        self.sink.write_all(data).await.map_err(Error::Io)?;
        self.state = resume_as;
        Ok(())
    }

    /// Everything that can be decided before the entry's data is touched.
    ///
    /// Reading first and judging afterwards costs the caller the read, and for
    /// a source that cannot be rewound, the data: an out-of-order entry under
    /// `deterministic` was refused only once its stream had been consumed.
    /// Every public path that takes data runs this before any I/O.
    fn checks_before_reading(&self, archive_path: &ArchivePath) -> Result<()> {
        self.ensure_accepting_entries()?;
        self.check_order(archive_path)
    }

    /// Ensures the writer is in the AcceptingEntries state.
    fn ensure_accepting_entries(&self) -> Result<()> {
        if self.state == AsyncWriterState::Failed {
            return Err(Error::InvalidFormat(
                "an earlier entry failed partway through writing; \
                 this archive cannot be completed"
                    .into(),
            ));
        }
        if self.state != AsyncWriterState::AcceptingEntries {
            return Err(Error::InvalidFormat(
                "Writer is not accepting entries".into(),
            ));
        }

        // This writer emits its own header and supports none of the following.
        // Accepting an option and writing as though it had not been set is how
        // a caller ends up with an archive that is not what they asked for -
        // unencrypted data being the worst of them, but a missing filter or a
        // non-solid archive are wrong in the same way.
        #[cfg(feature = "aes")]
        if self.options.is_encrypted() || self.options.encrypt_data || self.options.encrypt_header {
            return Err(Error::UnsupportedFeature {
                feature: "encryption in the async writer",
            });
        }

        if self.options.filter.is_active() {
            return Err(Error::UnsupportedFeature {
                feature: "pre-compression filters in the async writer",
            });
        }

        if self.options.solid.is_solid() {
            return Err(Error::UnsupportedFeature {
                feature: "solid archives in the async writer",
            });
        }

        if self.options.comment.is_some() {
            return Err(Error::UnsupportedFeature {
                feature: "archive comments in the async writer",
            });
        }

        // The same checks the blocking writer makes, so a method this build
        // cannot use is refused before the source is read rather than after:
        // `add_stream` reads its input whole before compressing any of it.
        self.options.validate()?;

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
fn compress_data_sync(data: &[u8], options: &WriteOptions) -> Result<Compressed> {
    // The blocking writer's own dispatch, so the two cannot drift: everything
    // an option controls - level, dictionary, threads, memory - applies here
    // exactly as it does there.
    //
    // Which includes what decides whether a stream is cut into blocks. The
    // blocking writer splits an entry only once it is large enough to be
    // written on its own; below that it batches, and a batched entry is left
    // whole. Asking the same question here is what keeps the two APIs writing
    // the same archive for the same input - they are the same format, and a
    // caller who moves from one to the other should not find the bytes
    // change under them.
    let concurrency = crate::write::codecs::Concurrency::for_entry(options, data.len());
    crate::write::compression::compress_data(options, data, concurrency)
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

    // Writing needs a codec; the default is LZMA2.
    #[cfg(feature = "lzma2")]
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
}
