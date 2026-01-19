//! Encrypted header encoding.
//!
//! This module provides functions for encoding encrypted archive headers
//! using LZMA2 compression and AES-256 encryption.

#[cfg(feature = "aes")]
use std::io::{Seek, Write};

#[cfg(feature = "aes")]
use crate::format::property_id;
#[cfg(feature = "aes")]
use crate::format::reader::write_variable_u64;
#[cfg(feature = "aes")]
use crate::{Error, Result};

#[cfg(feature = "aes")]
use super::Writer;

#[cfg(feature = "aes")]
impl<W: Write + Seek> Writer<W> {
    /// Encodes an encrypted header.
    ///
    /// This wraps the plain header in an ENCODED_HEADER structure with:
    /// 1. LZMA2 compression
    /// 2. AES-256 encryption
    ///
    /// The result is:
    /// - ENCODED_HEADER marker
    /// - StreamsInfo describing the AES + LZMA2 coders
    /// - The encrypted, compressed header data
    pub(crate) fn encode_encrypted_header(&self, plain_header: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::method;
        use crate::crypto::{Aes256Encoder, AesProperties, derive_key};

        let password =
            self.options.password.as_ref().ok_or_else(|| {
                Error::InvalidFormat("header encryption requires a password".into())
            })?;

        // Step 1: Compress the header with LZMA2
        let compressed = {
            use lzma_rust2::{Lzma2Options, Lzma2Writer};

            let mut compressed = Vec::new();
            let options = Lzma2Options::with_preset(5);
            let mut encoder = Lzma2Writer::new(&mut compressed, options);
            encoder.write_all(plain_header).map_err(Error::Io)?;
            encoder.finish().map_err(Error::Io)?;
            compressed
        };

        // Step 2: Encrypt the compressed data with AES-256
        let (salt, iv) = self.options.nonce_policy.generate()?;
        let key = derive_key(
            password,
            &salt,
            self.options.nonce_policy.num_cycles_power(),
        )?;

        let encrypted = {
            let mut output = Vec::new();
            let mut encoder = Aes256Encoder::with_key_iv(&mut output, key, iv);
            encoder.write_all(&compressed).map_err(Error::Io)?;
            encoder.finish().map_err(Error::Io)?;
            output
        };

        // Step 3: Build the ENCODED_HEADER structure
        let mut encoded = Vec::new();

        // ENCODED_HEADER marker
        encoded.push(property_id::ENCODED_HEADER);

        // PackInfo
        encoded.push(property_id::PACK_INFO);
        write_variable_u64(&mut encoded, 0)?; // pack_pos = 0 (relative to this stream)
        write_variable_u64(&mut encoded, 1)?; // num_pack_streams = 1

        // Pack size
        encoded.push(property_id::SIZE);
        write_variable_u64(&mut encoded, encrypted.len() as u64)?;
        encoded.push(property_id::END);

        // UnpackInfo
        encoded.push(property_id::UNPACK_INFO);
        encoded.push(property_id::FOLDER);
        write_variable_u64(&mut encoded, 1)?; // num_folders = 1
        encoded.push(0); // external = 0 (inline)

        // Folder with 2 coders: AES-256 (decryption) -> LZMA2 (decompression)
        // 7z coder chain order: coders[1] applied first, coders[0] applied second
        // For encrypted headers: packed -> AES decrypt (coder 1) -> LZMA2 decompress (coder 0) -> plain
        //
        // Number of coders
        encoded.push(0x02);

        // Coder 0: LZMA2 (outer - decompression, applied SECOND when reading)
        // LZMA2 properties: dictionary size encoded as single byte
        // For level 5, dictionary size is typically 16 MB (0x18 = 24)
        let lzma2_props = [0x18u8]; // 16 MB dictionary
        // LZMA2 method ID: 0x21
        let lzma2_flags = (method::LZMA2.len() as u8) | 0x20; // 1 byte + has properties
        encoded.push(lzma2_flags);
        encoded.extend_from_slice(method::LZMA2);
        write_variable_u64(&mut encoded, lzma2_props.len() as u64)?;
        encoded.extend_from_slice(&lzma2_props);

        // Coder 1: AES-256 (inner - decryption, applied FIRST when reading)
        let aes_props =
            AesProperties::encode(self.options.nonce_policy.num_cycles_power(), &salt, &iv);
        // AES method ID: 0x06, 0xF1, 0x07, 0x01
        let aes_flags = (method::AES.len() as u8) | 0x20; // 4 bytes + has properties
        encoded.push(aes_flags);
        encoded.extend_from_slice(method::AES);
        write_variable_u64(&mut encoded, aes_props.len() as u64)?;
        encoded.extend_from_slice(&aes_props);

        // BindPair: connect AES output (stream 1) to LZMA2 input (stream 0)
        // For a 2-coder chain, we have 1 bind pair
        // The inner coder (AES) output goes to the outer coder (LZMA2) input
        write_variable_u64(&mut encoded, 0)?; // in_index (LZMA2's input, stream 0)
        write_variable_u64(&mut encoded, 1)?; // out_index (AES's output, stream 1)

        // Unpack sizes (for both coders' outputs)
        // Coder 0 (LZMA2) output = final uncompressed size
        // Coder 1 (AES) output = compressed size (LZMA2 output)
        encoded.push(property_id::CODERS_UNPACK_SIZE);
        write_variable_u64(&mut encoded, plain_header.len() as u64)?; // LZMA2 output = final size
        write_variable_u64(&mut encoded, compressed.len() as u64)?; // AES output = compressed size

        encoded.push(property_id::END); // End UnpackInfo
        encoded.push(property_id::END); // End streams (no SubStreamsInfo needed)

        // Append the encrypted data
        encoded.extend_from_slice(&encrypted);

        Ok(encoded)
    }
}
