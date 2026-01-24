//! AES-256 encryption support for 7z archives.
//!
//! This module implements the 7z AES-256-SHA256 encryption scheme which uses:
//! - SHA-256 iterated key derivation from password
//! - AES-256-CBC for data encryption
//! - PKCS7 padding
//!
//! # Key Derivation Caching
//!
//! Key derivation is computationally expensive (e.g., 524,288 SHA-256 iterations
//! for num_cycles_power=19). Use [`KeyCache`] to avoid re-deriving the same key
//! when processing multiple entries with the same password/salt combination.

mod password;
mod properties;

use aes::Aes256;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::sync::Mutex;

use crate::Result;
use crate::s3fifo::S3FifoCache;

pub use password::Password;
pub use properties::{AesProperties, NoncePolicy};

type Aes256CbcDec = cbc::Decryptor<Aes256>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;

/// Acquires a mutex lock, recovering from poisoned state if necessary.
///
/// Key cache data can be safely recovered because:
/// - Cached keys are deterministically derivable from password/salt
/// - Statistics are non-critical diagnostic information
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        log::warn!("KeyCache mutex was poisoned, recovering");
        poisoned.into_inner()
    })
}

/// AES block size in bytes.
const BLOCK_SIZE: usize = 16;

/// Maximum allowed value for `num_cycles_power` in key derivation.
///
/// This limits key derivation to 2^30 = ~1 billion iterations, which takes
/// several seconds on modern hardware. Higher values are rejected to prevent
/// denial-of-service attacks via malicious archives with extreme iteration counts.
///
/// For reference:
/// - `num_cycles_power = 19`: 524,288 iterations (typical 7z default, ~10ms)
/// - `num_cycles_power = 24`: 16,777,216 iterations (~300ms)
/// - `num_cycles_power = 30`: 1,073,741,824 iterations (~20s, our limit)
/// - `num_cycles_power = 63`: Would require ~292 years at 1B iterations/sec
pub const MAX_NUM_CYCLES_POWER: u8 = 30;

/// Derives an AES-256 key from a password using 7z's SHA-256 iteration scheme.
///
/// # Arguments
///
/// * `password` - The password to derive the key from
/// * `salt` - Salt bytes (0-16 bytes)
/// * `num_cycles_power` - Number of iterations = 2^num_cycles_power
///
/// # Returns
///
/// A 32-byte key suitable for AES-256.
///
/// # Errors
///
/// Returns [`crate::Error::ResourceLimitExceeded`] if `num_cycles_power` exceeds
/// [`MAX_NUM_CYCLES_POWER`] (30), which would require over 1 billion iterations.
/// This prevents denial-of-service attacks via malicious archives.
pub fn derive_key(password: &Password, salt: &[u8], num_cycles_power: u8) -> Result<[u8; 32]> {
    if num_cycles_power > MAX_NUM_CYCLES_POWER {
        log::warn!(
            "Key derivation cycles_power {} exceeds maximum {}, rejecting",
            num_cycles_power,
            MAX_NUM_CYCLES_POWER
        );
        return Err(crate::Error::ResourceLimitExceeded(format!(
            "key derivation cycles_power {} exceeds maximum {} (would require {} iterations)",
            num_cycles_power,
            MAX_NUM_CYCLES_POWER,
            1u64.checked_shl(num_cycles_power as u32)
                .unwrap_or(u64::MAX)
        )));
    }

    let iterations = 1u64 << num_cycles_power;
    let password_bytes = password.as_utf16_le();

    // 7z uses a streaming hash approach
    let mut hash_input = Vec::with_capacity(salt.len() + password_bytes.len() + 8);
    let mut sha = Sha256::new();

    for i in 0..iterations {
        hash_input.clear();
        hash_input.extend_from_slice(salt);
        hash_input.extend_from_slice(&password_bytes);
        hash_input.extend_from_slice(&i.to_le_bytes());
        sha.update(&hash_input);
    }

    Ok(sha.finalize().into())
}

/// Cache key type: (password_hash, salt_bytes, num_cycles_power)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    /// Hash of the password for lookup (we don't store the actual password)
    password_hash: [u8; 32],
    /// Salt bytes (up to 16 bytes)
    salt: Vec<u8>,
    /// Number of cycles power
    num_cycles_power: u8,
}

