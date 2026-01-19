# SubStreams Info

This document specifies the SubStreamsInfo structure that defines how folder outputs are divided into individual file data.

## Overview

SubStreamsInfo describes:

- How many files are stored in each folder
- The size of each file's data within the folder
- CRC checksums for individual files

This information is essential for solid archives where multiple files share a single compression stream.

## Structure

```
SubStreamsInfo ::= 0x08 [NumUnpackStream] [Sizes] [Digests] 0x00

NumUnpackStream ::= 0x0D NumStreamsPerFolder[NumFolders]
Sizes ::= 0x09 SubStreamSize*
Digests ::= 0x0A DigestsData
```

## When SubStreamsInfo is Present

SubStreamsInfo is:

- **Required** when any folder contains more than one file (solid archive)
- **Optional** when each folder contains exactly one file
- **Absent** when each folder maps to exactly one file with known size

## Default Behavior (No SubStreamsInfo)

When SubStreamsInfo is absent:

- Each folder contains exactly 1 file
- File size = folder's final unpack size
- File CRC = folder's unpack digest (if present)

## NumUnpackStream Section

**Property ID:** `0x0D`

Specifies the number of files in each folder:

```
NumUnpackStream ::= 0x0D NumStreams[NumFolders]
NumStreams ::= NUMBER
```

**Note:** NumUnpackStream has no End marker. The count of NumStreams values is determined by NumFolders from UnpackInfo. The `0x00` End marker appears at the end of SubStreamsInfo, not after NumUnpackStream.

**Default value:** 1 for each folder (if section absent)

**Example:** 3 folders with 1, 4, and 2 files respectively:

```
0D                  # NumUnpackStream property ID
  01                # Folder 0: 1 file
  04                # Folder 1: 4 files (solid block)
  02                # Folder 2: 2 files (solid block)
                    # (no End marker here; continues with Sizes or End of SubStreamsInfo)
```

## Sizes Section

**Property ID:** `0x09`

Contains the size of each file within its folder:

```
Sizes ::= 0x09 SubStreamSize* 0x00
```

**Important:** The last file's size in each folder is NOT stored. It is calculated as:

```
last_size = folder_unpack_size - sum(preceding_sizes)
```

**Total sizes stored:** `sum(NumStreams[i] - 1)` for all folders

### Size Calculation Example

Folder with 4 files (sizes: 100, 200, 150, 50), folder unpack size = 500:

```
Stored sizes: 100, 200, 150 (3 values)
Last size calculated: 500 - 100 - 200 - 150 = 50
```

Encoding:

```
09                  # Sizes property ID
  64                # 100
  80 48             # 200 (NUMBER encoding)
  80 16             # 150
                    # (no value for last file)
00                  # End
```

## Digests Section

**Property ID:** `0x0A`

CRC-32 checksums for substreams. This follows the BooleanList pattern defined in `[04-DATA-ENCODING](/7z/04-data-encoding#booleanlist)`.

```
Digests ::= 0x0A AllAreDefined [BitField] CRC*
```

**Coverage:** CRCs are for files where:

1. The file's folder already has a folder-level CRC in UnpackDigests, OR
2. The file is explicitly listed here

**Typical pattern:**

- If folder has 1 file with folder CRC: no entry needed in SubStreams Digests
- If folder has multiple files: each file needs its own CRC

### Digest Counting

The number of entries in the digests section depends on which files need CRCs:

```
count = 0
For each folder (index f):
    if UnpackInfo.UnpackDigests has a defined CRC for folder f
       AND NumUnpackStream[f] == 1:
        # Single file inherits folder CRC, no entry needed
        continue
    for each file in folder f:
        # Entry needed for this file
        count += 1
```

**Explanation:**

- `UnpackInfo.UnpackDigests` refers to the Digests section (property ID 0x0A) within UnpackInfo (see `[07-UNPACK-INFO](/7z/07-unpack-info)`)
- A CRC is "defined" for a folder when the corresponding bit is set in the AllAreDefined bitmap (or AllAreDefined == 0x01)
- When a folder has exactly one file AND a folder-level CRC, that CRC applies to the single file

## Complete Example

Archive with 2 folders:

- Folder 0: 1 file (non-solid)
- Folder 1: 3 files (solid)

```
08                      # SubStreamsInfo

0D                      # NumUnpackStream
  01                    # Folder 0: 1 file
  03                    # Folder 1: 3 files
                        # (no End marker for NumUnpackStream)

09                      # Sizes (for folder 1 only, folder 0 is implicit)
  80 00 01              # File 0 size: 256
  80 00 02              # File 1 size: 512
                        # File 2 size: calculated from folder size
00                      # End Sizes

0A                      # Digests
  01                    # AllAreDefined = true
                        # (folder 0's file uses folder CRC)
  11 22 33 44           # Folder 1, File 0 CRC
  55 66 77 88           # Folder 1, File 1 CRC
  99 AA BB CC           # Folder 1, File 2 CRC

00                      # End SubStreamsInfo
```

## File-to-Folder Mapping

Files are assigned to folders in order:

```
File Index | Folder Index | Stream Index Within Folder
-----------|--------------|---------------------------
    0      |      0       |            0
    1      |      1       |            0
    2      |      1       |            1
    3      |      1       |            2
    4      |      2       |            0
   ...     |     ...      |           ...
```

To find which folder contains file `i`:

```
function file_to_folder(file_index, num_streams_per_folder):
    folder = 0
    count = 0
    for ns in num_streams_per_folder:
        if count + ns > file_index:
            stream_in_folder = file_index - count
            return (folder, stream_in_folder)
        count += ns
        folder += 1
    error("File index out of range")
```

## Extraction Order

For solid folders, files MUST be extracted in order:

1. Start decompressing folder
2. Read first `SubStreamSize[0]` bytes → File 0
3. Read next `SubStreamSize[1]` bytes → File 1
4. Continue until folder exhausted

Random access within a solid folder requires decompressing and discarding all preceding data.

## Empty Files in Solid Folders

Empty files (size 0) CAN appear in solid folders:

- Stored with `SubStreamSize = 0`
- No data is consumed from the decompression stream
- CRC is typically undefined or 0x00000000

## Validation

Implementations MUST verify:

1. **NumStreams positive:** Each folder has at least 1 stream
2. **Sizes sum correctly:** Sum of substream sizes equals folder unpack size
3. **File count matches:** Total substreams equals number of non-empty-stream files
4. **Digest count matches:** Number of CRCs equals expected count
5. **No negative sizes:** Calculated last size is non-negative

## Relationship to FilesInfo

SubStreamsInfo indexes files with data. The mapping to FilesInfo:

- FilesInfo contains ALL files (including empty streams/directories)
- SubStreamsInfo only covers files with actual data
- EmptyStream property in FilesInfo marks which files have no data

File ordering:

```
FilesInfo[0]  ──▶  (EmptyStream? skip : SubStream[0])
FilesInfo[1]  ──▶  (EmptyStream? skip : SubStream[1])
...
```

## See Also

- [Unpack Info](/7z/07-unpack-info) - Folder definitions
- [Files Info](/7z/09-files-info) - File metadata and EmptyStream
- [Solid Archives](/7z/13-solid-archives) - Solid archive concepts
- [CRC Algorithm](/7z/appendix/c-crc-algorithm) - CRC-32 calculation
