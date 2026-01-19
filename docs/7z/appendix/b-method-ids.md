# Appendix B: Method IDs

This appendix provides a complete reference of compression, filter, and encryption method IDs used in 7z archives.

## Method ID Format

Method IDs are variable-length byte sequences (1-15 bytes). The length is encoded in the coder flags byte.

Method ID bytes are stored and compared in the order shown in this document. For example, the LZMA method ID `03 01 01` is stored with `03` first, followed by `01`, then `01`. This is the natural byte order (not reversed).

## Method Categories

| Category   | ID Prefix  | Description                |
| ---------- | ---------- | -------------------------- |
| Simple     | 0x00-0x20  | Basic methods              |
| 7z Native  | 0x03XXXX   | LZMA, PPMd, BCJ filters    |
| ZIP Compat | 0x04XXXX   | Deflate, BZip2             |
| Extended   | 0x04F7XXXX | Modern codecs (Zstd, etc.) |
| Crypto     | 0x06XXXX   | Encryption methods         |

## Compression Methods

### Mandatory Methods

Implementations MUST support these methods:

| Method ID  | Name  | Properties | Description         |
| ---------- | ----- | ---------- | ------------------- |
| `00`       | Copy  | None       | No compression      |
| `03 01 01` | LZMA  | 5 bytes    | LZMA compression    |
| `21`       | LZMA2 | 1 byte     | Modern LZMA variant |

### Recommended Methods

Implementations SHOULD support these methods:

| Method ID  | Name    | Properties | Description      |
| ---------- | ------- | ---------- | ---------------- |
| `04 01 08` | Deflate | None       | ZIP-compatible   |
| `04 02 02` | BZip2   | None       | BWT compression  |
| `03 04 01` | PPMd    | 5 bytes    | Prediction-based |

### Optional Methods

Implementations MAY support these methods:

| Method ID     | Name      | Properties | Description      |
| ------------- | --------- | ---------- | ---------------- |
| `04 01 09`    | Deflate64 | None       | Extended Deflate |
| `04 F7 11 01` | Zstandard | Variable   | Facebook codec   |
| `04 F7 11 02` | Brotli    | Variable   | Google codec     |
| `04 F7 11 04` | LZ4       | Variable   | Fast compression |
| `04 F7 11 05` | LZ5/LZS   | Variable   | LZ5 variant      |
| `04 F7 11 06` | Lizard    | Variable   | LZ5 variant      |

## Filter Methods

### Mandatory Filters

| Method ID | Name  | Properties | Description    |
| --------- | ----- | ---------- | -------------- |
| `03`      | Delta | 1 byte     | Delta encoding |

### BCJ Filters (Recommended)

| Method ID     | Short ID | Name      | Architecture |
| ------------- | -------- | --------- | ------------ |
| `03 03 01 03` | `04`     | BCJ       | x86/x64      |
| `03 03 02 05` | `05`     | BCJ_PPC   | PowerPC      |
| `03 03 04 01` | `06`     | BCJ_IA64  | Itanium      |
| `03 03 05 01` | `07`     | BCJ_ARM   | ARM 32-bit   |
| `03 03 07 01` | `08`     | BCJ_ARMT  | ARM Thumb    |
| `03 03 08 05` | `09`     | BCJ_SPARC | SPARC        |

### Modern Filters (Optional)

| Method ID | Name   | Description       |
| --------- | ------ | ----------------- |
| `0A`      | ARM64  | ARM 64-bit filter |
| `0B`      | RISC-V | RISC-V filter     |

### Complex Filters

| Method ID     | Name | Streams      | Description         |
| ------------- | ---- | ------------ | ------------------- |
| `03 03 01 1B` | BCJ2 | 4 in / 1 out | Advanced x86 filter |

## Encryption Methods

| Method ID     | Name           | Description            |
| ------------- | -------------- | ---------------------- |
| `06 F1 07 01` | AES-256-SHA256 | 7z standard encryption |

### Crypto Method ID Structure

```
06        - Crypto category
F1        - 7z crypto
07        - Key size: 07 = 256-bit
01        - Hash: 01 = SHA-256
```

### Key Size Variants

| Byte | Key Size |
| ---- | -------- |
| 01   | 128 bits |
| 03   | 192 bits |
| 07   | 256 bits |

**Note:** Only AES-256-SHA256 (0x06F10701) is commonly used.

## Legacy Methods

These methods exist but are rarely used:

| Method ID  | Name         | Notes               |
| ---------- | ------------ | ------------------- |
| `02 03 02` | SWAP2        | Byte swap (2 bytes) |
| `02 03 04` | SWAP4        | Byte swap (4 bytes) |
| `04 01`    | MISC_ZIP     | ZIP method marker   |
| `04 05`    | MISC_Z       | LZW compression     |
| `04 06`    | MISC_LZH     | LZH compression     |
| `04 09 01` | NSIS_DEFLATE | NSIS Deflate        |
| `04 09 02` | NSIS_BZIP2   | NSIS BZip2          |

## Method Properties

### Copy (0x00)

No properties. Data passes through unchanged.

### LZMA (0x030101)

5 bytes:

| Byte | Description              |
| ---- | ------------------------ |
| 0    | `lc + lp * 9 + pb * 45`  |
| 1-4  | Dictionary size (UINT32) |

Constraints:

- lc: 0-8 (literal context bits)
- lp: 0-4 (literal position bits)
- pb: 0-4 (position bits)
- lc + lp ≤ 4

Default: `5D 00 00 10 00` (lc=3, lp=0, pb=2, dict=16 MiB)

### LZMA2 (0x21)

1 byte encoding dictionary size. See `[10-COMPRESSION-METHODS](/7z/10-compression-methods#lzma2)` for the complete dictionary size formula and encoding table.

**Quick reference (common values):**

- `0x14`: 1 MiB
- `0x18`: 4 MiB
- `0x1C`: 16 MiB
- `0x1E`: 32 MiB
- `0x20`: 64 MiB
- `0x28`: 4 GiB - 1 (maximum)
- `0x29+`: Reserved; implementations SHOULD reject

### PPMd (0x030401)

5 bytes:

| Byte | Description               |
| ---- | ------------------------- |
| 0    | Order (model order, 2-16) |
| 1-4  | Memory size (UINT32)      |

### Delta (0x03)

1 byte (optional, default 1):

| Byte | Description                   |
| ---- | ----------------------------- |
| 0    | Distance - 1 (0 = distance 1) |

### BCJ Filters

Optional 4-byte property:

| Bytes | Description                      |
| ----- | -------------------------------- |
| 0-3   | Start offset (UINT32, default 0) |

### AES-256-SHA256 (0x06F10701)

2-18 bytes:

| Byte  | Description                                          |
| ----- | ---------------------------------------------------- |
| 0     | `(salt_size) \| (iv_size << 4)`                      |
| 1     | `num_cycles_power` (iterations = 2^n, MUST be <= 30) |
| 2-n   | Salt bytes (0-16)                                    |
| n+1-m | IV bytes (0-16)                                      |

See `[12-ENCRYPTION](/7z/12-encryption)` for complete encryption details and security limits.

## Method Support Matrix

| Method  | zesven | 7-Zip | p7zip | py7zr |
| ------- | ------ | ----- | ----- | ----- |
| Copy    | R/W    | R/W   | R/W   | R/W   |
| LZMA    | R/W    | R/W   | R/W   | R/W   |
| LZMA2   | R/W    | R/W   | R/W   | R/W   |
| Deflate | R/W    | R/W   | R/W   | R/W   |
| BZip2   | R/W    | R/W   | R/W   | R/W   |
| PPMd    | R/W    | R/W   | R/W   | R/W   |
| BCJ     | R/W    | R/W   | R/W   | R/W   |
| BCJ2    | R/W    | R/W   | R/W   | -     |
| Delta   | R/W    | R/W   | R/W   | R/W   |
| AES     | R/W    | R/W   | R/W   | R/W   |
| Zstd    | R/W    | R/W\* | R/W\* | R/W   |
| LZ4     | R/W    | R/W\* | -     | -     |
| Brotli  | R/W    | R/W\* | -     | -     |

R = Read, W = Write, \* = Requires 7-Zip-zstd fork

## Unknown Methods

When encountering an unknown method:

1. Report method ID in error
2. Skip the entry (cannot decompress)
3. Continue processing other entries

```
function handle_unknown_method(method_id):
    error(ERROR_UNSUPPORTED,
        "Unknown compression method: 0x{}".format(hex(method_id)))
```

## See Also

- [Unpack Info](/7z/07-unpack-info) - Coder definitions in folders
- [Compression Methods](/7z/10-compression-methods) - Compression algorithm details
- [Filters](/7z/11-filters) - Filter specifications
- [Encryption](/7z/12-encryption) - Encryption details
