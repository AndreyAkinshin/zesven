//! Compression codec infrastructure for 7z archives.
//!
//! This module provides the abstraction layer for compression codecs
//! and integrates LZMA/LZMA2 support.

#[cfg(feature = "lzma")]
pub mod lzma;

#[cfg(all(feature = "lzma", feature = "parallel"))]
pub mod lzma2_parallel;

#[cfg(feature = "fast-lzma2")]
pub mod fast_lzma2;

#[cfg(feature = "fast-lzma2")]
pub mod fast_lzma2_encode;

// Internal fast-lzma2 implementation modules (not part of public API)
#[cfg(feature = "fast-lzma2")]
pub(crate) mod lzma_rc;

#[cfg(feature = "fast-lzma2")]
pub(crate) mod lzma_context;

#[cfg(feature = "fast-lzma2")]
pub(crate) mod radix_mf;

#[cfg(feature = "deflate")]
pub mod deflate;

#[cfg(feature = "bzip2")]
pub mod bzip2;

#[cfg(feature = "ppmd")]
pub mod ppmd;

#[cfg(feature = "lz4")]
pub mod lz4;

#[cfg(feature = "zstd")]
pub mod zstd;

#[cfg(feature = "brotli")]
pub mod brotli;

// LZ5 and Lizard codecs (pure Rust implementations)
pub mod lizard;
pub mod lz5;

// Parallel frame-based compression
pub mod parallel_frames;

#[cfg(feature = "lzma")]
pub mod filters;

// BCJ filter encoders (pure Rust implementations)
pub mod bcj_encoders;

pub mod bcj2;

mod copy;

use crate::{Error, Result};
#[allow(unused_imports)]
use std::io::{self, Cursor, Read, Write};

#[allow(unused_imports)]
use crate::format::streams::{Coder, Folder};

/// A decoder that reads compressed data and produces uncompressed output.
pub trait Decoder: Read + Send {
    /// Returns the method ID for this decoder.
    fn method_id(&self) -> &'static [u8];
}

/// An encoder that takes uncompressed data and produces compressed output.
pub trait Encoder: Write + Send {
    /// Returns the method ID for this encoder.
    fn method_id(&self) -> &'static [u8];

    /// Finishes encoding and flushes any remaining data.
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// Copy decoder (no compression).
pub use copy::CopyDecoder;

#[cfg(feature = "lzma")]
pub use lzma::{
    Lzma2Decoder, Lzma2Encoder, Lzma2EncoderOptions, LzmaDecoder, LzmaEncoder, LzmaEncoderOptions,
};

#[cfg(all(feature = "lzma", feature = "parallel"))]
pub use lzma::Lzma2DecoderMt;

#[cfg(all(feature = "lzma", feature = "parallel"))]
pub use lzma2_parallel::{
    Lzma2CompressionResult, ParallelLzma2Encoder, ParallelLzma2Options,
    StreamingParallelLzma2Encoder,
};

#[cfg(feature = "deflate")]
pub use deflate::{DeflateDecoder, DeflateEncoder, DeflateEncoderOptions};

#[cfg(feature = "bzip2")]
pub use bzip2::{Bzip2Decoder, Bzip2Encoder, Bzip2EncoderOptions};

#[cfg(feature = "ppmd")]
pub use ppmd::{PpmdDecoder, PpmdEncoder, PpmdEncoderOptions, SizedPpmdDecoder};

#[cfg(feature = "lz4")]
pub use lz4::{Lz4Decoder, Lz4Encoder, Lz4EncoderOptions};

#[cfg(feature = "zstd")]
pub use self::zstd::{
    ZstdDecoderWithDict, ZstdDictionary, ZstdEncoderOptions, ZstdEncoderOptionsWithDict,
    ZstdEncoderWithDict, ZstdStreamDecoder, ZstdStreamEncoder,
};

#[cfg(feature = "brotli")]
pub use brotli::{BrotliDecoder, BrotliEncoder, BrotliEncoderOptions};

// LZ5 and Lizard codec exports
pub use lizard::{LizardDecoder, LizardEncoder, LizardEncoderOptions};
pub use lz5::{Lz5Decoder, Lz5Encoder, Lz5EncoderOptions};

// Parallel frame exports
pub use parallel_frames::{
    FRAME_MAGIC, FrameCodec, FrameCompressionResult, FrameIndex, FrameInfo, ParallelFrameDecoder,
    ParallelFrameEncoder,
};

#[cfg(feature = "lzma")]
pub use filters::{
    BcjArm64Decoder, BcjArmDecoder, BcjArmThumbDecoder, BcjIa64Decoder, BcjPpcDecoder,
    BcjRiscvDecoder, BcjSparcDecoder, BcjX86Decoder, DeltaDecoder,
};

pub use bcj2::{Bcj2Decoder, Bcj2DecoderWrapper, Bcj2EncodedStreams, RangeDecoder, bcj2_encode};

/// Method IDs for compression algorithms.
pub mod method {
    /// Copy (no compression).
    pub const COPY: &[u8] = &[0x00];
    /// LZMA compression.
    pub const LZMA: &[u8] = &[0x03, 0x01, 0x01];
    /// LZMA2 compression.
    pub const LZMA2: &[u8] = &[0x21];
    /// Deflate compression.
    pub const DEFLATE: &[u8] = &[0x04, 0x01, 0x08];
    /// BZip2 compression.
    pub const BZIP2: &[u8] = &[0x04, 0x02, 0x02];
    /// PPMd compression.
    pub const PPMD: &[u8] = &[0x03, 0x04, 0x01];
    /// LZ4 compression.
    pub const LZ4: &[u8] = &[0x04, 0xF7, 0x11, 0x04];
    /// ZSTD compression.
    pub const ZSTD: &[u8] = &[0x04, 0xF7, 0x11, 0x01];
    /// Brotli compression.
    pub const BROTLI: &[u8] = &[0x04, 0xF7, 0x11, 0x02];
    /// LZ5 compression.
    pub const LZ5: &[u8] = &[0x04, 0xF7, 0x11, 0x05];
    /// Lizard compression.
    pub const LIZARD: &[u8] = &[0x04, 0xF7, 0x11, 0x06];
    /// BCJ (x86) filter.
    pub const BCJ_X86: &[u8] = &[0x03, 0x03, 0x01, 0x03];
    /// BCJ (ARM) filter.
    pub const BCJ_ARM: &[u8] = &[0x03, 0x03, 0x05, 0x01];
    /// BCJ (ARM64/AArch64) filter.
    pub const BCJ_ARM64: &[u8] = &[0x0A];
    /// BCJ (ARM Thumb) filter.
    pub const BCJ_ARM_THUMB: &[u8] = &[0x03, 0x03, 0x07, 0x01];
    /// BCJ (PowerPC) filter.
    pub const BCJ_PPC: &[u8] = &[0x03, 0x03, 0x02, 0x05];
    /// BCJ (SPARC) filter.
    pub const BCJ_SPARC: &[u8] = &[0x03, 0x03, 0x08, 0x05];
    /// BCJ (IA64) filter.
    pub const BCJ_IA64: &[u8] = &[0x03, 0x03, 0x04, 0x01];
    /// BCJ (RISC-V) filter.
    pub const BCJ_RISCV: &[u8] = &[0x0B];
    /// BCJ2 (4-stream x86) filter.
    pub const BCJ2: &[u8] = &[0x03, 0x03, 0x01, 0x1B];
    /// Delta filter.
    pub const DELTA: &[u8] = &[0x03];
    /// AES-256 encryption.
    pub const AES: &[u8] = &[0x06, 0xF1, 0x07, 0x01];

