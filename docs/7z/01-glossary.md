# Glossary

This document provides canonical definitions for all terms used throughout the 7z format specification. Terms are defined exactly once here and referenced elsewhere.

## Archive Structure Terms

### Archive

A complete 7z file containing a signature header, optional compressed data, and a header database. An archive is the top-level container for all stored content.

### Entry

A single item stored in an archive. An entry may be:

- A regular file (with data)
- An empty file (zero bytes)
- A directory
- A symbolic link
- An anti-item (deletion marker)

### Folder

A compression processing unit within an archive. A folder:

- Contains one or more coders arranged in a chain
- Produces one logical output stream (unpack stream)
- May contain data for one or more files (solid mode)

Folders are the fundamental unit of compression—data within a folder is compressed together.

### Pack Stream

A contiguous block of compressed data in the archive. Pack streams are the raw compressed bytes that result from encoding. Multiple pack streams may exist if:

- Multiple folders exist
- A complex coder (like BCJ2) produces multiple outputs

### Pack Data

The region of the archive file containing all pack streams concatenated contiguously. "Pack data" refers to the physical bytes in the archive file; "pack streams" refers to the logical division of that data as defined by PackInfo.

### Unpack Stream

The decompressed output of a folder. An unpack stream is the logical result of applying all coders in a folder's chain to the pack stream(s).

### Substream

An individual file's data within a folder's unpack stream. In solid archives, a folder's unpack stream is divided into substreams, one per file. Substream boundaries are defined by [08-SUBSTREAMS-INFO](/7z/08-substreams-info).

## Compression Terms

### Coder

A single transformation step in a compression pipeline. A coder may be:

- A compressor (LZMA, LZMA2, Deflate, etc.)
- A filter (BCJ, Delta)
- An encryptor (AES-256)

Each coder has:

- A method ID identifying the algorithm
- Optional properties configuring the algorithm
- Input stream count (typically 1)
- Output stream count (typically 1)

### Simple Coder

A coder with exactly one input stream and one output stream. The stream counts are implicit (not stored in the archive). Most coders are simple coders, including LZMA, LZMA2, Deflate, BZip2, PPMd, BCJ (x86), Delta, and AES.

### Complex Coder

A coder with multiple input streams and/or multiple output streams. The stream counts are explicitly stored in the archive. The primary example is BCJ2, which has 4 input streams and 1 output stream.

### Coder Chain

An ordered sequence of coders applied to data. Data flows through the chain:

```
Input → [Coder 1] → [Coder 2] → ... → [Coder N] → Output
```

A typical chain might be: `BCJ → LZMA2` (filter then compress).

### Bind Pair

A connection between a coder's output and another coder's input within a folder. Bind pairs define the data flow topology when multiple coders exist.

### Method ID

A unique identifier for a compression, filter, or encryption algorithm. Method IDs are variable-length (1-15 bytes). See [B-METHOD-IDS](/7z/appendix/b-method-ids) for the complete list.

### Properties

Algorithm-specific configuration data attached to a coder. Properties are a byte array whose interpretation depends on the method. For example:

- LZMA: 5 bytes (lc/lp/pb + dictionary size)
- LZMA2: 1 byte (dictionary size encoding)
- AES: Variable (salt, IV, iteration count)

## Header Terms

### Signature Header

The fixed 32-byte header at the start of every 7z archive. Also called the "start header." Contains:

- 6-byte signature
- 2-byte version
- CRC and pointer to the next header

See [03-SIGNATURE-HEADER](/7z/03-signature-header).

### Main Header

The uncompressed header database containing all archive metadata:

- Pack information (compressed stream sizes)
- Unpack information (folder and coder definitions)
- Substreams information (per-file sizes within folders)
- Files information (names, times, attributes)

### Encoded Header

A compressed and/or encrypted main header. When the header is encoded:

1. The main header is compressed (typically with LZMA2)
2. A small "header info" structure points to the compressed data
3. Reading requires first decompressing the header

### Next Header

Generic term for the header data pointed to by the signature header. This is either:

- A main header (unencoded)
- An encoded header wrapper

