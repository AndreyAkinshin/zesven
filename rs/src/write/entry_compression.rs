//! Entry compression methods.
//!
//! This module provides functions for compressing individual entries,
//! including solid and non-solid compression modes, and BCJ2 filter handling.

use std::io::{Read, Seek, Write};

use crate::{ArchivePath, Error, Result};

use super::compression::filter_and_compress_data;
use super::options::{EntryMeta, WriteOptions};
use super::{Bcj2FolderInfo, BufferedEntry, PendingEntry, Writer};

/// How much uncompressed data a non-solid batch holds before it is compressed.
///
/// Each entry in the batch is resident until the batch is written, so this is
/// the writer's memory budget as much as its unit of parallelism. It matches
/// the default solid block size, which is the same trade made for the same
/// reason.
const BATCH_BYTES: u64 = 64 * 1024 * 1024;

/// Returns how many encoders may run at once, for entries of this size.
///
/// Never zero, and never more than the caller allowed: a single encoder runs
/// whatever it costs, since refusing to compress would be worse than exceeding
/// a budget. The share the batch itself occupies is taken off the top, so the
/// encoders and the data waiting for them are counted against the same limit.
#[cfg(feature = "parallel")]
pub(crate) fn workers_within_budget(
    options: &super::options::WriteOptions,
    data_len: usize,
) -> usize {
    let threads = options.threads.count();
    let per_encoder = super::codecs::encoder_memory_usage(options, data_len);

    let for_encoders = options
        .memory_limit
        .bytes()
        .saturating_sub(batch_bytes(options));

    for_encoders
        .checked_div(per_encoder)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(threads)
        .clamp(1, threads)
}

/// Returns how much uncompressed data a non-solid batch may hold.
///
/// A quarter of the budget, so that the entries waiting to be compressed
/// cannot crowd out the encoders that have to compress them, and never more
/// than [`BATCH_BYTES`], which is as much as parallelism can use.
pub(crate) fn batch_bytes(options: &super::options::WriteOptions) -> u64 {
    (options.memory_limit.bytes() / 4).min(BATCH_BYTES)
}

/// One entry's compressed output, waiting to be written.
struct CompressedEntry {
    /// The compressed bytes and the coder properties describing them.
    compressed: super::codecs::Compressed,
    /// Filter info for this folder, if a filter is configured.
    filter_info: Option<super::FilteredFolderInfo>,
}

impl<W: Write + Seek> Writer<W> {
    /// Compresses an entry in non-solid mode.
    ///
    /// Entries are gathered into a batch rather than compressed one at a time:
    /// each one becomes its own folder and so is independent of the others,
    /// which is what lets the batch be compressed across cores.
    pub(crate) fn compress_entry_non_solid(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        // Read all data and compute CRC
        let mut data = Vec::new();
        source.read_to_end(&mut data).map_err(Error::Io)?;

        // Check if BCJ2 filter is active - route to dedicated method.
        // BCJ2 writes its four streams itself, so it cannot go through the
        // batch; anything already batched has to reach the sink first to keep
        // the folders in the order the entries were added.
        if self.options.filter.is_bcj2() {
            self.flush_pending_batch()?;
            return self.compress_entry_bcj2(archive_path, &data, meta);
        }

        let crc = crc32fast::hash(&data);
        let options = self.active_options.clone();
        self.pending_batch_size += data.len() as u64;
        self.pending_batch.push(BufferedEntry {
            options,
            path: archive_path,
            data,
            meta,
            crc,
        });

        if self.pending_batch_size >= batch_bytes(&self.options) || self.batch_can_fill_the_cores()
        {
            self.flush_pending_batch()?;
        }

        Ok(())
    }

