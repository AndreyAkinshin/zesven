# Unpack Info

This document specifies the UnpackInfo structure that defines folders and their coder chains for decompression.

## Overview

UnpackInfo describes:

- Folders (compression units)
- Coders (compression/filter algorithms)
- How coders are connected (bind pairs)
- Which pack streams feed into which coders
- Uncompressed sizes of coder outputs
- Optional CRCs for decompressed data

## Structure

```
UnpackInfo ::= 0x07 FoldersSection [CodersUnpackSize] [UnpackDigests] 0x00

FoldersSection ::= 0x0B NumFolders External Folders

NumFolders ::= NUMBER
External ::= BYTE  # 0x00 = inline, 0x01 = external stream

Folders ::= Folder[NumFolders]  # If External == 0x00
         |  DataIndex            # If External == 0x01

CodersUnpackSize ::= 0x0C UnpackSize*
UnpackDigests ::= 0x0A DigestsData
```

## Folder Structure

Each folder defines a coder chain:

```
Folder ::= NumCoders Coder[NumCoders] BindPair* PackStreamIndex* UnpackSize*
```

### NumCoders

**Type:** NUMBER
**Description:** Number of coders in this folder's chain.
**Constraints:** MUST be >= 1. A folder with zero coders MUST be rejected with ERROR_ARCHIVE.
**Typical values:** 1-3

### Coder Structure

```
Coder ::= Flags MethodID [NumInStreams NumOutStreams] [Properties]

Flags ::= BYTE
MethodID ::= BYTE[IDSize]
NumInStreams ::= NUMBER  # Present if IsComplex
NumOutStreams ::= NUMBER  # Present if IsComplex
Properties ::= PropertiesSize PropertyData
PropertiesSize ::= NUMBER
PropertyData ::= BYTE[PropertiesSize]
```

### Coder Flags Byte

| Bit | Name          | Description                       |
| --- | ------------- | --------------------------------- |
| 0-3 | IDSize        | Length of MethodID (1-15 bytes)   |
| 4   | IsComplex     | Has multiple input/output streams |
| 5   | HasProperties | Properties data follows           |
| 6-7 | Reserved      | MUST be 0                         |

**IDSize constraint:** IDSize MUST be >= 1. A value of 0 is invalid and MUST cause ERROR_ARCHIVE. Method IDs cannot be zero-length.

**Extracting fields:**

```
id_size = flags & 0x0F
is_complex = (flags & 0x10) != 0
has_properties = (flags & 0x20) != 0
```

### Simple Coder

When `IsComplex == false`:

- NumInStreams = 1 (implicit)
- NumOutStreams = 1 (implicit)

Most coders are simple:

- LZMA, LZMA2, Deflate, BZip2, PPMd
- BCJ (x86), Delta
- AES encryption

### Complex Coder

When `IsComplex == true`:

- NumInStreams and NumOutStreams are explicit
- Used for BCJ2 (4 inputs, 1 output)

### BindPair

Connects a coder's output stream to another coder's input stream:

```
BindPair ::= InIndex OutIndex
InIndex ::= NUMBER   # Input stream index being bound
OutIndex ::= NUMBER  # Output stream index providing data
```

**Number of bind pairs:** `TotalOutStreams - 1`

Each output stream (except the final one) binds to exactly one input stream.

### Stream Index Domain

Input and output stream indices are **global** across all coders in the folder, numbered sequentially:

**Input stream indices:**

```
For coder[0]: inputs 0 to (num_in_streams[0] - 1)
For coder[1]: inputs num_in_streams[0] to (num_in_streams[0] + num_in_streams[1] - 1)
...
```

**Output stream indices:**

```
For coder[0]: outputs 0 to (num_out_streams[0] - 1)
For coder[1]: outputs num_out_streams[0] to (num_out_streams[0] + num_out_streams[1] - 1)
...
```

**Example:** Folder with 2 simple coders (each having 1 input, 1 output):

- Coder 0: input index 0, output index 0
- Coder 1: input index 1, output index 1
- BindPair `(InIndex=1, OutIndex=0)` connects coder 1's input to coder 0's output

### PackStreamIndex

Identifies which pack streams feed the folder's unbound input streams:

```
PackStreamIndex ::= NUMBER  # Index into PackInfo's stream list
```

**Number of pack stream indices:**

- If `NumPackedStreams == 1`: implicit, no data stored
- If `NumPackedStreams > 1`: explicit indices stored

Where: `NumPackedStreams = TotalInStreams - NumBindPairs`

### UnpackSize

Size of each coder's output stream:

```
UnpackSize ::= NUMBER[TotalOutStreams per folder]
```

Stored as: for each folder, for each coder output stream, one NUMBER.

## CodersUnpackSize Section

**Property ID:** `0x0C`

Contains unpack sizes for all folders' coder outputs:

```
CodersUnpackSize ::= 0x0C UnpackSize*
```

The total count of UnpackSize values = sum of all folders' total output streams.

## UnpackDigests Section

**Property ID:** `0x0A`

CRC-32 of each folder's final uncompressed output:

```
UnpackDigests ::= 0x0A AllAreDefined [BitField] CRC[NumDefined]
```

One CRC per folder (not per coder output).

## Data Flow Example

### Simple Folder (LZMA2)

```
Pack Stream 0 ──▶ [LZMA2] ──▶ Uncompressed Output
```

```
Folder:
  NumCoders = 1
  Coder[0]:
    Flags = 0x21 (IDSize=1, HasProperties)
    MethodID = 21 (LZMA2)
    Properties = [dictionary_byte]
  BindPairs: (none, single output)
  PackStreamIndices: 0 (implicit)
  UnpackSize[0] = output_size
```

