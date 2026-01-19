# Compression Methods

This document specifies the compression methods supported in 7z archives.

## Overview

Compression methods transform data to reduce size. Each method has:

- A unique method ID
- Optional properties configuring the algorithm
- Input/output stream requirements

## Method Categories

| Category | Description                |
| -------- | -------------------------- |
| Copy     | No compression             |
| Standard | Methods in official 7-Zip  |
| Extended | Methods in 7-Zip-zstd fork |

## Support Levels

| Level       | Meaning                             |
| ----------- | ----------------------------------- |
| Mandatory   | MUST support for compliance         |
| Recommended | SHOULD support for interoperability |
| Optional    | MAY support                         |

## Copy (No Compression)

**Method ID:** `0x00`
**Properties:** None
**Support:** Mandatory

Passes data through unchanged. Used for:

- Already compressed data
- Small files where compression overhead exceeds savings
- Testing

## LZMA

**Method ID:** `0x03 0x01 0x01`
**Properties:** 5 bytes
**Support:** Mandatory

The original Lempel-Ziv-Markov chain algorithm.

### LZMA Properties

| Byte | Description                                 |
| ---- | ------------------------------------------- |
| 0    | Encoder parameters: `lc + lp * 9 + pb * 45` |
| 1-4  | Dictionary size (UINT32, little-endian)     |

**Parameter constraints:**

- `lc` (literal context bits): 0-8
- `lp` (literal position bits): 0-4
- `pb` (position bits): 0-4
- `lc + lp` ≤ 4

**Constraint violation:** If any parameter exceeds its valid range, or if `lc + lp > 4`, implementations MUST reject the archive with ERROR_ARCHIVE. These constraints are enforced by the LZMA algorithm; violating them produces undefined decoder behavior.

**Default values:** `lc=3, lp=0, pb=2` → byte 0 = `0x5D`

**Dictionary size:** Power of 2 from 4 KiB to 4 GiB (typical: 16 MiB)

### Example Properties

```
5D 00 00 10 00          # lc=3, lp=0, pb=2, dict=16 MiB (0x01000000)
5D 00 00 00 02          # lc=3, lp=0, pb=2, dict=32 MiB (0x02000000)
```

## LZMA2

**Method ID:** `0x21`
**Properties:** 1 byte
**Support:** Mandatory

Modern LZMA variant with:

- Reset capability for parallel encoding
- Improved handling of incompressible data
- Single-byte properties

### LZMA2 Properties

Single byte encoding dictionary size:

| Value (decimal) | Value (hex) | Dictionary Size                         |
| --------------- | ----------- | --------------------------------------- |
| 0-39            | 0x00-0x27   | Calculated (see formula)                |
| 40              | 0x28        | 4 GiB - 1 (0xFFFFFFFF)                  |
| 41+             | 0x29+       | Reserved; implementations SHOULD reject |

**Dictionary size formula (for values 0-39):**

```
base = 2 + (value & 1)
shift = value / 2 + 11
dict_size = base << shift
```

**Common values:**

| Byte | Dictionary Size |
| ---- | --------------- |
| 0x14 | 1 MiB           |
| 0x15 | 1.5 MiB         |
| 0x16 | 2 MiB           |
| 0x17 | 3 MiB           |
| 0x18 | 4 MiB           |
| 0x19 | 6 MiB           |
| 0x1A | 8 MiB           |
| 0x1B | 12 MiB          |
| 0x1C | 16 MiB          |
| 0x1D | 24 MiB          |
| 0x1E | 32 MiB          |
| 0x1F | 48 MiB          |
| 0x20 | 64 MiB          |

## Deflate

**Method ID:** `0x04 0x01 0x08`
**Properties:** None
**Support:** Recommended

ZIP-compatible deflate compression.

- Uses LZ77 + Huffman coding
- Maximum dictionary size: 32 KiB
- Good for compatibility with ZIP tools

## Deflate64

**Method ID:** `0x04 0x01 0x09`
**Properties:** None
**Support:** Optional

Enhanced deflate with 64 KiB dictionary. Proprietary Microsoft extension.

