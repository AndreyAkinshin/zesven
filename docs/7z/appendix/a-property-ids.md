# Appendix A: Property IDs

This appendix provides a complete reference of all property IDs used in 7z archive headers.

## Property ID Table

| ID  | Hex  | Name                  | Context        | Description                 |
| --- | ---- | --------------------- | -------------- | --------------------------- |
| 0   | 0x00 | End                   | All            | End of section marker       |
| 1   | 0x01 | Header                | Top-level      | Main header marker          |
| 2   | 0x02 | ArchiveProperties     | Header         | Archive-wide properties     |
| 3   | 0x03 | AdditionalStreamsInfo | Header         | Additional data streams     |
| 4   | 0x04 | MainStreamsInfo       | Header         | Primary stream information  |
| 5   | 0x05 | FilesInfo             | Header         | File metadata section       |
| 6   | 0x06 | PackInfo              | StreamsInfo    | Pack stream information     |
| 7   | 0x07 | UnpackInfo            | StreamsInfo    | Unpack/folder information   |
| 8   | 0x08 | SubStreamsInfo        | StreamsInfo    | Per-file stream information |
| 9   | 0x09 | Size                  | Various        | Size array marker           |
| 10  | 0x0A | CRC                   | Various        | CRC/digest marker           |
| 11  | 0x0B | Folder                | UnpackInfo     | Folder definition marker    |
| 12  | 0x0C | CodersUnpackSize      | UnpackInfo     | Coder output sizes          |
| 13  | 0x0D | NumUnpackStream       | SubStreamsInfo | Streams per folder          |
| 14  | 0x0E | EmptyStream           | FilesInfo      | Empty stream bitmap         |
| 15  | 0x0F | EmptyFile             | FilesInfo      | Empty file bitmap           |
| 16  | 0x10 | Anti                  | FilesInfo      | Anti-item bitmap            |
| 17  | 0x11 | Name                  | FilesInfo      | File names                  |
| 18  | 0x12 | CTime                 | FilesInfo      | Creation time               |
| 19  | 0x13 | ATime                 | FilesInfo      | Access time                 |
| 20  | 0x14 | MTime                 | FilesInfo      | Modification time           |
| 21  | 0x15 | Attributes            | FilesInfo      | Windows attributes          |
| 22  | 0x16 | Comment               | FilesInfo      | Archive comment             |
| 23  | 0x17 | EncodedHeader         | Top-level      | Encoded header marker       |
| 24  | 0x18 | StartPos              | Rare           | Start position              |
| 25  | 0x19 | Dummy                 | FilesInfo      | Padding/alignment           |

## Property Contexts

### Top-Level Context

Properties that can appear at the start of header data:

| ID   | Property                          |
| ---- | --------------------------------- |
| 0x01 | Header (plain header)             |
| 0x17 | EncodedHeader (compressed header) |

### Header Context

Properties within a Header (0x01) section:

| ID   | Property          | Required |
| ---- | ----------------- | -------- |
| 0x02 | ArchiveProperties | Optional |
| 0x04 | MainStreamsInfo   | Optional |
| 0x05 | FilesInfo         | Optional |
| 0x00 | End               | Required |

### StreamsInfo Context

Properties within MainStreamsInfo (0x04):

| ID   | Property       | Required |
| ---- | -------------- | -------- |
| 0x06 | PackInfo       | Optional |
| 0x07 | UnpackInfo     | Optional |
| 0x08 | SubStreamsInfo | Optional |
| 0x00 | End            | Required |

### PackInfo Context

Properties within PackInfo (0x06):

| ID   | Property | Required              |
| ---- | -------- | --------------------- |
| 0x09 | Size     | If NumPackStreams > 0 |
| 0x0A | CRC      | Optional              |
| 0x00 | End      | Required              |

### UnpackInfo Context

Properties within UnpackInfo (0x07):

| ID   | Property         | Required |
| ---- | ---------------- | -------- |
| 0x0B | Folder           | Required |
| 0x0C | CodersUnpackSize | Required |
| 0x0A | CRC              | Optional |
| 0x00 | End              | Required |

### SubStreamsInfo Context

Properties within SubStreamsInfo (0x08):

| ID   | Property        | Required               |
| ---- | --------------- | ---------------------- |
| 0x0D | NumUnpackStream | Optional               |
| 0x09 | Size            | If multiple substreams |
| 0x0A | CRC             | Optional               |
| 0x00 | End             | Required               |

### FilesInfo Context

Properties within FilesInfo (0x05):

| ID   | Property    | Required    | Order |
| ---- | ----------- | ----------- | ----- |
| 0x0E | EmptyStream | Optional    | 1     |
| 0x0F | EmptyFile   | Optional    | 2     |
| 0x10 | Anti        | Optional    | 3     |
| 0x11 | Name        | Recommended | 4     |
| 0x12 | CTime       | Optional    | 5     |
| 0x13 | ATime       | Optional    | 6     |
| 0x14 | MTime       | Optional    | 7     |
| 0x15 | Attributes  | Optional    | 8     |
| 0x16 | Comment     | Optional    | 9     |
| 0x19 | Dummy       | Optional    | 10    |
| 0x00 | End         | Required    | Last  |

