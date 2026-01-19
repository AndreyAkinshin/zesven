# Solid Archives

This document specifies solid archive behavior where multiple files are compressed together in a single stream.

## Overview

In a solid archive, multiple files are concatenated and compressed as a single unit. This exploits redundancy across files for better compression, but restricts random access.

## Concept

### Non-Solid Archive

Each file is in its own folder:

```
Folder 0: [File A] → [Compress] → Pack Stream 0
Folder 1: [File B] → [Compress] → Pack Stream 1
Folder 2: [File C] → [Compress] → Pack Stream 2
```

### Solid Archive

Multiple files share one folder:

```
Folder 0: [File A + File B + File C] → [Compress] → Pack Stream 0
```

## Format Representation

### Non-Solid

- SubStreamsInfo absent or `NumUnpackStream[folder] = 1` for all folders
- Each file has its own folder
- UnpackInfo defines one folder per file

### Solid

- SubStreamsInfo present with `NumUnpackStream[folder] > 1`
- SubStreamSizes define file boundaries
- Multiple files share folder's unpack stream

## Detection

An archive is solid if any folder contains multiple substreams:

```
function is_solid(archive):
    for folder in archive.folders:
        if substreams_count(folder) > 1:
            return true
    return false
```

## Solid Block Structure

Within a solid folder, file data is concatenated:

```
┌─────────────────────────────────────────────┐
│              Folder Unpack Stream            │
├──────────────┬──────────────┬───────────────┤
│   File A     │   File B     │    File C     │
│  (size 100)  │  (size 200)  │   (size 50)   │
└──────────────┴──────────────┴───────────────┘

SubStreamSizes = [100, 200]  # Last size implicit
```

## Extraction Behavior

### Sequential Extraction

Extracting all files:

1. Begin decompressing folder
2. Read `SubStreamSize[0]` bytes → File A
3. Read `SubStreamSize[1]` bytes → File B
4. Read remaining bytes → File C

### Selective Extraction

Extracting only File C (third file):

1. Begin decompressing folder
2. **Decompress and discard** 100 bytes (File A)
3. **Decompress and discard** 200 bytes (File B)
4. Read remaining 50 bytes → File C

**Key limitation:** Cannot skip directly to File C's data.

## Compression Benefits

Solid archives achieve better compression when files share content:

| Scenario                   | Solid Benefit |
| -------------------------- | ------------- |
| Similar text files         | Excellent     |
| Source code (same project) | Very good     |
| Library versions           | Very good     |
| Mixed file types           | Moderate      |
| Already compressed         | Minimal       |

**Typical improvement:** 10-40% smaller than non-solid for similar files.

## Performance Trade-offs

### Advantages

1. Better compression ratio
2. Smaller archive size
3. Efficient for backup/archiving
4. Dictionary reuse across files

### Disadvantages

1. Slow random access
2. Cannot extract single file efficiently
3. More memory during extraction
4. Cannot update single file without recompression

## Solid Block Size

7-Zip allows configuring solid block size:

| Setting | Behavior                      |
| ------- | ----------------------------- |
| Off     | Non-solid (1 file per folder) |
| 1 MB    | ~1 MB per solid block         |
| 10 MB   | ~10 MB per solid block        |
| Solid   | All files in one block        |

**File grouping:** Files are typically grouped by extension within solid blocks to maximize similarity.

## Implementation Guidelines

### Reading Solid Archives

```
function extract_file(archive, file_index):
    folder_index, stream_index = file_to_folder_mapping(file_index)

    # Position within solid block
    skip_size = sum(substream_sizes[0:stream_index])

    # Start folder decompression
    decoder = create_decoder(folder_index)

    # Discard preceding data
    discard(decoder, skip_size)

    # Read target file
    file_size = get_file_size(file_index)
    return read(decoder, file_size)
```

### Efficient Batch Extraction

When extracting multiple files from same solid block:

1. Sort files by position within block
2. Extract in order (no backtracking)
3. Decompress once, splitting to multiple outputs

### Writing Solid Archives

```
function create_solid_archive(files, options):
    if options.solid:
        # Sort files by type/extension for better compression
        files = sort_by_extension(files)

        # Group into solid blocks
        blocks = group_files(files, options.solid_block_size)

        for block in blocks:
            # Concatenate all files in block
            combined = concatenate(block)

            # Compress as single unit
            compressed = compress(combined, options.method)

            # Record substream sizes
            sizes = [file.size for file in block[:-1]]
```

## CRC Handling

### Folder CRC

Single CRC for entire decompressed folder output.

### SubStream CRCs

Individual CRC per file within solid block. Stored in SubStreamsInfo digests.

### Verification

1. Decompress entire folder
2. Verify folder CRC (if present)
3. Split into substreams
4. Verify each substream CRC (if present)

## Memory Considerations

Solid archives may require significant memory:

| Phase         | Memory Use                           |
| ------------- | ------------------------------------ |
| Decompression | Compressor dictionary (up to 1.5 GB) |
| Buffering     | Depends on access pattern            |
| Random access | May need to buffer preceding data    |

### Streaming Extraction

For memory efficiency:

1. Extract files in order
2. Don't seek backward
3. Pipeline decompressed data directly to output

## Error Recovery

In solid archives, corruption affects downstream files:

| Corruption Location | Impact                      |
| ------------------- | --------------------------- |
| Start of block      | All files corrupted         |
| Middle              | Subsequent files corrupted  |
| End                 | Only last file(s) corrupted |

**Recovery:** Smaller solid blocks limit corruption scope.

## Compatibility Notes

- All 7-Zip versions support solid archives
- Most third-party tools support solid archives
- Some tools may not support efficient partial extraction

## Example Archive

Archive with 3 files in one solid block:

```
PackInfo:
  PackPos = 0
  NumPackStreams = 1
  PackSize[0] = 500

UnpackInfo:
  NumFolders = 1
  Folder[0]:
    Coders: [LZMA2]
    UnpackSize = 1000

SubStreamsInfo:
  NumUnpackStream[0] = 3
  SubStreamSize = [300, 400]  # Third = 1000 - 300 - 400 = 300
  Digests = [CRC_A, CRC_B, CRC_C]

FilesInfo:
  NumFiles = 3
  Names = ["a.txt", "b.txt", "c.txt"]
```

## See Also

- [Unpack Info](/7z/07-unpack-info) - Folder definitions
- [Substreams Info](/7z/08-substreams-info) - Substream specifications
- [Philosophy](/7z/00-philosophy) - Design trade-offs
