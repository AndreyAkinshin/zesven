# Data Encoding

This document specifies the primitive data types and encoding schemes used throughout 7z archive headers.

## Integer Types

### BYTE

An 8-bit unsigned integer (0-255). The fundamental unit of storage.

### UINT32

A 32-bit unsigned integer stored in little-endian byte order.

| Byte | Significance                  |
| ---- | ----------------------------- |
| 0    | Least significant (bits 0-7)  |
| 1    | Bits 8-15                     |
| 2    | Bits 16-23                    |
| 3    | Most significant (bits 24-31) |

**Example:** Value 0x12345678 is stored as `78 56 34 12`

**Range:** 0 to 4,294,967,295

### UINT64

A 64-bit unsigned integer stored in little-endian byte order.

| Byte | Significance                  |
| ---- | ----------------------------- |
| 0    | Least significant (bits 0-7)  |
| 1    | Bits 8-15                     |
| 2    | Bits 16-23                    |
| 3    | Bits 24-31                    |
| 4    | Bits 32-39                    |
| 5    | Bits 40-47                    |
| 6    | Bits 48-55                    |
| 7    | Most significant (bits 56-63) |

**Example:** Value 0x123456789ABCDEF0 is stored as `F0 DE BC 9A 78 56 34 12`

**Range:** 0 to 18,446,744,073,709,551,615

## NUMBER Encoding

NUMBER is a variable-length encoding for unsigned 64-bit integers that optimizes for small values.

### Encoding Rules

The first byte determines the total encoding length. The number of leading 1-bits indicates how many additional bytes follow.

| First Byte Pattern | Additional Bytes | Value Calculation                                        |
| ------------------ | ---------------- | -------------------------------------------------------- |
| `0xxxxxxx`         | 0                | `value_bits` (7 bits from first byte)                    |
| `10xxxxxx`         | 1                | `(value_bits << 8) + extra[0]`                           |
| `110xxxxx`         | 2                | `(value_bits << 16) + uint16_le(extra)`                  |
| `1110xxxx`         | 3                | `(value_bits << 24) + uint24_le(extra)`                  |
| `11110xxx`         | 4                | `(value_bits << 32) + uint32_le(extra)`                  |
| `111110xx`         | 5                | `(value_bits << 40) + uint40_le(extra)`                  |
| `1111110x`         | 6                | `(value_bits << 48) + uint48_le(extra)`                  |
| `11111110`         | 7                | `uint56_le(extra)` (first byte contributes 0 value bits) |
| `11111111`         | 8                | `uint64_le(extra)` (first byte is marker only)           |

**Note:** For the 7-byte case (`11111110`), the single `x` bit is always 0, so `value_bits = 0` and the result equals `uint56_le(extra)` directly. For the 8-byte case (`11111111`), the first byte serves only as a length marker; the full 64-bit value comes from the following 8 bytes.

**Terminology:**

- `value_bits`: The `x` bits extracted from the first byte (after the length prefix)
- `extra`: The additional bytes that follow the first byte, read in little-endian order
- `extra[0]`: The first additional byte (immediately after the first byte)
- `uint16_le(extra)`: The first 2 additional bytes interpreted as little-endian UINT16

### Decoding Algorithm

```
function decode_number(stream) -> u64:
    first_byte = stream.read_byte()

    if first_byte < 0x80:           # 0xxxxxxx
        return first_byte

    # Count leading 1 bits to determine length
    mask = 0x80
    extra_bytes = 0
    while (first_byte & mask) != 0 and extra_bytes < 8:
        extra_bytes += 1
        mask >>= 1

    if extra_bytes == 8:
        # Full 64-bit value follows
        return stream.read_u64_le()

    # Extract value bits from first byte
    value_mask = (1 << (7 - extra_bytes)) - 1
    value = (first_byte & value_mask) as u64
    value <<= (extra_bytes * 8)

    # Read and add extra bytes
    for i in 0..extra_bytes:
        value += (stream.read_byte() as u64) << (i * 8)

    return value
```