## Property Structures

### End (0x00)

No data. Marks end of containing section.

### Header (0x01)

```
Header ::= 0x01 [ArchiveProperties] [MainStreamsInfo] [FilesInfo] 0x00
```

### ArchiveProperties (0x02)

```
ArchiveProperties ::= 0x02 Property* 0x00
Property ::= PropertyID Size Data
```

Rarely used. Contains archive-level settings.

### AdditionalStreamsInfo (0x03)

**Status:** Deprecated. This property is rarely used in practice and implementations SHOULD NOT generate archives using it. Implementations MUST support reading archives that contain this property for backward compatibility.

```
AdditionalStreamsInfo ::= 0x03 StreamsInfo
```

Contains auxiliary streams for external data storage. This was originally intended for storing encrypted file names or other metadata separately from the main data streams. In modern archives, external data references (via `External == 0x01` in various properties) typically point to streams defined within AdditionalStreamsInfo.

### MainStreamsInfo (0x04)

```
MainStreamsInfo ::= 0x04 [PackInfo] [UnpackInfo] [SubStreamsInfo] 0x00
```

### FilesInfo (0x05)

```
FilesInfo ::= 0x05 NumFiles Property* 0x00
```

### PackInfo (0x06)

```
PackInfo ::= 0x06 PackPos NumPackStreams [Sizes] [Digests] 0x00
```

### UnpackInfo (0x07)

```
UnpackInfo ::= 0x07 Folders [CodersUnpackSize] [Digests] 0x00
```

### SubStreamsInfo (0x08)

```
SubStreamsInfo ::= 0x08 [NumUnpackStream] [Sizes] [Digests] 0x00
```

### Size (0x09)

```
Sizes ::= 0x09 NUMBER* 0x00
```

Array of size values (context-dependent count).

### CRC (0x0A)

```
Digests ::= 0x0A AllAreDefined [BitField] CRC*
```

Array of CRC-32 checksums.

### Folder (0x0B)

```
Folders ::= 0x0B NumFolders External FolderData
```

### CodersUnpackSize (0x0C)

```
CodersUnpackSize ::= 0x0C NUMBER*
```

Unpack sizes for all coder outputs across all folders.

### NumUnpackStream (0x0D)

```
NumUnpackStream ::= 0x0D NUMBER[NumFolders]
```

Number of files per folder. Note: This section has no End marker; the count of NUMBER values equals NumFolders from UnpackInfo. See [08-SUBSTREAMS-INFO](/7z/08-substreams-info) for details.

### EmptyStream (0x0E)

```
EmptyStream ::= 0x0E Size BitField
```

Bitmap: 1 = entry has no data stream.

### EmptyFile (0x0F)

```
EmptyFile ::= 0x0F Size BitField
```

Bitmap for empty streams: 1 = file, 0 = directory.

### Anti (0x10)

```
Anti ::= 0x10 Size BitField
```

Bitmap for empty streams: 1 = anti-item (deletion marker).

### Name (0x11)

```
Name ::= 0x11 Size External [DataIndex | UTF16Names]
```

File names in UTF-16-LE, null-terminated.

### CTime (0x12)

```
CTime ::= 0x12 Size AllAreDefined [BitField] External [DataIndex | Times]
```

Creation timestamps (FILETIME).

### ATime (0x13)

```
ATime ::= 0x13 Size AllAreDefined [BitField] External [DataIndex | Times]
```

Access timestamps (FILETIME).

### MTime (0x14)

```
MTime ::= 0x14 Size AllAreDefined [BitField] External [DataIndex | Times]
```

Modification timestamps (FILETIME).

### Attributes (0x15)

```
Attributes ::= 0x15 Size AllAreDefined [BitField] External [DataIndex | Attrs]
```

Windows/Unix file attributes (UINT32).

### Comment (0x16)

```
Comment ::= 0x16 Size External [DataIndex | UTF16Text]
```

Archive comment in UTF-16-LE.

### EncodedHeader (0x17)

```
EncodedHeader ::= 0x17 PackInfo UnpackInfo 0x00
```

Indicates header is compressed/encrypted.

### StartPos (0x18)

**Status:** Deprecated. This property is rarely used in practice and implementations SHOULD NOT generate archives using it. Implementations SHOULD ignore this property when reading.

```
StartPos ::= 0x18 Size Data
```

Originally intended for storing file start positions within streams. This property is not used by modern versions of 7-Zip and has no defined semantics. Archives containing this property are extremely rare.

### Dummy (0x19)

```
Dummy ::= 0x19 Size Zeros
```

Padding bytes (all zeros). Used for alignment.

## Unknown Properties

Properties with unknown IDs SHOULD be skipped:

```
function skip_unknown_property(stream, property_id):
    size = stream.read_number()
    stream.skip(size)
    log_warning("Unknown property ID: 0x{:02X}", property_id)
```

This enables forward compatibility with future format extensions.

## See Also

- [Header Structure](/7z/05-header-structure) - Header organization
- [Files Info](/7z/09-files-info) - File property details
- [Data Encoding](/7z/04-data-encoding) - NUMBER and BitField encoding
