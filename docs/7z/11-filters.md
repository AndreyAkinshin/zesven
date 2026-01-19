# Filters

This document specifies the filter methods used in 7z archives for pre-processing data before compression.

## Overview

Filters transform data to improve subsequent compression. They do not reduce size themselves but make patterns more compressible.

**Typical usage:** `Data → [Filter] → [Compressor] → Compressed`

## Filter Types

| Type                   | Purpose                                  |
| ---------------------- | ---------------------------------------- |
| BCJ (Branch/Call/Jump) | Transform relative addresses to absolute |
| Delta                  | Transform adjacent byte differences      |

## Delta Filter

**Method ID:** `0x03`
**Properties:** 1 byte (optional)
**Support:** Mandatory

Transforms data by storing differences between bytes at fixed intervals.

### Delta Properties

| Byte | Description                       |
| ---- | --------------------------------- |
| 0    | Delta distance (1-256, default 1) |

**Encoding:** Property byte = distance - 1

| Property | Distance    |
| -------- | ----------- |
| 0x00     | 1 (default) |
| 0x01     | 2           |
| 0xFF     | 256         |

### Delta Algorithm

**Encoding:**

```
output[i] = input[i] - input[i - distance]
```

**Decoding:**

```
output[i] = input[i] + output[i - distance]
```

For `i < distance`, treat missing values as 0.

### Use Cases

- Audio samples (distance = bytes per sample)
- Multi-channel data
- Gradual value changes

## BCJ Filters

BCJ (Branch/Call/Jump) filters transform relative addresses in executable code to absolute addresses. This improves compression because absolute addresses are more predictable across nearby instructions.

### Common BCJ Properties

Most BCJ filters accept an optional 4-byte property:

| Bytes | Description                                                  |
| ----- | ------------------------------------------------------------ |
| 0-3   | Start offset for address calculation (UINT32, little-endian) |

**Property presence:**

- If HasProperties flag is clear or PropertiesSize is 0: start offset = 0
- If present: 4 bytes interpreted as UINT32 little-endian start offset

Zero-length properties (HasProperties set but PropertiesSize = 0) are equivalent to absent properties.

### x86/x64 BCJ

**Method ID:** `0x03 0x03 0x01 0x03`
**Alternate ID:** `0x04` (simple form)
**Support:** Mandatory

Transforms x86/x64 CALL and JMP instructions.

**Targeted instructions:**

- `E8` (CALL rel32)
- `E9` (JMP rel32)

**Alignment:** 5 bytes (opcode + 4-byte offset)

### x86 BCJ2

**Method ID:** `0x03 0x03 0x01 0x1B`
**Support:** Optional (decompression only for some implementations)

Advanced x86 filter that separates branch target addresses into dedicated streams for better compression.

**Input streams:** 4 (during decompression)
**Output streams:** 1

#### BCJ2 Stream Definition (Normative)

| Stream Index | Content                             | Encoding                               |
| ------------ | ----------------------------------- | -------------------------------------- |
| 0            | Main data with address placeholders | Raw bytes                              |
| 1            | CALL (E8) displacement values       | Little-endian UINT32 sequence          |
| 2            | JMP (E9) displacement values        | Little-endian UINT32 sequence          |
| 3            | Selector stream                     | Range coder (selects CALL/JMP/neither) |

**Stream ordering requirement:** Stream indices 0, 1, 2, 3 MUST appear in ascending order in the folder's PackStreamIndex array. Implementations MUST NOT reorder these streams. During decompression, the BCJ2 decoder reads from all 4 input streams simultaneously and produces a single output stream with reconstructed relative addresses.

BCJ2 provides better compression than BCJ but is more complex to implement.

### ARM BCJ

**Method ID:** `0x03 0x03 0x05 0x01`
**Alternate ID:** `0x07`
**Support:** Recommended

Transforms ARM (32-bit) branch instructions.

**Alignment:** 4 bytes

### ARM64 BCJ

**Method ID:** `0x0A`
**Support:** Recommended

Transforms ARM64 (AArch64) branch instructions.

**Targeted instructions:**

- BL (branch with link)
- B (unconditional branch)

**Alignment:** 4 bytes

### ARM Thumb BCJ

**Method ID:** `0x03 0x03 0x07 0x01`
**Alternate ID:** `0x08`
**Support:** Optional

Transforms ARM Thumb mode instructions.

**Alignment:** 2 bytes

### PowerPC BCJ

**Method ID:** `0x03 0x03 0x02 0x05`
**Alternate ID:** `0x05`
**Support:** Optional

Transforms PowerPC branch instructions.

**Alignment:** 4 bytes

### IA-64 BCJ

