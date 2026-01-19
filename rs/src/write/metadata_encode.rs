//! Metadata encoding for archive entries.
//!
//! This module provides functions for encoding file names, comments, and timestamps
//! in the 7z archive format.

use std::io::{Seek, Write};

use super::encoding_utils::encode_bool_vector;
use super::{PendingEntry, Writer};

impl<W: Write + Seek> Writer<W> {
    /// Encodes file names as UTF-16LE.
    ///
    /// Each name is followed by a null terminator (two zero bytes).
    pub(crate) fn encode_names(&self) -> Vec<u8> {
        let mut data = Vec::new();
        for entry in &self.entries {
            for c in entry.path.as_str().encode_utf16() {
                data.extend_from_slice(&c.to_le_bytes());
            }
            // Null terminator
            data.extend_from_slice(&[0, 0]);
        }
        data
    }

    /// Encodes an archive comment as UTF-16LE with external flag.
    ///
    /// The comment is prefixed with an external flag (0 = inline) and
    /// followed by a null terminator.
    pub(crate) fn encode_comment(&self, comment: &str) -> Vec<u8> {
        let mut data = Vec::new();
        // External flag = 0 (inline data)
        data.push(0);
        // UTF-16LE encoded string
        for c in comment.encode_utf16() {
            data.extend_from_slice(&c.to_le_bytes());
        }
        // Null terminator
        data.extend_from_slice(&[0, 0]);
        data
    }

    /// Encodes timestamps for entries.
    ///
    /// Takes a predicate function to extract the timestamp from each entry,
    /// and a boolean vector indicating which entries have the timestamp defined.
    pub(crate) fn encode_times<F>(&self, defined: &[bool], getter: F) -> Vec<u8>
    where
        F: Fn(&PendingEntry) -> Option<u64>,
    {
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
        for entry in &self.entries {
            if let Some(time) = getter(entry) {
                data.extend_from_slice(&time.to_le_bytes());
            }
        }

        data
    }
}