### Property ID

A single-byte identifier for a data section within the header. Property IDs define the structure of header data. See [A-PROPERTY-IDS](/7z/appendix/a-property-ids).

### Property (Header)

A tagged data section within the archive header, identified by a single-byte Property ID. Each property contains specific archive metadata such as file names (0x11), timestamps (0x12-0x14), or compression information (0x06-0x08). Properties are the building blocks of the header structure.

### Properties (Coder)

Algorithm-specific configuration data attached to a coder. Not to be confused with header properties. See "Properties" under Compression Terms for details.

## Data Encoding Terms

### NUMBER

A variable-length encoded unsigned 64-bit integer. The encoding uses 1-9 bytes, with the first byte indicating the length. See [04-DATA-ENCODING](/7z/04-data-encoding#number-encoding).

### BitField

A packed array of boolean values, 8 per byte. Bit 7 (MSB) of the first byte is the first value. See [04-DATA-ENCODING](/7z/04-data-encoding#bitfield).

### BooleanList

A space-optimized boolean array with an "all defined" shortcut. If all values are true, only a single `0x01` byte is stored instead of the full bit array.

### Defined (CRC/Digest Context)

A CRC or digest value is _defined_ for a particular item when:

- `AllAreDefined` is non-zero (all items have CRCs), OR
- `AllAreDefined` is `0x00` AND the corresponding bit in the BitField is set (value 1)

When a CRC is _undefined_, no value is stored for that item. The count of defined CRCs determines how many CRC values follow the BooleanList. Implementations cannot verify integrity for items with undefined CRCs.

### UINT32

A 32-bit unsigned integer in little-endian byte order.

### UINT64

A 64-bit unsigned integer in little-endian byte order.

### FILETIME

A 64-bit timestamp representing 100-nanosecond intervals since January 1, 1601 (UTC). This is the Windows FILETIME format. See [16-TIMESTAMPS-ATTRIBUTES](/7z/16-timestamps-attributes).

## Archive Mode Terms

### Solid Archive

An archive where multiple files are compressed together in a single folder. Benefits:

- Better compression ratio (inter-file redundancy exploited)
- Smaller archive size

Drawbacks:

- Extracting one file requires decompressing all preceding files
- No random access within solid blocks

### Non-Solid Archive

An archive where each file is in its own folder. Benefits:

- Random access to any file
- Independent extraction

Drawbacks:

- Larger archive size (no inter-file compression)

### Multi-Volume Archive

An archive split across multiple files (volumes). Used for:

- Distribution on size-limited media
- Splitting large archives

Volumes use the naming convention: `archive.7z.001`, `archive.7z.002`, etc.

### Self-Extracting Archive (SFX)

An archive with an executable stub prepended. The stub:

- Contains code to extract the archive
- May include a configuration block
- Makes the archive directly executable

## File Terms

### Empty Stream

A file entry that has no associated data stream. This includes:

- Empty files (0 bytes)
- Directories

The `EmptyStream` property marks which entries have no data.

### Empty File

A subset of empty streams that are files (not directories). Distinguished from directories by the `EmptyFile` property.

### Anti-Item

A deletion marker in incremental backups. Anti-items indicate that a file should be removed when applying the archive. Marked by the `Anti` property.

### Symbolic Link

A file that references another path. In 7z:

- Stored as a file with link target as content
- Marked with Unix symlink attributes or Windows reparse point attribute
- Target path is UTF-8 encoded

## Security Terms

### Path Traversal

An attack where archive paths escape the extraction directory using `..` components or absolute paths. Implementations MUST prevent path traversal.

### Compression Bomb

An archive designed to expand to an extremely large size, exhausting memory or disk space. Detected by monitoring compression ratio.

### Resource Limit

A bound on resource consumption (memory, entries, sizes) to prevent denial-of-service attacks.

## See Also

- [Property IDs](/7z/appendix/a-property-ids) - Property ID definitions
- [Method IDs](/7z/appendix/b-method-ids) - Method ID definitions
- [Data Encoding](/7z/04-data-encoding) - Data type encodings