### Encoding Algorithm

```
function encode_number(value: u64) -> bytes:
    if value < 0x80:
        return [value as u8]

    # Determine minimum bytes needed
    bytes_needed = 1
    temp = value >> 7
    while temp > 0:
        bytes_needed += 1
        temp >>= 8

    if bytes_needed > 8:
        bytes_needed = 8

    result = []

    if bytes_needed == 8:
        result.push(0xFF)
        result.extend(value.to_le_bytes())
    else:
        # Calculate first byte with length prefix
        first_byte_value_bits = 8 - bytes_needed - 1
        prefix_mask = 0xFF << (first_byte_value_bits + 1)
        value_from_first = (value >> ((bytes_needed - 1) * 8)) & ((1 << first_byte_value_bits) - 1)
        first_byte = (prefix_mask & 0xFF) | value_from_first
        result.push(first_byte)

        # Remaining bytes in little-endian
        for i in 0..(bytes_needed - 1):
            result.push((value >> (i * 8)) & 0xFF)

    return result
```

### Examples

| Value    | Encoding                     | Explanation                                            |
| -------- | ---------------------------- | ------------------------------------------------------ | -------------------------------------------------------------------------- |
| 0        | `00`                         | Single byte, value_bits = 0                            |
| 1        | `01`                         | Single byte, value_bits = 1                            |
| 127      | `7F`                         | Maximum single-byte value, value_bits = 127            |
| 128      | `80 80`                      | First byte `0x80` = `10                                | 000000`, value_bits = 0, extra[0] = 128; result = (0 << 8) + 128 = 128     |
| 255      | `80 FF`                      | value_bits = 0, extra[0] = 255; result = 0 + 255 = 255 |
| 256      | `81 00`                      | First byte `0x81` = `10                                | 000001`, value_bits = 1, extra[0] = 0; result = (1 << 8) + 0 = 256         |
| 16383    | `BF FF`                      | First byte `0xBF` = `10                                | 111111`, value_bits = 63, extra[0] = 255; result = (63 << 8) + 255 = 16383 |
| 16384    | `C0 00 40`                   | First byte `0xC0` = `110                               | 00000`, value_bits = 0, extra = `00 40` = 16384; result = 0 + 16384        |
| 65535    | `C0 FF FF`                   | value_bits = 0, extra = `FF FF` = 65535                |
| 2^32 - 1 | `F0 FF FF FF FF`             | 4 additional bytes                                     |
| 2^64 - 1 | `FF FF FF FF FF FF FF FF FF` | 9 bytes total (prefix + 8 additional bytes)            |

### Canonical Form

The encoding is NOT required to be canonical. Both `00` and `80 00` represent zero. Implementations:

- MUST accept any valid encoding
- SHOULD produce minimal (canonical) encoding when writing

## BitField

A BitField packs boolean values into bytes, with the first value in the most significant bit.

### Layout

```
Byte 0:  [b0][b1][b2][b3][b4][b5][b6][b7]
          ^                          ^
          MSB (bit 7)                LSB (bit 0)
          = item 0                   = item 7

Byte 1:  [b8][b9][b10][b11][b12][b13][b14][b15]
          = item 8                        = item 15
```

### Size Calculation

For N items:

```
num_bytes = (N + 7) / 8  # Integer division, rounds up
```

### Padding

If N is not a multiple of 8, the remaining bits in the last byte are padding bits.

- **Writers** MUST set padding bits to zero.
- **Readers** MUST ignore padding bits (treat them as if they were zero regardless of actual value).

Non-zero padding bits do not make the archive invalid but indicate a non-conforming writer.

### Access

To access item `i`:

```
byte_index = i / 8
bit_index = 7 - (i % 8)  # MSB first
value = (bytes[byte_index] >> bit_index) & 1
```

### Example