impl CacheKey {
    fn new(password: &Password, salt: &[u8], num_cycles_power: u8) -> Self {
        // Hash the password for use as cache key (security: don't store plaintext)
        let password_bytes = password.as_utf16_le();
        let password_hash: [u8; 32] = Sha256::digest(&password_bytes).into();

        Self {
            password_hash,
            salt: salt.to_vec(),
            num_cycles_power,
        }
    }
}

/// Cache for derived AES keys.
///
/// Key derivation is expensive (e.g., 524,288 SHA-256 iterations for typical 7z files).
/// This cache stores derived keys to avoid re-computation when processing multiple
/// entries in the same archive with the same password.
///
/// # Thread Safety
///
/// The cache is thread-safe and can be shared across threads using `Arc<KeyCache>`.
///
/// # Example
///
/// ```rust,ignore
/// use zesven::crypto::{KeyCache, Password};
///
/// let cache = KeyCache::new(8);
/// let password = Password::new("secret");
/// let salt = [0u8; 8];
///
/// // First call derives the key (expensive)
/// let key1 = cache.derive_key(&password, &salt, 19);
///
/// // Second call returns cached key (fast)
/// let key2 = cache.derive_key(&password, &salt, 19);
///
/// assert_eq!(key1, key2);
/// ```
pub struct KeyCache {
    cache: Mutex<S3FifoCache<CacheKey, [u8; 32]>>,
    stats: Mutex<CacheStats>,
}

/// Statistics for key cache usage.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Total iterations avoided by caching.
    pub iterations_saved: u64,
}

impl CacheStats {
    /// Returns the cache hit ratio (0.0 to 1.0).
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl KeyCache {
    /// Creates a new key cache with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of keys to cache. A typical archive has
    ///   one password/salt combination, but multi-password scenarios exist.
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);
        Self {
            cache: Mutex::new(S3FifoCache::new(cap)),
            stats: Mutex::new(CacheStats::default()),
        }
    }

    /// Derives a key, using the cache if available.
    ///
    /// If the key for the given password/salt/cycles combination is cached,
    /// it is returned immediately. Otherwise, the key is derived and cached.
    ///
    /// # Errors
    ///
    /// Returns an error if `num_cycles_power` exceeds [`MAX_NUM_CYCLES_POWER`].
    pub fn derive_key(
        &self,
        password: &Password,
        salt: &[u8],
        num_cycles_power: u8,
    ) -> Result<[u8; 32]> {
        let cache_key = CacheKey::new(password, salt, num_cycles_power);

        // Check cache first
        {
            let mut cache = lock_or_recover(&self.cache);
            if let Some(&key) = cache.get(&cache_key) {
                let mut stats = lock_or_recover(&self.stats);
                stats.hits += 1;
                stats.iterations_saved += 1u64 << num_cycles_power;
                return Ok(key);
            }
        }

        // Derive key (expensive)
        let key = derive_key(password, salt, num_cycles_power)?;

        // Store in cache
        {
            let mut cache = lock_or_recover(&self.cache);
            cache.insert(cache_key, key);

            let mut stats = lock_or_recover(&self.stats);
            stats.misses += 1;
        }

        Ok(key)
    }

    /// Returns the cache statistics.
    pub fn stats(&self) -> CacheStats {
        lock_or_recover(&self.stats).clone()
    }

    /// Resets the cache statistics.
    pub fn reset_stats(&self) {
        *lock_or_recover(&self.stats) = CacheStats::default();
    }

    /// Clears all cached keys.
    pub fn clear(&self) {
        lock_or_recover(&self.cache).clear();
    }

    /// Returns the current number of cached keys.
    pub fn len(&self) -> usize {
        lock_or_recover(&self.cache).len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        lock_or_recover(&self.cache).is_empty()
    }
}

