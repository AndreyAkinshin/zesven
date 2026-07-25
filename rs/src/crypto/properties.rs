//! AES properties parsing and encoding.

use crate::{Error, Result};

/// Parsed AES encryption properties from 7z coder info.
#[derive(Debug, Clone)]
pub struct AesProperties {
    /// Number of SHA-256 iterations = 2^num_cycles_power.
    pub num_cycles_power: u8,
    /// Salt for key derivation (0-16 bytes).
    pub salt: Vec<u8>,
    /// Initialization vector (padded to 16 bytes).
    pub iv: Vec<u8>,
}

impl AesProperties {
    /// Parses AES properties from the coder property bytes.
    ///
    /// The format is:
    /// - Byte 0: (salt_flag << 7) | (iv_flag << 6) | num_cycles_power
    /// - Byte 1: (salt_size_extra << 4) | iv_size_extra
    /// - Remaining bytes: salt followed by IV
    ///
    /// Where:
    /// - salt_size = salt_flag + salt_size_extra (if salt_flag=1) or 0
    /// - iv_size = iv_flag + iv_size_extra (if iv_flag=1) or 0
    pub fn parse(properties: &[u8]) -> Result<Self> {
        let first_byte = *properties
            .first()
            .ok_or_else(|| Error::InvalidFormat("AES properties are empty".into()))?;

        let num_cycles_power = first_byte & 0x3F;
        let salt_flag = (first_byte >> 7) & 1;
        let iv_flag = (first_byte >> 6) & 1;

        // With neither a salt nor an IV the blob is a single byte and there is
        // no size byte to read. Demanding two rejected archives that 7-Zip
        // reads without complaint.
        if salt_flag == 0 && iv_flag == 0 {
            if properties.len() != 1 {
                return Err(Error::InvalidFormat(format!(
                    "AES properties declare no salt and no IV but carry {} bytes",
                    properties.len()
                )));
            }
            return Ok(Self {
                num_cycles_power,
                salt: Vec::new(),
                iv: vec![0u8; 16],
            });
        }

        let second_byte = *properties.get(1).ok_or_else(|| {
            Error::InvalidFormat(
                "AES properties declare a salt or IV but carry no size byte".into(),
            )
        })?;

        let salt_size_extra = (second_byte >> 4) & 0x0F;
        let iv_size_extra = second_byte & 0x0F;

        let salt_size = if salt_flag == 1 {
            (1 + salt_size_extra) as usize
        } else {
            0
        };

        let iv_size = if iv_flag == 1 {
            (1 + iv_size_extra) as usize
        } else {
            0
        };

        let data_start = 2;
        let salt_end = data_start + salt_size;
        let iv_end = salt_end + iv_size;

        // The blob is exactly the header plus the salt plus the IV. Accepting a
        // longer one means accepting a blob whose sizes disagree with its
        // contents, and then decrypting with whatever that leaves as the IV.
        if properties.len() != iv_end {
            return Err(Error::InvalidFormat(format!(
                "AES properties declare {} bytes of salt and IV but carry {}",
                iv_end - data_start,
                properties.len().saturating_sub(data_start)
            )));
        }

        let salt = properties[data_start..salt_end].to_vec();

        // IV is padded to 16 bytes with zeros
        let mut iv = vec![0u8; 16];
        let iv_data = &properties[salt_end..iv_end];
        iv[..iv_data.len()].copy_from_slice(iv_data);

        Ok(Self {
            num_cycles_power,
            salt,
            iv,
        })
    }

    /// Encodes AES properties to bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the salt exceeds 16 bytes, which is the largest the
    /// size nibble can express. Truncating it instead produced an archive whose
    /// declared salt and actual salt disagreed, and which nothing could decrypt.
    pub fn encode(num_cycles_power: u8, salt: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
        if salt.len() > 16 {
            return Err(Error::InvalidFormat(format!(
                "salt is {} bytes; the 7z AES property encoding allows at most 16",
                salt.len()
            )));
        }

        let salt_size = salt.len();
        let iv_size = iv.len().min(16);

        let salt_flag = if salt_size > 0 { 1u8 } else { 0u8 };
        let iv_flag = if iv_size > 0 { 1u8 } else { 0u8 };

        let salt_size_extra = if salt_size > 0 {
            (salt_size - 1) as u8
        } else {
            0
        };
        let iv_size_extra = if iv_size > 0 { (iv_size - 1) as u8 } else { 0 };

        let first_byte = (salt_flag << 7) | (iv_flag << 6) | (num_cycles_power & 0x3F);
        let second_byte = (salt_size_extra << 4) | iv_size_extra;

        let mut result = vec![first_byte, second_byte];
        result.extend_from_slice(salt);
        result.extend_from_slice(&iv[..iv_size]);

        Ok(result)
    }
}