    /// Returns whether the batch already holds enough entries to busy every
    /// worker that is going to run.
    ///
    /// Holding more than that buys no parallelism and only defers the work:
    /// the call that eventually flushes pays for the whole batch, and a caller
    /// timing individual calls sees that as one long call among instant ones.
    /// Flushing as soon as the cores can be filled keeps that pause as short as
    /// the work allows.
    fn batch_can_fill_the_cores(&self) -> bool {
        // Without threads there is nothing to fill, and batching would only
        // delay work that is about to happen anyway.
        #[cfg(not(feature = "parallel"))]
        {
            true
        }

        #[cfg(feature = "parallel")]
        {
            // Deliberately not the batch-aware count, which is capped by how
            // many entries are already here and so would always look reached.
            let largest = self
                .pending_batch
                .iter()
                .map(|e| e.data.len())
                .max()
                .unwrap_or(0);
            self.pending_batch.len() >= workers_within_budget(&self.options, largest)
        }
    }

    /// Compresses every entry in the pending batch and writes them in order.
    ///
    /// Any failure past this point has already consumed entries the caller was
    /// told were accepted, and they cannot be handed back - so the writer is
    /// finished rather than left to produce an archive quietly missing them.
    /// Wrapped here rather than at each step inside, so a step added later
    /// cannot forget.
    pub(crate) fn flush_pending_batch(&mut self) -> Result<()> {
        if self.pending_batch.is_empty() {
            return Ok(());
        }
        match self.flush_pending_batch_inner() {
            Ok(()) => Ok(()),
            Err(e) => self.fail(e),
        }
    }

    fn flush_pending_batch_inner(&mut self) -> Result<()> {
        // Taken from the entries themselves, so it cannot be read after they
        // have been moved out of the buffer - at which point the buffer would
        // answer with whatever is current instead.
        let options = self
            .pending_batch
            .first()
            .map(|entry| entry.options.clone())
            .unwrap_or_else(|| self.active_options.clone());
        let batch = std::mem::take(&mut self.pending_batch);
        self.pending_batch_size = 0;

        // Entries of a non-solid archive are compressed alongside each other,
        // so the codec itself never splits one into chunks. Letting it do so
        // when an entry happened to be alone in its batch would have made the
        // bytes depend on how many entries the machine's core count let the
        // batch gather - the same input giving different archives on different
        // hardware.
        let compressed = self.compress_batch(&batch, &options, false)?;

        for (entry, compressed) in batch.into_iter().zip(compressed) {
            self.write_compressed_entry(entry, compressed, &options)?;
        }

        Ok(())
    }

    /// Compresses a batch of entries, across cores where that is available.
    ///
    /// Results come back in input order, so the archive does not depend on
    /// which entry a worker happened to finish first.
    fn compress_batch(
        &self,
        batch: &[BufferedEntry],
        options: &WriteOptions,
        may_thread: bool,
    ) -> Result<Vec<Option<CompressedEntry>>> {
        // Only the options cross into the workers: the writer itself owns the
        // sink, which is neither shareable nor needed to compress.
        let compress_one = |entry: &BufferedEntry| -> Result<Option<CompressedEntry>> {
            // Empty files are carried by kEmptyStream and never get a folder.
            if entry.data.is_empty() {
                return Ok(None);
            }
            let (compressed, filter_info) =
                filter_and_compress_data(options, &entry.data, may_thread)?;
            Ok(Some(CompressedEntry {
                compressed,
                filter_info,
            }))
        };

        #[cfg(feature = "parallel")]
        {
            use rayon::prelude::*;

            let workers = self.compression_workers(batch, options);
            if batch.len() > 1 && workers > 1 {
                // A pool of our own, rather than the global one, so the number
                // of encoders alive at once stays within the memory budget.
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .map_err(|e| Error::Io(std::io::Error::other(e)))?;
                return pool.install(|| batch.par_iter().map(compress_one).collect());
            }
        }

        batch.iter().map(compress_one).collect()
    }

    /// Returns how many entries of a batch may be compressed at once.
    ///
    /// Bounded by the cores available and by what the encoders would reserve:
    /// at the higher levels a single encoder's match finder runs to hundreds of
    /// megabytes, and one per core is how a writer runs a machine out of memory.
    #[cfg(feature = "parallel")]
    fn compression_workers(&self, batch: &[BufferedEntry], options: &WriteOptions) -> usize {
        // The largest entry decides the footprint of an encoder.
        let largest = batch.iter().map(|e| e.data.len()).max().unwrap_or(0);

        // From the entries' own options, like everything else about them:
        // `threads` and `memory_limit` are part of what they were accepted
        // under, even though the count changes speed rather than bytes.
        workers_within_budget(options, largest).min(batch.len())
    }

