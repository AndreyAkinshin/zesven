//! Writer initialization and finalization.
//!
//! This module provides methods for creating writers and finishing archive writing,
//! including signature header writing.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::format::{SIGNATURE, SIGNATURE_HEADER_SIZE};
use crate::volume::{MultiVolumeWriter, VolumeConfig};
use crate::{ArchivePath, Error, Result};

use super::options::{WriteOptions, WriteResult};
use super::{PendingEntry, StreamInfo, Writer, WriterState};

impl Writer<BufWriter<File>> {
    /// Creates a new archive file at the given path.
    ///
    /// The file is created, and an existing one truncated, before a single
    /// entry has been written - so a run that fails partway leaves neither the
    /// finished archive nor what was at that path before. Where that matters,
    /// write to a path of your own beside it and rename onto the destination
    /// when [`Writer::finish`] returns; that is what this crate's own
    /// command-line tool does.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the archive file to create
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created.
    pub fn create_path(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::create(path.as_ref()).map_err(Error::Io)?;
        let writer = BufWriter::new(file);
        Self::create(writer)
    }

    /// Finishes writing the archive.
    ///
    /// # Returns
    ///
    /// A WriteResult with statistics about the written archive.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing fails.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<std::io::Cursor<Vec<u8>>> {
    /// Finishes writing the archive to an owned cursor.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<std::io::Cursor<&mut Vec<u8>>> {
    /// Finishes writing the archive to a borrowed cursor.
    pub fn finish(self) -> Result<WriteResult> {
        let (result, _sink) = self.finish_into_inner()?;
        Ok(result)
    }
}

impl Writer<MultiVolumeWriter> {
    /// Creates a new multi-volume archive writer.
    ///
    /// The archive will be split across multiple files when each volume
    /// reaches the configured size limit.
    ///
    /// # Arguments
    ///
    /// * `config` - Volume configuration specifying size and base path
    ///
    /// Each volume is created, and an existing file of that name truncated, as
    /// the archive reaches it - so a run that fails partway leaves a set of
    /// volumes that is neither complete nor what was there before.
    ///
    /// # Errors
    ///
    /// Returns an error if the first volume file cannot be created.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zesven::{Writer, VolumeConfig, ArchivePath};
    ///
    /// let config = VolumeConfig::new("archive.7z", 50 * 1024 * 1024); // 50 MB volumes
    /// let mut writer = Writer::create_multivolume(config)?;
    /// writer.add_bytes(ArchivePath::new("data.bin")?, &large_data)?;
    /// let result = writer.finish()?;
    /// println!("Created {} volumes", result.volume_count);
    /// ```
    pub fn create_multivolume(config: VolumeConfig) -> Result<Self> {
        let writer = MultiVolumeWriter::create(config)?;
        Self::create(writer)
    }

    /// Finishes writing the multi-volume archive.
    ///
    /// This finalizes all volumes and returns a WriteResult with volume information.
    ///
    /// # Returns
    ///
    /// A WriteResult with statistics including volume_count and volume_sizes.
    ///
    /// # Errors
    ///
    /// Returns an error if header writing or volume finalization fails.
    pub fn finish(self) -> Result<WriteResult> {
        let (mut result, mv_writer) = self.finish_into_inner()?;

        // Finalize the multi-volume writer and get volume sizes
        let volume_sizes = mv_writer.finish()?;
        result.volume_count = volume_sizes.len() as u32;
        result.volume_sizes = volume_sizes;

        Ok(result)
    }
}

impl<W: Write + Seek> Writer<W> {
    /// Creates a new archive writer.
    ///
    /// # Arguments
    ///
    /// * `sink` - The writer to output archive data to
    ///
    /// # Errors
    ///
    /// Returns an error if the initial seek fails.
    pub fn create(mut sink: W) -> Result<Self> {
        // Where the archive begins, which is wherever the sink happens to be
        // rather than nought: a caller may be writing after a prefix of their
        // own. Everything measured or seeked to is relative to this.
        let start_pos = sink.stream_position().map_err(Error::Io)?;

        // Reserve space for signature header (32 bytes) by writing zeros
        let placeholder = [0u8; SIGNATURE_HEADER_SIZE as usize];
        sink.write_all(&placeholder).map_err(Error::Io)?;

        Ok(Self {
            sink,
            start_pos,
            options: WriteOptions::default(),
            state: WriterState::AcceptingEntries,
            entries: Vec::new(),
            stream_info: StreamInfo::default(),
            compressed_bytes: 0,
            accepted_bytes: 0,
            announced: Vec::new(),
            solid_buffer: Vec::new(),
            solid_buffer_size: 0,
            pending_batch: Vec::new(),
            pending_batch_size: 0,
            active_options: std::sync::Arc::new(WriteOptions::default()),
            last_path: None,
            #[cfg(feature = "aes")]
            archive_salt: None,
            #[cfg(feature = "aes")]
            archive_password: None,
            progress: None,
            #[cfg(feature = "parallel")]
            ahead: None,
        })
    }