    /// Returns true if the method ID represents a filter (BCJ, Delta) rather than a codec.
    ///
    /// Filters are applied after decompression to reverse transformations like
    /// executable code preprocessing. They don't compress data themselves.
    pub fn is_filter(method_id: &[u8]) -> bool {
        matches!(
            method_id,
            BCJ_X86
                | BCJ_ARM
                | BCJ_ARM64
                | BCJ_ARM_THUMB
                | BCJ_PPC
                | BCJ_SPARC
                | BCJ_IA64
                | BCJ_RISCV
                | DELTA
        )
    }

    /// Returns a human-readable name for a method ID.
    pub fn name(id: &[u8]) -> &'static str {
        match id {
            COPY => "Copy",
            LZMA => "LZMA",
            LZMA2 => "LZMA2",
            DEFLATE => "Deflate",
            BZIP2 => "BZip2",
            PPMD => "PPMd",
            LZ4 => "LZ4",
            ZSTD => "ZSTD",
            BROTLI => "Brotli",
            LZ5 => "LZ5",
            LIZARD => "Lizard",
            BCJ_X86 => "BCJ (x86)",
            BCJ_ARM => "BCJ (ARM)",
            BCJ_ARM64 => "BCJ (ARM64)",
            BCJ_ARM_THUMB => "BCJ (ARM Thumb)",
            BCJ_PPC => "BCJ (PowerPC)",
            BCJ_SPARC => "BCJ (SPARC)",
            BCJ_IA64 => "BCJ (IA64)",
            BCJ_RISCV => "BCJ (RISC-V)",
            BCJ2 => "BCJ2",
            DELTA => "Delta",
            AES => "AES-256",
            _ => "Unknown",
        }
    }
}