/// Policy for generating salt and IV for encryption.
///
/// # Security Considerations
///
/// [`Random`][Self::Random], the default, draws from the operating system's
/// CSPRNG. AES-CBC needs an unpredictable IV, so this is the variant to use for
/// anything that matters.
///
/// [`Deterministic`][Self::Deterministic] and [`Explicit`][Self::Explicit] hand
/// nonce generation to the caller. Both reuse the same IV whenever they are given
/// the same input, which leaks whether two encrypted streams start with the same
/// bytes; they exist for reproducible builds and for callers who bring their own
/// CSPRNG, not as a way to make archives smaller or faster.
///
/// | Use Case | Recommended Policy |
/// |----------|--------------------|
/// | Anything encrypted for real | [`Random`][Self::Random] (default) |
/// | Reproducible builds | [`Deterministic`][Self::Deterministic] |
/// | Caller-supplied nonces | [`Explicit`][Self::Explicit] |
#[derive(Debug, Clone)]
pub enum NoncePolicy {
    /// Generate salt and IV from the operating system's CSPRNG.
    Random {
        /// Number of iterations for key derivation (2^num_cycles_power).
        num_cycles_power: u8,
        /// Salt size (0-16 bytes).
        salt_size: usize,
    },
    /// Generate deterministic salt and IV from a seed.
    Deterministic {
        /// Number of iterations for key derivation.
        num_cycles_power: u8,
        /// Seed for deterministic generation.
        seed: [u8; 32],
    },
    /// Use explicit salt and IV values.
    Explicit {
        /// Number of iterations for key derivation.
        num_cycles_power: u8,
        /// Salt bytes.
        salt: Vec<u8>,
        /// IV bytes.
        iv: Vec<u8>,
    },
}

impl Default for NoncePolicy {
    fn default() -> Self {
        Self::Random {
            num_cycles_power: 19, // 2^19 = 524288 iterations (7-Zip default)
            salt_size: 8,
        }
    }
}

impl NoncePolicy {
    /// Creates a random nonce policy with default parameters.
    pub fn random() -> Self {
        Self::default()
    }

    /// Creates a random nonce policy with specified parameters.
    pub fn random_with_params(num_cycles_power: u8, salt_size: usize) -> Self {
        Self::Random {
            num_cycles_power,
            salt_size: salt_size.min(16),
        }
    }

    /// Creates an explicit nonce policy.
    pub fn explicit(num_cycles_power: u8, salt: Vec<u8>, iv: Vec<u8>) -> Self {
        Self::Explicit {
            num_cycles_power,
            salt,
            iv,
        }
    }

    /// Returns the num_cycles_power for this policy.
    pub fn num_cycles_power(&self) -> u8 {
        match self {
            Self::Random {
                num_cycles_power, ..
            } => *num_cycles_power,
            Self::Deterministic {
                num_cycles_power, ..
            } => *num_cycles_power,
            Self::Explicit {
                num_cycles_power, ..
            } => *num_cycles_power,
        }
    }