    /// Reports what is being written to `reporter` as the archive is built.
    ///
    /// Writing an archive is mostly silent from the outside: entries are
    /// gathered and compressed together, so the call that accepts an entry
    /// usually returns before anything has been compressed and the work lands
    /// on whichever call fills the batch, or on `finish`. A caller timing its
    /// own calls sees that as one long pause among instant ones, which is a
    /// description of the batching rather than of the work.
    ///
    /// The reporter is told what actually happens: which entries are being
    /// worked on, how far an entry large enough to be compressed on its own has
    /// got, and when each one is in.
    ///
    /// The three say different things, and each says what it can honestly say:
    ///
    /// - `on_entry_start` comes before the work, and for a batch it comes for
    ///   every entry in it at once, because they are compressed at the same
    ///   time as each other and no one of them is the one being worked on.
    /// - `on_progress` reports bytes of archive produced against the size the
    ///   entry declared, and only for an entry compressed on its own. It is the
    ///   work rather than the reading: such an entry is taken in far faster
    ///   than it is compressed, so a count of what has been read would reach
    ///   the end within a second and leave the rest of the wait silent.
    /// - `on_ratio` comes with each completed entry and covers the archive so
    ///   far on both sides, which is the only total a writer knows: what it
    ///   will hold in the end depends on calls that have not been made yet.
    ///
    /// Entries do not begin and end one at a time. An entry large enough to be
    /// compressed on its own is left finishing while the entry after it is
    /// read, and a batch is compressed while a large entry is, so a reporter
    /// can be told an entry has started before it is told the one before it has
    /// finished. What is guaranteed is the order they are finished in, which is
    /// the order they were added: an entry is reported finished when its bytes
    /// are in the archive, and they go in as they were accepted.
    ///
    /// A reporter can call the writing off from either of two places. Answering
    /// `on_progress` with `false` stops the entry being written where it
    /// stands, and is the only way to stop during one; `should_cancel` is asked
    /// before each entry is accepted. Both give the caller [`Error::Cancelled`],
    /// but they leave different things behind: an entry stopped partway has put
    /// bytes in the sink that belong to no folder, so the archive cannot be
    /// finished, while one refused before it started leaves an archive that can.
    ///
    /// ```rust,ignore
    /// use zesven::{StatisticsProgress, write::Writer};
    ///
    /// let writer = Writer::create_path("archive.7z")?
    ///     .progress(StatisticsProgress::new());
    /// ```
    pub fn progress(mut self, reporter: impl crate::progress::ProgressReporter + 'static) -> Self {
        self.progress = Some(std::sync::Arc::new(std::sync::Mutex::new(Box::new(
            reporter,
        ))));
        self
    }

    /// Tells the reporter which entries a block about to be compressed holds.
    ///
    /// A batch, or a solid block, is the shape that makes writing look like it
    /// is doing nothing: the calls that accept entries return at once, and the
    /// work lands on whichever call fills it, or on `finish`. Saying which
    /// entries went in before that work starts is the difference between a
    /// caller who can show what is happening and one who can only show a pause.
    ///
    /// Takes what to say rather than the entries themselves, because the solid
    /// buffer is reached through the writer this borrows.
    pub(crate) fn announce_entries(&mut self, entries: Vec<(String, u64)>) {
        for (name, size) in entries {
            self.announce(name, size);
        }
    }

    /// Announces one entry and remembers that it is outstanding.
    fn announce(&mut self, name: String, size: u64) {
        let Some(reporter) = self.progress.as_ref() else {
            return;
        };
        if let Ok(mut held) = reporter.lock() {
            held.on_entry_start(&name, size);
        }
        self.announced.push(name);
    }

    /// Puts an entry in the archive and tells the reporter it is in.
    ///
    /// The one way an entry reaches the file list. Every path that writes one
    /// ends here, and each calls it once its bytes are safely in the sink, so
    /// nothing is reported finished that has still to be written.
    ///
    /// It exists because reporting used to be attached to the two paths that
    /// happened to be under the hand at the time, while entries reach the
    /// archive by six: batched, streamed straight through, solid, a solid block
    /// of nothing but empty entries, BCJ2, and the directories and anti-items
    /// that are written directly. The other four were silent, which for a solid
    /// archive meant every entry in it.
    pub(crate) fn record_entry(&mut self, entry: PendingEntry) {
        let (path, accepted) = (entry.path.clone(), entry.uncompressed_size);
        self.entries.push(entry);
        self.report_entry_written(&path, accepted);
    }

