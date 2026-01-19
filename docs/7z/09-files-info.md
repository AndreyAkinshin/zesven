# Files Info

This document specifies the FilesInfo structure that contains metadata for all entries in a 7z archive.

## Overview

FilesInfo contains:

- File and directory names
- Empty stream/file markers
- Anti-item markers (deletion)
- Timestamps (creation, access, modification)
- File attributes (Windows and Unix)
- Comments (rarely used)

## Structure

```
FilesInfo ::= 0x05 NumFiles Property* 0x00

NumFiles ::= NUMBER
Property ::= PropertyID Size PropertyData
```

## Property Order

Properties MUST appear in ascending order by property ID:

| Order | ID   | Name        | Description                                |
| ----- | ---- | ----------- | ------------------------------------------ |
| 1     | 0x0E | EmptyStream | Marks entries without data                 |
| 2     | 0x0F | EmptyFile   | Distinguishes empty files from directories |
| 3     | 0x10 | Anti        | Marks deletion entries                     |
| 4     | 0x11 | Name        | File/directory names                       |
| 5     | 0x12 | CTime       | Creation time                              |
| 6     | 0x13 | ATime       | Last access time                           |
| 7     | 0x14 | MTime       | Last modification time                     |
| 8     | 0x15 | Attributes  | Windows/Unix attributes                    |
| 9     | 0x16 | Comment     | Archive comment                            |
| 10    | 0x19 | Dummy       | Padding for alignment                      |

## EmptyStream Property (0x0E)

Marks entries that have no associated data stream.

```
EmptyStream ::= 0x0E Size BitField

Size ::= NUMBER  # Total byte count of data following this field (i.e., BitField only)
BitField ::= BYTE[(NumFiles + 7) / 8]
```

**Size field semantics:** The Size field specifies the number of bytes in the BitField that follows. For EmptyStream, this equals `(NumFiles + 7) / 8`. Readers MUST verify that Size matches the expected BitField length; mismatches indicate a corrupt or malformed archive.

**Semantics:**

- Bit set = entry has no data (empty file or directory)
- Bit clear = entry has data in SubStreamsInfo

**Index mapping:** Empty stream entries are skipped when indexing into SubStreamsInfo.

## EmptyFile Property (0x0F)

Distinguishes empty files from directories among EmptyStream entries.

```
EmptyFile ::= 0x0F Size BitField
```

**Only applies to entries where EmptyStream bit is set.**

**Semantics:**

- Bit set = empty file (0 bytes)
- Bit clear = directory

**BitField size:** `(NumEmptyStreams + 7) / 8`

## Anti Property (0x10)

Marks entries for deletion in incremental backups.

```
Anti ::= 0x10 Size BitField
```

**Only applies to entries where EmptyStream bit is set.**

**Semantics:**

- Bit set = anti-item (delete this file/directory)
- Bit clear = normal entry

Anti-items indicate that a previously archived file should be removed when applying the archive.

**Extraction behavior:** When extracting an archive containing anti-items:

- If the target path exists, implementations SHOULD delete it (file or directory)
- If the target path does not exist, implementations SHOULD ignore the anti-item (no error)
- Implementations MAY prompt the user before deleting files
- Directory anti-items SHOULD only delete empty directories; non-empty directories MAY be skipped with a warning

**Security consideration:** Anti-items can be used maliciously to delete files outside the intended extraction scope. Path validation (see `[17-SECURITY](/7z/17-security)`) MUST be applied to anti-item paths.

## Name Property (0x11)

File and directory names.

```
Name ::= 0x11 Size External [DataIndex | Names]

External ::= BYTE  # 0x00 = inline, 0x01 = external
DataIndex ::= NUMBER
Names ::= Name[NumFiles]
Name ::= UTF16LE_Char* 0x0000
```

**Encoding:** UTF-16-LE with null terminator (2 zero bytes).

**Path format:**

- Path separator: `/` (forward slash)
- Paths MUST be relative (no leading `/`)
- Paths MUST NOT contain `..` components
- Directory entries SHOULD end with `/`

**Examples:**

```
"file.txt"          →  66 00 69 00 6C 00 65 00 2E 00 74 00 78 00 74 00 00 00
"dir/"              →  64 00 69 00 72 00 2F 00 00 00
"dir/subfile.txt"   →  64 00 69 00 72 00 2F 00 73 00 ... 00 00
```

## Time Properties (0x12, 0x13, 0x14)

Creation, access, and modification times.

```
TimeProperty ::= PropertyID Size AllAreDefined [BitField] External [DataIndex | Times]

PropertyID ::= 0x12 | 0x13 | 0x14
AllAreDefined ::= BYTE
External ::= BYTE
Times ::= FILETIME[NumDefined]
FILETIME ::= UINT64  # 100-nanosecond intervals since 1601-01-01
```

**AllAreDefined:**

- `0x01`: All files have this time, no BitField
- `0x00`: BitField indicates which files have times

**External:**

- `0x00`: Times stored inline
- `0x01`: Times stored in external stream

See `[16-TIMESTAMPS-ATTRIBUTES](/7z/16-timestamps-attributes)` for FILETIME details.

## Attributes Property (0x15)

Windows and Unix file attributes.

```
Attributes ::= 0x15 Size AllAreDefined [BitField] External [DataIndex | Attrs]

Attrs ::= UINT32[NumDefined]
```

**Attribute format:**

| Bits  | Description                 |
| ----- | --------------------------- |
| 0-15  | Windows attributes          |
| 16-31 | Unix mode (when bit 15 set) |

