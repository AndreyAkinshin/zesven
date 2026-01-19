//! Core compression logic for archive writing.
//!
//! This module provides the main compression interface that dispatches
//! to codec-specific implementations, and handles filter application.

use std::io::{Seek, Write};

use crate::{Error, Result};

use super::options::WriteFilter;
use super::{FilteredFolderInfo, Writer};

#[cfg(feature = "aes")]
use super::EncryptedFolderInfo;

impl<W: Write + Seek> Writer<W> {
    /// Compresses data using the configured method.
    pub(crate) fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::codec::CodecMethod;

        match self.options.method {
            CodecMethod::Copy => Ok(data.to_vec()),
            #[cfg(feature = "lzma2")]
            CodecMethod::Lzma2 => self.compress_lzma2(data),
            #[cfg(feature = "lzma")]
            CodecMethod::Lzma => self.compress_lzma(data),
            #[cfg(feature = "deflate")]
            CodecMethod::Deflate => self.compress_deflate(data),
            #[cfg(feature = "bzip2")]
            CodecMethod::BZip2 => self.compress_bzip2(data),
            #[cfg(feature = "zstd")]
            CodecMethod::Zstd => self.compress_zstd(data),
            #[cfg(feature = "lz4")]
            CodecMethod::Lz4 => self.compress_lz4(data),
            #[cfg(feature = "brotli")]
            CodecMethod::Brotli => self.compress_brotli(data),
            #[cfg(feature = "ppmd")]
            CodecMethod::PPMd => self.compress_ppmd(data),
            #[allow(unreachable_patterns)]
            _ => Err(Error::UnsupportedMethod {
                method_id: self.options.method.method_id(),
            }),
        }
    }

    /// Applies the configured filter to data.
    ///
    /// Returns the filtered data. If no filter is configured, returns None.
    pub(crate) fn filter_data(&self, data: &[u8]) -> Result<Option<Vec<u8>>> {
        use crate::codec::bcj_encoders::*;

        match self.options.filter {
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

    /// Filters and compresses data, returning the compressed data and filter info.
    pub(crate) fn filter_and_compress_data(
        &self,
        data: &[u8],
    ) -> Result<(Vec<u8>, Option<FilteredFolderInfo>)> {
        // Apply filter if configured
        let (data_to_compress, filter_info) = if self.options.filter.is_active() {
            let filtered = self.filter_data(data)?.unwrap_or_else(|| data.to_vec());
            let filtered_size = filtered.len() as u64;
            let info = FilteredFolderInfo {
                filter_method: self.options.filter.method_id().unwrap_or(&[]).to_vec(),
                filter_properties: self.options.filter.properties(),
                filtered_size,
            };
            (filtered, Some(info))
        } else {
            (data.to_vec(), None)
        };

        // Compress the (possibly filtered) data
        let compressed = self.compress_data(&data_to_compress)?;
        Ok((compressed, filter_info))
    }

    /// Filters, compresses, and encrypts data.
    #[cfg(feature = "aes")]
    pub(crate) fn filter_compress_and_encrypt_data(
        &self,
        data: &[u8],
    ) -> Result<(Vec<u8>, Option<FilteredFolderInfo>, EncryptedFolderInfo)> {
        use crate::crypto::{Aes256Encoder, AesProperties, derive_key};

        // Apply filter if configured
        let (data_to_compress, filter_info) = if self.options.filter.is_active() {
            let filtered = self.filter_data(data)?.unwrap_or_else(|| data.to_vec());
            let filtered_size = filtered.len() as u64;
            let info = FilteredFolderInfo {
                filter_method: self.options.filter.method_id().unwrap_or(&[]).to_vec(),
                filter_properties: self.options.filter.properties(),
                filtered_size,
            };
            (filtered, Some(info))
        } else {
            (data.to_vec(), None)
        };

        // Compress the (possibly filtered) data
        let compressed = self.compress_data(&data_to_compress)?;
        let compressed_size = compressed.len() as u64;

        // Encrypt
        let password = self
            .options
            .password
            .as_ref()
            .ok_or_else(|| Error::InvalidFormat("encryption requires a password".into()))?;

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

        let aes_properties =
            AesProperties::encode(self.options.nonce_policy.num_cycles_power(), &salt, &iv);

        Ok((
            encrypted,
            filter_info,
            EncryptedFolderInfo {
                aes_properties,
                compressed_size,
            },
        ))
    }
}