/// Builds a decoder for a given coder specification.
///
/// # Arguments
///
/// * `input` - The compressed data source
/// * `coder` - Coder specification from the archive header
/// * `uncompressed_size` - Expected size of uncompressed output
///
/// # Errors
///
/// Returns an error if the compression method is unsupported.
pub(crate) fn build_decoder<R: Read + Send + 'static>(
    input: R,
    coder: &Coder,
    uncompressed_size: u64,
) -> Result<Box<dyn Decoder>> {
    let method_id = &coder.method_id;
    #[allow(unused_variables)]
    let properties = coder.properties.as_deref().unwrap_or(&[]);

    match method_id.as_slice() {
        method::COPY => Ok(Box::new(CopyDecoder::new(input, uncompressed_size))),

        #[cfg(feature = "lzma")]
        method::LZMA => {
            let decoder = lzma::LzmaDecoder::new(input, properties, uncompressed_size)?;
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "lzma")]
        method::LZMA2 => {
            let decoder = lzma::Lzma2Decoder::new(input, properties)?;
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "deflate")]
        method::DEFLATE => {
            let buf_reader = std::io::BufReader::new(input);
            let decoder = deflate::DeflateDecoder::new(buf_reader);
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "bzip2")]
        method::BZIP2 => {
            let decoder = bzip2::Bzip2Decoder::new(input);
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "ppmd")]
        method::PPMD => {
            // PPMd doesn't have an end-of-stream marker, so we need to use
            // SizedPpmdDecoder which stops after uncompressed_size bytes
            let decoder = ppmd::SizedPpmdDecoder::new(input, properties, uncompressed_size)?;
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "lz4")]
        method::LZ4 => {
            let decoder = lz4::Lz4Decoder::new(input);
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "zstd")]
        method::ZSTD => {
            let decoder = zstd::ZstdStreamDecoder::new(input)
                .map_err(|e| Error::InvalidFormat(format!("ZSTD init error: {}", e)))?;
            Ok(Box::new(decoder))
        }

        #[cfg(feature = "brotli")]
        method::BROTLI => {
            let decoder = brotli::BrotliDecoder::new(input);
            Ok(Box::new(decoder))
        }

        // LZ5 - pure Rust implementation (no external dependencies)
        method::LZ5 => {
            let decoder = lz5::Lz5Decoder::new(input);
            Ok(Box::new(decoder))
        }

        // Lizard - pure Rust implementation (no external dependencies)
        method::LIZARD => {
            let decoder = lizard::LizardDecoder::new(input);
            Ok(Box::new(decoder))
        }

        // BCJ filters
        #[cfg(feature = "lzma")]
        method::BCJ_X86 => Ok(Box::new(filters::BcjX86Decoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_ARM => Ok(Box::new(filters::BcjArmDecoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_ARM64 => Ok(Box::new(filters::BcjArm64Decoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_ARM_THUMB => Ok(Box::new(filters::BcjArmThumbDecoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_PPC => Ok(Box::new(filters::BcjPpcDecoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_SPARC => Ok(Box::new(filters::BcjSparcDecoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_IA64 => Ok(Box::new(filters::BcjIa64Decoder::new(input))),

        #[cfg(feature = "lzma")]
        method::BCJ_RISCV => Ok(Box::new(filters::BcjRiscvDecoder::new(input))),

        // Delta filter
        #[cfg(feature = "lzma")]
        method::DELTA => Ok(Box::new(filters::DeltaDecoder::new(input, properties))),

        // AES requires password - use build_decoder_encrypted instead
        #[cfg(feature = "aes")]
        method::AES => Err(Error::PasswordRequired),

        _ => {
            let method_id_u64 = coder.method_id_u64();
            Err(Error::UnsupportedMethod {
                method_id: method_id_u64,
            })
        }
    }
}

/// Builds a decoder chain for a folder, handling filter+codec combinations.
///
/// This function supports:
/// - Single coder (simple decompression)
/// - Two coders with filter (filter + codec, e.g., BCJ + LZMA2)
/// - Two coders without filter (sequential codec chain)
///
/// For encrypted folders, use [`build_encrypted_folder_decoder`] instead.
///
/// # Arguments
///
/// * `input` - The compressed data source
/// * `folder` - Folder containing coder specifications
/// * `uncompressed_size` - Expected size of final uncompressed output
///
/// # Data Flow
///
/// For filter + codec combinations:
/// - Coders in folder: `[filter, codec]`
/// - Data flow: `packed → codec → filter → output`
///
/// The bind_pair in the folder connects the filter's input to the codec's output.
pub(crate) fn build_decoder_chain<R: Read + Send + 'static>(
    input: R,
    folder: &Folder,
    uncompressed_size: u64,
) -> Result<Box<dyn Read + Send>> {
    match folder.coders.len() {
        0 => Err(Error::InvalidFormat("folder has no coders".into())),

        1 => {
            // Single coder - simple case
            let coder = &folder.coders[0];
            let decoder = build_decoder(input, coder, uncompressed_size)?;
            Ok(Box::new(decoder))
        }

        2 => {
            // Two coders - typically filter + codec
            // In 7z, the coder order in the list is: [filter, codec]
            // But data flows: packed -> codec -> filter -> output
            // The bind_pair connects them: filter's input comes from codec's output

            let filter_coder = &folder.coders[0];
            let codec_coder = &folder.coders[1];

            // Check if first coder is a filter (BCJ, Delta)
            let is_filter = method::is_filter(&filter_coder.method_id);

            if is_filter {
                // First decompress with the codec
                let codec_output_size = folder
                    .unpack_sizes
                    .get(1)
                    .copied()
                    .unwrap_or(uncompressed_size);
                let codec_decoder = build_decoder(input, codec_coder, codec_output_size)?;

                // Then apply the filter
                let filter_decoder = build_decoder(codec_decoder, filter_coder, uncompressed_size)?;

                Ok(Box::new(filter_decoder))
            } else {
                // Not a standard filter chain - try sequential decoding
                // First coder processes packed data
                let first_output_size = folder
                    .unpack_sizes
                    .first()
                    .copied()
                    .unwrap_or(uncompressed_size);
                let first_decoder = build_decoder(input, filter_coder, first_output_size)?;

                // Second coder processes first decoder's output
                let second_decoder = build_decoder(first_decoder, codec_coder, uncompressed_size)?;

                Ok(Box::new(second_decoder))
            }
        }

        _ => {
            // Complex chains with 3+ coders need special handling
            // For now, fall back to first coder only (BCJ2 handled separately)
            let coder = &folder.coders[0];
            let decoder = build_decoder(input, coder, uncompressed_size)?;
            Ok(Box::new(decoder))
        }
    }
}

/// Builds a decoder for an encrypted coder specification.
///
/// This function handles AES-encrypted codec chains. The password is used
/// to derive the decryption key.
///
/// # Arguments
///
/// * `input` - The encrypted data source
/// * `coder` - Coder specification from the archive header
/// * `uncompressed_size` - Expected size of uncompressed output
/// * `password` - Password for decryption
///
/// # Errors
///
/// Returns an error if decryption fails or the password is wrong.
#[cfg(feature = "aes")]
pub(crate) fn build_decoder_encrypted<R: Read + Send + 'static>(
    input: R,
    coder: &Coder,
    uncompressed_size: u64,
    password: &crate::crypto::Password,
) -> Result<Box<dyn Decoder>> {
    let method_id = &coder.method_id;
    let properties = coder.properties.as_deref().unwrap_or(&[]);

    if method_id.as_slice() == method::AES {
        // Create AES decoder
        let aes_decoder = crate::crypto::Aes256Decoder::new(input, properties, password)?;

        // Return as boxed decoder
        Ok(Box::new(AesDecoderWrapper { inner: aes_decoder }))
    } else {
        // Not AES - delegate to regular build_decoder
        build_decoder(input, coder, uncompressed_size)
    }
}

/// Wrapper to make Aes256Decoder implement the Decoder trait.
#[cfg(feature = "aes")]
struct AesDecoderWrapper<R: Read + Send> {
    inner: crate::crypto::Aes256Decoder<R>,
}

#[cfg(feature = "aes")]
impl<R: Read + Send> Read for AesDecoderWrapper<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(feature = "aes")]
impl<R: Read + Send> Decoder for AesDecoderWrapper<R> {
    fn method_id(&self) -> &'static [u8] {
        method::AES
    }
}

/// Builds a decoder chain for an encrypted folder.
///
/// This function handles folders where AES encryption is combined with compression.
/// It decrypts the data first, then applies the compression decoder.
///
/// # Arguments
///
/// * `input` - The encrypted packed data
/// * `folder` - The folder containing coder specifications
/// * `uncompressed_size` - Expected size of uncompressed output
/// * `password` - Password for decryption
///
/// # Returns
///
/// A boxed decoder that handles decryption and decompression.
///
/// # Early Password Validation
///
/// This function validates the password early by checking if the first
/// decrypted block looks like valid compression data. This avoids wasting
/// time decompressing garbage data when the password is wrong.
#[cfg(feature = "aes")]
pub(crate) fn build_encrypted_folder_decoder<R: Read + Send + 'static>(
    input: R,
    folder: &Folder,
    uncompressed_size: u64,
    password: &crate::crypto::Password,
) -> Result<Box<dyn Decoder>> {
    if folder.coders.is_empty() {
        return Err(Error::InvalidFormat("folder has no coders".into()));
    }

    // Find AES coder position
    let aes_coder_idx = folder
        .coders
        .iter()
        .position(|c| c.method_id.as_slice() == method::AES);

    match (folder.coders.len(), aes_coder_idx) {
        // Single AES coder - just decrypt (data is encrypted but not compressed)
        (1, Some(0)) => {
            let coder = &folder.coders[0];
            build_decoder_encrypted(input, coder, uncompressed_size, password)
        }

        // Two coders: AES (outer) + compression (inner)
        // Data flow: packed -> AES decrypt -> decompression -> output
        (2, Some(0)) => {
            let aes_coder = &folder.coders[0];
            let compression_coder = &folder.coders[1];
            let properties = aes_coder.properties.as_deref().unwrap_or(&[]);

            // Create AES decoder with early validation
            let mut aes_decoder = crate::crypto::Aes256Decoder::new(input, properties, password)?;

            // Get compression method for validation
            let compression_method = &compression_coder.method_id;

            // Perform early password validation
            if !aes_decoder.validate_first_block(compression_method)? {
                return Err(Error::WrongPassword {
                    entry_index: None,
                    entry_name: None,
                    detection_method: crate::error::PasswordDetectionMethod::EarlyHeaderValidation,
                });
            }

            // Get intermediate unpack size
            let intermediate_size = folder
                .unpack_sizes
                .first()
                .copied()
                .unwrap_or(uncompressed_size);

            // Now build the compression decoder on top of the AES decoder
            build_decoder(aes_decoder, compression_coder, intermediate_size)
        }

        // Two coders: compression (outer) + AES (inner) - less common order
        // Data flow: packed -> decompression -> AES decrypt -> output
        (2, Some(1)) => {
            let compression_coder = &folder.coders[0];
            let aes_coder = &folder.coders[1];

            // First decompress
            let intermediate_size = folder
                .unpack_sizes
                .first()
                .copied()
                .unwrap_or(uncompressed_size);
            let decompressed = build_decoder(input, compression_coder, intermediate_size)?;

            // Then decrypt
            build_decoder_encrypted(decompressed, aes_coder, uncompressed_size, password)
        }

        // Three coders: AES (outer) + filter + compression
        (3, Some(0)) => {
            let aes_coder = &folder.coders[0];
            let filter_coder = &folder.coders[1];
            let compression_coder = &folder.coders[2];
            let properties = aes_coder.properties.as_deref().unwrap_or(&[]);

            // Create AES decoder with early validation
            let mut aes_decoder = crate::crypto::Aes256Decoder::new(input, properties, password)?;

            // Validate against filter (or compression if filter doesn't have recognizable header)
            let validation_method = &compression_coder.method_id;
            if !aes_decoder.validate_first_block(validation_method)? {
                return Err(Error::WrongPassword {
                    entry_index: None,
                    entry_name: None,
                    detection_method: crate::error::PasswordDetectionMethod::EarlyHeaderValidation,
                });
            }

            // Build chain: AES -> compression -> filter
            let compression_size = folder
                .unpack_sizes
                .get(1)
                .copied()
                .unwrap_or(uncompressed_size);
            let decompressed = build_decoder(aes_decoder, compression_coder, compression_size)?;

            let filter_size = folder
                .unpack_sizes
                .first()
                .copied()
                .unwrap_or(uncompressed_size);
            build_decoder(decompressed, filter_coder, filter_size)
        }

        // No encryption - delegate to non-encrypted decoder
        (_, None) => {
            // This folder is not encrypted - use regular decoder chain
            Err(Error::InvalidFormat(
                "build_encrypted_folder_decoder called on non-encrypted folder".into(),
            ))
        }

        // Unsupported configuration
        _ => Err(Error::UnsupportedFeature {
            feature: "encrypted folder with unsupported coder arrangement",
        }),
    }
}

/// Validates a password against an encrypted folder without full decompression.
///
/// This function performs early password validation by decrypting the first
/// block and checking if it looks like valid compression data.
///
/// # Arguments
///
/// * `packed_data` - The encrypted packed data
/// * `folder` - The folder containing coder specifications
/// * `password` - Password to validate
///
/// # Returns
///
/// `true` if the password appears correct, `false` if definitely wrong.
/// Note: A return of `true` doesn't guarantee the password is correct,
/// only that the first block looks valid. CRC verification after full
/// decompression provides definitive confirmation.
#[cfg(feature = "aes")]
pub fn validate_encrypted_folder_password(
    packed_data: &[u8],
    folder: &Folder,
    password: &crate::crypto::Password,
) -> Result<bool> {
    // Find AES coder
    let aes_coder = folder
        .coders
        .iter()
        .find(|c| c.method_id.as_slice() == method::AES)
        .ok_or_else(|| Error::InvalidFormat("folder has no AES coder".into()))?;

    // Find compression coder (for header validation)
    let compression_coder = folder.coders.iter().find(|c| {
        matches!(
            c.method_id.as_slice(),
            method::LZMA | method::LZMA2 | method::DEFLATE | method::BZIP2 | method::PPMD
        )
    });

    let compression_method = compression_coder
        .map(|c| c.method_id.as_slice())
        .unwrap_or(&[]);
    let properties = aes_coder.properties.as_deref().unwrap_or(&[]);

    // Create AES decoder
    let cursor = std::io::Cursor::new(packed_data);
    let mut aes_decoder = crate::crypto::Aes256Decoder::new(cursor, properties, password)?;

    // Validate first block
    Ok(aes_decoder.validate_first_block(compression_method)?)
}

/// Builds a multi-threaded decoder for LZMA2 streams.
///
/// Falls back to single-threaded decoder for non-LZMA2 methods.
///
/// # Arguments
///
/// * `input` - The compressed data source
/// * `coder` - Coder specification from the archive header
/// * `uncompressed_size` - Expected size of uncompressed output
/// * `num_threads` - Number of worker threads for LZMA2 (ignored for other codecs)
///
/// # Feature
///
/// Requires the `parallel` feature for multi-threaded LZMA2.
/// Without it, this is identical to `build_decoder`.
#[cfg(all(feature = "lzma", feature = "parallel"))]
#[allow(dead_code)] // Reserved for future multi-threaded decompression
pub(crate) fn build_decoder_mt<R: Read + Send + 'static>(
    input: R,
    coder: &Coder,
    uncompressed_size: u64,
    num_threads: u32,
) -> Result<Box<dyn Decoder>> {
    let method_id = &coder.method_id;
    let properties = coder.properties.as_deref().unwrap_or(&[]);

    // Only LZMA2 supports multi-threaded decoding
    if method_id.as_slice() == method::LZMA2 {
        let decoder = lzma::Lzma2DecoderMt::new(input, properties, num_threads)?;
        return Ok(Box::new(decoder));
    }

    // Fall back to single-threaded for other codecs
    build_decoder(input, coder, uncompressed_size)
}

/// Builds a multi-threaded decoder using available CPU cores.
///
/// Automatically determines thread count from system.
///
/// # Arguments
///
/// * `input` - The compressed data source
/// * `coder` - Coder specification from the archive header
/// * `uncompressed_size` - Expected size of uncompressed output
#[cfg(all(feature = "lzma", feature = "parallel"))]
#[allow(dead_code)] // Reserved for future multi-threaded decompression
pub(crate) fn build_decoder_mt_auto<R: Read + Send + 'static>(
    input: R,
    coder: &Coder,
    uncompressed_size: u64,
) -> Result<Box<dyn Decoder>> {
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    build_decoder_mt(input, coder, uncompressed_size, num_threads)
}

/// Builds a decoder for a BCJ2 folder.
///
/// BCJ2 is a 4-stream filter that requires special handling:
/// - Stream 0 (Main): Main code stream
/// - Stream 1 (Call): CALL destinations
/// - Stream 2 (Jump): JMP destinations
/// - Stream 3 (Range): Range-coded selector bits
///
/// Each BCJ2 input typically comes from an LZMA2 decoder fed by a packed stream.
///
/// # Arguments
///
/// * `folder` - The folder structure with coders and bind_pairs
/// * `packed_data` - The raw compressed data for each packed stream
///
/// # Returns
///
/// A boxed decoder that reads from BCJ2 and produces the final uncompressed output.
#[cfg(feature = "lzma")]
pub(crate) fn build_bcj2_folder_decoder(
    folder: &Folder,
    packed_data: &[Vec<u8>],
) -> Result<Box<dyn Decoder>> {
    // Find the BCJ2 coder
    let bcj2_coder_idx = folder
        .coders
        .iter()
        .position(|c| c.method_id.as_slice() == method::BCJ2)
        .ok_or_else(|| Error::InvalidFormat("No BCJ2 coder in folder".into()))?;

    let bcj2_coder = &folder.coders[bcj2_coder_idx];

    // BCJ2 must have exactly 4 inputs
    if bcj2_coder.num_in_streams != 4 {
        return Err(Error::InvalidFormat(format!(
            "BCJ2 expected 4 inputs, got {}",
            bcj2_coder.num_in_streams
        )));
    }

    // Get stream offsets for all coders
    let offsets = folder.coder_stream_offsets();
    let bcj2_first_in = offsets[bcj2_coder_idx].0 as u64;

    // Build readers for each of BCJ2's 4 inputs
    let mut bcj2_inputs: Vec<Vec<u8>> = Vec::with_capacity(4);

    for i in 0..4 {
        let in_stream_idx = bcj2_first_in + i;
        let input_data = resolve_input_stream(folder, &offsets, in_stream_idx, packed_data)?;
        bcj2_inputs.push(input_data);
    }

    // Create Bcj2Decoder with Cursor readers
    let main = Cursor::new(bcj2_inputs.remove(0));
    let call = Cursor::new(bcj2_inputs.remove(0));
    let jump = Cursor::new(bcj2_inputs.remove(0));
    let range = Cursor::new(bcj2_inputs.remove(0));

    let decoder = bcj2::Bcj2Decoder::new(main, call, jump, range)?;

    Ok(Box::new(bcj2::Bcj2DecoderWrapper::new(decoder)))
}

/// Resolves an input stream to its decompressed data.
///
/// An input stream either:
/// 1. Comes from a packed_stream (raw compressed data from archive)
/// 2. Comes from a bind_pair (output of another coder)
#[cfg(feature = "lzma")]
fn resolve_input_stream(
    folder: &Folder,
    offsets: &[(usize, usize)],
    in_stream_idx: u64,
    packed_data: &[Vec<u8>],
) -> Result<Vec<u8>> {
    // Check if this input comes from a packed stream
    if let Some(pack_idx) = folder.find_packed_stream_index(in_stream_idx) {
        if pack_idx >= packed_data.len() {
            return Err(Error::InvalidFormat(format!(
                "Pack index {} out of bounds (have {} streams)",
                pack_idx,
                packed_data.len()
            )));
        }

        // Find the coder that uses this input
        let (coder_idx, _local_in_idx) = find_coder_for_input(folder, offsets, in_stream_idx)?;
        let coder = &folder.coders[coder_idx];

        // If this input belongs to BCJ2 directly (not to a compression coder),
        // return the raw data without decoding. BCJ2's Call/Jump/Range streams
        // are often stored uncompressed in the archive.
        if coder.method_id.as_slice() == method::BCJ2 {
            return Ok(packed_data[pack_idx].clone());
        }

        // Get the unpack size for this coder
        let unpack_size = if coder_idx < folder.unpack_sizes.len() {
            folder.unpack_sizes[coder_idx]
        } else {
            // If no explicit unpack size, we don't know the size
            u64::MAX
        };

        // Build decoder for this coder and decompress
        let input = Cursor::new(packed_data[pack_idx].clone());
        let mut decoder = build_decoder(input, coder, unpack_size)?;

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).map_err(Error::Io)?;

        return Ok(output);
    }

    // Check if this input comes from a bind_pair (another coder's output)
    if let Some(bp) = folder.find_bind_pair_for_in_stream(in_stream_idx) {
        // Find which coder produces this output
        let (src_coder_idx, _local_out_idx) = find_coder_for_output(folder, offsets, bp.out_index)?;

        // Find the source coder's input
        let src_coder = &folder.coders[src_coder_idx];
        let src_first_in = offsets[src_coder_idx].0 as u64;

        // For single-input coders, recursively resolve
        if src_coder.num_in_streams == 1 {
            let src_in_stream = src_first_in;
            return resolve_input_stream(folder, offsets, src_in_stream, packed_data);
        }

        // For multi-input coders (like BCJ2), this shouldn't happen in typical archives
        return Err(Error::UnsupportedFeature {
            feature: "nested multi-input coders",
        });
    }

    Err(Error::InvalidFormat(format!(
        "Input stream {} not found in packed_streams or bind_pairs",
        in_stream_idx
    )))
}

/// Finds the coder index and local input index for a given global input stream index.
#[cfg(feature = "lzma")]
fn find_coder_for_input(
    folder: &Folder,
    offsets: &[(usize, usize)],
    in_stream_idx: u64,
) -> Result<(usize, usize)> {
    for (coder_idx, coder) in folder.coders.iter().enumerate() {
        let first_in = offsets[coder_idx].0 as u64;
        let last_in = first_in + coder.num_in_streams;
        if in_stream_idx >= first_in && in_stream_idx < last_in {
            return Ok((coder_idx, (in_stream_idx - first_in) as usize));
        }
    }
    Err(Error::InvalidFormat(format!(
        "No coder found for input stream {}",
        in_stream_idx
    )))
}

/// Finds the coder index and local output index for a given global output stream index.
#[cfg(feature = "lzma")]
fn find_coder_for_output(
    folder: &Folder,
    offsets: &[(usize, usize)],
    out_stream_idx: u64,
) -> Result<(usize, usize)> {
    for (coder_idx, coder) in folder.coders.iter().enumerate() {
        let first_out = offsets[coder_idx].1 as u64;
        let last_out = first_out + coder.num_out_streams;
        if out_stream_idx >= first_out && out_stream_idx < last_out {
            return Ok((coder_idx, (out_stream_idx - first_out) as usize));
        }
    }
    Err(Error::InvalidFormat(format!(
        "No coder found for output stream {}",
        out_stream_idx
    )))
}

/// Codec method types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecMethod {
    /// No compression.
    Copy,
    /// LZMA compression.
    Lzma,
    /// LZMA2 compression.
    Lzma2,
    /// Deflate compression.
    Deflate,
    /// BZip2 compression.
    BZip2,
    /// PPMd compression.
    PPMd,
    /// LZ4 compression.
    Lz4,
    /// ZSTD compression.
    Zstd,
    /// Brotli compression.
    Brotli,
}

impl CodecMethod {
    /// Creates a CodecMethod from a coder specification.
    pub fn from_coder(coder: &Coder) -> Result<Self> {
        match coder.method_id.as_slice() {
            method::COPY => Ok(Self::Copy),
            method::LZMA => Ok(Self::Lzma),
            method::LZMA2 => Ok(Self::Lzma2),
            method::DEFLATE => Ok(Self::Deflate),
            method::BZIP2 => Ok(Self::BZip2),
            method::PPMD => Ok(Self::PPMd),
            method::LZ4 => Ok(Self::Lz4),
            method::ZSTD => Ok(Self::Zstd),
            method::BROTLI => Ok(Self::Brotli),
            _ => Err(Error::UnsupportedMethod {
                method_id: coder.method_id_u64(),
            }),
        }
    }

    /// Returns whether this is a compression codec (vs. a filter).
    pub fn is_compression(&self) -> bool {
        true // All CodecMethod variants are compression codecs
    }

    /// Returns the method ID as a u64.
    pub fn method_id(&self) -> u64 {
        match self {
            Self::Copy => 0x00,
            Self::Lzma => 0x030101,
            Self::Lzma2 => 0x21,
            Self::Deflate => 0x040108,
            Self::BZip2 => 0x040202,
            Self::PPMd => 0x030401,
            Self::Lz4 => 0x04F71104,
            Self::Zstd => 0x04F71101,
            Self::Brotli => 0x04F71102,
        }
    }

    /// Returns whether this codec is available in the current build.
    ///
    /// Some codecs require optional features to be enabled at compile time.
    /// This method allows runtime checking of codec availability.
    ///
    /// # Examples
    ///
    /// ```
    /// use zesven::codec::CodecMethod;
    ///
    /// // Copy is always available
    /// assert!(CodecMethod::Copy.is_available());
    ///
    /// // LZMA requires the "lzma" feature
    /// if CodecMethod::Lzma.is_available() {
    ///     println!("LZMA compression is available");
    /// }
    /// ```
    pub fn is_available(&self) -> bool {
        match self {
            Self::Copy => true,
            Self::Lzma | Self::Lzma2 => cfg!(feature = "lzma"),
            Self::Deflate => cfg!(feature = "deflate"),
            Self::BZip2 => cfg!(feature = "bzip2"),
            Self::PPMd => cfg!(feature = "ppmd"),
            Self::Lz4 => cfg!(feature = "lz4"),
            Self::Zstd => cfg!(feature = "zstd"),
            Self::Brotli => cfg!(feature = "brotli"),
        }
    }

    /// Returns the feature flag name required for this codec, if any.
    ///
    /// Returns `None` for codecs that are always available (e.g., `Copy`).
    ///
    /// # Examples
    ///
    /// ```
    /// use zesven::codec::CodecMethod;
    ///
    /// assert_eq!(CodecMethod::Copy.required_feature(), None);
    /// assert_eq!(CodecMethod::Lzma.required_feature(), Some("lzma"));
    /// assert_eq!(CodecMethod::Zstd.required_feature(), Some("zstd"));
    /// ```
    pub fn required_feature(&self) -> Option<&'static str> {
        match self {
            Self::Copy => None,
            Self::Lzma | Self::Lzma2 => Some("lzma"),
            Self::Deflate => Some("deflate"),
            Self::BZip2 => Some("bzip2"),
            Self::PPMd => Some("ppmd"),
            Self::Lz4 => Some("lz4"),
            Self::Zstd => Some("zstd"),
            Self::Brotli => Some("brotli"),
        }
    }
}