**Windows attributes (bits 0-15):**

| Bit | Value  | Name                         |
| --- | ------ | ---------------------------- |
| 0   | 0x0001 | FILE_ATTRIBUTE_READONLY      |
| 1   | 0x0002 | FILE_ATTRIBUTE_HIDDEN        |
| 2   | 0x0004 | FILE_ATTRIBUTE_SYSTEM        |
| 4   | 0x0010 | FILE_ATTRIBUTE_DIRECTORY     |
| 5   | 0x0020 | FILE_ATTRIBUTE_ARCHIVE       |
| 10  | 0x0400 | FILE_ATTRIBUTE_REPARSE_POINT |
| 11  | 0x0800 | FILE_ATTRIBUTE_COMPRESSED    |
| 15  | 0x8000 | Unix extension flag          |

**Unix mode (bits 16-31, when bit 15 set):**

| Bits  | Mask         | Description             |
| ----- | ------------ | ----------------------- |
| 16-18 | 0x0007 << 16 | Other permissions (rwx) |
| 19-21 | 0x0038 << 16 | Group permissions       |
| 22-24 | 0x01C0 << 16 | Owner permissions       |
| 25    | 0x0200 << 16 | Sticky bit              |
| 26    | 0x0400 << 16 | Set GID                 |
| 27    | 0x0800 << 16 | Set UID                 |
| 28-31 | 0xF000 << 16 | File type               |

**Unix file types (bits 28-31):**

| Value | Type             |
| ----- | ---------------- |
| 0x1   | FIFO             |
| 0x2   | Character device |
| 0x4   | Directory        |
| 0x6   | Block device     |
| 0x8   | Regular file     |
| 0xA   | Symbolic link    |
| 0xC   | Socket           |

See `[16-TIMESTAMPS-ATTRIBUTES](/7z/16-timestamps-attributes)` for complete attribute details.

## Comment Property (0x16)

Archive comment (rarely used).

```
Comment ::= 0x16 Size External [DataIndex | CommentText]
CommentText ::= UTF16LE_Char* 0x0000
```

## Dummy Property (0x19)

Padding for alignment.

```
Dummy ::= 0x19 Size Zeros
Zeros ::= 0x00[Size]
```

Used to align subsequent data to word boundaries when needed.

## Complete Example

Archive with 3 entries:

- `readme.txt` (100 bytes)
- `src/` (directory)
- `src/main.rs` (500 bytes)

```
05                      # FilesInfo
03                      # NumFiles = 3

0E                      # EmptyStream
  01                    # Size = 1
  40                    # BitField: 01000000 (entry 1 is empty)

0F                      # EmptyFile
  01                    # Size = 1
  00                    # BitField: 00000000 (not empty file, so directory)

11                      # Name
  36                    # Size = 54 bytes
  00                    # External = inline
  # "readme.txt\0"
  72 00 65 00 61 00 64 00 6D 00 65 00 2E 00 74 00 78 00 74 00 00 00
  # "src/\0"
  73 00 72 00 63 00 2F 00 00 00
  # "src/main.rs\0"
  73 00 72 00 63 00 2F 00 6D 00 61 00 69 00 6E 00 2E 00 72 00 73 00 00 00

14                      # MTime
  03                    # Size
  01                    # AllAreDefined = true
  00                    # External = inline
  # 3 FILETIME values (8 bytes each)
  00 80 3E D5 DE B1 9D 01  # readme.txt mtime
  00 80 3E D5 DE B1 9D 01  # src/ mtime
  00 80 3E D5 DE B1 9D 01  # src/main.rs mtime

15                      # Attributes
  0E                    # Size
  01                    # AllAreDefined = true
  00                    # External = inline
  20 00 00 00           # readme.txt: ARCHIVE
  10 00 00 00           # src/: DIRECTORY
  20 00 00 00           # src/main.rs: ARCHIVE

00                      # End FilesInfo
```

## Entry Type Determination

To determine entry type:

```
function get_entry_type(file_index):
    is_empty_stream = empty_stream_bits[file_index]

    if not is_empty_stream:
        return FILE_WITH_DATA

    empty_stream_index = count_preceding_empty_streams(file_index)
    is_empty_file = empty_file_bits[empty_stream_index]
    is_anti = anti_bits[empty_stream_index]

    if is_anti:
        return ANTI_ITEM
    if is_empty_file:
        return EMPTY_FILE
    return DIRECTORY
```

## Symbolic Links

Symbolic links are stored as:

- EmptyStream = false (has data)
- Data = link target path in UTF-8
- Attributes = Unix symlink type (0xA000 in bits 28-31) OR Windows reparse point

See `[16-TIMESTAMPS-ATTRIBUTES](/7z/16-timestamps-attributes)` "Symbolic Links" section.

## Validation

Implementations MUST verify:

1. **Name uniqueness:** Implementations MUST reject archives containing byte-identical duplicate paths. Additionally, on case-insensitive filesystems, implementations MUST reject archives where multiple entries would resolve to the same path after case normalization. On case-sensitive filesystems, paths differing only in case are permitted.
2. **Path safety:** No `..` components or absolute paths
3. **BitField sizes:** Match expected number of entries
4. **External indices:** Reference valid streams
5. **UTF-16 validity:** Names are valid UTF-16

## See Also

- [Substreams Info](/7z/08-substreams-info) - File data within folders
- [Timestamps & Attributes](/7z/16-timestamps-attributes) - Time and attribute details
- [Security](/7z/17-security) - Path validation requirements
- [Property IDs](/7z/appendix/a-property-ids) - Property ID reference