/// Validates if decrypted data looks like valid compression header.
///
/// This function performs early detection of wrong passwords by checking if
/// the first bytes of decrypted data match expected compression header patterns.
///
/// # Supported Compression Methods
///
/// - **LZMA**: Validates the properties byte (must satisfy lc < 9, lp < 5, pb < 5)
/// - **LZMA2**: Validates the control byte patterns (0x00 = end, 0x01-0x7F = uncompressed, 0x80+ = compressed)
/// - **Deflate**: Validates the first bits match valid deflate block types
/// - **Copy**: Any data is valid (no header to check)
///
/// # Arguments
///
/// * `decrypted_data` - The first block of decrypted data (at least 16 bytes recommended)
/// * `compression_method` - The method ID of the compression used after encryption
///
/// # Returns
///
/// `true` if the data looks like valid compression header, `false` if it's likely garbage
/// from a wrong password.
pub fn validate_decrypted_header(decrypted_data: &[u8], compression_method: &[u8]) -> bool {
    if decrypted_data.is_empty() {
        return false;
    }

    // Method IDs (from codec/mod.rs)
    const LZMA: &[u8] = &[0x03, 0x01, 0x01];
    const LZMA2: &[u8] = &[0x21];
    const DEFLATE: &[u8] = &[0x04, 0x01, 0x08];
    const BZIP2: &[u8] = &[0x04, 0x02, 0x02];
    const PPMD: &[u8] = &[0x03, 0x04, 0x01];
    const COPY: &[u8] = &[0x00];

    match compression_method {
        LZMA => validate_lzma_header(decrypted_data),
        LZMA2 => validate_lzma2_header(decrypted_data),
        DEFLATE => validate_deflate_header(decrypted_data),
        BZIP2 => validate_bzip2_header(decrypted_data),
        PPMD => validate_ppmd_header(decrypted_data),
        COPY => true, // Copy method has no header to validate
        _ => true,    // Unknown methods - can't validate, assume OK
    }
}

/// Validates LZMA header.
/// LZMA properties byte encodes: lc + lp * 9 + pb * 45
/// where lc < 9, lp < 5, pb < 5
fn validate_lzma_header(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    let props_byte = data[0];

    // Decode and validate LZMA properties
    // props_byte = lc + lp * 9 + pb * 45
    // Valid range: 0 <= props_byte < 9 + 5*9 + 5*45 = 9 + 45 + 225 = 279
    // But typically props_byte < 9*5*5 = 225 since pb < 5
    // Common values: 0x5D (93), 0x00 (0), etc.

    let pb = props_byte / 45;
    let remainder = props_byte % 45;
    let lp = remainder / 9;
    let lc = remainder % 9;

    // Validate constraints
    if pb >= 5 || lp >= 5 || lc >= 9 {
        return false;
    }

    // Additional heuristic: dictionary size should be reasonable
    // (if we have enough data)
    if data.len() >= 5 {
        let dict_size = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        // Dict size should be power of 2 or close to it, and <= 1GB
        if dict_size > 1 << 30 {
            return false;
        }
    }

    true
}

/// Validates LZMA2 control byte.
/// LZMA2 chunks start with a control byte:
/// - 0x00: End of stream
/// - 0x01-0x7F: Uncompressed chunk
/// - 0x80-0xFF: LZMA compressed chunk
fn validate_lzma2_header(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    let control = data[0];

    // Valid control byte patterns:
    // 0x00 = end marker (valid but unusual as first byte)
    // 0x01 = uncompressed, reset dictionary
    // 0x02 = uncompressed, no dictionary reset
    // 0x80-0xFF = compressed chunks with various flags

    // The control byte should follow these patterns:
    // - Low 7 bits (0x01-0x02) for uncompressed
    // - High bit set (0x80+) for compressed

    // Invalid patterns: 0x03-0x7F (reserved)
    if (0x03..0x80).contains(&control) {
        return false;
    }

    true
}

/// Validates Deflate stream header.
/// Deflate blocks start with 3 bits: BFINAL (1 bit) + BTYPE (2 bits)
/// BTYPE: 00 = stored, 01 = fixed Huffman, 10 = dynamic Huffman, 11 = reserved (invalid)
fn validate_deflate_header(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    let first_byte = data[0];

    // Extract BTYPE (bits 1-2)
    let btype = (first_byte >> 1) & 0x03;

    // BTYPE 11 is reserved and invalid
    if btype == 3 {
        return false;
    }

    true
}

