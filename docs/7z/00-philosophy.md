# Design Philosophy

This document defines the design principles, goals, and invariants that govern the 7z format specification and zesven implementation.

## Goals

### 1. Safety First

Security and safety take precedence over performance and features:

- **Path traversal protection**: Archive entries MUST NOT escape the extraction directory
- **Resource limits**: Implementations MUST enforce limits on memory, entries, and sizes
- **Input validation**: All header data MUST be validated before use
- **Compression bomb detection**: Implementations SHOULD detect and reject archives with extreme compression ratios

### 2. Correctness Over Speed

Correct behavior is more important than performance:

- **Data integrity**: All CRC checks MUST be performed and verified
- **Format conformance**: Strict adherence to format rules
- **Predictable behavior**: Same input produces same output (deterministic)
- **Fail-safe defaults**: When in doubt, reject rather than guess

### 3. Streaming Support

The format supports memory-efficient streaming operations:

- **Bounded memory**: Decompression can operate with fixed memory buffers
- **Sequential access**: Archives can be read without seeking (with limitations)
- **Progress reporting**: Operations can report progress during execution

### 4. Interoperability

Maximize compatibility with existing tools:

- **7-Zip compatibility**: Archives created by zesven SHOULD be readable by 7-Zip
- **Cross-platform**: Archives work identically across operating systems
- **Version tolerance**: Accept archives from older and newer format versions within reason

## Normative vs Informative Text

This specification uses RFC 2119 keywords to indicate requirement levels.

### Normative Sections

Text containing **MUST**, **MUST NOT**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **MAY**, **REQUIRED**, or **OPTIONAL** (capitalized) is normative and defines requirements that implementations must follow.

### Informative Sections

The following are informative (non-binding):

- **Examples**: Code blocks and hex dumps illustrating concepts
- **Rationale**: Explanations of why rules exist (marked with "Rationale:")
- **Implementation Notes**: Suggestions for implementers (marked with "Implementation Notes" or similar)
- **See Also**: Cross-references to related documentation

Informative text helps understanding but does not define requirements. When informative text conflicts with normative text, the normative text takes precedence.

## Non-Goals

### Not Supported

The following are explicitly outside the scope of this specification:

1. **Other archive formats**: Only 7z format is supported (not ZIP, RAR, TAR, etc.)
2. **In-place modification**: Archives cannot be modified without rewriting
3. **Streaming compression**: Writing archives requires knowing all content upfront
4. **Partial extraction without seeking**: Extracting arbitrary files from solid archives requires decompressing preceding data
5. **Infinite archives**: Maximum archive size is limited by 64-bit offsets

### Intentionally Omitted Features

Some 7-Zip features are intentionally not specified:

1. **Legacy compression methods**: Methods deprecated by 7-Zip
2. **Undocumented properties**: Internal 7-Zip properties not meant for interchange
3. **Platform-specific metadata**: Windows-specific features beyond basic attributes

## Core Invariants

These invariants MUST hold for all valid 7z archives:

### Structural Invariants

1. **Signature present**: Every archive starts with the 6-byte signature `37 7A BC AF 27 1C`
2. **Header integrity**: Start header CRC covers bytes 12-31; next header CRC covers header data
3. **Offset validity**: `NextHeaderOffset + NextHeaderSize` MUST NOT exceed file size minus 32
4. **No overlapping regions**: Pack data, encoded header data, and main header occupy disjoint regions

### Data Flow Invariants

1. **Coder chain validity**: Every coder output (except final) binds to exactly one coder input
2. **Stream accounting**: Sum of pack sizes equals total compressed data size
3. **File accounting**: Sum of substream sizes equals folder unpack size
4. **Entry ordering**: File metadata order matches substream data order

### Security Invariants

1. **No path escape**: Normalized paths contain no `..` components and are not absolute
2. **Bounded recursion**: Encoded header nesting depth is limited (typically 4)
3. **Bounded iteration**: Key derivation iterations have a maximum (2^30)

### Invariant Violation Consequences

When an invariant is violated, implementations MUST:

1. **Reject the archive**: Return an error indicating the specific violation
2. **Not produce partial output**: Do not extract some files while rejecting others (unless the user explicitly requests best-effort extraction)
3. **Log the violation**: Provide sufficient detail for debugging (violation type, location in archive)

Implementations MUST NOT attempt to "repair" archives that violate invariants, as this may mask malicious content or produce incorrect results.

## Compatibility Stance

### Reading (Decoding)

**Accept liberal, validate strict:**

- Accept archives with minor deviations from specification
- Validate all CRCs and structural integrity
- Reject archives that would compromise security
- Warn on deprecated or unusual features

### Writing (Encoding)

**Emit conservative, maximize compatibility:**

- Produce archives readable by 7-Zip 9.20 and later
- Use only well-supported compression methods by default
- Avoid experimental or extended features unless explicitly requested
- Prefer smaller, simpler structures when equivalent

## Evolution Principles

### Versioning

The format uses a two-byte version (major.minor):

- **Major version 0**: Current and expected to remain stable
- **Minor version 4**: Current version with full feature set

Version handling rules:

- **Major > 0**: MUST reject (incompatible future format)
- **Minor > 4**: SHOULD accept with warning (forward compatible)
- **Minor < 4**: MUST accept (backward compatible)

### Extension Mechanism

New features are added through:

1. **New property IDs**: Unknown property IDs MAY be skipped (size-prefixed)
2. **New method IDs**: Unknown methods result in UnsupportedMethod error
3. **Reserved bits**: Reserved fields MUST be zero when writing. When reading, non-zero reserved bits indicate either a non-conforming writer or a future format extension. Implementations SHOULD emit a warning and MAY reject the archive depending on context and strictness level.

### Extensibility Rules

**Property ID allocation:**

- IDs 0x00-0x19 are defined by this specification
- IDs 0x1A-0x7F are reserved for future standard extensions
- IDs 0x80-0xFF are reserved for vendor-specific or experimental use; these MUST NOT appear in archives intended for interchange
- Implementations MUST include a size field after any new property ID to enable forward-compatible skipping

**Method ID allocation:**

- Method IDs in the 0x04F7XXXX range are used by the 7-Zip-zstd fork for extended codecs
- Other ranges follow 7-Zip conventions; new methods require coordination with the 7-Zip project
- Vendor-specific methods SHOULD use IDs unlikely to conflict with future standard methods

**Backward compatibility commitment:**

- Archives created with documented features will remain readable by future specification versions
- Deprecated features will be documented for at least two years before removal from the specification
- Removal from specification does not require removal from implementations

### Deprecation Policy

Features are deprecated by:

1. Documentation marking them as deprecated
2. Implementation warnings when encountered
3. Eventual removal from specification (with multi-year notice)

## Implementation Guidance

### Recommended Defaults

| Setting            | Default Value | Rationale                       |
| ------------------ | ------------- | ------------------------------- |
| Compression        | LZMA2         | Best balance of ratio and speed |
| Dictionary         | 16 MiB        | Good for most file sizes        |
| Solid              | Disabled      | Enables random access           |
| Threads            | CPU count     | Maximum parallelism             |
| Header compression | LZMA2         | Standard practice               |
| Header encryption  | Disabled      | Allows listing without password |

### Performance vs Safety Trade-offs

When performance and safety conflict:

1. **Always perform CRC checks** - Data integrity is non-negotiable
2. **Enforce resource limits** - Memory exhaustion affects system stability
3. **Validate paths** - Path traversal is a critical vulnerability
4. **Limit iterations** - DoS via expensive operations is a real threat

### Error Handling Philosophy

1. **Fail early**: Detect errors as soon as possible
2. **Fail informatively**: Provide context (offset, field name, expected vs actual)
3. **Fail safely**: Clean up resources, don't leave partial state
4. **Recoverable when possible**: Some errors (wrong password) allow retry

## See Also

- [Security](/7z/17-security) - Detailed security requirements
- [Error Conditions](/7z/18-error-conditions) - Error handling specification
- [Compatibility](/7z/appendix/d-compatibility) - Compatibility notes with 7-Zip