/// Filter method types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterMethod {
    /// BCJ (x86) filter.
    BcjX86,
    /// BCJ (ARM) filter.
    BcjArm,
    /// BCJ (ARM64) filter.
    BcjArm64,
    /// BCJ (ARM Thumb) filter.
    BcjArmThumb,
    /// BCJ (PowerPC) filter.
    BcjPpc,
    /// BCJ (SPARC) filter.
    BcjSparc,
    /// BCJ (IA64) filter.
    BcjIa64,
    /// BCJ (RISC-V) filter.
    BcjRiscv,
    /// Delta filter.
    Delta,
}

impl FilterMethod {
    /// Creates a FilterMethod from a coder specification.
    pub fn from_coder(coder: &Coder) -> Result<Self> {
        match coder.method_id.as_slice() {
            method::BCJ_X86 => Ok(Self::BcjX86),
            method::BCJ_ARM => Ok(Self::BcjArm),
            method::BCJ_ARM64 => Ok(Self::BcjArm64),
            method::BCJ_ARM_THUMB => Ok(Self::BcjArmThumb),
            method::BCJ_PPC => Ok(Self::BcjPpc),
            method::BCJ_SPARC => Ok(Self::BcjSparc),
            method::BCJ_IA64 => Ok(Self::BcjIa64),
            method::BCJ_RISCV => Ok(Self::BcjRiscv),
            method::DELTA => Ok(Self::Delta),
            _ => Err(Error::UnsupportedMethod {
                method_id: coder.method_id_u64(),
            }),
        }
    }
}