    /// Writes one already-compressed entry and records it in the header data.
    fn write_compressed_entry(
        &mut self,
        entry: BufferedEntry,
        compressed: Option<CompressedEntry>,
        options: &WriteOptions,
    ) -> Result<()> {
        let uncompressed_size = entry.data.len() as u64;

        // Add entry (always, even for empty files)
        self.entries.push(PendingEntry {
            path: entry.path,
            meta: entry.meta,
            uncompressed_size,
        });

        // Empty files don't get a folder/stream - they're marked as
        // EmptyStream/EmptyFile in the header.
        let Some(CompressedEntry {
            compressed,
            filter_info,
        }) = compressed
        else {
            return Ok(());
        };

        // Encryption is applied here rather than in the batch: it needs a fresh
        // IV per stream, and it is cheap next to compression.
        #[cfg(feature = "aes")]
        let (output_data, encryption_info) = if options.is_data_encrypted() {
            let (encrypted, enc_info) = self.encrypt_compressed_with(compressed, options)?;
            (encrypted, Some(enc_info))
        } else {
            (compressed, None)
        };

        #[cfg(not(feature = "aes"))]
        let (output_data, encryption_info) = (compressed, Option::<()>::None);

        let packed_size = output_data.data.len() as u64;

        // Write compressed (and possibly encrypted) data
        self.write_entry_bytes(&output_data.data)?;
        self.compressed_bytes += packed_size;

        // Track stream info (only for non-empty files)
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(uncompressed_size);
        self.stream_info.coder_methods.push(options.method);
        self.stream_info
            .coder_properties
            .push(output_data.properties);
        // The checksum goes in SubStreamsInfo, where every reader looks for it.
        // Recording it as the folder CRC as well would make the header declare
        // more digests than the format says follow it.
        self.stream_info.crcs.push(None);
        self.stream_info.substream_sizes.push(uncompressed_size);
        self.stream_info.substream_crcs.push(entry.crc);

        // Track encryption info for header writing
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(encryption_info);

        // Track filter info for header writing
        self.stream_info.filter_info.push(filter_info);

        // Track that this is not a BCJ2 folder
        self.stream_info.bcj2_folder_info.push(None);

        // Suppress unused variable warning when aes feature is disabled
        #[cfg(not(feature = "aes"))]
        let _ = encryption_info;

        // Track that this is a non-solid folder (1 stream per folder)
        self.stream_info.num_unpack_streams_per_folder.push(1);

        Ok(())
    }

    /// Compresses an entry using BCJ2 4-stream filter.
    ///
    /// BCJ2 splits x86 code into 4 streams for improved compression:
    /// - Stream 0: Main code
    /// - Stream 1: CALL destinations (big-endian)
    /// - Stream 2: JMP destinations (big-endian)
    /// - Stream 3: Range-coded selector bits
    pub(crate) fn compress_entry_bcj2(
        &mut self,
        archive_path: ArchivePath,
        data: &[u8],
        meta: EntryMeta,
    ) -> Result<()> {
        use crate::codec::bcj2::bcj2_encode;

        let crc = crc32fast::hash(data);
        let uncompressed_size = data.len() as u64;

        // Add entry
        let entry = PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        };
        self.entries.push(entry);

        // Empty files don't get a folder/stream
        if data.is_empty() {
            return Ok(());
        }

        // Encode with BCJ2 - produces 4 streams
        let streams = bcj2_encode(data);

        // The filter only rearranges: it moves branch targets out of the code
        // and into streams of their own, where they compress far better than
        // they did interleaved with instructions. It does not compress
        // anything itself, so each of those three streams still has to go
        // through the codec - which is what makes BCJ2 worth asking for.
        //
        // The fourth stream is the range coder's own output. It is already
        // dense, and 7-Zip stores it as it is; so does this.
        let options = self.active_options.clone();
        let main = super::compression::compress_data(&options, &streams.main, true)?;
        let call = super::compression::compress_data(&options, &streams.call, true)?;
        let jump = super::compression::compress_data(&options, &streams.jump, true)?;

