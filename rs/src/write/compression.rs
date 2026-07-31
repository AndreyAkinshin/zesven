//! Core compression logic for archive writing.
//!
//! This module provides the main compression interface that dispatches
//! to codec-specific implementations, and handles filter application.

use std::borrow::Cow;
use std::io::Write;

use crate::{Error, Result};

#[cfg(feature = "aes")]
use super::EncryptedFolderInfo;
use super::FilteredFolderInfo;
// The codec module is only reached when some codec is compiled in.
#[allow(unused_imports)]
use super::codecs::{self, Compressed};
use super::options::{WriteFilter, WriteOptions};

/// Compresses data using the configured method.
///
/// `concurrency` says whether this stream has the cores to itself, and so may
/// be cut into blocks compressed alongside each other, or whether other
/// entries are already being compressed at the same time.
pub(crate) fn compress_data(
    options: &WriteOptions,
    data: &[u8],
    #[cfg_attr(not(feature = "lzma2"), allow(unused_variables))] concurrency: codecs::Concurrency,
) -> Result<Compressed> {
    use crate::codec::CodecMethod;

    match options.method {
        CodecMethod::Copy => Ok(Compressed::without_properties(data.to_vec())),
        #[cfg(feature = "lzma2")]
        CodecMethod::Lzma2 => codecs::compress_lzma2(options, data, concurrency),
        #[cfg(feature = "lzma")]
        CodecMethod::Lzma => codecs::compress_lzma(options, data),
        #[cfg(feature = "deflate")]
        CodecMethod::Deflate => codecs::compress_deflate(options, data),
        #[cfg(feature = "bzip2")]
        CodecMethod::BZip2 => codecs::compress_bzip2(options, data),
        #[cfg(feature = "zstd")]
        CodecMethod::Zstd => codecs::compress_zstd(options, data),
        #[cfg(feature = "lz4")]
        CodecMethod::Lz4 => codecs::compress_lz4(options, data),
        #[cfg(feature = "brotli")]
        CodecMethod::Brotli => codecs::compress_brotli(options, data),
        #[cfg(feature = "ppmd")]
        CodecMethod::PPMd => codecs::compress_ppmd(options, data),
        #[allow(unreachable_patterns)]
        _ => Err(Error::UnsupportedMethod {
            method_id: options.method.method_id(),
        }),
    }
}

/// Applies the configured filter to data.
///
/// Returns the filtered data. If no filter is configured, returns None.
pub(crate) fn filter_data(options: &WriteOptions, data: &[u8]) -> Result<Option<Vec<u8>>> {
    use crate::codec::bcj_encoders::*;

    match options.filter {
        WriteFilter::None => Ok(None),
        WriteFilter::BcjX86 => {
            let mut output = Vec::new();
            let mut encoder = BcjX86Encoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjArm => {
            let mut output = Vec::new();
            let mut encoder = BcjArmEncoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjArm64 => {
            let mut output = Vec::new();
            let mut encoder = BcjArm64Encoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjArmThumb => {
            let mut output = Vec::new();
            let mut encoder = BcjArmThumbEncoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjPpc => {
            let mut output = Vec::new();
            let mut encoder = BcjPpcEncoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjSparc => {
            let mut output = Vec::new();
            let mut encoder = BcjSparcEncoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjIa64 => {
            let mut output = Vec::new();
            let mut encoder = BcjIa64Encoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::BcjRiscv => {
            let mut output = Vec::new();
            let mut encoder = BcjRiscvEncoder::new(&mut output);
            encoder.write_all(data).map_err(Error::Io)?;
            encoder.try_finish().map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::Delta { distance } => {
            let mut output = Vec::new();
            let mut encoder = DeltaEncoder::new(&mut output, distance);
            encoder.write_all(data).map_err(Error::Io)?;
            Ok(Some(output))
        }
        WriteFilter::Bcj2 => {
            // BCJ2 is handled separately via compress_entry_bcj2(),
            // not through the standard filter_data() path.
            // This case should not be reached, but return None if it is.
            Ok(None)
        }
    }
}

/// Applies the configured filter, if any, without copying when there is none.
///
/// A solid block is 64 MiB by default, so copying it just to hand it to the
/// codec doubles the writer's peak memory for nothing.
fn apply_filter<'a>(
    options: &WriteOptions,
    data: &'a [u8],
) -> Result<(Cow<'a, [u8]>, Option<FilteredFolderInfo>)> {
    if !options.filter.is_active() {
        return Ok((Cow::Borrowed(data), None));
    }

    let filtered = match filter_data(options, data)? {
        Some(filtered) => Cow::Owned(filtered),
        None => Cow::Borrowed(data),
    };
    let info = FilteredFolderInfo {
        filter_method: options.filter.method_id().unwrap_or(&[]).to_vec(),
        filter_properties: options.filter.properties(),
        filtered_size: filtered.len() as u64,
    };
    Ok((filtered, Some(info)))
}

/// Filters and compresses data, returning the compressed data and filter info.
///
/// This is the whole of the expensive work for one folder and touches nothing
/// but the options, which is what lets several folders run at once.
pub(crate) fn filter_and_compress_data(
    options: &WriteOptions,
    data: &[u8],
    concurrency: codecs::Concurrency,
) -> Result<(Compressed, Option<FilteredFolderInfo>)> {
    let (data_to_compress, filter_info) = apply_filter(options, data)?;
    let compressed = compress_data(options, &data_to_compress, concurrency)?;
    Ok((compressed, filter_info))
}

impl<W: std::io::Write + std::io::Seek> super::Writer<W> {
    /// Encrypts already-compressed bytes, returning them and the folder's AES info.
    ///
    /// Encryption stays on the writer because each stream needs a fresh IV, and
    /// handing out nonces is the one part of this that is not a pure function of
    /// the options. It is also cheap next to compression, so keeping it
    /// sequential costs nothing.
    /// Encryption follows the options the entry was accepted under, not
    /// whatever is set by the time it is compressed: a buffered entry is
    /// written later, and reading the live options applied settings the entry
    /// was never offered - including none at all, which sent data out in the
    /// clear after encryption had been asked for.
    #[cfg(feature = "aes")]
    pub(crate) fn encrypt_compressed_with(
        &mut self,
        compressed: Compressed,
        options: &super::options::WriteOptions,
    ) -> Result<(Compressed, EncryptedFolderInfo)> {
        use crate::crypto::{Aes256Encoder, AesProperties, derive_key_cached};

        let compressed_size = compressed.data.len() as u64;

        let password = options
            .password
            .clone()
            .ok_or_else(|| Error::InvalidFormat("encryption requires a password".into()))?;
        // Fixed here, where the key is actually derived from it.
        self.hold_password(&password)?;
        let (salt, iv) = self.nonce_for_stream_under(&options.nonce_policy)?;

        let key = derive_key_cached(&password, &salt, options.nonce_policy.num_cycles_power())?;

        let encrypted = {
            let mut output = Vec::new();
            let mut encoder = Aes256Encoder::with_key_iv(&mut output, key, iv);
            encoder.write_all(&compressed.data).map_err(Error::Io)?;
            encoder.finish().map_err(Error::Io)?;
            output
        };

        let aes_properties =
            AesProperties::encode(options.nonce_policy.num_cycles_power(), &salt, &iv)?;

        Ok((
            // The bytes are the encrypted ones, but the coder properties still
            // describe the codec underneath: that is the coder they belong to.
            Compressed {
                data: encrypted,
                properties: compressed.properties,
            },
            EncryptedFolderInfo {
                aes_properties,
                compressed_size,
            },
        ))
    }
}
