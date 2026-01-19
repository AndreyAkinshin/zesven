# Encryption

This document specifies the AES-256 encryption method used in 7z archives.

## Overview

7z uses AES-256 in CBC mode with a SHA-256-based key derivation function. Encryption can protect:

- File data (content encryption)
- Archive header (metadata encryption)

## Method Identification

**Method ID:** `0x06 0xF1 0x07 0x01`
**Properties:** 2-18 bytes
**Support:** Recommended

The method ID indicates:

- `0x06`: Crypto category
- `0xF1`: 7z crypto
- `0x07`: AES-256 (256-bit key)
- `0x01`: SHA-256 key derivation

## Properties Format

```
Properties ::= FirstByte SecondByte [Salt] [IV]

FirstByte ::= (SaltSize & 0x0F) | ((IVSize & 0x0F) << 4)
SecondByte ::= NumCyclesPower
Salt ::= BYTE[SaltSize]
IV ::= BYTE[IVSize]
```

**FirstByte breakdown:**

- Bits 0-3: Salt size (0-15 bytes)
- Bits 4-7: IV size (0-15 bytes)

**Constraints:**

- Salt size: 0-15 bytes. Writers SHOULD use >= 8 bytes for adequate security.
- IV size: 0-15 bytes. Writers SHOULD use >= 8 bytes. Remaining bytes to reach 16 are zero-padded during decryption.
- Salt size of 0 is valid but SHOULD NOT be used by writers (reduces key uniqueness across archives).

**SecondByte:** Number of SHA-256 iterations as power of 2

### Properties Parsing

```
function parse_aes_properties(data):
    first_byte = data[0]
    salt_size = first_byte & 0x0F
    iv_size = (first_byte >> 4) & 0x0F

    num_cycles_power = data[1]

    salt = data[2 : 2 + salt_size]
    iv = data[2 + salt_size : 2 + salt_size + iv_size]

    # Pad IV with zeros to 16 bytes
    iv = iv + [0] * (16 - len(iv))

    return (salt, iv, num_cycles_power)
```

## Key Derivation

The encryption key is derived from the password using iterated SHA-256.

### Algorithm

```
function derive_key(password, salt, num_cycles_power):
    iterations = 2^num_cycles_power
    password_bytes = encode_utf16le(password)

    sha256 = SHA256()
    for i in 0 to iterations - 1:
        sha256.update(salt)
        sha256.update(password_bytes)
        counter_bytes = i.to_le_bytes(8)  # 8-byte little-endian encoding
        sha256.update(counter_bytes)

    return sha256.finalize()  # 32 bytes
```

**Counter encoding:** The iteration counter `i` is always encoded as an 8-byte (64-bit) unsigned integer in little-endian byte order.

**Example:** For iteration 256 (0x100), the counter bytes are: `00 01 00 00 00 00 00 00`

### Password Encoding

Passwords are converted to UTF-16-LE (Little Endian) without BOM or null terminator.

**Example:** "test" → `74 00 65 00 73 00 74 00`

### Iteration Count

| num_cycles_power | Iterations    | Typical Time |
| ---------------- | ------------- | ------------ |
| 14               | 16,384        | ~0.3 ms      |
| 19               | 524,288       | ~10 ms       |
| 24               | 16,777,216    | ~300 ms      |
| 30               | 1,073,741,824 | ~20 s        |

**Default:** 19 (524,288 iterations)

**Security limit:** Implementations MUST reject `num_cycles_power > 30` to prevent DoS attacks.

## AES-256-CBC Encryption

### Parameters

| Parameter  | Value                       |
| ---------- | --------------------------- |
| Algorithm  | AES                         |
| Key size   | 256 bits (32 bytes)         |
| Block size | 128 bits (16 bytes)         |
| Mode       | CBC (Cipher Block Chaining) |
| Padding    | PKCS#7                      |

### Encryption Process

