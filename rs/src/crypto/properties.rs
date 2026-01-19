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
        if properties.len() < 2 {
            return Err(Error::InvalidFormat(
                "AES properties too short (need at least 2 bytes)".into(),
            ));
        }

        let first_byte = properties[0];
        let second_byte = properties[1];

        let num_cycles_power = first_byte & 0x3F;
        let salt_flag = (first_byte >> 7) & 1;
        let iv_flag = (first_byte >> 6) & 1;

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

        if properties.len() < iv_end {
            return Err(Error::InvalidFormat(format!(
                "AES properties too short: expected {} bytes, got {}",
                iv_end,
                properties.len()
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
    pub fn encode(num_cycles_power: u8, salt: &[u8], iv: &[u8]) -> Vec<u8> {
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
        result.extend_from_slice(&salt[..salt_size.min(16)]);
        result.extend_from_slice(&iv[..iv_size]);

        result
    }
}

/// Policy for generating salt and IV for encryption.
///
/// # Security Considerations
///
/// The [`Random`][Self::Random] variant uses weak entropy sources (system time and
/// thread ID) rather than a cryptographically secure random number generator (CSPRNG).
/// This provides basic unpredictability but **is not suitable for high-security
/// applications**.
///
/// For security-critical use cases, prefer [`Explicit`][Self::Explicit] with values
/// generated from a proper CSPRNG like `getrandom` or `rand::rngs::OsRng`.
///
/// ## Entropy Sources in Random Mode
///
/// The random mode mixes:
/// - System time (nanoseconds since Unix epoch)
/// - Thread ID hash
///
/// These sources can be predicted in some scenarios:
/// - Time is observable to an attacker
/// - Thread IDs are predictable in single-threaded programs
/// - No hardware entropy is used
///
/// ## Recommendations
///
/// | Use Case | Recommended Policy |
/// |----------|--------------------|
/// | Development/testing | [`Random`][Self::Random] (default) |
/// | Production archives | [`Explicit`][Self::Explicit] with CSPRNG |
/// | Reproducible builds | [`Deterministic`][Self::Deterministic] |
#[derive(Debug, Clone)]
pub enum NoncePolicy {
    /// Generate salt and IV using weak entropy sources.
    ///
    /// **Warning:** Uses system time and thread ID, NOT a CSPRNG.
    /// See [`NoncePolicy`] docs for security implications.
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
                use std::time::{SystemTime, UNIX_EPOCH};

                // WARNING: This uses weak entropy sources (time + thread ID).
                // For security-critical applications, use NoncePolicy::Explicit
                // with values from a proper CSPRNG (e.g., getrandom, OsRng).
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();

                let mut salt = vec![0u8; *salt_size];
                let mut iv = [0u8; 16];

                // Mix time-based entropy into salt and IV
                // Use different parts of the timestamp for salt and IV
                for (i, byte) in salt.iter_mut().enumerate() {
                    let shift = (i % 16) * 8;
                    *byte = ((now >> shift) & 0xFF) as u8;
                }

                // XOR thread_id into IV for additional entropy
                let thread_id = std::thread::current().id();
                let thread_hash = {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    thread_id.hash(&mut hasher);
                    hasher.finish()
                };

                for (i, byte) in iv.iter_mut().enumerate() {
                    let shift = (i % 16) * 8;
                    let time_byte = ((now >> shift) & 0xFF) as u8;
                    let thread_byte = ((thread_hash >> (i % 8 * 8)) & 0xFF) as u8;
                    *byte = time_byte ^ thread_byte ^ (i as u8);
                }

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
        // Minimal: no salt, no IV, cycles=19
        let props = vec![0x13, 0x00]; // num_cycles_power = 19
        let parsed = AesProperties::parse(&props).unwrap();
        assert_eq!(parsed.num_cycles_power, 19);
        assert!(parsed.salt.is_empty());
        assert_eq!(parsed.iv, vec![0u8; 16]);
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
        let props = vec![0x13]; // Only 1 byte
        assert!(AesProperties::parse(&props).is_err());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let salt = vec![1, 2, 3, 4];
        let iv = vec![5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let num_cycles_power = 19;

        let encoded = AesProperties::encode(num_cycles_power, &salt, &iv);
        let decoded = AesProperties::parse(&encoded).unwrap();

        assert_eq!(decoded.num_cycles_power, num_cycles_power);
        assert_eq!(decoded.salt, salt);
        // IV is padded to 16 bytes
        let mut expected_iv = iv.clone();
        expected_iv.resize(16, 0);
        assert_eq!(decoded.iv, expected_iv);
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
