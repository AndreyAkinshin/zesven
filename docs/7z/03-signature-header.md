# Signature Header

This document specifies the 32-byte signature header (also called "start header") that begins every 7z archive.

## Structure

The signature header is exactly 32 bytes with the following layout:

| Offset | Size | Field            | Type    | Description                       |
| ------ | ---- | ---------------- | ------- | --------------------------------- |
| 0x00   | 6    | Signature        | BYTE[6] | Magic bytes identifying 7z format |
| 0x06   | 1    | VersionMajor     | BYTE    | Archive format major version      |
| 0x07   | 1    | VersionMinor     | BYTE    | Archive format minor version      |
| 0x08   | 4    | StartHeaderCRC   | UINT32  | CRC-32 of bytes 0x0C-0x1F         |
| 0x0C   | 8    | NextHeaderOffset | UINT64  | Offset to header from byte 32     |
| 0x14   | 8    | NextHeaderSize   | UINT64  | Size of next header in bytes      |
| 0x1C   | 4    | NextHeaderCRC    | UINT32  | CRC-32 of next header data        |

Total size: 32 bytes (0x20)

## Field Specifications

### Signature

**Offset:** 0x00
**Size:** 6 bytes
**Value:** `37 7A BC AF 27 1C`

The signature bytes identify the file as a 7z archive:

| Byte | Hex  | ASCII | Description    |
| ---- | ---- | ----- | -------------- |
| 0    | 0x37 | '7'   | ASCII digit 7  |
| 1    | 0x7A | 'z'   | ASCII letter z |
| 2    | 0xBC | -     | Magic byte     |
| 3    | 0xAF | -     | Magic byte     |
| 4    | 0x27 | -     | Magic byte     |
| 5    | 0x1C | -     | Magic byte     |

Implementations MUST reject files that do not begin with this exact sequence (or for SFX, do not contain this sequence within the search range).

### VersionMajor

**Offset:** 0x06
**Size:** 1 byte
**Current Value:** 0x00

The major version indicates fundamental format compatibility:

| Value  | Meaning                                  |
| ------ | ---------------------------------------- |
| 0x00   | Current version, MUST accept             |
| > 0x00 | Future incompatible version, MUST reject |

### VersionMinor

**Offset:** 0x07
**Size:** 1 byte
**Current Value:** 0x04

The minor version indicates feature additions within the major version:

| Value     | Meaning                                    |
| --------- | ------------------------------------------ |
| 0x00-0x04 | Known versions, MUST accept                |
| > 0x04    | Future version, SHOULD accept with warning |

Minor version history:

- 0x00: Original format
- 0x01-0x03: Incremental additions (internal 7-Zip development)
- 0x04: Current version with all documented features

**Version differences:** The minor version primarily tracks internal 7-Zip development history. In practice, all versions 0x00-0x04 are structurally compatible and use the same header format. The version number does not indicate which compression methods or features are used in the archive; feature availability is determined by examining the coder method IDs in UnpackInfo.

### StartHeaderCRC

**Offset:** 0x08
**Size:** 4 bytes
**Type:** UINT32 (little-endian)

CRC-32 checksum of the 20 bytes from offset 0x0C to 0x1F (inclusive). This covers:

- NextHeaderOffset (8 bytes)
- NextHeaderSize (8 bytes)
- NextHeaderCRC (4 bytes)

Calculation:

```
StartHeaderCRC = CRC32(bytes[0x0C..0x20])
```

See [C-CRC-ALGORITHM](/7z/appendix/c-crc-algorithm) for the CRC-32 algorithm specification.

### NextHeaderOffset

**Offset:** 0x0C
**Size:** 8 bytes
**Type:** UINT64 (little-endian)

Offset in bytes from the end of the signature header (byte 32) to the start of the next header. The absolute file position of the next header is:

```
next_header_position = 32 + NextHeaderOffset
```

**Constraints:**

- MAY be 0 for archives with no pack data
- MUST satisfy: `32 + NextHeaderOffset + NextHeaderSize <= file_size`

### NextHeaderSize