**Method ID:** `0x03 0x03 0x04 0x01`
**Alternate ID:** `0x06`
**Support:** Optional

Transforms Intel IA-64 (Itanium) branch instructions.

**Alignment:** 16 bytes

### SPARC BCJ

**Method ID:** `0x03 0x03 0x08 0x05`
**Alternate ID:** `0x09`
**Support:** Optional

Transforms SPARC branch instructions.

**Alignment:** 4 bytes

### RISC-V BCJ

**Method ID:** `0x0B`
**Support:** Optional

Transforms RISC-V branch and jump instructions.

**Alignment:** 2 bytes (compressed) or 4 bytes (standard)

## Filter Summary Table

| Filter    | Method ID     | Properties  | Alignment |
| --------- | ------------- | ----------- | --------- |
| Delta     | `03`          | 1 byte      | N/A       |
| BCJ x86   | `03 03 01 03` | 4 bytes opt | 5         |
| BCJ2 x86  | `03 03 01 1B` | None        | Complex   |
| BCJ PPC   | `03 03 02 05` | 4 bytes opt | 4         |
| BCJ IA64  | `03 03 04 01` | 4 bytes opt | 16        |
| BCJ ARM   | `03 03 05 01` | 4 bytes opt | 4         |
| BCJ ARMT  | `03 03 07 01` | 4 bytes opt | 2         |
| BCJ SPARC | `03 03 08 05` | 4 bytes opt | 4         |
| ARM64     | `0A`          | 4 bytes opt | 4         |
| RISC-V    | `0B`          | 4 bytes opt | 2/4       |

## Alternate (Short) Method IDs

Some filters have short alternate IDs for backward compatibility:

| Short ID | Full ID       | Filter    |
| -------- | ------------- | --------- |
| `04`     | `03 03 01 03` | BCJ x86   |
| `05`     | `03 03 02 05` | BCJ PPC   |
| `06`     | `03 03 04 01` | BCJ IA64  |
| `07`     | `03 03 05 01` | BCJ ARM   |
| `08`     | `03 03 07 01` | BCJ ARMT  |
| `09`     | `03 03 08 05` | BCJ SPARC |

Implementations MUST accept both forms when reading.

**Writing guidance:** Writers SHOULD use the short form for maximum compatibility with older tools. The short form is universally supported; the long form may not be recognized by all implementations.

## Filter Chaining

Filters are typically chained with compressors:

### Common Chains

**Executables (x86):**

```
Data → [BCJ x86] → [LZMA2] → Compressed
```

**Multi-channel audio:**

```
Data → [Delta, distance=2] → [LZMA2] → Compressed
```

### Coder Order in Folder

In folder definitions, coders are listed in decompression order (reverse of compression):

**Compression:** Input → BCJ → LZMA2 → Output
**Folder coders:** [LZMA2, BCJ]
**Bind pairs:** BCJ input binds to LZMA2 output

## BCJ Algorithm Details

### x86 BCJ Transformation

**Encoding (compression):**

```
for each position i:
    if byte[i] == 0xE8 or byte[i] == 0xE9:
        if (i + 5) is aligned:
            offset = read_i32_le(i + 1)
            absolute = i + 5 + offset
            write_i32_le(i + 1, absolute)
```

**Decoding (decompression):**

```
for each position i:
    if byte[i] == 0xE8 or byte[i] == 0xE9:
        if (i + 5) is aligned:
            absolute = read_i32_le(i + 1)
            offset = absolute - (i + 5)
            write_i32_le(i + 1, offset)
```

### State Management

BCJ filters maintain state (current position) for address calculation. When used in solid archives:

- State continues across file boundaries
- Or state resets at each file (implementation-defined)

## Filter Selection Guidance

| Content Type          | Recommended Filter |
| --------------------- | ------------------ |
| x86/x64 executables   | BCJ x86            |
| ARM executables       | BCJ ARM or ARM64   |
| Audio (16-bit stereo) | Delta, distance=4  |
| Audio (16-bit mono)   | Delta, distance=2  |
| Generic binary        | None               |
| Text                  | None               |

## Implementation Notes

### Filter Detection

Filters are transparent to data integrity—filtered + unfiltered data produces identical results after decompression.

### Performance

- BCJ filters are very fast (simple byte scanning)
- Delta filter is extremely fast (single subtraction per byte)
- Filtering overhead is negligible compared to compression

### Reversibility

All filters are reversible. Applying encode then decode (or vice versa) produces original data.

## See Also

- [Unpack Info](/7z/07-unpack-info) - Coder chain definition
- [Compression Methods](/7z/10-compression-methods) - Compression algorithms
- [Method IDs](/7z/appendix/b-method-ids) - Complete method ID table