### Filtered Folder (BCJ + LZMA2)

```
Pack Stream 0 ──▶ [LZMA2] ──▶ [BCJ] ──▶ Uncompressed Output
                    ↑          ↑
                  Coder 0    Coder 1
                  Output 0   Output 1 (final)
```

Data flow (decompression order):

1. Pack stream → LZMA2 → intermediate data
2. Intermediate data → BCJ → final output

```
Folder:
  NumCoders = 2
  Coder[0]: LZMA2 (simple, 1 in, 1 out)
  Coder[1]: BCJ (simple, 1 in, 1 out)
  BindPairs:
    InIndex=1, OutIndex=0  # Coder 1's input binds to Coder 0's output
  PackStreamIndices: 0 (implicit)
  UnpackSize[0] = intermediate_size
  UnpackSize[1] = final_size
```

### Complex Folder (BCJ2 + LZMA)

BCJ2 is a complex coder with 4 inputs and 1 output:

```
Pack Stream 0 ──▶ [LZMA] ──┐
Pack Stream 1 ──▶ [LZMA] ──┤
Pack Stream 2 ──▶ [LZMA] ──┼──▶ [BCJ2] ──▶ Output
Pack Stream 3 ──▶ [LZMA] ──┘
```

## Resolving Data Flow

Algorithm to determine decompression order:

```
function resolve_folder(folder) -> DecompressionPlan:
    # Find unbound output (the final output)
    bound_outputs = set()
    for bind_pair in folder.bind_pairs:
        bound_outputs.add(bind_pair.out_index)

    final_output = None
    for i, coder in enumerate(folder.coders):
        for j in range(coder.num_out_streams):
            out_idx = output_index(folder, i, j)
            if out_idx not in bound_outputs:
                final_output = out_idx
                break

    # Find unbound inputs (fed by pack streams)
    bound_inputs = set()
    for bind_pair in folder.bind_pairs:
        bound_inputs.add(bind_pair.in_index)

    pack_stream_inputs = []
    for i, coder in enumerate(folder.coders):
        for j in range(coder.num_in_streams):
            in_idx = input_index(folder, i, j)
            if in_idx not in bound_inputs:
                pack_stream_inputs.append(in_idx)

    # Build execution order (reverse topology)
    return topological_sort(folder, final_output, pack_stream_inputs)
```

## Complete Example

Archive with one file compressed with BCJ + LZMA2:

```
07                      # UnpackInfo

0B                      # Folders section
  01                    # NumFolders = 1
  00                    # External = inline

  # Folder 0:
  02                    # NumCoders = 2

  # Coder 0: LZMA2
  21                    # Flags: IDSize=1, HasProperties
  21                    # MethodID: LZMA2
  15                    # PropertiesSize = 1
  18                    # Dictionary property (16 MiB)

  # Coder 1: BCJ (x86)
  04                    # Flags: IDSize=4, no props
  03 03 01 03           # MethodID: BCJ

  # BindPairs: (TotalOutStreams - 1 = 1)
  01                    # InIndex = 1 (Coder 1's input)
  00                    # OutIndex = 0 (Coder 0's output)

  # PackStreamIndices: (implicit, only 1)

0C                      # CodersUnpackSize
  80 00 80              # UnpackSize[0] = 32768 (LZMA2 output)
  80 00 80              # UnpackSize[1] = 32768 (BCJ output, same)

0A                      # UnpackDigests
  01                    # AllAreDefined
  12 34 56 78           # CRC of folder 0

00                      # End UnpackInfo
```

## Validation

Implementations MUST verify:

1. **Coder count:** NumCoders MUST be >= 1. Archives with NumCoders == 0 MUST be rejected as invalid.
2. **Flags reserved bits:** Bits 6-7 are 0
3. **Bind pair count:** Exactly `TotalOutStreams - 1` bind pairs
4. **All outputs bound:** Except one (the final output)
5. **Pack stream coverage:** All unbound inputs have pack stream indices
6. **Index validity:** All indices reference valid streams
7. **No cycles:** Bind pairs form a DAG (directed acyclic graph)

### Cycle Detection Algorithm

To detect cycles in bind pairs:

```
function validate_no_cycles(coders, bind_pairs):
    # Build adjacency: output_index -> input_index (coder that consumes it)
    consumers = {}
    for (in_idx, out_idx) in bind_pairs:
        consumers[out_idx] = in_idx

    # Track visited outputs to detect cycles
    visited = set()

    function trace_output(out_idx):
        if out_idx in visited:
            reject("Cycle detected in coder chain")
        visited.add(out_idx)

        if out_idx in consumers:
            # This output feeds another coder; trace forward
            consuming_coder = find_coder_for_input(consumers[out_idx])
            for coder_out in outputs_of(consuming_coder):
                trace_output(coder_out)

    # Start from all unbound outputs (entry points)
    for coder in coders:
        for out_idx in outputs_of(coder):
            if out_idx not in consumers:
                trace_output(out_idx)
```

Archives with cyclic bind pairs MUST be rejected with ERROR_ARCHIVE.

## See Also

- [Pack Info](/7z/06-pack-info) - Pack stream definitions
- [Substreams Info](/7z/08-substreams-info) - How folder outputs map to files
- [Compression Methods](/7z/10-compression-methods) - Coder method specifications
- [Filters](/7z/11-filters) - Filter coder specifications
- [Method IDs](/7z/appendix/b-method-ids) - Method ID reference