    /// Generates salt and IV according to the policy.
    ///
    /// # Returns
    ///
    /// A tuple of (salt, iv).
    pub fn generate(&self) -> Result<(Vec<u8>, [u8; 16])> {
        match self {
            Self::Random { salt_size, .. } => {
                let mut salt = vec![0u8; *salt_size];
                let mut iv = [0u8; 16];

                getrandom::getrandom(&mut salt)
                    .map_err(|e| Error::InvalidFormat(format!("salt generation failed: {e}")))?;
                getrandom::getrandom(&mut iv)
                    .map_err(|e| Error::InvalidFormat(format!("IV generation failed: {e}")))?;

                Ok((salt, iv))
            }
            Self::Deterministic { seed, .. } => {
                // Use the seed to generate deterministic salt and IV
                use sha2::{Digest, Sha256};

                let mut hasher = Sha256::new();
                hasher.update(seed);
                hasher.update(b"salt");
                let salt_hash = hasher.finalize();

                let mut hasher = Sha256::new();
                hasher.update(seed);
                hasher.update(b"iv");
                let iv_hash = hasher.finalize();

                let salt = salt_hash[..8].to_vec();
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&iv_hash[..16]);

                Ok((salt, iv))
            }
            Self::Explicit { salt, iv, .. } => {
                let mut iv_arr = [0u8; 16];
                let iv_len = iv.len().min(16);
                iv_arr[..iv_len].copy_from_slice(&iv[..iv_len]);
                Ok((salt.clone(), iv_arr))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_properties() {
        // With neither salt nor IV the blob is a single byte: there is no size
        // nibble to describe sizes that are not there.
        let parsed = AesProperties::parse(&[0x13]).unwrap();
        assert_eq!(parsed.num_cycles_power, 19);
        assert!(parsed.salt.is_empty());
        assert_eq!(parsed.iv, vec![0u8; 16]);

        // A size byte with no sizes to describe means the blob and its header
        // disagree.
        assert!(AesProperties::parse(&[0x13, 0x00]).is_err());
    }

    #[test]
    fn test_parse_with_salt_and_iv() {
        // salt_flag=1, iv_flag=1, num_cycles_power=19
        // salt_size_extra=7 (8 bytes total), iv_size_extra=15 (16 bytes total)
        let mut props = vec![0xD3, 0x7F]; // 0xD3 = 1101_0011, 0x7F = 0111_1111
        props.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]); // salt
        props.extend_from_slice(&[
            9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        ]); // IV

        let parsed = AesProperties::parse(&props).unwrap();
        assert_eq!(parsed.num_cycles_power, 19);
        assert_eq!(parsed.salt, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            parsed.iv,
            vec![
                9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24
            ]
        );
    }

    #[test]
    fn test_parse_too_short() {
        // A single byte is the legal shape when neither salt nor IV is present:
        // 7-Zip reads it, so this must too.
        let decoded = AesProperties::parse(&[0x13]).expect("a bare cycle count is legal");
        assert_eq!(decoded.num_cycles_power, 0x13);
        assert!(decoded.salt.is_empty());

        // A blob that declares a salt but stops short is not.
        assert!(AesProperties::parse(&[0x93]).is_err());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let salt = vec![1, 2, 3, 4];
        let iv = vec![5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let num_cycles_power = 19;

        let encoded = AesProperties::encode(num_cycles_power, &salt, &iv).unwrap();
        let decoded = AesProperties::parse(&encoded).unwrap();

        assert_eq!(decoded.num_cycles_power, num_cycles_power);
        assert_eq!(decoded.salt, salt);
        // IV is padded to 16 bytes
        let mut expected_iv = iv.clone();
        expected_iv.resize(16, 0);
        assert_eq!(decoded.iv, expected_iv);
    }

    /// A salt the encoding cannot express must be refused, not truncated.
    #[test]
    fn test_encode_rejects_oversized_salt() {
        let salt = vec![0u8; 17];
        let iv = vec![0u8; 16];

        let error = AesProperties::encode(19, &salt, &iv)
            .expect_err("a 17-byte salt does not fit the size nibble");
        assert!(error.to_string().contains("at most 16"), "{error}");
    }

    /// Trailing bytes mean the declared sizes and the contents disagree.
    #[test]
    fn test_parse_rejects_trailing_bytes() {
        let mut encoded = AesProperties::encode(19, &[1, 2, 3, 4], &[7u8; 16]).unwrap();
        encoded.push(0xFF);

        assert!(AesProperties::parse(&encoded).is_err());
    }

    #[test]
    fn test_nonce_policy_explicit() {
        let policy = NoncePolicy::explicit(19, vec![1, 2, 3], vec![4, 5, 6, 7]);
        let (salt, iv) = policy.generate().unwrap();
        assert_eq!(salt, vec![1, 2, 3]);
        assert_eq!(&iv[..4], &[4, 5, 6, 7]);
    }

    #[test]
    fn test_nonce_policy_deterministic() {
        let seed = [42u8; 32];
        let policy = NoncePolicy::Deterministic {
            num_cycles_power: 19,
            seed,
        };

        let (salt1, iv1) = policy.generate().unwrap();
        let (salt2, iv2) = policy.generate().unwrap();

        // Should produce same results
        assert_eq!(salt1, salt2);
        assert_eq!(iv1, iv2);
    }
}
