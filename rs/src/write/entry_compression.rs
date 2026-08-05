//! Entry compression methods.
//!
//! This module provides functions for compressing individual entries,
//! including solid and non-solid compression modes, and BCJ2 filter handling.

use std::io::{Seek, Write};

use crate::{ArchivePath, Result};

use super::compression::filter_and_compress_data;
use super::options::{EntryMeta, WriteOptions};
use super::{Bcj2FolderInfo, BufferedEntry, PendingEntry, Writer};

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
/// cannot crowd out the encoders that have to compress them.
///
/// It bounds the parallelism as much as the memory: entries are compressed
/// alongside each other, so a batch that holds three entries keeps three cores
/// busy however many the machine has. A fixed ceiling used to sit on top of
/// this, and on a workload of twenty-megabyte files it was what decided the
/// answer - three at a time on every machine, from a laptop to a
/// sixty-four-core server, because the ceiling and not the budget was doing
/// the work.
pub(crate) fn batch_bytes(options: &super::options::WriteOptions) -> u64 {
    options.memory_limit.bytes() / 4
}

/// One entry's compressed output, waiting to be written.
pub(crate) struct CompressedEntry {
    /// The compressed bytes and the coder properties describing them.
    compressed: super::codecs::Compressed,
    /// Filter info for this folder, if a filter is configured.
    filter_info: Option<super::FilteredFolderInfo>,
}

/// A compressed batch entry and what writing it still needs.
///
/// The input is not carried: compression is the last thing that reads it, and a
/// batch waiting to be written would otherwise hold every entry's uncompressed
/// bytes for as long as its compressed ones.
pub(crate) struct BatchOutcome {
    path: ArchivePath,
    meta: EntryMeta,
    crc: u32,
    uncompressed_size: u64,
    /// Absent for an empty file, which is carried by the header and has no
    /// folder of its own.
    compressed: Option<CompressedEntry>,
}

