//! The source an archive opened from a path reads through.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use crate::volume::MultiVolumeReader;

/// Where an archive opened by path reads its bytes from.
///
/// A 7z archive on disk is either one file or a numbered set of volumes, and a
/// packed stream may run from one volume into the next. Opening a set as a
/// single file reads the header correctly and then truncates at the first volume
/// boundary, which surfaces as a corrupt-looking archive rather than as the
/// wrong reader having been chosen; the two cases share one type so that callers
/// do not have to know which they have.
#[derive(Debug)]
pub enum ArchiveSource {
    /// A single archive file.
    Single(BufReader<File>),
    /// A numbered set of volumes read as one continuous stream.
    Volumes(Box<MultiVolumeReader>),
}

impl Read for ArchiveSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Single(reader) => reader.read(buf),
            Self::Volumes(reader) => reader.read(buf),
        }
    }
}

impl Seek for ArchiveSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Single(reader) => reader.seek(pos),
            Self::Volumes(reader) => reader.seek(pos),
        }
    }
}