    /// Tells the reporter an entry has reached the archive.
    ///
    /// The ratio goes with it, which is what a writer can honestly say about
    /// the whole: how much has been accepted so far and how much of an archive
    /// that has become. What an archive will hold in the end is not known while
    /// it is being written, since entries arrive one call at a time.
    ///
    /// Both halves span the same thing. Reporting this entry's input against
    /// the archive's output would give a ratio that shrinks with every entry
    /// written, whatever the data does.
    ///
    /// An entry nobody announced is announced here, so that no caller is told
    /// something finished that it was never told had started. The paths that
    /// announce ahead of the work do so because the work is long and shared
    /// between entries; a directory or an anti-item has no work to speak of and
    /// begins and ends in the same breath.
    ///
    /// Progress within an entry is reported separately and means something
    /// narrower - see [`Writer::progress`].
    pub(crate) fn report_entry_written(&mut self, path: &ArchivePath, accepted: u64) {
        self.accepted_bytes = self.accepted_bytes.saturating_add(accepted);
        let (total_accepted, compressed) = (self.accepted_bytes, self.compressed_bytes);
        let announced_already = match self.announced.iter().position(|name| name == path.as_str()) {
            Some(at) => {
                self.announced.remove(at);
                true
            }
            None => false,
        };
        let Some(reporter) = self.progress.as_ref() else {
            return;
        };
        if let Ok(mut held) = reporter.lock() {
            if !announced_already {
                held.on_entry_start(path.as_str(), accepted);
            }
            held.on_entry_complete(path.as_str(), true);
            held.on_ratio(total_accepted, compressed);
        }
    }

    /// Whether the caller has asked for the archive to be abandoned.
    pub(crate) fn cancellation_requested(&self) -> bool {
        self.progress
            .as_ref()
            .is_some_and(|reporter| reporter.lock().is_ok_and(|held| held.should_cancel()))
    }

    /// Sets the write options.
    ///
    /// Entries already accepted keep the options they were accepted under.
    /// Anything still waiting is written out with those before the next entry
    /// is taken, so a change here never reaches back over work already done.
    pub fn options(mut self, options: WriteOptions) -> Self {
        self.options = options.clone();
        self.active_options = std::sync::Arc::new(options);
        self
    }

    /// Refuses a password that differs from the one this archive is keyed on.
    ///
    /// Checked before an entry is accepted, so a caller learns while the entry
    /// is still theirs to reconsider. It only reports; the password is fixed
    /// when one is actually used to derive a key, so an entry that is rejected,
    /// or one that never gets encrypted such as a directory, leaves the archive
    /// free to be keyed on something else.
    #[cfg(feature = "aes")]
    pub(crate) fn check_password(&self, options: &WriteOptions) -> Result<()> {
        let (Some(held), Some(wanted)) = (&self.archive_password, &options.password) else {
            return Ok(());
        };
        if !options.is_encrypted() || held.as_utf16_le() == wanted.as_utf16_le() {
            return Ok(());
        }

        Err(Error::InvalidFormat(
            "the password cannot be changed once an entry has been \
             encrypted: one archive is opened with one password, and \
             entries written under different ones cannot all be read"
                .into(),
        ))
    }