/// Compresses a whole batch and gives back what writing each entry needs.
///
/// Takes the entries rather than borrowing them so that it can run away from
/// the writer, on a thread of its own, while the writer gets on with an entry
/// that is being compressed straight into the sink.
///
/// Results come back in input order, so the archive does not depend on which
/// entry a worker happened to finish first.
pub(crate) fn compress_batch_owned(
    batch: Vec<BufferedEntry>,
    options: &WriteOptions,
) -> Result<Vec<BatchOutcome>> {
    let compress_one = |entry: BufferedEntry| -> Result<BatchOutcome> {
        let BufferedEntry {
            path,
            data,
            meta,
            crc,
            ..
        } = entry;
        let uncompressed_size = data.len() as u64;

        // Empty files are carried by kEmptyStream and never get a folder.
        if data.is_empty() {
            return Ok(BatchOutcome {
                path,
                meta,
                crc,
                uncompressed_size,
                compressed: None,
            });
        }
        // Alongside even when this entry turns out to be alone in its
        // batch. What a batch holds depends on the memory budget and the
        // core count, so letting that decide whether an entry is split
        // would make the same input produce different archives on
        // different machines.
        let (compressed, filter_info) =
            filter_and_compress_data(options, &data, super::codecs::Concurrency::Alongside)?;
        // Released as soon as it has been read rather than when the batch is
        // written. What a batch holds while it runs is what decides whether it
        // may run alongside a streamed entry, and keeping every input until the
        // last output exists would put both in memory at once - on
        // incompressible data, twice what it is charged for.
        drop(data);

        Ok(BatchOutcome {
            path,
            meta,
            crc,
            uncompressed_size,
            compressed: Some(CompressedEntry {
                compressed,
                filter_info,
            }),
        })
    };

    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;

        // The largest entry decides the footprint of an encoder.
        let largest = batch.iter().map(|e| e.data.len()).max().unwrap_or(0);
        // From the entries' own options, like everything else about them:
        // `threads` and `memory_limit` are part of what they were accepted
        // under, even though the count changes speed rather than bytes.
        let workers = workers_within_budget(options, largest).min(batch.len());

        if batch.len() > 1 && workers > 1 {
            // A pool of our own, rather than the global one, so the number
            // of encoders alive at once stays within the memory budget.
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .map_err(|e| crate::Error::Io(std::io::Error::other(e)))?;
            return pool.install(|| batch.into_par_iter().map(compress_one).collect());
        }
    }

    batch.into_iter().map(compress_one).collect()
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
        data: Vec<u8>,
        meta: EntryMeta,
    ) -> Result<()> {
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

        // Announced before the work rather than after it: the entries of a
        // batch are compressed at the same time as each other, so there is no
        // moment at which one of them is the one being worked on. What a
        // caller can be told is which entries have gone in, and that is worth
        // saying at the start of the wait rather than at the end of it.
        self.announce_entries(
            batch
                .iter()
                .map(|entry| (entry.path.as_str().to_string(), entry.data.len() as u64))
                .collect(),
        );

        // Entries of a non-solid archive are compressed alongside each other,
        // so the codec itself never splits one into chunks. Letting it do so
        // when an entry happened to be alone in its batch would have made the
        // bytes depend on how many entries the machine's core count let the
        // batch gather - the same input giving different archives on different
        // hardware.
        let outcomes = compress_batch_owned(batch, &options)?;

        self.write_batch_outcomes(outcomes, &options)
    }

    /// Writes a compressed batch to the sink, in the order the entries came.
    pub(crate) fn write_batch_outcomes(
        &mut self,
        outcomes: Vec<BatchOutcome>,
        options: &WriteOptions,
    ) -> Result<()> {
        for outcome in outcomes {
            self.write_compressed_entry(outcome, options)?;
        }
        Ok(())
    }

    /// Writes one already-compressed entry and records it in the header data.
    fn write_compressed_entry(
        &mut self,
        entry: BatchOutcome,
        options: &WriteOptions,
    ) -> Result<()> {
        let uncompressed_size = entry.uncompressed_size;

        // Recorded last, once the bytes are in the sink: the entry is reported
        // finished as it goes in, and nothing that has still to be written is
        // reported as done.
        let pending = PendingEntry {
            path: entry.path,
            meta: entry.meta,
            uncompressed_size,
        };

        // Empty files don't get a folder/stream - they're marked as
        // EmptyStream/EmptyFile in the header.
        let Some(CompressedEntry {
            compressed,
            filter_info,
        }) = entry.compressed
        else {
            self.record_entry(pending);
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

        self.record_entry(pending);

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

        // Recorded once its streams are in the sink, so that an entry is
        // reported finished only when it is.
        let pending = PendingEntry {
            path: archive_path,
            meta,
            uncompressed_size,
        };

        // Empty files don't get a folder/stream
        if data.is_empty() {
            self.record_entry(pending);
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
        let concurrency = super::codecs::Concurrency::Alongside;
        let main = super::compression::compress_data(&options, &streams.main, concurrency)?;
        let call = super::compression::compress_data(&options, &streams.call, concurrency)?;
        let jump = super::compression::compress_data(&options, &streams.jump, concurrency)?;

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

        self.record_entry(pending);

        Ok(())
    }

    /// Buffers an entry for solid compression.
    pub(crate) fn buffer_entry_solid(
        &mut self,
        archive_path: ArchivePath,
        data: Vec<u8>,
        meta: EntryMeta,
    ) -> Result<()> {
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

        // Announced before the work, for the reason a batch is: a solid block
        // is compressed as one stream, so no entry in it is the one being
        // worked on, and the whole block is the wait a caller is sitting
        // through.
        self.announce_entries(
            self.solid_buffer
                .iter()
                .map(|entry| (entry.path.as_str().to_string(), entry.data.len() as u64))
                .collect(),
        );

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
            for entry in self.solid_buffer.drain(..).collect::<Vec<_>>() {
                self.record_entry(PendingEntry {
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
        let concurrency = super::codecs::Concurrency::alone(&options, combined.len());
        let (compressed, filter_info) = filter_and_compress_data(&options, &combined, concurrency)?;

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
        for entry in self.solid_buffer.drain(..).collect::<Vec<_>>() {
            let uncompressed_size = entry.data.len() as u64;
            self.record_entry(PendingEntry {
                path: entry.path,
                meta: entry.meta,
                uncompressed_size,
            });
        }

        self.solid_buffer_size = 0;

        Ok(())
    }
}