For items [true, false, true, true, false, false, true, false, true]:

```
Item:    0     1     2     3     4     5     6     7     8
Value:   1     0     1     1     0     0     1     0     1
         ─────────────────────────────────────     ────────
Byte 0:  1 0 1 1 0 0 1 0  = 0xB2
Byte 1:  1 0 0 0 0 0 0 0  = 0x80  (padded with zeros)

Encoding: B2 80
```

## BooleanList

A BooleanList is an optimized boolean array with an "all true" shortcut.

### Structure

```
BooleanList ::= AllAreDefined [BitField]
AllAreDefined ::= BYTE
```

### Semantics

| AllAreDefined Value       | Meaning                                  |
| ------------------------- | ---------------------------------------- |
| 0x00                      | BitField follows with individual values  |
| Non-zero (typically 0x01) | All values are true, no BitField follows |

**Note:** While 0x01 is the canonical "all true" value, implementations MUST treat any non-zero value as "all true" for compatibility. Implementations SHOULD write 0x01 when encoding.

### Decoding

```
function decode_boolean_list(stream, count) -> [bool]:
    all_defined = stream.read_byte()

    if all_defined != 0:  # Any non-zero value means all true
        return [true; count]

    result = []
    num_bytes = (count + 7) / 8
    bytes = stream.read_bytes(num_bytes)

    for i in 0..count:
        byte_idx = i / 8
        bit_idx = 7 - (i % 8)
        result.push((bytes[byte_idx] >> bit_idx) & 1 == 1)

    return result
```

### Example

For 5 items all true:

```
01              # AllAreDefined = 1, no BitField needed
```

For 5 items [true, false, true, true, false]:

```
00              # AllAreDefined = 0
B8              # BitField: 10111000 (items 0-4 in bits 7-3, padding in 2-0)
```

## String Encoding

### UTF-16-LE

File names and comments are encoded as UTF-16-LE (Little Endian) with null termination.

**Character encoding:**

- BMP characters (U+0000 to U+FFFF): 2 bytes, little-endian
- Supplementary characters (U+10000+): 4 bytes (surrogate pair)

**Termination:** Two zero bytes (`00 00`)

**Example:** "test" = `74 00 65 00 73 00 74 00 00 00`

| Character    | UTF-16-LE |
| ------------ | --------- |
| 't' (U+0074) | `74 00`   |
| 'e' (U+0065) | `65 00`   |
| 's' (U+0073) | `73 00`   |
| 't' (U+0074) | `74 00`   |
| NUL          | `00 00`   |

### UTF-8

Symbolic link targets are encoded as UTF-8 (no BOM, no null terminator in stream).

### UTF-16 Validation

When reading UTF-16-LE strings, implementations MUST handle these edge cases:

| Condition                                                               | Handling                      |
| ----------------------------------------------------------------------- | ----------------------------- |
| Unpaired high surrogate (U+D800-U+DBFF not followed by U+DC00-U+DFFF)   | Replace with U+FFFD or reject |
| Unpaired low surrogate (U+DC00-U+DFFF without preceding high surrogate) | Replace with U+FFFD or reject |
| Embedded null (0x00 0x00) before expected terminator                    | Truncate string at first null |
| Odd byte count (not divisible by 2)                                     | Reject as invalid encoding    |

Writers MUST produce valid UTF-16-LE without unpaired surrogates.

## Size-Prefixed Data

Some header sections use size-prefixed data for forward compatibility:

```
SizedData ::= PropertyID Size Data
Size ::= NUMBER
Data ::= BYTE[Size]
```

Unknown property IDs can be skipped by reading and discarding Size bytes.

## See Also

- [Files Info](/7z/09-files-info#name) - File name encoding
- [CRC Algorithm](/7z/appendix/c-crc-algorithm) - CRC-32 for UINT32 checksums
- [Timestamps & Attributes](/7z/16-timestamps-attributes) - FILETIME encoding
