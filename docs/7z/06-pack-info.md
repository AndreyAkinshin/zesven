# Pack Info

This document specifies the PackInfo structure that describes compressed data streams in a 7z archive.

## Overview

PackInfo defines:

- Where compressed data starts in the archive
- How many compressed streams exist
- The size of each compressed stream
- Optional CRC checksums for each stream

## Structure

```
PackInfo ::= 0x06 PackPos NumPackStreams [Sizes] [Digests] 0x00

PackPos ::= NUMBER
NumPackStreams ::= NUMBER
Sizes ::= 0x09 PackSize* 0x00
Digests ::= 0x0A DigestsData
PackSize ::= NUMBER
```

**Optional section semantics:** When an optional section (e.g., `[Sizes]`) is absent, the entire structure including its property ID is omitted from the byte stream. For example, when Sizes is absent, the `0x09` byte does not appear. The parser proceeds directly from `NumPackStreams` to either `Digests` (if present) or the terminating `0x00`.

## Fields

### Property ID

**Value:** `0x06`

Identifies the start of PackInfo section.

### PackPos

**Type:** NUMBER
**Description:** Offset from end of signature header (byte 32) to the first pack stream.

The absolute file position of pack data is:

```
pack_data_start = 32 + PackPos
```

**Typical value:** 0 (pack data immediately follows signature header)

**Constraints:**

- MUST be valid offset within file
- `32 + PackPos + sum(PackSizes)` MUST NOT exceed NextHeaderOffset + 32

### NumPackStreams

**Type:** NUMBER
**Description:** Number of packed (compressed) streams.

**Range:** 0 to implementation limit (typically 2^31)

**Value of 0:** Valid for archives with no compressed data (only empty files/directories)

### Sizes Section

**Property ID:** `0x09`

Contains the size of each pack stream.

```
Sizes ::= 0x09 PackSize[NumPackStreams] 0x00
```

**Presence:**

- MUST be present if NumPackStreams > 0
- MUST be absent if NumPackStreams == 0

**PackSize:** Size in bytes of the corresponding pack stream.

### Digests Section

**Property ID:** `0x0A`

Contains optional CRC-32 checksums for pack streams. This follows the BooleanList pattern defined in [04-DATA-ENCODING](/7z/04-data-encoding#booleanlist).

```
Digests ::= 0x0A AllAreDefined [BitField] CRC[NumDefined]

AllAreDefined ::= BYTE  # 0x00 or 0x01
BitField ::= # Present only if AllAreDefined == 0x00
CRC ::= UINT32
```

**Presence:** Optional. If absent, no pack stream CRCs are available.

**AllAreDefined:**

- `0x01`: All streams have CRCs, BitField absent
- `0x00`: BitField indicates which streams have CRCs

**CRCs:** Only present for streams where defined. Count = number of true values in the defined bitmap.

## Complete Example

Archive with 3 pack streams:

```
06                  # PackInfo property ID
00                  # PackPos = 0
03                  # NumPackStreams = 3

09                  # Sizes property ID
  80 00 01          # PackSize[0] = 256 (NUMBER encoding)
  80 00 02          # PackSize[1] = 512
  80 00 04          # PackSize[2] = 1024
00                  # End of Sizes

0A                  # Digests property ID
  01                # AllAreDefined = true
  78 56 34 12       # CRC[0] = 0x12345678
  AB CD EF 01       # CRC[1] = 0x01EFCDAB
  11 22 33 44       # CRC[2] = 0x44332211

00                  # End of PackInfo
```

## Partial Digests Example

When only some streams have CRCs:

```
0A                  # Digests property ID
  00                # AllAreDefined = false
  A0                # BitField = 0xA0: bit 7 (item 0) = 1, bit 5 (item 2) = 1
  78 56 34 12       # CRC for stream 0
  11 22 33 44       # CRC for stream 2
                    # (no CRC for stream 1)
```

See [04-DATA-ENCODING](/7z/04-data-encoding#bitfield) for BitField encoding details.

## Pack Stream Layout

Pack streams are stored contiguously in the archive:

```
Offset 32 + PackPos:
┌─────────────────────┐
│   Pack Stream 0     │  PackSize[0] bytes
├─────────────────────┤
│   Pack Stream 1     │  PackSize[1] bytes
├─────────────────────┤
│        ...          │
├─────────────────────┤
│ Pack Stream N-1     │  PackSize[N-1] bytes
└─────────────────────┘
```

## Relationship to Folders

Pack streams are consumed by folders (see [07-UNPACK-INFO](/7z/07-unpack-info)):

- Each folder references one or more pack streams
- Simple coders (LZMA, LZMA2) use 1 pack stream per folder
- Complex coders (BCJ2) may use multiple pack streams per folder
- The mapping is defined in UnpackInfo

## Empty PackInfo

For archives with no compressed data:

```
06          # PackInfo
00          # PackPos = 0
00          # NumPackStreams = 0
00          # End (no Sizes or Digests)
```

## Zero-Length Pack Streams

A pack stream MAY have a size of zero (PackSize = 0). This is valid but unusual.

**When zero-length streams occur:**

- A coder that outputs no data (e.g., compressing zero-length input)
- Placeholder streams in complex coder configurations

**Behavior:**

- Zero-length pack streams consume no bytes in the pack data region
- The stream still exists in the pack stream index
- Reading a zero-length stream returns an empty byte sequence
- CRCs for zero-length streams, if present, MUST be 0x00000000 (the CRC-32 of an empty byte sequence; see [C-CRC-ALGORITHM](/7z/appendix/c-crc-algorithm) "Empty Data" section)

## Validation

Implementations MUST verify:

1. **Pack position valid:** `32 + PackPos` is within file
2. **Sizes count matches:** Number of PackSize values equals NumPackStreams
3. **Total size valid:** `32 + PackPos + sum(PackSizes) <= NextHeaderOffset + 32`
4. **CRC count valid:** Number of CRCs equals number of defined streams
5. **No overlaps:** Pack data region doesn't overlap header region

## Implementation Notes

### Reading Pack Data

To read pack stream `i`:

```
offset = 32 + PackPos + sum(PackSize[0..i])
size = PackSize[i]
data = read_bytes_at(offset, size)
```

### Writing Pack Data

When writing:

1. Set PackPos = 0 (data starts right after signature header)
2. Write each folder's compressed output sequentially
3. Record sizes as you write
4. Calculate CRCs during compression

### Memory Efficiency

For archives with many streams:

- Store offsets lazily (calculate from sizes on demand)
- Consider memory-mapped access for large pack data regions

## See Also

- [Archive Structure](/7z/02-archive-structure) - Archive layout showing pack data position
- [Unpack Info](/7z/07-unpack-info) - How folders reference pack streams
- [CRC Algorithm](/7z/appendix/c-crc-algorithm) - CRC-32 calculation
- [Property IDs](/7z/appendix/a-property-ids) - Property ID reference