/// Validates BZip2 header.
/// BZip2 streams start with 'BZ' magic.
fn validate_bzip2_header(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }

    // BZip2 starts with 'BZ' followed by version ('h') and block size ('1'-'9')
    data[0] == b'B' && data[1] == b'Z'
}

/// Validates PPMd header.
/// PPMd in 7z format has specific property encoding.
fn validate_ppmd_header(_data: &[u8]) -> bool {
    // PPMd doesn't have a distinctive header in the data stream itself
    // The properties are in the coder properties, not the data
    // So we can't reliably validate from data alone
    true
}

/// AES-256 decoder for reading encrypted streams.
pub struct Aes256Decoder<R> {
    inner: R,
    buffer: Vec<u8>,
    pos: usize,
    key: [u8; 32],
    iv: [u8; 16],
    finished: bool,
}

impl<R> std::fmt::Debug for Aes256Decoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Aes256Decoder").finish_non_exhaustive()
    }
}

impl<R: Read + Send> Aes256Decoder<R> {
    /// Creates a new AES-256 decoder.
    ///
    /// # Arguments
    ///
    /// * `input` - The encrypted data source
    /// * `properties` - AES properties from the coder specification
    /// * `password` - The password to decrypt with
    ///
    /// # Errors
    ///
    /// Returns an error if properties are invalid or if `num_cycles_power`
    /// exceeds [`MAX_NUM_CYCLES_POWER`].
    pub fn new(input: R, properties: &[u8], password: &Password) -> Result<Self> {
        let props = AesProperties::parse(properties)?;
        let key = derive_key(password, &props.salt, props.num_cycles_power)?;

        let mut iv = [0u8; 16];
        let iv_len = props.iv.len().min(16);
        iv[..iv_len].copy_from_slice(&props.iv[..iv_len]);

        Ok(Self {
            inner: input,
            buffer: Vec::new(),
            pos: 0,
            key,
            iv,
            finished: false,
        })
    }

    /// Creates a decoder with explicit key and IV.
    pub fn with_key_iv(input: R, key: [u8; 32], iv: [u8; 16]) -> Self {
        Self {
            inner: input,
            buffer: Vec::new(),
            pos: 0,
            key,
            iv,
            finished: false,
        }
    }

    /// Validates the password by decrypting the first block and checking if it
    /// looks like valid compression data.
    ///
    /// This method provides early detection of wrong passwords without needing
    /// to decompress the entire stream. It reads and decrypts the first block,
    /// then checks if the decrypted data matches expected compression header patterns.
    ///
    /// # Arguments
    ///
    /// * `compression_method` - The method ID of the compression used after encryption
    ///
    /// # Returns
    ///
    /// `true` if the decrypted data looks valid, `false` if it appears to be garbage
    /// (indicating wrong password).
    ///
    /// # Note
    ///
    /// This method consumes the first block of data. After calling this, you should
    /// either continue reading from the decoder (the validated data is buffered) or
    /// create a new decoder if validation fails.
    pub fn validate_first_block(&mut self, compression_method: &[u8]) -> io::Result<bool> {
        // Ensure we have data in the buffer
        if self.buffer.is_empty() && !self.finished {
            self.decrypt_buffer()?;
        }

        if self.buffer.is_empty() {
            // No data to validate - this is unusual but not necessarily wrong
            return Ok(true);
        }

        // Validate the decrypted data against expected compression header
        Ok(validate_decrypted_header(&self.buffer, compression_method))
    }

    /// Returns a reference to the currently buffered decrypted data.
    ///
    /// This can be used after `validate_first_block()` to inspect the decrypted data.
    pub fn buffered_data(&self) -> &[u8] {
        &self.buffer[self.pos..]
    }