1. Derive 32-byte key from password
2. Pad plaintext to 16-byte boundary (PKCS#7)
3. Initialize AES-256-CBC with key and IV
4. Encrypt plaintext blocks

### Decryption Process

1. Derive 32-byte key from password
2. Initialize AES-256-CBC with key and IV
3. Decrypt ciphertext blocks
4. Remove PKCS#7 padding
5. Verify padding is valid

### PKCS#7 Padding

Pad data to 16-byte boundary:

- If data length mod 16 = n, add (16-n) bytes of value (16-n)
- If data length mod 16 = 0, add 16 bytes of value 16

**Example:** Data is 13 bytes → add 3 bytes of value `03 03 03`

## Coder Chain Position

Encryption is typically the last coder in the chain:

```
[Filter] → [Compressor] → [AES] → Pack Stream
```

**Decompression order:**

```
Pack Stream → [AES Decrypt] → [Decompress] → [Filter Decode] → Output
```

### Example Folder

BCJ + LZMA2 + AES encryption:

```
Folder:
  NumCoders = 3
  Coder[0]: AES-256-SHA256
  Coder[1]: LZMA2
  Coder[2]: BCJ

  BindPairs:
    LZMA2 input ← AES output
    BCJ input ← LZMA2 output
```

## Header Encryption

When the archive header is encrypted:

1. Main header is compressed (typically LZMA2)
2. Compressed header is encrypted with AES
3. Encoded header property (`0x17`) contains encryption info
4. File names and metadata are hidden without password

### Detection

Header encryption is detected when:

- Next header starts with `0x17` (EncodedHeader)
- Coder chain includes AES method

### Listing Encrypted Archives

Without correct password:

- Cannot read file names
- Cannot determine file count
- Only signature header is readable

## Salt Generation

**Requirements:**

- MUST use cryptographically secure random bytes
- SHOULD be at least 8 bytes
- Maximum 15 bytes (limited by 4-bit encoding)

**Uniqueness:** Different salt for each encryption operation ensures unique derived keys even with the same password.

## IV (Initialization Vector)

**Requirements:**

- MUST be unique per encryption
- SHOULD use cryptographically secure random bytes
- Exactly 16 bytes (padded with zeros if shorter)

**Storage:** Only non-zero prefix stored to save space.

## Security Considerations

### Password Strength

Key derivation iterations slow brute-force attacks but cannot compensate for weak passwords.

**Recommendations:**

- Minimum 12 characters
- Mix of character types
- Avoid dictionary words

### Key Caching

Implementations SHOULD cache derived keys:

- Avoid re-computing expensive derivation
- Use secure memory (prevent swapping)
- Clear cache on timeout or explicit logout

### Wrong Password Detection

Detecting wrong password:

1. **Early detection:** Invalid PKCS#7 padding after decryption
2. **CRC mismatch:** Decompressed data CRC doesn't match
3. **Decompression failure:** Invalid compressed data

Implementations SHOULD detect wrong passwords as early as possible to avoid wasting computation.

### Timing Attacks

Key derivation time is constant (depends only on iterations, not password). This prevents timing-based password guessing.

## Properties Example

Archive with:

- Salt: 8 bytes `01 02 03 04 05 06 07 08`
- IV: 8 bytes `11 12 13 14 15 16 17 18`
- Iterations: 2^19 = 524,288

```
Properties (18 bytes):
88                      # SaltSize=8, IVSize=8
13                      # NumCyclesPower = 19 (0x13)
01 02 03 04 05 06 07 08 # Salt
11 12 13 14 15 16 17 18 # IV (padded to 16 with zeros)
```

## Implementation Notes

### Memory Security

- Zero password memory after use
- Use secure memory allocation if available
- Avoid logging password-related data

### Error Messages

Do not distinguish between:

- Wrong password
- Corrupt data
- Missing password

Generic "decryption failed" prevents information leakage.

## See Also

- [Header Structure](/7z/05-header-structure) - Encoded header structure
- [Unpack Info](/7z/07-unpack-info) - Coder chain definition
- [Security](/7z/17-security) - Security limits
- [Method IDs](/7z/appendix/b-method-ids) - Method ID reference