        self.write_entry_bytes(&main.data)?;
        self.write_entry_bytes(&call.data)?;
        self.write_entry_bytes(&jump.data)?;
        self.write_entry_bytes(&streams.range)?;

        let total_packed =
            (main.data.len() + call.data.len() + jump.data.len() + streams.range.len()) as u64;
        self.compressed_bytes += total_packed;

        // Track BCJ2 folder info
        let bcj2_info = Bcj2FolderInfo {
            pack_sizes: [
                main.data.len() as u64,
                call.data.len() as u64,
                jump.data.len() as u64,
                streams.range.len() as u64,
            ],
            stream_sizes: [
                streams.main.len() as u64,
                streams.call.len() as u64,
                streams.jump.len() as u64,
            ],
            properties: [main.properties, call.properties, jump.properties],
            method: options.method,
        };

        // For BCJ2, we don't use pack_sizes (handled separately)
        // Store unpack_size and CRC
        self.stream_info.unpack_sizes.push(uncompressed_size);
        // BCJ2 folders write their own coder chain and never consult these, but
        // the per-folder vectors are indexed together and must stay aligned.
        self.stream_info.coder_methods.push(self.options.method);
        self.stream_info.coder_properties.push(Vec::new());
        // The checksum goes in SubStreamsInfo, where every reader looks for it.
        // Recording it as the folder CRC as well would make the header declare
        // more digests than the format says follow it.
        self.stream_info.crcs.push(None);
        self.stream_info.substream_sizes.push(uncompressed_size);
        self.stream_info.substream_crcs.push(crc);

        // Track filter info as None (BCJ2 handled separately)
        self.stream_info.filter_info.push(None);

        // Track BCJ2 folder info
        self.stream_info.bcj2_folder_info.push(Some(bcj2_info));

        // Track encryption info (BCJ2 + encryption not supported yet)
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(None);

        // Track that this is a non-solid folder (1 stream per folder)
        self.stream_info.num_unpack_streams_per_folder.push(1);