    fn decrypt_buffer(&mut self) -> io::Result<()> {
        // Read up to 4KB at a time (must be multiple of 16)
        let mut encrypted = vec![0u8; 4096];
        let n = self.inner.read(&mut encrypted)?;

        if n == 0 {
            self.finished = true;
            return Ok(());
        }

        // AES-CBC requires 16-byte alignment
        let aligned_len = (n / BLOCK_SIZE) * BLOCK_SIZE;
        if aligned_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encrypted data not block-aligned",
            ));
        }

        encrypted.truncate(aligned_len);

        // Save the last block for IV update before decrypting
        let next_iv: [u8; 16] = if encrypted.len() >= BLOCK_SIZE {
            encrypted[encrypted.len() - BLOCK_SIZE..]
                .try_into()
                .expect("slice is exactly BLOCK_SIZE bytes after length check")
        } else {
            self.iv
        };

        // Decrypt in place
        let decryptor = Aes256CbcDec::new(&self.key.into(), &self.iv.into());
        let decrypted = decryptor
            .decrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut encrypted)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        // Update IV for next block (CBC mode uses last ciphertext block as next IV)
        self.iv = next_iv;

        self.buffer = decrypted.to_vec();
        self.pos = 0;

        Ok(())
    }
}

impl<R: Read + Send> Read for Aes256Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.buffer.len() && !self.finished {
            self.decrypt_buffer()?;
        }

        if self.pos >= self.buffer.len() {
            return Ok(0);
        }

        let available = &self.buffer[self.pos..];
        let to_copy = available.len().min(buf.len());
        buf[..to_copy].copy_from_slice(&available[..to_copy]);
        self.pos += to_copy;

        Ok(to_copy)
    }
}

/// AES-256 encoder for writing encrypted streams.
pub struct Aes256Encoder<W> {
    inner: W,
    buffer: Vec<u8>,
    key: [u8; 32],
    iv: [u8; 16],
}

impl<W> std::fmt::Debug for Aes256Encoder<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Aes256Encoder").finish_non_exhaustive()
    }
}

impl<W: Write + Send> Aes256Encoder<W> {
    /// Creates a new AES-256 encoder.
    ///
    /// # Arguments
    ///
    /// * `output` - The destination for encrypted data
    /// * `password` - The password to encrypt with
    /// * `nonce_policy` - Policy for generating salt and IV
    ///
    /// # Errors
    ///
    /// Returns an error if nonce generation fails or if `num_cycles_power`
    /// exceeds [`MAX_NUM_CYCLES_POWER`].
    pub fn new(output: W, password: &Password, nonce_policy: &NoncePolicy) -> Result<Self> {
        let (salt, iv) = nonce_policy.generate()?;
        let key = derive_key(password, &salt, nonce_policy.num_cycles_power())?;

        Ok(Self {
            inner: output,
            buffer: Vec::new(),
            key,
            iv,
        })
    }

    /// Creates an encoder with explicit key and IV.
    pub fn with_key_iv(output: W, key: [u8; 32], iv: [u8; 16]) -> Self {
        Self {
            inner: output,
            buffer: Vec::new(),
            key,
            iv,
        }
    }