**Offset:** 0x14
**Size:** 8 bytes
**Type:** UINT64 (little-endian)

Size in bytes of the next header data.

**Constraints:**

- MUST be > 0 (minimum header is 2 bytes: `0x01 0x00`)
- SHOULD be < 64 MiB for typical archives
- MAY trigger resource limit checks for very large values

### NextHeaderCRC

**Offset:** 0x1C
**Size:** 4 bytes
**Type:** UINT32 (little-endian)

CRC-32 checksum of the next header data.

```
NextHeaderCRC = CRC32(next_header_bytes[0..NextHeaderSize])
```

## Hex Dump Example

A minimal valid 7z archive (empty, no files):

```
Offset    00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F
────────  ─────────────────────────────────────────────────
00000000  37 7A BC AF 27 1C 00 04  08 A8 34 B8 00 00 00 00
00000010  00 00 00 00 02 00 00 00  00 00 00 00 BE 23 C2 58
00000020  01 00
```

Interpretation:

- `37 7A BC AF 27 1C`: Signature
- `00`: VersionMajor = 0
- `04`: VersionMinor = 4
- `08 A8 34 B8`: StartHeaderCRC = 0xB834A808
- `00 00 00 00 00 00 00 00`: NextHeaderOffset = 0
- `02 00 00 00 00 00 00 00`: NextHeaderSize = 2
- `BE 23 C2 58`: NextHeaderCRC = 0x58C223BE
- `01 00`: Header data (Header ID + End ID)

## Validation Procedure

Implementations MUST perform these checks in order:

1. **File size check**: File MUST be at least 32 bytes. Reject if smaller.
2. **Signature check**: Bytes 0-5 MUST equal `37 7A BC AF 27 1C`. MUST reject if mismatch.
3. **Version check**: VersionMajor MUST be 0. MUST reject if greater.
4. **StartHeaderCRC check**: Calculate CRC-32 of bytes 12-31, compare to StartHeaderCRC. MUST reject if CRC does not match.
5. **Offset bounds check**: `32 + NextHeaderOffset + NextHeaderSize` MUST NOT exceed file size. MUST reject if out of bounds.
6. **Read next header**: Read NextHeaderSize bytes from position `32 + NextHeaderOffset`.
7. **NextHeaderCRC check**: Calculate CRC-32 of read bytes, compare to NextHeaderCRC. MUST reject if CRC does not match.

**Rejection requirement:** If any check fails, the archive MUST be rejected with an appropriate error. Implementations MUST NOT attempt to process an archive that fails validation.

## SFX Archive Detection

For self-extracting archives, the signature header is not at offset 0. Detection procedure:

1. Check if file starts with signature; if yes, archive starts at offset 0
2. Search for signature bytes within first 1 MiB of file
3. For each candidate match:
   - Verify VersionMajor is 0x00
   - Verify VersionMinor is ≤ 0x04 (or acceptable)
   - Attempt to validate StartHeaderCRC
4. If valid match found, use that offset as the archive start
5. All subsequent offsets are relative to the found signature position

See [15-SFX-ARCHIVES](/7z/15-sfx-archives) for complete SFX handling.

## Implementation Notes

### Endianness

All multi-byte integers (UINT32, UINT64) are stored in little-endian byte order. On big-endian systems, byte swapping is required.

### Streaming Reads

The signature header can be read in a single 32-byte operation. No seeking is required until reading the next header.

### Writing

When writing an archive:

1. Reserve 32 bytes for signature header
2. Write pack data starting at offset 32
3. Write header after pack data
4. Calculate NextHeaderOffset = (pack data size)
5. Calculate NextHeaderSize = (header size)
6. Calculate NextHeaderCRC
7. Calculate StartHeaderCRC
8. Seek to offset 0 and write complete signature header

## See Also

- [Archive Structure](/7z/02-archive-structure) - Overall archive layout
- [Header Structure](/7z/05-header-structure) - Next header format
- [SFX Archives](/7z/15-sfx-archives) - Self-extracting archive handling
- [CRC Algorithm](/7z/appendix/c-crc-algorithm) - CRC-32 calculation