    /// Records the password a key is about to be derived from.
    ///
    /// Called from the places that actually encrypt, so what is remembered is
    /// what the archive is really keyed on.
    #[cfg(feature = "aes")]
    pub(crate) fn hold_password(&mut self, password: &crate::crypto::Password) -> Result<()> {
        match &self.archive_password {
            Some(held) if held.as_utf16_le() != password.as_utf16_le() => {
                Err(Error::InvalidFormat(
                    "the password cannot be changed once an entry has been \
                     encrypted: one archive is opened with one password, and \
                     entries written under different ones cannot all be read"
                        .into(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.archive_password = Some(password.clone());
                Ok(())
            }
        }
    }

    /// Writes out anything waiting under options that are no longer current.
    ///
    /// Called before an entry is accepted, so the buffers never hold work from
    /// two different sets of options.
    pub(crate) fn settle_stale_buffers(&mut self) -> Result<()> {
        let stale = self
            .pending_batch
            .first()
            .or_else(|| self.solid_buffer.first())
            .is_some_and(|entry| !std::sync::Arc::ptr_eq(&entry.options, &self.active_options));

        if stale {
            self.flush_buffered_entries()?;
        }
        Ok(())
    }

    /// Writes out every entry waiting to be compressed, in the order they came.
    pub(crate) fn flush_buffered_entries(&mut self) -> Result<()> {
        self.flush_pending_batch()?;
        if !self.solid_buffer.is_empty() {
            self.flush_solid_buffer()?;
        }
        Ok(())
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
    /// use zesven::write::Writer;
    /// use std::io::Cursor;
    ///
    /// let mut writer = Writer::create(Cursor::new(Vec::new()))?;
    /// writer.add_bytes("test.txt".try_into()?, b"Hello")?;
    /// let (result, cursor) = writer.finish_into_inner()?;
    /// let archive_bytes = cursor.into_inner();
    /// ```
    pub fn finish_into_inner(mut self) -> Result<(WriteResult, W)> {
        // Also rejects a writer poisoned by a partial write, so a failure
        // cannot be turned into an archive by ignoring its error.
        self.ensure_accepting_entries()?;
        // The header is encrypted with the archive's password, which must be
        // the one its entries were encrypted with.
        #[cfg(feature = "aes")]
        {
            let options = self.options.clone();
            self.check_password(&options)?;
        }
        self.state = WriterState::Building;

        // Everything still waiting, written with the options it was accepted
        // under and in the order it arrived.
        self.flush_buffered_entries()?;
        // And whatever was left running behind it. The header is built from the
        // entry list, so an entry still being compressed has to be in it before
        // that list is read - not merely before the file is closed.
        self.settle_ahead()?;

        let header_data = self.encode_header()?;

        // An encrypted header is stored as a packed stream in the data area,
        // followed by the small structure that describes it. The structure is the
        // archive's next header, so it must be written last and its position is
        // what the signature header points at.
        #[cfg(feature = "aes")]
        if self.options.is_header_encrypted() {
            let payload_pos = self.sink.stream_position().map_err(Error::Io)?;
            let nonce = self.nonce_for_stream()?;
            let (payload, structure) = self.encode_encrypted_header(
                &header_data,
                payload_pos - self.start_pos - SIGNATURE_HEADER_SIZE,
                nonce,
            )?;

            self.sink.write_all(&payload).map_err(Error::Io)?;
            let header_pos = self.sink.stream_position().map_err(Error::Io)?;
            self.sink.write_all(&structure).map_err(Error::Io)?;
            let archive_len = self.sink.stream_position().map_err(Error::Io)?;
            self.write_signature_header(header_pos, &structure)?;

            return self.finish_state(archive_len);
        }

        // A plain header is written compressed when that is smaller, in the same
        // shape as the encrypted one: payload in the data area, structure last.
        #[cfg(feature = "lzma2")]
        {
            let payload_pos = self.sink.stream_position().map_err(Error::Io)?;
            if let Some((payload, structure)) = super::header_compression::encode_compressed_header(
                &header_data,
                payload_pos - self.start_pos - SIGNATURE_HEADER_SIZE,
            )? {
                self.sink.write_all(&payload).map_err(Error::Io)?;
                let header_pos = self.sink.stream_position().map_err(Error::Io)?;
                self.sink.write_all(&structure).map_err(Error::Io)?;
                let archive_len = self.sink.stream_position().map_err(Error::Io)?;
                self.write_signature_header(header_pos, &structure)?;

                return self.finish_state(archive_len);
            }
        }

        let header_pos = self.sink.stream_position().map_err(Error::Io)?;
        self.sink.write_all(&header_data).map_err(Error::Io)?;
        let archive_len = self.sink.stream_position().map_err(Error::Io)?;

        // Write signature header at start
        self.write_signature_header(header_pos, &header_data)?;

        self.finish_state(archive_len)
    }

    /// Marks the writer finished and builds the write result.
    ///
    /// `archive_len` is where writing ended, taken before the signature header
    /// was written: that seeks back to the start, so asking the sink afterwards
    /// reported the 32 bytes of the signature as the size of the archive.
    fn finish_state(mut self, archive_len: u64) -> Result<(WriteResult, W)> {
        // The signature header is written last, over the start of the archive,
        // and a buffered sink may still be holding it. Flushing here rather
        // than leaving it to `Drop` is what turns a failed write into an error
        // the caller sees: `BufWriter::drop` discards the result.
        self.sink.flush().map_err(Error::Io)?;

        self.state = WriterState::Finished;

        // Build result
        let result = WriteResult {
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

    /// Writes the signature header at the start of the file.
    pub(crate) fn write_signature_header(
        &mut self,
        header_pos: u64,
        header_data: &[u8],
    ) -> Result<()> {
        // Calculate values
        let next_header_offset = header_pos - self.start_pos - SIGNATURE_HEADER_SIZE;
        let next_header_size = header_data.len() as u64;
        let next_header_crc = crc32fast::hash(header_data);

        // Build start header (20 bytes)
        let mut start_header = Vec::with_capacity(20);
        start_header.extend_from_slice(&next_header_offset.to_le_bytes());
        start_header.extend_from_slice(&next_header_size.to_le_bytes());
        start_header.extend_from_slice(&next_header_crc.to_le_bytes());

        let start_header_crc = crc32fast::hash(&start_header);

        // Seek to where the archive starts, which is not necessarily the start
        // of the sink.
        let start = self.start_pos;
        self.sink.seek(SeekFrom::Start(start)).map_err(Error::Io)?;

        // Write signature (6 bytes)
        self.sink.write_all(SIGNATURE).map_err(Error::Io)?;

        // Write version (2 bytes)
        self.sink.write_all(&[0x00, 0x04]).map_err(Error::Io)?;

        // Write start header CRC (4 bytes)
        self.sink
            .write_all(&start_header_crc.to_le_bytes())
            .map_err(Error::Io)?;

        // Write start header (20 bytes)
        self.sink.write_all(&start_header).map_err(Error::Io)?;

        Ok(())
    }

    /// The header's view of what this writer has produced.
    pub(crate) fn header_model(&self) -> super::header_encode::HeaderModel<'_> {
        super::header_encode::HeaderModel {
            stream_info: &self.stream_info,
            entries: &self.entries,
            options: &self.options,
        }
    }

    /// Encodes the archive header from that view.
    pub(crate) fn encode_header(&self) -> Result<Vec<u8>> {
        self.header_model().encode_header()
    }

    /// Writes to the sink, marking the writer unusable if it fails.
    ///
    /// Once any of an entry's bytes are in the sink, a failure has left data
    /// that belongs to no folder, and every folder written after it would be
    /// found at the wrong offset. Every write of entry data goes through here
    /// so that no path can quietly leave the writer usable.
    pub(crate) fn write_entry_bytes(&mut self, data: &[u8]) -> Result<()> {
        // Whatever was accepted before these bytes goes in front of them. Here
        // rather than at each caller because every path that puts an entry's
        // data in the sink ends up here, and one that forgot would write its
        // folder into the middle of an earlier entry's stream.
        self.settle_ahead()?;
        match self.sink.write_all(data) {
            Ok(()) => Ok(()),
            Err(e) => self.fail(Error::Io(e)),
        }
    }

    /// Marks the writer unusable and returns the error that caused it.
    ///
    /// For failures that happen once bytes are already in the sink: whatever
    /// was written belongs to no folder, and every folder after it would be
    /// found at the wrong offset.
    pub(crate) fn fail<T>(&mut self, error: Error) -> Result<T> {
        self.state = WriterState::Failed;
        // Nothing outstanding is going to be written now, and a thread left
        // running against a writer that has given up holds everything it was
        // given until the process ends.
        self.abandon_ahead();
        // Whatever was announced and never finished ends here. A batch is
        // announced before it is compressed, so a failure in between leaves a
        // caller showing entries as still running - with no further call coming
        // - for as long as it keeps the reporter.
        let outstanding = std::mem::take(&mut self.announced);
        if let Some(mut held) = self.progress.as_ref().and_then(|held| held.lock().ok()) {
            for name in outstanding {
                held.on_entry_complete(&name, false);
            }
        }
        Err(error)
    }

    /// Ensures the writer is in the AcceptingEntries state.
    pub(crate) fn ensure_accepting_entries(&self) -> Result<()> {
        if self.state == WriterState::Failed {
            return Err(Error::InvalidFormat(
                "an earlier entry failed partway through writing; \
                 this archive cannot be completed"
                    .into(),
            ));
        }
        if self.state != WriterState::AcceptingEntries {
            return Err(Error::InvalidFormat(
                "Writer is not accepting entries".into(),
            ));
        }
        // Asked between entries rather than during one: an entry already being
        // compressed is bytes on their way to the sink, and stopping partway
        // through would leave an archive that has to be thrown away rather than
        // one that simply ends early.
        if self.cancellation_requested() {
            return Err(Error::Cancelled);
        }

        self.options.validate()?;

        Ok(())
    }
}
