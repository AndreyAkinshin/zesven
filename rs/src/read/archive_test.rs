//! Archive integrity testing.
//!
//! This module provides methods for testing archive integrity by verifying
//! CRC checksums without extraction.

use std::io::{Read, Seek, SeekFrom};

use crate::streaming::Crc32Sink;
use crate::{Error, Result};

use super::{Archive, EntrySelector, ExtractionLimits, TestOptions, TestResult};

impl<R: Read + Seek> Archive<R> {
    /// Tests the archive for integrity.
    ///
    /// This decompresses all selected entries and verifies their CRC checksums
    /// without writing any files.
    ///
    /// # Arguments
    ///
    /// * `selector` - Selects which entries to test
    /// * `options` - Test options
    ///
    /// # Returns
    ///
    /// A TestResult containing the results of the integrity check.
    pub fn test(
        &mut self,
        selector: impl EntrySelector,
        _options: &TestOptions,
    ) -> Result<TestResult> {
        let mut result = TestResult::default();

        // Collect entries to test
        let entries_to_test: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selector.select(e))
            .map(|(idx, _)| idx)
            .collect();

        for idx in entries_to_test {
            let entry = &self.entries[idx];
            let entry_path = entry.path.as_str().to_string();

            result.entries_tested += 1;

            // Directories always pass
            if entry.is_directory {
                result.entries_passed += 1;
                continue;
            }

            // Test this entry
            match self.test_entry_by_index(idx) {
                Ok(()) => {
                    result.entries_passed += 1;
                }
                Err(e) => {
                    result.entries_failed += 1;
                    result.failures.push((entry_path, e.to_string()));
                }
            }
        }

        Ok(result)
    }

    /// Tests a single entry by decompressing and verifying CRC.
    pub(crate) fn test_entry_by_index(&mut self, entry_idx: usize) -> Result<()> {
        let entry_size = self.entries[entry_idx].size;
        let entry_crc = self.entries[entry_idx].crc32;
        let folder_index = self.entries[entry_idx].folder_index;
        let stream_index = self.entries[entry_idx].stream_index;

        // Empty files always pass
        let folder_idx = match folder_index {
            Some(idx) => idx,
            None => return Ok(()),
        };

        // Get folder and pack info - clone folder to release borrow before mutable operations
        let (folder, pack_size) = {
            let unpack_info = self
                .header
                .unpack_info
                .as_ref()
                .ok_or_else(|| Error::InvalidFormat("missing unpack info".into()))?;

            let folder = unpack_info
                .folders
                .get(folder_idx)
                .ok_or_else(|| {
                    Error::InvalidFormat(format!("folder index {} out of range", folder_idx))
                })?
                .clone();

            let pack_info = self
                .header
                .pack_info
                .as_ref()
                .ok_or_else(|| Error::InvalidFormat("missing pack info".into()))?;

            let pack_size = pack_info
                .pack_sizes
                .get(folder_idx)
                .copied()
                .ok_or_else(|| Error::InvalidFormat("missing pack size".into()))?;

            (folder, pack_size)
        };

        // Calculate pack position
        let pack_pos = self.calculate_pack_position(folder_idx)?;

        // Seek and read packed data
        self.reader
            .seek(SeekFrom::Start(pack_pos))
            .map_err(Error::Io)?;
        let mut packed_data = vec![0u8; pack_size as usize];
        self.reader
            .read_exact(&mut packed_data)
            .map_err(Error::Io)?;

        // Decompress to CRC sink
        let mut sink = Crc32Sink::new();

        // Use unlimited limits for test operations (CRC verification only)
        let limits = ExtractionLimits::unlimited();

        // Dispatch to appropriate decompression path
        #[cfg(feature = "lzma")]
        if folder.uses_bcj2() {
            // BCJ2 requires special multi-stream decompression (LZMA feature only)
            self.extract_bcj2(&folder, folder_idx, stream_index, &mut sink, &limits)?;
        } else {
            self.decompress_standard_entry(
                packed_data,
                &folder,
                folder_idx,
                stream_index,
                entry_size,
                &mut sink,
                &limits,
            )?;
        }

        #[cfg(not(feature = "lzma"))]
        self.decompress_standard_entry(
            packed_data,
            &folder,
            folder_idx,
            stream_index,
            entry_size,
            &mut sink,
            &limits,
        )?;

        // Verify CRC if available
        if let Some(expected_crc) = entry_crc {
            let actual_crc = sink.finalize();
            if actual_crc != expected_crc {
                return Err(Error::CrcMismatch {
                    entry_index: entry_idx,
                    entry_name: Some(self.entries[entry_idx].path.as_str().to_string()),
                    expected: expected_crc,
                    actual: actual_crc,
                });
            }
        }

        Ok(())
    }
}