    /// Returns the AES properties for this encoder.
    pub fn properties(&self, salt: &[u8], num_cycles_power: u8) -> Vec<u8> {
        AesProperties::encode(num_cycles_power, salt, &self.iv)
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Encrypt complete blocks
        let complete_blocks = (self.buffer.len() / BLOCK_SIZE) * BLOCK_SIZE;
        if complete_blocks == 0 {
            return Ok(());
        }

        let mut to_encrypt = self.buffer[..complete_blocks].to_vec();
        let encryptor = Aes256CbcEnc::new(&self.key.into(), &self.iv.into());

        let encrypted = encryptor
            .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(
                &mut to_encrypt,
                complete_blocks,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        self.inner.write_all(encrypted)?;

        // Update IV for next block
        if encrypted.len() >= BLOCK_SIZE {
            self.iv
                .copy_from_slice(&encrypted[encrypted.len() - BLOCK_SIZE..]);
        }

        // Keep remaining bytes
        self.buffer = self.buffer[complete_blocks..].to_vec();

        Ok(())
    }

    /// Finishes encoding, applying PKCS7 padding and encrypting final block.
    pub fn finish(mut self) -> io::Result<W> {
        // Flush complete blocks first
        self.flush_buffer()?;

        // Apply PKCS7 padding to remaining data
        let pad_len = BLOCK_SIZE - (self.buffer.len() % BLOCK_SIZE);
        self.buffer
            .extend(std::iter::repeat_n(pad_len as u8, pad_len));

        // Encrypt final padded block
        let buffer_len = self.buffer.len();
        let encryptor = Aes256CbcEnc::new(&self.key.into(), &self.iv.into());
        let encrypted = encryptor
            .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(
                &mut self.buffer,
                buffer_len,
            )
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        self.inner.write_all(encrypted)?;
        self.inner.flush()?;

        Ok(self.inner)
    }
}

impl<W: Write + Send> Write for Aes256Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);

        // Encrypt complete blocks as we accumulate them
        if self.buffer.len() >= 4096 {
            self.flush_buffer()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_buffer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_derive_key() {
        let password = Password::new("test");
        let salt = b"saltsalt";
        let key = derive_key(&password, salt, 10).unwrap();

        // Key should be 32 bytes
        assert_eq!(key.len(), 32);

        // Same inputs should produce same key
        let key2 = derive_key(&password, salt, 10).unwrap();
        assert_eq!(key, key2);

        // Different password should produce different key
        let password2 = Password::new("test2");
        let key3 = derive_key(&password2, salt, 10).unwrap();
        assert_ne!(key, key3);
    }

    #[test]
    fn test_derive_key_max_cycles_power() {
        let password = Password::new("test");
        let salt = b"saltsalt";

        // MAX_NUM_CYCLES_POWER should succeed (but takes a while, use smaller value)
        let key = derive_key(&password, salt, 10).unwrap();
        assert_eq!(key.len(), 32);

        // One above MAX should fail
        let result = derive_key(&password, salt, MAX_NUM_CYCLES_POWER + 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::Error::ResourceLimitExceeded(_)));
    }

    #[test]
    fn test_aes_roundtrip() {
        let data = b"Hello, World! This is test data for AES encryption.";
        let key = [0u8; 32];
        let iv = [0u8; 16];

        // Encrypt
        let mut encrypted = Vec::new();
        {
            let mut encoder = Aes256Encoder::with_key_iv(Cursor::new(&mut encrypted), key, iv);
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap();
        }

        // Decrypt
        let mut decoder = Aes256Decoder::with_key_iv(Cursor::new(&encrypted), key, iv);
        let mut decrypted = Vec::new();
        decoder.read_to_end(&mut decrypted).unwrap();

        // Due to PKCS7 padding, decrypted data may have extra bytes
        // Trim padding
        if let Some(&pad_len) = decrypted.last() {
            if (pad_len as usize) <= BLOCK_SIZE {
                decrypted.truncate(decrypted.len() - pad_len as usize);
            }
        }

        assert_eq!(&decrypted[..], &data[..]);
    }

    #[test]
    fn test_password_utf16le() {
        let password = Password::new("test");
        let bytes = password.as_utf16_le();
        // "test" in UTF-16LE: t(0x74 0x00) e(0x65 0x00) s(0x73 0x00) t(0x74 0x00)
        assert_eq!(bytes, vec![0x74, 0x00, 0x65, 0x00, 0x73, 0x00, 0x74, 0x00]);
    }

    #[test]
    fn test_key_cache_basic() {
        let cache = KeyCache::new(4);
        let password = Password::new("test");
        let salt = b"saltsalt";

        // First call - cache miss
        let key1 = cache.derive_key(&password, salt, 5).unwrap();
        let stats1 = cache.stats();
        assert_eq!(stats1.misses, 1);
        assert_eq!(stats1.hits, 0);
        assert_eq!(cache.len(), 1);

        // Second call - cache hit
        let key2 = cache.derive_key(&password, salt, 5).unwrap();
        let stats2 = cache.stats();
        assert_eq!(stats2.misses, 1);
        assert_eq!(stats2.hits, 1);
        assert_eq!(stats2.iterations_saved, 32); // 2^5 = 32

        // Keys should be identical
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_key_cache_different_params() {
        let cache = KeyCache::new(4);
        let password = Password::new("test");
        let salt1 = b"salt1111";
        let salt2 = b"salt2222";

        // Different salts should produce different keys
        let key1 = cache.derive_key(&password, salt1, 5).unwrap();
        let key2 = cache.derive_key(&password, salt2, 5).unwrap();
        assert_ne!(key1, key2);

        let stats = cache.stats();
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
    }

    #[test]
    fn test_key_cache_clear() {
        let cache = KeyCache::new(4);
        let password = Password::new("test");
        let salt = b"saltsalt";

        cache.derive_key(&password, salt, 5).unwrap();
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());

        // After clear, same call should be a miss
        cache.derive_key(&password, salt, 5).unwrap();
        let stats = cache.stats();
        assert_eq!(stats.misses, 2); // Both calls were misses
    }

    #[test]
    fn test_key_cache_stats_reset() {
        let cache = KeyCache::new(4);
        let password = Password::new("test");
        let salt = b"saltsalt";

        cache.derive_key(&password, salt, 5).unwrap();
        cache.derive_key(&password, salt, 5).unwrap();

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);

        cache.reset_stats();
        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[test]
    fn test_cache_stats_hit_ratio() {
        let stats = CacheStats {
            hits: 3,
            misses: 1,
            iterations_saved: 1000,
        };
        assert!((stats.hit_ratio() - 0.75).abs() < f64::EPSILON);

        let empty = CacheStats::default();
        assert!((empty.hit_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_lzma_header() {
        // Valid LZMA properties byte (0x5D = lc=5, lp=0, pb=2)
        assert!(validate_lzma_header(&[0x5D, 0x00, 0x00, 0x10, 0x00]));

        // Valid LZMA properties byte (0x00 = lc=0, lp=0, pb=0)
        assert!(validate_lzma_header(&[0x00, 0x00, 0x00, 0x01, 0x00]));

        // Invalid: pb >= 5 would require props_byte >= 225
        // 225 = 0 + 0*9 + 5*45, which is invalid since pb must be < 5
        assert!(!validate_lzma_header(&[0xE1])); // 225

        // Empty data
        assert!(!validate_lzma_header(&[]));
    }

    #[test]
    fn test_validate_lzma2_header() {
        // Valid: end marker
        assert!(validate_lzma2_header(&[0x00]));

        // Valid: uncompressed chunk with dictionary reset
        assert!(validate_lzma2_header(&[0x01]));

        // Valid: uncompressed chunk without dictionary reset
        assert!(validate_lzma2_header(&[0x02]));

        // Valid: compressed chunk
        assert!(validate_lzma2_header(&[0x80]));
        assert!(validate_lzma2_header(&[0xFF]));

        // Invalid: reserved range 0x03-0x7F
        assert!(!validate_lzma2_header(&[0x03]));
        assert!(!validate_lzma2_header(&[0x50]));
        assert!(!validate_lzma2_header(&[0x7F]));

        // Empty data
        assert!(!validate_lzma2_header(&[]));
    }

    #[test]
    fn test_validate_deflate_header() {
        // Valid: BTYPE = 00 (stored)
        assert!(validate_deflate_header(&[0b00000000])); // BFINAL=0, BTYPE=00
        assert!(validate_deflate_header(&[0b00000001])); // BFINAL=1, BTYPE=00

        // Valid: BTYPE = 01 (fixed Huffman)
        assert!(validate_deflate_header(&[0b00000010])); // BFINAL=0, BTYPE=01
        assert!(validate_deflate_header(&[0b00000011])); // BFINAL=1, BTYPE=01

        // Valid: BTYPE = 10 (dynamic Huffman)
        assert!(validate_deflate_header(&[0b00000100])); // BFINAL=0, BTYPE=10
        assert!(validate_deflate_header(&[0b00000101])); // BFINAL=1, BTYPE=10

        // Invalid: BTYPE = 11 (reserved)
        assert!(!validate_deflate_header(&[0b00000110])); // BFINAL=0, BTYPE=11
        assert!(!validate_deflate_header(&[0b00000111])); // BFINAL=1, BTYPE=11

        // Empty data
        assert!(!validate_deflate_header(&[]));
    }

    #[test]
    fn test_validate_bzip2_header() {
        // Valid BZip2 header
        assert!(validate_bzip2_header(b"BZh9"));

        // Invalid: wrong magic
        assert!(!validate_bzip2_header(b"PK"));
        assert!(!validate_bzip2_header(b"7z"));

        // Too short
        assert!(!validate_bzip2_header(b"B"));
        assert!(!validate_bzip2_header(&[]));
    }

    #[test]
    fn test_validate_decrypted_header() {
        const LZMA: &[u8] = &[0x03, 0x01, 0x01];
        const LZMA2: &[u8] = &[0x21];
        const DEFLATE: &[u8] = &[0x04, 0x01, 0x08];
        const BZIP2: &[u8] = &[0x04, 0x02, 0x02];
        const COPY: &[u8] = &[0x00];

        // Valid LZMA
        assert!(validate_decrypted_header(
            &[0x5D, 0x00, 0x00, 0x10, 0x00],
            LZMA
        ));

        // Valid LZMA2
        assert!(validate_decrypted_header(&[0x80], LZMA2));

        // Invalid LZMA2 (reserved control byte)
        assert!(!validate_decrypted_header(&[0x50], LZMA2));

        // Valid Deflate
        assert!(validate_decrypted_header(&[0x00], DEFLATE));

        // Invalid Deflate (BTYPE=11)
        assert!(!validate_decrypted_header(&[0x06], DEFLATE));

        // Valid BZip2
        assert!(validate_decrypted_header(b"BZh9data", BZIP2));

        // Copy method always valid
        assert!(validate_decrypted_header(&[0xFF, 0xFF, 0xFF], COPY));

        // Unknown method - assume valid
        assert!(validate_decrypted_header(&[0xFF], &[0x99, 0x99]));
    }

    // =========================================================================
    // Key Derivation Extended Tests (moved from tests/resource_limits.rs)
    // =========================================================================

    /// Tests key derivation with various salt patterns.
    ///
    /// The 7z format uses a 16-byte salt for AES key derivation.
    /// This test verifies that key derivation works correctly with
    /// different salt values, not just zero-filled salts.
    #[test]
    fn test_derive_key_with_varied_salts() {
        let password = Password::new("test_password");
        let cycles_power = 10; // Use lower value for faster test execution

        // Test with different salt patterns
        let salt_patterns: [([u8; 16], &str); 5] = [
            ([0u8; 16], "all zeros"),
            ([0xFFu8; 16], "all ones"),
            (
                [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                "sequential",
            ),
            (
                [
                    0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78, 0x9A,
                    0xBC, 0xDE, 0xF0,
                ],
                "mixed bytes",
            ),
            (
                [
                    0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x01,
                ],
                "boundary values",
            ),
        ];

        let mut derived_keys = Vec::new();

        for (salt, pattern_name) in &salt_patterns {
            let result = derive_key(&password, salt, cycles_power);
            assert!(
                result.is_ok(),
                "Key derivation should succeed with {} salt",
                pattern_name
            );

            let key = result.unwrap();
            // Key should be 32 bytes (AES-256)
            assert_eq!(
                key.len(),
                32,
                "Derived key should be 32 bytes for {} salt",
                pattern_name
            );

            // Store key for uniqueness check
            derived_keys.push((key, pattern_name));
        }

        // Verify that different salts produce different keys
        // (cryptographic property - same password + different salt = different key)
        for i in 0..derived_keys.len() {
            for j in (i + 1)..derived_keys.len() {
                assert_ne!(
                    derived_keys[i].0, derived_keys[j].0,
                    "Salt '{}' and '{}' should produce different keys",
                    derived_keys[i].1, derived_keys[j].1
                );
            }
        }
    }

    /// Tests that same salt + password always produces same key (deterministic).
    #[test]
    fn test_derive_key_deterministic() {
        let password = Password::new("determinism_test");
        let salt = [0x42u8; 16];
        let cycles_power = 10; // Use lower value for faster test execution

        let key1 = derive_key(&password, &salt, cycles_power).unwrap();
        let key2 = derive_key(&password, &salt, cycles_power).unwrap();

        assert_eq!(key1, key2, "Same inputs should produce same key");
    }

    /// Tests that extreme cycles_power values are rejected.
    #[test]
    fn test_derive_key_extreme_values_rejected() {
        let password = Password::new("test");
        let salt = [0u8; 16];

        // Values above MAX_NUM_CYCLES_POWER should fail
        let result = derive_key(&password, &salt, 62);
        assert!(result.is_err(), "cycles_power=62 should be rejected");

        let result = derive_key(&password, &salt, 63);
        assert!(result.is_err(), "cycles_power=63 should be rejected");
    }
}
