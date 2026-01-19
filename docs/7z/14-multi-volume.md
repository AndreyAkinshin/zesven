# Multi-Volume Archives

This document specifies multi-volume (split) archive handling in 7z format.

## Overview

Multi-volume archives split a single logical archive across multiple physical files (volumes). This enables:

- Distribution on size-limited media
- Easier transfer of large archives
- Incremental download/upload

## Naming Convention

Volumes use zero-padded numeric extensions:

```
archive.7z.001    # First volume
archive.7z.002    # Second volume
archive.7z.003    # Third volume
...
archive.7z.NNN    # Nth volume
```

**Extension format:** `.NNN` (always 3 digits, zero-padded)

**Maximum volumes:** 999 (extensions 001-999)

**Beyond 999:** Extensions `.1000` and higher are undefined by this specification. Implementations encountering such extensions SHOULD treat them as continuation volumes using implementation-defined behavior, or MAY reject the archive. Writers MUST NOT create archives with more than 999 volumes.

## Volume Structure

### First Volume

Contains the signature header:

```
┌─────────────────────────┐
│    Signature Header     │  32 bytes
├─────────────────────────┤
│    Pack Data (start)    │  Up to volume boundary
└─────────────────────────┘
```

### Middle Volumes

Contain only pack data:

```
┌─────────────────────────┐
│  Pack Data (continued)  │  Full volume
└─────────────────────────┘
```

### Last Volume

Contains pack data end and header:

```
┌─────────────────────────┐
│   Pack Data (end)       │  Remaining compressed data
├─────────────────────────┤
│   Next Header           │  Archive metadata
└─────────────────────────┘
```

## Virtual Stream

Volumes concatenate to form a virtual stream:

```
Volume 1:  [Signature][Data 0x00-0x0FFF]
Volume 2:  [Data 0x1000-0x1FFF]
Volume 3:  [Data 0x2000-0x27FF][Header]

Virtual:   [Signature][Data 0x00-0x27FF][Header]
```

## Header Location

The header is always in the last volume:

1. `NextHeaderOffset` in signature header points past all pack data
2. Seeking to header position requires seeking into appropriate volume
3. Header must fit entirely within the last volume

## Volume Size Calculation

When creating multi-volume archives:

```
function write_volumes(data, header, volume_size):
    total_data = len(data) + len(header)

    # First volume has 32-byte signature
    first_volume_data = volume_size - 32
    remaining = total_data - first_volume_data

    # Calculate needed volumes
    middle_volumes = remaining / volume_size
    last_volume_size = remaining % volume_size

    # Ensure header fits in last volume
    if last_volume_size < len(header):
        # Adjust distribution
```

## Reading Multi-Volume Archives

### Discovery

1. Open first volume (`*.001`)
2. Read signature header
3. Discover subsequent volumes by incrementing extension
4. Verify all volumes exist and have expected sizes

### Virtual Seek

```
function seek(position):
    if position < 32:
        # Within signature header (volume 1)
        return volume[0].seek(position)

    data_position = position - 32
    volume_index = 0
    remaining = data_position

    # Skip past first volume's data portion
    first_data_size = volume_sizes[0] - 32
    if remaining < first_data_size:
        return volume[0].seek(32 + remaining)
    remaining -= first_data_size
    volume_index = 1

    # Find correct middle/last volume
    while volume_index < len(volumes):
        vol_size = volume_sizes[volume_index]
        if remaining < vol_size:
            return volume[volume_index].seek(remaining)
        remaining -= vol_size
        volume_index += 1

    error("Seek past end of archive")
```

### Virtual Read

```
function read(position, size):
    result = []
    while len(result) < size:
        volume, offset = translate_position(position + len(result))
        available = volume.size - offset
        chunk = min(size - len(result), available)
        result.extend(volume.read(offset, chunk))
    return result
```

## Writing Multi-Volume Archives

### Process

1. Determine volume size
2. Write signature header to volume 1
3. Compress and write pack data, splitting across volumes
4. Write header to last volume
5. Finalize signature header offsets

### Volume Boundary Handling

Pack streams may span volume boundaries:

```
Pack Stream: [AAAA|BBBB|CCCC]
                  ^    ^
             Vol boundary

Volume 1: [Sig][AAAA]
Volume 2: [BBBB]
Volume 3: [CCCC][Header]
```

This is transparent to the compression layer.

## Incomplete Volume Sets

### Detection

Check for missing volumes:

- Verify sequential numbering (001, 002, 003...)
- Verify each volume size (except last may differ)
- Verify total size matches signature header expectations

### Error Handling

| Condition        | Action                                   |
| ---------------- | ---------------------------------------- |
| Missing volume   | Abort with error indicating which volume |
| Truncated volume | Abort with corruption error              |
| Extra files      | Ignore (may be backup copies)            |
| Wrong order      | Volume numbers ensure correct ordering   |

## Atomic Operations

Updating multi-volume archives:

1. Cannot modify in-place (would require shifting all volumes)
2. Create new volume set for updates
3. Delete old volumes only after new set is complete

## Size Recommendations

| Media            | Volume Size |
| ---------------- | ----------- |
| CD-R             | 700 MiB     |
| DVD              | 4.7 GiB     |
| FAT32            | < 4 GiB     |
| Cloud upload     | 100-500 MiB |
| Email attachment | 10-25 MiB   |

## Implementation Notes

### Memory Mapping

For large volumes, use memory-mapped I/O or streaming reads rather than loading entire volumes.

### Parallel Processing

Volumes can be read in parallel when extracting from non-solid archives (if files are in different volumes).

### Network Resilience

For network transfers:

- Verify each volume checksum before processing
- Support resumable downloads per-volume
- Allow partial extraction from available volumes

## Example

Archive split into 3 volumes of 10 MiB each:

```
archive.7z.001:
  Offset 0x00: Signature Header
  Offset 0x20: Pack data bytes 0-10485727

archive.7z.002:
  Offset 0x00: Pack data bytes 10485728-20971455

archive.7z.003:
  Offset 0x00: Pack data bytes 20971456-25000000
  Offset after pack: Next Header (remaining space)
```

Signature header contains:

- `NextHeaderOffset = 25000001 - 32 = 24999969`
- `NextHeaderSize = (size of header)`

## Compatibility

- 7-Zip: Full support
- p7zip: Full support
- Most third-party tools: Support varies

Some tools only support single-file archives; always test compatibility for target environment.

## See Also

- [Archive Structure](/7z/02-archive-structure) - Single-volume layout
- [Signature Header](/7z/03-signature-header) - Header offset interpretation
- [Error Conditions](/7z/18-error-conditions) - Volume error handling
