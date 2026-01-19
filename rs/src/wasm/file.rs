//! Browser file access abstraction
//!
//! Provides a wrapper around web_sys::File that implements Read + Seek
//! for use with the zesven archive APIs.

use js_sys::Uint8Array;
use std::io::{self, Read, Seek, SeekFrom};
use web_sys::File;

/// Wrapper around web_sys::File that implements Read + Seek
#[allow(dead_code)] // Used for future browser File API integration
///
/// Since WASM cannot perform synchronous file reads with seeking,
/// the file must be loaded into memory first via `ensure_loaded()`.
pub struct JsFile {
    /// The original web_sys::File (kept for metadata access)
    #[allow(dead_code)] // File reference kept for potential future metadata queries
    file: File,
    /// Current read position
    pos: u64,
    /// Total file size
    size: u64,
    /// Cached file content (required for seeking)
    buffer: Option<Vec<u8>>,
}

#[allow(dead_code)] // Methods used for future browser File API integration
impl JsFile {
    /// Create a new JsFile wrapper
    ///
    /// Note: You must call `ensure_loaded()` before using Read/Seek operations.
    pub fn new(file: File) -> Self {
        let size = file.size() as u64;
        Self {
            file,
            pos: 0,
            size,
            buffer: None,
        }
    }

    /// Create from pre-loaded buffer (for Uint8Array data)
    pub fn from_buffer(buffer: Vec<u8>) -> Self {
        let size = buffer.len() as u64;
        Self {
            file: File::new_with_u8_array_sequence(&js_sys::Array::new(), "")
                .expect("Failed to create dummy File"),
            pos: 0,
            size,
            buffer: Some(buffer),
        }
    }

    /// Get the file size
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Check if the file has been loaded into memory
    pub fn is_loaded(&self) -> bool {
        self.buffer.is_some()
    }

    /// Set the buffer directly (used by async loading)
    pub fn set_buffer(&mut self, buffer: Vec<u8>) {
        self.size = buffer.len() as u64;
        self.buffer = Some(buffer);
    }

    /// Get a reference to the buffer if loaded
    pub fn buffer(&self) -> Option<&[u8]> {
        self.buffer.as_deref()
    }
}

impl Read for JsFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let buffer = self.buffer.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                "File not loaded - call ensure_loaded() first or use from_buffer()",
            )
        })?;

        let start = self.pos as usize;
        if start >= buffer.len() {
            return Ok(0);
        }

        let end = (start + buf.len()).min(buffer.len());
        let len = end - start;

        buf[..len].copy_from_slice(&buffer[start..end]);
        self.pos = end as u64;

        Ok(len)
    }
}

impl Seek for JsFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.size as i64 + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };

        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Seek before start of file",
            ));
        }

        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

/// Create a JsFile from a Uint8Array
#[allow(dead_code)] // Used for future browser File API integration
pub fn js_file_from_uint8_array(data: &Uint8Array) -> JsFile {
    let mut buffer = vec![0u8; data.length() as usize];
    data.copy_to(&mut buffer);
    JsFile::from_buffer(buffer)
}

/// Utility to convert a Uint8Array to a Vec<u8>
pub fn uint8_array_to_vec(data: &Uint8Array) -> Vec<u8> {
    let mut buffer = vec![0u8; data.length() as usize];
    data.copy_to(&mut buffer);
    buffer
}

/// Utility to convert a Vec<u8> to a Uint8Array
pub fn vec_to_uint8_array(data: &[u8]) -> Uint8Array {
    let array = Uint8Array::new_with_length(data.len() as u32);
    array.copy_from(data);
    array
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js_file_from_buffer() {
        let data = vec![1, 2, 3, 4, 5];
        let mut file = JsFile::from_buffer(data.clone());

        assert!(file.is_loaded());
        assert_eq!(file.size(), 5);

        let mut buf = [0u8; 3];
        assert_eq!(file.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf, &[1, 2, 3]);

        assert_eq!(file.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], &[4, 5]);

        assert_eq!(file.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_js_file_seek() {
        let data = vec![1, 2, 3, 4, 5];
        let mut file = JsFile::from_buffer(data);

        assert_eq!(file.seek(SeekFrom::Start(2)).unwrap(), 2);
        let mut buf = [0u8; 1];
        assert_eq!(file.read(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], 3);

        assert_eq!(file.seek(SeekFrom::End(-2)).unwrap(), 3);
        assert_eq!(file.read(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], 4);

        assert_eq!(file.seek(SeekFrom::Current(-1)).unwrap(), 3);
        assert_eq!(file.read(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], 4);
    }

    #[test]
    fn test_js_file_seek_before_start() {
        let data = vec![1, 2, 3];
        let mut file = JsFile::from_buffer(data);

        assert!(file.seek(SeekFrom::Start(0)).is_ok());
        assert!(file.seek(SeekFrom::Current(-1)).is_err());
    }
}
