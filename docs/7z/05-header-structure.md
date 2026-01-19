# Header Structure

This document specifies the organization of the header database in a 7z archive.

## Overview

The header database contains all metadata about the archive contents:

- Compressed stream information (pack info)
- Decompression instructions (unpack info)
- File data boundaries within streams (substreams info)
- File metadata (files info)

The header may be stored in plain form or encoded (compressed/encrypted).

## Header Types

### Plain Header

A plain header starts with property ID `0x01` (Header) and contains metadata directly:

```
PlainHeader ::= 0x01 [MainStreamsInfo] [FilesInfo] 0x00
```

### Encoded Header

An encoded header starts with property ID `0x17` (EncodedHeader) and contains compressed metadata:

```
EncodedHeader ::= 0x17 StreamsInfo
```

Where `StreamsInfo` describes how to decompress the actual header data. After decompression, the result is parsed as a `PlainHeader`.

## Main Header Structure

```
Header ::= 0x01 [ArchiveProperties] [MainStreamsInfo] [FilesInfo] 0x00

ArchiveProperties ::= 0x02 [Property]* 0x00

MainStreamsInfo ::= 0x04 [PackInfo] [UnpackInfo] [SubStreamsInfo] 0x00

FilesInfo ::= 0x05 NumFiles [FileProperty]* 0x00
```

## Property ID Ordering

Property IDs within a structure MUST be emitted in ascending numerical order by writers.

Readers MUST accept property IDs in any order. Out-of-order properties indicate a non-conforming writer but do not make the archive invalid. Readers SHOULD emit a warning when property IDs appear out of order to assist in identifying non-conforming archives.

**Duplicate property IDs:** If the same property ID appears more than once within a structure, implementations MUST reject the archive with ERROR_ARCHIVE. Duplicate properties indicate either a corrupt archive or a malformed writer; there is no defined semantics for merging or selecting among duplicates.

**Rationale:** While the format requires ascending order, many historical and third-party implementations produce archives with out-of-order properties. Rejecting such archives would break compatibility. 7-Zip itself accepts properties in any order during reading, though it always writes them in ascending order.

### Required Order for Writers

**In Header:**

1. `0x01` Header (always first)
2. `0x02` ArchiveProperties (optional)
3. `0x04` MainStreamsInfo (optional)
4. `0x05` FilesInfo (optional)
5. `0x00` End (always last)

**In MainStreamsInfo:**

1. `0x04` MainStreamsInfo marker
2. `0x06` PackInfo (optional)
3. `0x07` UnpackInfo (optional)
4. `0x08` SubStreamsInfo (optional)
5. `0x00` End

## Encoded Header Processing

When the next header starts with `0x17` (EncodedHeader):

### Structure

```
EncodedHeader ::= 0x17 PackInfo UnpackInfo 0x00
```

The PackInfo and UnpackInfo describe how to decode the actual header:

- PackInfo points to compressed header data (typically after main pack data)
- UnpackInfo defines the decompression method (typically LZMA2)

### Decoding Procedure

1. Parse `0x17` marker
2. Parse PackInfo to find compressed header location and size
3. Parse UnpackInfo to get decompression method
4. Read compressed header data
5. Decompress using specified method
6. Parse decompressed data as PlainHeader

### Recursive Encoding

Headers may be recursively encoded (encoded header containing another encoded header). Implementations:

- MUST support at least 1 level of encoding
- SHOULD support up to 4 levels for compatibility with all known archives
- MUST reject archives with more than 4 levels of nesting to prevent stack overflow and denial-of-service attacks (see [17-SECURITY](/7z/17-security))

### Encrypted Headers

When combined with encryption:

- The EncodedHeader's coder chain includes AES encryption
- Password is required to read file names
- Provides metadata privacy

## Archive Properties

```
ArchiveProperties ::= 0x02 [Property]* 0x00

Property ::= PropertyID Size Data
PropertyID ::= BYTE
Size ::= NUMBER
Data ::= BYTE[Size]
```

Archive properties are rarely used. Known properties are implementation-specific. Unknown properties SHOULD be ignored.

## Streams Info Structure

MainStreamsInfo contains three optional sections:

```
MainStreamsInfo ::= 0x04 [PackInfo] [UnpackInfo] [SubStreamsInfo] 0x00
```

| Section        | Property ID | Description                      |
| -------------- | ----------- | -------------------------------- |
| PackInfo       | 0x06        | Compressed stream sizes and CRCs |
| UnpackInfo     | 0x07        | Folder and coder definitions     |
| SubStreamsInfo | 0x08        | Per-file sizes within folders    |

See individual specification documents for each section:

- [06-PACK-INFO](/7z/06-pack-info)
- [07-UNPACK-INFO](/7z/07-unpack-info)
- [08-SUBSTREAMS-INFO](/7z/08-substreams-info)

## Files Info Structure

```
FilesInfo ::= 0x05 NumFiles [FileProperty]* 0x00

NumFiles ::= NUMBER

FileProperty ::= PropertyID Size [PropertyData]
```

File properties define metadata for each entry. See [09-FILES-INFO](/7z/09-files-info).

## External Data

Some properties support "external" storage, where the actual data is stored in an additional stream rather than inline:

```
PropertyData ::= External [DataIndex | InlineData]
External ::= BYTE  # 0x00 = inline, 0x01 = external
DataIndex ::= NUMBER  # Zero-based index into AdditionalStreamsInfo
InlineData ::= BYTE*  # Direct data
```

### DataIndex Resolution

When `External == 0x01`:

- `DataIndex` is a zero-based index into the streams defined by AdditionalStreamsInfo (property ID 0x03)
- The referenced stream contains the actual property data
- Implementations MUST decompress the referenced stream using the coders specified in AdditionalStreamsInfo

See [A-PROPERTY-IDS](/7z/appendix/a-property-ids) for AdditionalStreamsInfo (0x03) documentation.

External storage is used when:

- File names are encrypted separately
- Data is too large for inline storage
- Additional streams provide the data

**Note:** External storage via AdditionalStreamsInfo (0x03) is rarely used in modern archives. Most implementations store all property data inline (`External == 0x00`). See [A-PROPERTY-IDS](/7z/appendix/a-property-ids#additionalstreamsinfo) for deprecation status.

### External Reference Validation

When `External == 0x01`:

- AdditionalStreamsInfo (property ID 0x03) MUST be present in the header. If absent, implementations MUST reject the archive with ERROR_ARCHIVE.
- DataIndex MUST be less than the number of streams in AdditionalStreamsInfo. Out-of-bounds indices MUST cause ERROR_ARCHIVE.
- The referenced stream's decompressed content is interpreted according to the property type.
- Empty referenced streams are valid only for properties that permit zero items (e.g., Names for an archive with NumFiles = 0).

When `External == 0x00`, data follows inline and no AdditionalStreamsInfo reference is made.

## Parsing Algorithm

```
function parse_header(data: bytes) -> Header:
    stream = ByteStream(data)
    header = Header()

    first_byte = stream.read_byte()

    if first_byte == 0x17:  # EncodedHeader
        encoded_info = parse_encoded_header_info(stream)
        decoded_data = decode_header(encoded_info)
        return parse_header(decoded_data)  # Recursive

    if first_byte != 0x01:  # Must be Header
        error("Invalid header: expected 0x01 or 0x17")

    while true:
        property_id = stream.read_byte()

        match property_id:
            0x00:  # End
                break
            0x02:  # ArchiveProperties
                header.archive_props = parse_archive_properties(stream)
            0x04:  # MainStreamsInfo
                header.streams_info = parse_streams_info(stream)
            0x05:  # FilesInfo
                header.files_info = parse_files_info(stream)
            _:
                # Unknown property - skip it
                size = stream.read_number()
                stream.skip(size)

    return header
```

## Empty Header

A minimal valid header for an empty archive:

```
01 00
```

- `01`: Header property ID
- `00`: End property ID

This represents an archive with no files, no streams, and no metadata.

## Header with Files Only

An archive with empty files (no compressed data):

```
01              # Header
  05            # FilesInfo
    02          # NumFiles = 2
    0E          # EmptyStream
      01        # Size = 1
      C0        # BitField: both files are empty streams (bits 7,6 = 1,1)
    11          # Name
      0E        # Size = 14
      00        # External = inline
      61 00 00 00     # "a" + null
      62 00 00 00     # "b" + null
    00          # End of FilesInfo
  00            # End of Header
```

## Implementation Notes

### Memory Considerations

For large archives:

- Header may be megabytes in size
- Consider streaming parser for memory efficiency
- Enforce header size limits (see [17-SECURITY](/7z/17-security))

### Validation

- Verify property IDs are in ascending order
- Check that Size values don't exceed remaining data
- Validate cross-references (stream indices, file counts)

### Forward Compatibility

Unknown property IDs with size prefixes can be safely skipped. This allows older implementations to read archives created by newer versions (with feature limitations).

## See Also

- [Pack Info](/7z/06-pack-info) - PackInfo structure
- [Unpack Info](/7z/07-unpack-info) - UnpackInfo and folder structure
- [Substreams Info](/7z/08-substreams-info) - SubStreamsInfo structure
- [Files Info](/7z/09-files-info) - FilesInfo structure
- [Property IDs](/7z/appendix/a-property-ids) - Complete property ID reference