- Decompression: SHOULD support
- Compression: MAY support

## BZip2

**Method ID:** `0x04 0x02 0x02`
**Properties:** None
**Support:** Recommended

Burrows-Wheeler transform compression.

- Better compression than Deflate
- Slower than LZMA/LZMA2
- Block-based (good for parallel processing)

## PPMd

**Method ID:** `0x03 0x04 0x01`
**Properties:** 5 bytes
**Support:** Recommended

Prediction by Partial Matching compression.

### PPMd Properties

| Bytes | Description                                  |
| ----- | -------------------------------------------- |
| 0     | Order (model order, typically 2-16)          |
| 1-4   | Memory size in bytes (UINT32, little-endian) |

**Typical values:**

- Order: 6-8 for text, 2-4 for binary
- Memory: 16 MiB to 256 MiB

**Example:**

```
06 00 00 00 10          # Order=6, Memory=256 MiB (0x10000000)
```

## Extended Methods (7-Zip-zstd)

These methods are supported by the 7-Zip-zstd fork and compatible implementations.

### Zstandard (Zstd)

**Method ID:** `0x04 0xF7 0x11 0x01`
**Properties:** Variable (1-5 bytes)
**Support:** Optional

Facebook's modern compression algorithm.

**Properties format:**
| Byte | Description |
|------|-------------|
| 0 | Compression level (1-22) |
| 1-4 | Dictionary size (optional) |

### Brotli

**Method ID:** `0x04 0xF7 0x11 0x02`
**Properties:** 1-3 bytes
**Support:** Optional

Google's compression algorithm, optimized for web content.

**Properties:**
| Byte | Description |
|------|-------------|
| 0 | Quality (0-11) |
| 1 | Window size (optional) |

### LZ4

**Method ID:** `0x04 0xF7 0x11 0x04`
**Properties:** Variable
**Support:** Optional

Extremely fast compression with moderate ratio.

### Lizard (LZ5)

**Method ID:** `0x04 0xF7 0x11 0x06`
**Properties:** Variable
**Support:** Optional

Balance between LZ4 speed and better compression.

## Method Selection Guidance

| Use Case            | Recommended Method          |
| ------------------- | --------------------------- |
| General purpose     | LZMA2                       |
| Maximum compression | LZMA2 with large dictionary |
| Fast compression    | Zstd or LZ4                 |
| Text files          | PPMd                        |
| ZIP compatibility   | Deflate                     |
| Pre-compressed data | Copy                        |
| Executables         | BCJ + LZMA2                 |

## Compression Levels

Most methods support compression levels affecting the speed/ratio trade-off:

| Level | Description          |
| ----- | -------------------- |
| 0     | Store only / fastest |
| 1-3   | Fast compression     |
| 4-6   | Balanced             |
| 7-9   | Maximum compression  |

Implementation-specific; not stored in archive.

## Stream Requirements

| Method  | Inputs | Outputs | Notes  |
| ------- | ------ | ------- | ------ |
| Copy    | 1      | 1       | Simple |
| LZMA    | 1      | 1       | Simple |
| LZMA2   | 1      | 1       | Simple |
| Deflate | 1      | 1       | Simple |
| BZip2   | 1      | 1       | Simple |
| PPMd    | 1      | 1       | Simple |
| Zstd    | 1      | 1       | Simple |

All standard compression methods are simple coders (1 input, 1 output).

## Implementation Notes

### Decompression

1. Read coder properties from folder definition
2. Initialize decoder with properties
3. Feed pack stream data to decoder
4. Collect unpack stream output
5. Verify output size matches expected unpack size

### Compression

1. Select method based on content analysis or user preference
2. Configure encoder with appropriate level/dictionary
3. Compress data to produce pack stream
4. Record method ID and properties in folder definition
5. Record pack size and unpack size

## See Also

- [Unpack Info](/7z/07-unpack-info) - How coders are defined in folders
- [Filters](/7z/11-filters) - Pre-compression filters
- [Encryption](/7z/12-encryption) - Encryption coder
- [Method IDs](/7z/appendix/b-method-ids) - Complete method ID table
