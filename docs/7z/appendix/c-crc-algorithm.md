# Appendix C: CRC Algorithm

This appendix specifies the CRC-32 algorithm used for data integrity verification in 7z archives.

## Overview

7z uses the standard CRC-32 algorithm (same as PNG, GZIP, and many other formats). CRCs are used to verify:

- Signature header integrity (StartHeaderCRC)
- Next header integrity (NextHeaderCRC)
- Pack stream integrity
- File data integrity

## Algorithm Parameters

| Parameter         | Value                  |
| ----------------- | ---------------------- |
| Width             | 32 bits                |
| Polynomial        | 0xEDB88320 (reflected) |
| Initial value     | 0xFFFFFFFF             |
| Input reflection  | Yes (LSB first)        |
| Output reflection | Yes                    |
| Final XOR         | 0xFFFFFFFF             |

**Empty data:** The CRC-32 of an empty byte sequence (zero-length input) is `0x00000000`. This follows from the algorithm: initial value `0xFFFFFFFF` XORed with final XOR `0xFFFFFFFF` yields `0x00000000`.

## Polynomial

The CRC-32 polynomial in standard (unreflected) form:

```
x^32 + x^26 + x^23 + x^22 + x^16 + x^12 + x^11 + x^10 + x^8 + x^7 + x^5 + x^4 + x^2 + x + 1
```

In hexadecimal:

- Unreflected: 0x04C11DB7
- Reflected: 0xEDB88320 (used in implementation)

## Reference Implementation

### Table Generation

```c
uint32_t crc_table[256];

void init_crc_table() {
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t crc = i;
        for (int j = 0; j < 8; j++) {
            if (crc & 1) {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc = crc >> 1;
            }
        }
        crc_table[i] = crc;
    }
}
```

### CRC Calculation

```c
uint32_t crc32(const uint8_t* data, size_t length) {
    uint32_t crc = 0xFFFFFFFF;

    for (size_t i = 0; i < length; i++) {
        uint8_t index = (crc ^ data[i]) & 0xFF;
        crc = (crc >> 8) ^ crc_table[index];
    }

    return crc ^ 0xFFFFFFFF;
}
```

### Incremental Calculation

```c
uint32_t crc32_init() {
    return 0xFFFFFFFF;
}

uint32_t crc32_update(uint32_t crc, const uint8_t* data, size_t length) {
    for (size_t i = 0; i < length; i++) {
        uint8_t index = (crc ^ data[i]) & 0xFF;
        crc = (crc >> 8) ^ crc_table[index];
    }
    return crc;
}

uint32_t crc32_finalize(uint32_t crc) {
    return crc ^ 0xFFFFFFFF;
}
```

## Lookup Table

Pre-computed CRC table (first 16 entries shown):

```
Index  CRC Value
0x00   0x00000000
0x01   0x77073096
0x02   0xEE0E612C
0x03   0x990951BA
0x04   0x076DC419
0x05   0x706AF48F
0x06   0xE963A535
0x07   0x9E6495A3
0x08   0x0EDB8832
0x09   0x79DCB8A4
0x0A   0xE0D5E91E
0x0B   0x97D2D988
0x0C   0x09B64C2B
0x0D   0x7EB17CBD
0x0E   0xE7B82D07
0x0F   0x90BF1D91
```

Full table available in any standard CRC-32 implementation.

## Test Vectors

### Standard Test

Input: ASCII string "123456789" (9 bytes)
Expected CRC: 0xCBF43926

```
Data (hex): 31 32 33 34 35 36 37 38 39
CRC-32:     0xCBF43926
```

### Empty Data

Input: Zero bytes
Expected CRC: 0x00000000

### Single Byte

| Input      | CRC-32     |
| ---------- | ---------- |
| 0x00       | 0xD202EF8D |
| 0xFF       | 0xFF000000 |
| 0x31 ('1') | 0x83DCEFB7 |

### 7z Header Test

Signature header bytes 12-31 (20 bytes) for minimal archive:

```
Data: 00 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00 17 0B 40 18
Expected StartHeaderCRC: 0x01D59B8D
Stored as: 8D 9B D5 01 (little-endian)
```

## CRC Locations in 7z

### StartHeaderCRC

| Field    | Offset    | Size     |
| -------- | --------- | -------- |
| Location | 0x08      | 4 bytes  |
| Coverage | 0x0C-0x1F | 20 bytes |

Covers: NextHeaderOffset, NextHeaderSize, NextHeaderCRC

### NextHeaderCRC

| Field    | Offset           | Size                 |
| -------- | ---------------- | -------------------- |
| Location | 0x1C             | 4 bytes              |
| Coverage | Next header data | NextHeaderSize bytes |

### Pack Stream CRCs

Optional. Stored in PackInfo section.

### File CRCs

Optional. Stored in SubStreamsInfo or UnpackInfo section.

## Verification Procedure

### Header Verification

```c
bool verify_start_header(const uint8_t* header) {
    // Header is 32 bytes
    uint32_t stored_crc = read_u32_le(&header[8]);
    uint32_t calculated_crc = crc32(&header[12], 20);
    return stored_crc == calculated_crc;
}

bool verify_next_header(const uint8_t* start_header,
                        const uint8_t* next_header,
                        size_t next_header_size) {
    uint32_t stored_crc = read_u32_le(&start_header[28]);
    uint32_t calculated_crc = crc32(next_header, next_header_size);
    return stored_crc == calculated_crc;
}
```

### File Verification

```c
bool verify_file_crc(const uint8_t* data, size_t size, uint32_t expected) {
    return crc32(data, size) == expected;
}
```

## Byte Order

CRC values are stored in little-endian format:

```
CRC value: 0x12345678
Stored as: 78 56 34 12
```

## Performance Optimizations

### Slice-by-4/8

Modern implementations use slice-by-4 or slice-by-8 techniques for faster computation with larger lookup tables.

### Hardware Acceleration

Many CPUs provide CRC-32 instructions:

- x86: `CRC32` (SSE 4.2)
- ARM: CRC32 extension

Libraries like `crc32fast` automatically use hardware acceleration when available.

## Related Algorithms

7z uses only CRC-32. Other related checksums NOT used in standard 7z:

- CRC-32C (different polynomial)
- CRC-64 (reserved for future use)
- SHA-256 (used only in key derivation)

## See Also

- [Signature Header](/7z/03-signature-header) - Header CRC fields
- [Pack Info](/7z/06-pack-info) - Pack stream CRCs
- [Substreams Info](/7z/08-substreams-info) - File CRCs