/// Represents a validated method chain.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MethodChain {
    /// Single compression codec.
    Codec(CodecMethod),
    /// Filter followed by compression codec.
    FilterThenCodec {
        /// The filter method.
        filter: FilterMethod,
        /// The compression codec.
        codec: CodecMethod,
    },
}

impl MethodChain {
    /// Validates and constructs a method chain from coders.
    ///
    /// Supports:
    /// - Single codec (LZMA, LZMA2, Deflate, etc.)
    /// - BCJ/Delta filter followed by codec
    pub fn validate(coders: &[Coder]) -> Result<Self> {
        match coders.len() {
            0 => Err(Error::InvalidFormat("empty method chain".into())),

            1 => Ok(Self::Codec(CodecMethod::from_coder(&coders[0])?)),

            2 => {
                // First coder should be a filter, second should be compression
                let filter = FilterMethod::from_coder(&coders[0])?;
                let codec = CodecMethod::from_coder(&coders[1])?;

                Ok(Self::FilterThenCodec { filter, codec })
            }

            _ => Err(Error::UnsupportedFeature {
                feature: "complex method chains with more than 2 coders",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_coder(method_id: &[u8]) -> Coder {
        Coder {
            method_id: method_id.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: None,
        }
    }

    #[test]
    fn test_method_names() {
        assert_eq!(method::name(method::COPY), "Copy");
        assert_eq!(method::name(method::LZMA), "LZMA");
        assert_eq!(method::name(method::LZMA2), "LZMA2");
        assert_eq!(method::name(&[0xFF, 0xFF]), "Unknown");
    }

    #[test]
    fn test_is_filter_identifies_filters() {
        // All BCJ filters should be identified
        assert!(method::is_filter(method::BCJ_X86));
        assert!(method::is_filter(method::BCJ_ARM));
        assert!(method::is_filter(method::BCJ_ARM64));
        assert!(method::is_filter(method::BCJ_ARM_THUMB));
        assert!(method::is_filter(method::BCJ_PPC));
        assert!(method::is_filter(method::BCJ_SPARC));
        assert!(method::is_filter(method::BCJ_IA64));
        assert!(method::is_filter(method::BCJ_RISCV));
        assert!(method::is_filter(method::DELTA));

        // Compression codecs should not be identified as filters
        assert!(!method::is_filter(method::COPY));
        assert!(!method::is_filter(method::LZMA));
        assert!(!method::is_filter(method::LZMA2));
        assert!(!method::is_filter(method::DEFLATE));
        assert!(!method::is_filter(method::BZIP2));
        assert!(!method::is_filter(method::PPMD));
        assert!(!method::is_filter(method::AES));
        assert!(!method::is_filter(&[0xFF, 0xFF])); // Unknown
    }

    #[test]
    fn test_codec_method_from_coder() {
        assert_eq!(
            CodecMethod::from_coder(&make_coder(method::COPY)).unwrap(),
            CodecMethod::Copy
        );
        assert_eq!(
            CodecMethod::from_coder(&make_coder(method::LZMA)).unwrap(),
            CodecMethod::Lzma
        );
        assert_eq!(
            CodecMethod::from_coder(&make_coder(method::LZMA2)).unwrap(),
            CodecMethod::Lzma2
        );
    }

    #[test]
    fn test_filter_method_from_coder() {
        assert_eq!(
            FilterMethod::from_coder(&make_coder(method::BCJ_X86)).unwrap(),
            FilterMethod::BcjX86
        );
        assert_eq!(
            FilterMethod::from_coder(&make_coder(method::DELTA)).unwrap(),
            FilterMethod::Delta
        );
    }

    #[test]
    fn test_method_chain_single_codec() {
        let coders = vec![make_coder(method::LZMA2)];
        let chain = MethodChain::validate(&coders).unwrap();
        assert!(matches!(chain, MethodChain::Codec(CodecMethod::Lzma2)));
    }

    #[test]
    fn test_method_chain_filter_then_codec() {
        let coders = vec![make_coder(method::BCJ_X86), make_coder(method::LZMA2)];
        let chain = MethodChain::validate(&coders).unwrap();
        assert!(matches!(
            chain,
            MethodChain::FilterThenCodec {
                filter: FilterMethod::BcjX86,
                codec: CodecMethod::Lzma2
            }
        ));
    }

    #[test]
    fn test_method_chain_empty() {
        let coders: Vec<Coder> = vec![];
        let err = MethodChain::validate(&coders).unwrap_err();
        assert!(matches!(err, Error::InvalidFormat(_)));
    }

    #[test]
    fn test_method_chain_too_many() {
        let coders = vec![
            make_coder(method::BCJ_X86),
            make_coder(method::DELTA),
            make_coder(method::LZMA2),
        ];
        let err = MethodChain::validate(&coders).unwrap_err();
        assert!(matches!(err, Error::UnsupportedFeature { .. }));
    }

    #[test]
    fn test_copy_decoder() {
        use std::io::Cursor;

        let data = b"Hello, World!";
        let cursor = Cursor::new(data.to_vec());
        let mut decoder = CopyDecoder::new(cursor, data.len() as u64);

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
    }

    #[test]
    fn test_codec_method_is_available_copy() {
        // Copy is always available (no feature required)
        assert!(CodecMethod::Copy.is_available());
    }

    #[test]
    fn test_codec_method_required_feature() {
        // Copy requires no feature
        assert_eq!(CodecMethod::Copy.required_feature(), None);

        // Other codecs require their respective features
        assert_eq!(CodecMethod::Lzma.required_feature(), Some("lzma"));
        assert_eq!(CodecMethod::Lzma2.required_feature(), Some("lzma"));
        assert_eq!(CodecMethod::Deflate.required_feature(), Some("deflate"));
        assert_eq!(CodecMethod::BZip2.required_feature(), Some("bzip2"));
        assert_eq!(CodecMethod::PPMd.required_feature(), Some("ppmd"));
        assert_eq!(CodecMethod::Lz4.required_feature(), Some("lz4"));
        assert_eq!(CodecMethod::Zstd.required_feature(), Some("zstd"));
        assert_eq!(CodecMethod::Brotli.required_feature(), Some("brotli"));
    }

    #[test]
    fn test_codec_method_is_available_consistency() {
        // Verify that is_available() and required_feature() are consistent:
        // if required_feature() is None, is_available() must be true
        for method in [
            CodecMethod::Copy,
            CodecMethod::Lzma,
            CodecMethod::Lzma2,
            CodecMethod::Deflate,
            CodecMethod::BZip2,
            CodecMethod::PPMd,
            CodecMethod::Lz4,
            CodecMethod::Zstd,
            CodecMethod::Brotli,
        ] {
            if method.required_feature().is_none() {
                assert!(
                    method.is_available(),
                    "{:?} has no required feature but is_available() returned false",
                    method
                );
            }
        }
    }

    // =========================================================================
    // build_decoder() Unit Tests
    // =========================================================================
    //
    // These tests verify that build_decoder() correctly creates decoders for
    // each supported codec and returns appropriate errors for unsupported cases.

    /// Tests that build_decoder() creates a working Copy decoder.
    #[test]
    fn test_build_decoder_copy() {
        let data = b"Hello, World! This is test data for copy decoder.";
        let coder = make_coder(method::COPY);
        let cursor = Cursor::new(data.to_vec());

        let mut decoder = build_decoder(cursor, &coder, data.len() as u64)
            .expect("Failed to create Copy decoder");

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
        assert_eq!(decoder.method_id(), method::COPY);
    }

    /// Tests that build_decoder() returns UnsupportedMethod for unknown method IDs.
    #[test]
    fn test_build_decoder_unsupported_method() {
        let unknown_method = &[0xFF, 0xFE, 0xFD, 0xFC];
        let coder = Coder {
            method_id: unknown_method.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: None,
        };
        let cursor = Cursor::new(vec![0u8; 100]);

        let result = build_decoder(cursor, &coder, 100);

        match result {
            Err(Error::UnsupportedMethod { method_id }) => {
                // Method ID should be decoded as u64
                assert_ne!(method_id, 0);
            }
            Err(other) => panic!("Expected UnsupportedMethod, got: {:?}", other),
            Ok(_) => panic!("Expected error for unknown method"),
        }
    }

    /// Tests that build_decoder() returns PasswordRequired when AES is used without password.
    #[cfg(feature = "aes")]
    #[test]
    fn test_build_decoder_aes_requires_password() {
        let coder = make_coder(method::AES);
        let cursor = Cursor::new(vec![0u8; 100]);

        let result = build_decoder(cursor, &coder, 100);

        match result {
            Err(Error::PasswordRequired) => {
                // Expected - password required for AES decoding
            }
            Err(other) => panic!("Expected PasswordRequired, got: {:?}", other),
            Ok(_) => panic!("Expected error for AES without password"),
        }
    }

    /// Tests that build_decoder() creates a working LZMA decoder.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_build_decoder_lzma() {
        // LZMA requires valid properties (5 bytes minimum)
        // Properties format: lc/lp/pb byte + dictionary size (4 bytes)
        let properties = vec![0x5D, 0x00, 0x00, 0x01, 0x00]; // Standard LZMA properties

        let coder = Coder {
            method_id: method::LZMA.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: Some(properties),
        };

        // Create minimal LZMA-compressed empty data
        // For this test, we just verify the decoder is created without error
        // Actual decompression is tested in integration tests
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        // Should succeed in creating decoder (may fail on actual read due to invalid data)
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::LZMA),
            Err(e) => panic!("Should create LZMA decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working LZMA2 decoder.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_build_decoder_lzma2() {
        // LZMA2 properties: single byte for dictionary size
        let properties = vec![0x18]; // Dictionary size indicator

        let coder = Coder {
            method_id: method::LZMA2.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: Some(properties),
        };

        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::LZMA2),
            Err(e) => panic!("Should create LZMA2 decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working Deflate decoder.
    #[cfg(feature = "deflate")]
    #[test]
    fn test_build_decoder_deflate() {
        let coder = make_coder(method::DEFLATE);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::DEFLATE),
            Err(e) => panic!("Should create Deflate decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working BZip2 decoder.
    #[cfg(feature = "bzip2")]
    #[test]
    fn test_build_decoder_bzip2() {
        let coder = make_coder(method::BZIP2);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::BZIP2),
            Err(e) => panic!("Should create BZip2 decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working PPMd decoder.
    #[cfg(feature = "ppmd")]
    #[test]
    fn test_build_decoder_ppmd() {
        // PPMd requires 5-byte properties: order (1) + mem_size (4)
        let properties = vec![0x06, 0x00, 0x00, 0x10, 0x00]; // order=6, mem=1MB

        let coder = Coder {
            method_id: method::PPMD.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: Some(properties),
        };

        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::PPMD),
            Err(e) => panic!("Should create PPMd decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working LZ4 decoder.
    #[cfg(feature = "lz4")]
    #[test]
    fn test_build_decoder_lz4() {
        let coder = make_coder(method::LZ4);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::LZ4),
            Err(e) => panic!("Should create LZ4 decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working Zstd decoder.
    #[cfg(feature = "zstd")]
    #[test]
    fn test_build_decoder_zstd() {
        let coder = make_coder(method::ZSTD);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::ZSTD),
            Err(e) => panic!("Should create Zstd decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working Brotli decoder.
    #[cfg(feature = "brotli")]
    #[test]
    fn test_build_decoder_brotli() {
        let coder = make_coder(method::BROTLI);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::BROTLI),
            Err(e) => panic!("Should create Brotli decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working LZ5 decoder.
    #[test]
    fn test_build_decoder_lz5() {
        let coder = make_coder(method::LZ5);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::LZ5),
            Err(e) => panic!("Should create LZ5 decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working Lizard decoder.
    #[test]
    fn test_build_decoder_lizard() {
        let coder = make_coder(method::LIZARD);
        let compressed = vec![0u8; 100];
        let cursor = Cursor::new(compressed);

        let result = build_decoder(cursor, &coder, 0);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::LIZARD),
            Err(e) => panic!("Should create Lizard decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working BCJ X86 filter.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_build_decoder_bcj_x86() {
        let coder = make_coder(method::BCJ_X86);
        let data = vec![0u8; 100];
        let cursor = Cursor::new(data);

        let result = build_decoder(cursor, &coder, 100);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::BCJ_X86),
            Err(e) => panic!("Should create BCJ X86 decoder: {}", e),
        }
    }

    /// Tests that build_decoder() creates a working Delta filter.
    #[cfg(feature = "lzma")]
    #[test]
    fn test_build_decoder_delta() {
        // Delta filter requires 1-byte properties (delta distance)
        let coder = Coder {
            method_id: method::DELTA.to_vec(),
            num_in_streams: 1,
            num_out_streams: 1,
            properties: Some(vec![0x01]), // delta=1
        };
        let data = vec![0u8; 100];
        let cursor = Cursor::new(data);

        let result = build_decoder(cursor, &coder, 100);
        match result {
            Ok(decoder) => assert_eq!(decoder.method_id(), method::DELTA),
            Err(e) => panic!("Should create Delta decoder: {}", e),
        }
    }

    /// Tests Copy decoder handles exact size correctly.
    #[test]
    fn test_copy_decoder_exact_size() {
        let data = b"Exactly this many bytes";
        let coder = make_coder(method::COPY);
        let cursor = Cursor::new(data.to_vec());

        let mut decoder =
            build_decoder(cursor, &coder, data.len() as u64).expect("Failed to create decoder");

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();

        assert_eq!(output.len(), data.len());
        assert_eq!(output, data);
    }

    /// Tests Copy decoder stops at specified size even if more data available.
    #[test]
    fn test_copy_decoder_size_limit() {
        let data = b"This is more data than we want to read";
        let limit = 10u64;
        let coder = make_coder(method::COPY);
        let cursor = Cursor::new(data.to_vec());

        let mut decoder = build_decoder(cursor, &coder, limit).expect("Failed to create decoder");

        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();

        assert_eq!(output.len(), limit as usize);
        assert_eq!(&output[..], &data[..limit as usize]);
    }
}
