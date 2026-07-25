//! Compressed header encoding.
//!
//! A plain header lists every entry's name, size, timestamps and attributes, so
//! it grows with the archive and compresses extremely well. 7-Zip compresses it
//! by default; leaving it raw meant that for a few hundred small files the
//! header was most of the archive.

#[cfg(feature = "lzma2")]
use std::io::{Seek, Write};

#[cfg(feature = "lzma2")]
use crate::Result;
#[cfg(feature = "lzma2")]
use crate::format::property_id;
#[cfg(feature = "lzma2")]
use crate::format::reader::write_variable_u64;

#[cfg(feature = "lzma2")]
use super::Writer;

/// Headers below this size are left alone.
///
/// Compressing a header costs a coder chain of roughly forty bytes to describe,
/// so for a small one the wrapper is most of what is saved.
#[cfg(feature = "lzma2")]
const COMPRESSION_THRESHOLD: usize = 512;

#[cfg(feature = "lzma2")]
impl<W: Write + Seek> Writer<W> {
    /// Compresses a header, returning `(payload, structure)`.
    ///
    /// The payload is an ordinary packed stream that belongs in the data area;
    /// the structure is the ENCODED_HEADER that points at it and becomes the
    /// archive's next header. `pack_pos` is the payload's offset relative to the
    /// end of the signature header.
    ///
    /// Returns `None` when compressing would not pay for itself.
    pub(crate) fn encode_compressed_header(
        &self,
        plain_header: &[u8],
        pack_pos: u64,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        use crate::codec::method;
        use crate::write::header_encryption::lzma2_dictionary_property;

        if plain_header.len() < COMPRESSION_THRESHOLD {
            return Ok(None);
        }

        let compressed = {
            use lzma_rust2::{Lzma2Options, Lzma2Writer};

            let mut compressed = Vec::new();
            let options = Lzma2Options::with_preset(5);
            let mut encoder = Lzma2Writer::new(&mut compressed, options);
            encoder.write_all(plain_header).map_err(crate::Error::Io)?;
            encoder.finish().map_err(crate::Error::Io)?;
            compressed
        };

        // Describing the compressed stream costs bytes of its own; if the result
        // is not smaller than what it replaces, write the header as it is.
        if compressed.len() >= plain_header.len() {
            return Ok(None);
        }

        let mut encoded = Vec::new();

        encoded.push(property_id::ENCODED_HEADER);

        // PackInfo: one stream, at pack_pos, of the compressed length.
        encoded.push(property_id::PACK_INFO);
        write_variable_u64(&mut encoded, pack_pos)?;
        write_variable_u64(&mut encoded, 1)?;
        encoded.push(property_id::SIZE);
        write_variable_u64(&mut encoded, compressed.len() as u64)?;
        encoded.push(property_id::END);

        // UnpackInfo: a single LZMA2 coder producing the original header.
        encoded.push(property_id::UNPACK_INFO);
        encoded.push(property_id::FOLDER);
        write_variable_u64(&mut encoded, 1)?;
        encoded.push(0); // external = 0

        encoded.push(0x01); // one coder
        let lzma2_props = [lzma2_dictionary_property(plain_header.len() as u64)];
        encoded.push((method::LZMA2.len() as u8) | 0x20);
        encoded.extend_from_slice(method::LZMA2);
        write_variable_u64(&mut encoded, lzma2_props.len() as u64)?;
        encoded.extend_from_slice(&lzma2_props);

        encoded.push(property_id::CODERS_UNPACK_SIZE);
        write_variable_u64(&mut encoded, plain_header.len() as u64)?;

        // The checksum of what the stream decodes to, so a reader can tell a
        // damaged header from a valid one before trying to parse it.
        encoded.push(property_id::CRC);
        encoded.push(1); // all defined
        encoded.extend_from_slice(&crc32fast::hash(plain_header).to_le_bytes());

        encoded.push(property_id::END); // End UnpackInfo
        encoded.push(property_id::END); // End streams

        Ok(Some((compressed, encoded)))
    }
}
