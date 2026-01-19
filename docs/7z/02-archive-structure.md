# Archive Structure

This document describes the high-level physical layout of a 7z archive file.

## Physical Layout

A 7z archive consists of three main regions arranged sequentially:

```
┌─────────────────────────────┐  Offset 0x00
│     Signature Header        │  32 bytes (fixed)
│     (Start Header)          │
├─────────────────────────────┤  Offset 0x20
│                             │
│     Pack Data               │  Variable size
│     (Compressed Streams)    │  (may be empty)
│                             │
├─────────────────────────────┤  Offset 0x20 + NextHeaderOffset
│                             │
│     Next Header             │  NextHeaderSize bytes
│     (Header Database)       │
│                             │
└─────────────────────────────┘  EOF
```

## Region Details

### Signature Header

**Offset:** 0x00
**Size:** 32 bytes (fixed)

The signature header is always present and always 32 bytes. It contains:

- Archive signature (magic bytes)
- Format version
- CRC of header fields
- Pointer to the next header (offset and size)
- CRC of next header data

See [03-SIGNATURE-HEADER](/7z/03-signature-header) for the complete format.

### Pack Data

**Offset:** 0x20 (immediately after signature header)
**Size:** Variable (may be zero)

The pack data region contains all compressed data streams concatenated together:

```
┌─────────────────────────────┐
│     Pack Stream 0           │  PackSize[0] bytes
├─────────────────────────────┤
│     Pack Stream 1           │  PackSize[1] bytes
├─────────────────────────────┤
│          ...                │
├─────────────────────────────┤
│     Pack Stream N-1         │  PackSize[N-1] bytes
└─────────────────────────────┘
```

Key properties:

- Pack streams are stored contiguously with no gaps
- The number and sizes of streams are defined in [06-PACK-INFO](/7z/06-pack-info)
- Total pack data size = sum of all PackSize values
- Region may be empty if archive contains only empty files/directories

### Next Header

**Offset:** 0x20 + NextHeaderOffset
**Size:** NextHeaderSize bytes

The next header contains the archive metadata. It may be:

1. **Plain Header** - Uncompressed metadata starting with property ID `0x01`
2. **Encoded Header** - Compressed/encrypted metadata starting with property ID `0x17`

See [05-HEADER-STRUCTURE](/7z/05-header-structure) for header organization.

## Data Flow

### Reading an Archive

1. Read and validate signature header (32 bytes)
2. Calculate next header position: `32 + NextHeaderOffset`
3. Seek to next header position
4. Read NextHeaderSize bytes
5. Verify NextHeaderCRC matches
6. Parse header (decompress if encoded)
7. Extract pack info, unpack info, substreams info, files info
8. For each file to extract:
   - Locate the folder containing the file
   - Decompress the folder's pack streams through coder chain
   - Extract the file's substream from the unpack stream

### Writing an Archive

1. Collect all files to archive
2. Organize files into folders (one per file, or solid)
3. For each folder:
   - Concatenate file data
   - Apply coder chain (filters, compression, encryption)
   - Write resulting pack streams
4. Build header structures:
   - PackInfo with stream positions and sizes
   - UnpackInfo with folder and coder definitions
   - SubStreamsInfo with per-file sizes
   - FilesInfo with metadata
5. Optionally encode (compress) the header
6. Calculate CRCs
7. Write signature header with correct offsets

## Empty Archive

An archive with no files has:

```
NextHeaderOffset = 0
NextHeaderSize = size of minimal header
```

The minimal header contains:

- `0x01` (Header property ID)
- `0x00` (End property ID)

Total empty archive size: 32 bytes (signature) + 2 bytes (minimal header) = 34 bytes.

## Size Constraints

### Maximum Sizes

| Field                  | Maximum Value          | Notes                         |
| ---------------------- | ---------------------- | ----------------------------- |
| NextHeaderOffset       | 2^64 - 1               | Theoretical maximum           |
| NextHeaderSize         | 2^64 - 1               | Theoretical maximum           |
| Total archive size     | 2^64 - 1               | ~18 EB theoretical            |
| Practical archive size | Implementation-defined | Often 2^63 for signed offsets |

### Minimum Sizes

| Archive Type      | Minimum Size | Breakdown                            |
| ----------------- | ------------ | ------------------------------------ |
| Empty archive     | 34 bytes     | 32 (signature) + 2 (header: `01 00`) |
| Single empty file | Variable     | 32 + header with FilesInfo           |
| Single small file | Variable     | 32 + pack data + header              |

**Empty archive calculation:**

- Signature header: 32 bytes (fixed)
- Minimal next header: 2 bytes (`0x01` Header + `0x00` End)
- Total: exactly 34 bytes

## Offset Calculations

All offsets in the header are relative to specific reference points:

| Offset Field     | Reference Point                   |
| ---------------- | --------------------------------- |
| NextHeaderOffset | End of signature header (byte 32) |
| PackPos          | End of signature header (byte 32) |

### Example Offset Calculation

For an archive with:

- 1000 bytes of pack data
- 200 bytes of header

```
Signature Header:  bytes 0-31     (32 bytes)
Pack Data:         bytes 32-1031  (1000 bytes)
Next Header:       bytes 1032-1231 (200 bytes)

NextHeaderOffset = 1000 (offset from byte 32 to byte 1032)
NextHeaderSize = 200
```

## Validation Requirements

Implementations MUST verify:

1. **Signature valid**: First 6 bytes match `37 7A BC AF 27 1C`
2. **Version acceptable**: Major version is 0
3. **Offset in bounds**: `32 + NextHeaderOffset + NextHeaderSize <= file_size`
4. **No overlap**: Pack data region does not overlap header region
5. **CRCs match**: StartHeaderCRC and NextHeaderCRC are correct

## Self-Extracting Archives

SFX archives prepend executable code before the signature header:

```
┌─────────────────────────────┐  Offset 0x00
│     Executable Stub         │  Variable size
│     (PE, ELF, or Mach-O)    │
├─────────────────────────────┤  SFX Offset
│     Signature Header        │  32 bytes
├─────────────────────────────┤
│     Pack Data               │
├─────────────────────────────┤
│     Next Header             │
└─────────────────────────────┘
```

To read an SFX archive:

1. Search for signature bytes (up to 1 MiB)
2. Validate version bytes after signature
3. Use found offset as new base for all calculations

See [15-SFX-ARCHIVES](/7z/15-sfx-archives) for complete SFX handling.

## See Also

- [Signature Header](/7z/03-signature-header) - Signature header format
- [Header Structure](/7z/05-header-structure) - Header organization
- [Pack Info](/7z/06-pack-info) - Pack stream information
- [SFX Archives](/7z/15-sfx-archives) - Self-extracting archives
