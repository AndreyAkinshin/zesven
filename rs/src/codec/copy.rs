//! Copy codec (no compression).

use std::io::{self, Read};

use super::{Decoder, method};

/// A decoder that passes data through unchanged (no compression).
pub struct CopyDecoder<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read + Send> CopyDecoder<R> {
    /// Creates a new copy decoder.
    ///
    /// # Arguments
    ///
    /// * `inner` - The data source
    /// * `size` - Expected size of the data
    pub fn new(inner: R, size: u64) -> Self {
        Self {
            inner,
            remaining: size,
        }
    }
}

impl<R: Read + Send> Read for CopyDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }

        let max_read = (self.remaining as usize).min(buf.len());
        let n = self.inner.read(&mut buf[..max_read])?;
        self.remaining = self.remaining.saturating_sub(n as u64);
        Ok(n)
    }
}

impl<R: Read + Send> Decoder for CopyDecoder<R> {
    fn method_id(&self) -> &'static [u8] {
        method::COPY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_copy_full_read() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let mut decoder = CopyDecoder::new(cursor, data.len() as u64);

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
    }

    #[test]
    fn test_copy_partial_read() {
        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let mut decoder = CopyDecoder::new(cursor, 5); // Only read "Hello"

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"Hello");
    }

    #[test]
    fn test_copy_empty() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let mut decoder = CopyDecoder::new(cursor, 0);

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn test_copy_method_id() {
        let cursor = Cursor::new(Vec::<u8>::new());
        let decoder = CopyDecoder::new(cursor, 0);
        assert_eq!(decoder.method_id(), method::COPY);
    }
}