        Ok(())
    }

    /// Buffers an entry for solid compression.
    pub(crate) fn buffer_entry_solid(
        &mut self,
        archive_path: ArchivePath,
        source: &mut dyn Read,
        meta: EntryMeta,
    ) -> Result<()> {
        // Read all data and compute CRC
        let mut data = Vec::new();
        source.read_to_end(&mut data).map_err(Error::Io)?;
        let crc = crc32fast::hash(&data);
        let data_size = data.len() as u64;

        // Buffer the entry
        self.solid_buffer_size += data_size;
        let options = self.active_options.clone();
        self.solid_buffer.push(BufferedEntry {
            options,
            path: archive_path,
            data,
            meta,
            crc,
        });

        // Check if buffer should be flushed
        let size_exceeded = self
            .options
            .solid
            .block_size
            .is_some_and(|limit| self.solid_buffer_size >= limit);
        let count_exceeded = self
            .options
            .solid
            .files_per_block
            .is_some_and(|limit| self.solid_buffer.len() >= limit);

        if size_exceeded || count_exceeded {
            self.flush_solid_buffer()?;
        }

        Ok(())
    }

    /// Flushes the solid buffer, compressing all buffered entries as one block.
    /// Writes the solid block out, under the options its entries were accepted
    /// under.
    ///
    /// Poisoned on any failure, for the same reason as the batch: the entries
    /// are gone by then.
    pub(crate) fn flush_solid_buffer(&mut self) -> Result<()> {
        if self.solid_buffer.is_empty() {
            return Ok(());
        }
        match self.flush_solid_buffer_inner() {
            Ok(()) => Ok(()),
            Err(e) => self.fail(e),
        }
    }

    fn flush_solid_buffer_inner(&mut self) -> Result<()> {
        // From the entries, before any of them are moved out.
        let options = self
            .solid_buffer
            .first()
            .map(|entry| entry.options.clone())
            .unwrap_or_else(|| self.active_options.clone());

        // Concatenate all entry data (only non-empty entries have data streams)
        let total_uncompressed: u64 = self.solid_buffer.iter().map(|e| e.data.len() as u64).sum();
        let mut combined = Vec::with_capacity(total_uncompressed as usize);

        // Collect sizes and CRCs for substreams (only non-empty entries)
        // Empty entries (size=0) are marked as EmptyStream and don't have data streams
        let mut sizes = Vec::new();
        let mut crcs = Vec::new();
        let mut num_streams = 0u64;

        for entry in &self.solid_buffer {
            if !entry.data.is_empty() {
                combined.extend_from_slice(&entry.data);
                sizes.push(entry.data.len() as u64);
                crcs.push(entry.crc);
                num_streams += 1;
            }
            // Empty entries are handled via EmptyStream/EmptyFile in FilesInfo
        }

        // A block of nothing but empty entries has no data to store. Emitting a
        // folder for it produces one with zero substreams, which 7-Zip rejects
        // outright; the entries are carried by kEmptyStream/kEmptyFile instead.
        if num_streams == 0 {
            for entry in self.solid_buffer.drain(..) {
                self.entries.push(PendingEntry {
                    path: entry.path,
                    meta: entry.meta,
                    uncompressed_size: 0,
                });
            }
            return Ok(());
        }

        // Process data through filter -> compress -> encrypt pipeline
        // A solid block is one folder, so there is nothing here to compress
        // alongside it: the codec is free to use every core itself.
        let (compressed, filter_info) = filter_and_compress_data(&options, &combined, true)?;

        #[cfg(feature = "aes")]
        let (output_data, encryption_info) = if options.is_data_encrypted() {
            let (encrypted, enc_info) = self.encrypt_compressed_with(compressed, &options)?;
            (encrypted, Some(enc_info))
        } else {
            (compressed, None)
        };

        #[cfg(not(feature = "aes"))]
        let (output_data, encryption_info) = (compressed, Option::<()>::None);

        let packed_size = output_data.data.len() as u64;

        // Write compressed (and possibly encrypted) data
        self.write_entry_bytes(&output_data.data)?;
        self.compressed_bytes += packed_size;

        // Record ONE folder with streams for non-empty entries only
        self.stream_info.pack_sizes.push(packed_size);
        self.stream_info.unpack_sizes.push(total_uncompressed);
        self.stream_info.coder_methods.push(options.method);
        self.stream_info
            .coder_properties
            .push(output_data.properties);

        // Track encryption info for header writing
        #[cfg(feature = "aes")]
        self.stream_info.encryption_info.push(encryption_info);

        // Track filter info for header writing
        self.stream_info.filter_info.push(filter_info);

        // Track that this is not a BCJ2 folder
        self.stream_info.bcj2_folder_info.push(None);

        // Suppress unused variable warning when aes feature is disabled
        #[cfg(not(feature = "aes"))]
        let _ = encryption_info;

        // Record number of streams in this folder (only non-empty entries)
        self.stream_info
            .num_unpack_streams_per_folder
            .push(num_streams);

        // For solid blocks with multiple streams, folder CRC is not used (substream CRCs are).
        // For solid blocks with exactly 1 stream, use folder CRC directly (no SubStreamsInfo needed).
        if num_streams == 1 {
            self.stream_info.crcs.push(None);
            self.stream_info.substream_sizes.extend_from_slice(&sizes);
            self.stream_info.substream_crcs.extend_from_slice(&crcs);
        } else {
            // Multiple non-empty files: the per-entry CRCs in SubStreamsInfo are
            // the meaningful ones.
            self.stream_info.crcs.push(None);
            self.stream_info.substream_sizes.extend_from_slice(&sizes);
            self.stream_info.substream_crcs.extend_from_slice(&crcs);
        }

        // Create entries for all buffered files
        for entry in self.solid_buffer.drain(..) {
            let uncompressed_size = entry.data.len() as u64;
            self.entries.push(PendingEntry {
                path: entry.path,
                meta: entry.meta,
                uncompressed_size,
            });
        }

        self.solid_buffer_size = 0;

        Ok(())
    }
}
