# Appendix D: Compatibility

This appendix documents compatibility considerations with 7-Zip and other implementations.

## 7-Zip Versions

### Version History

| Version | Release | Notable Changes          |
| ------- | ------- | ------------------------ |
| 9.20    | 2010    | Stable baseline          |
| 15.06   | 2015    | ARM64 filter             |
| 18.01   | 2018    | LZMA2 improvements       |
| 19.00   | 2019    | Encryption improvements  |
| 21.00   | 2021    | Performance improvements |
| 22.00   | 2022    | Bug fixes                |
| 23.00   | 2023    | RISC-V filter            |
| 24.00   | 2024    | Current stable           |

### Baseline Compatibility

For maximum compatibility, target 7-Zip 9.20 features:

- LZMA, LZMA2 compression
- BCJ filters (x86, ARM, PPC, IA64, SPARC)
- Delta filter
- AES-256 encryption
- Solid and non-solid modes

## Implementation Differences

### p7zip

Fork of 7-Zip for Unix/Linux:

- Same format support as 7-Zip
- May lag behind in new features
- Development less active since 2016

### 7-Zip-zstd

Fork with additional compression methods:

- Zstandard (zstd)
- Brotli
- LZ4, LZ5, Lizard

**Note:** Archives using these methods require compatible tools.

### py7zr

Python implementation:

- No BCJ2 support
- No Deflate64 support
- UTF-8 symlink targets

### libarchive

Multi-format library:

- Zstandard support
- Some edge cases handled differently

## Known Quirks

### File Name Encoding

7-Zip always uses UTF-16-LE for file names. Some implementations:

- May produce invalid UTF-16 for certain characters
- Handle surrogate pairs differently
- Normalize paths differently

**Recommendation:** Validate UTF-16 when reading, produce valid UTF-16 when writing.

### Timestamp Precision

7-Zip stores full FILETIME precision (100ns). Some implementations:

- Truncate to second precision
- Lose sub-second information
- Use different epoch conversions

**Recommendation:** Preserve full precision when possible.

### Unix Attributes

The UNIX_EXTENSION flag (bit 15) usage varies:

- Some tools always set it on Unix
- Some only when permissions differ from default
- Windows 7-Zip ignores it

**Recommendation:** Set UNIX_EXTENSION when storing Unix-specific attributes.

### Empty Files vs Directories

Distinction relies on EmptyFile property:

- If EmptyStream but not EmptyFile → directory
- Some tools mark empty files incorrectly

**Recommendation:** Also check DIRECTORY attribute (0x10) for robustness.

### Solid Block Sizes

Different tools use different solid block size defaults:

- 7-Zip GUI: Configurable
- Command line: Various defaults
- Other tools: Often "solid" or "non-solid" only

**Recommendation:** Document solid block size when relevant.

## Interoperability Testing

### Test Archive Sources

1. Create archives with 7-Zip
2. Create archives with p7zip
3. Download test archives from py7zr repository

### Test Scenarios

| Scenario            | Test                           |
| ------------------- | ------------------------------ |
| Basic extraction    | Extract 7-Zip created archive  |
| Compression methods | Test all common methods        |
| Solid archives      | Extract from solid blocks      |
| Encryption          | Various passwords and settings |
| Unicode names       | Non-ASCII characters           |
| Long paths          | Paths > 260 characters         |
| Symbolic links      | Unix and Windows styles        |
| Empty files         | Zero-byte files                |
| Directories         | Various attributes             |

### Validation Checklist

- [ ] CRCs match
- [ ] File sizes correct
- [ ] Timestamps preserved
- [ ] Attributes correct
- [ ] Paths correct
- [ ] Symlinks point to correct targets

## Writing Compatible Archives

### Recommended Settings

For maximum compatibility:

```
Compression: LZMA2
Dictionary: 16 MiB or less
Solid: Off (or small blocks)
Header compression: LZMA2
Header encryption: Off
Filters: BCJ for executables only
```

### Settings to Avoid

These reduce compatibility:

| Setting                | Compatibility Impact     |
| ---------------------- | ------------------------ |
| Zstd, Brotli, LZ4      | Requires 7-Zip-zstd      |
| Very large dictionary  | Memory issues on 32-bit  |
| BCJ2                   | py7zr cannot read        |
| Deflate64              | Some tools read-only     |
| Very long solid blocks | Memory during extraction |

## Error Messages

### Common Errors from Other Tools

| Error                | Likely Cause                      |
| -------------------- | --------------------------------- |
| "Unsupported method" | Extended compression (zstd, etc.) |
| "Data error"         | Corruption or wrong password      |
| "CRC failed"         | Corruption                        |
| "Unexpected end"     | Truncated archive                 |
| "Headers error"      | Header corruption                 |

### Our Errors for Their Archives

| Condition      | Appropriate Response         |
| -------------- | ---------------------------- |
| Unknown method | Skip entry with warning      |
| Invalid UTF-16 | Replace invalid chars        |
| Future version | Warn and attempt read        |
| Missing CRCs   | Proceed without verification |

## Multi-Volume Compatibility

### Naming

Standard: `.7z.001`, `.7z.002`, etc.

Some tools use:

- `.7z.1`, `.7z.2` (no padding)
- `.001`, `.002` (base name only)
- `.part1.7z`, `.part2.7z` (alternative)

**Recommendation:** Accept standard naming, produce standard naming.

### Splitting Boundaries

7-Zip splits at configured size. When joining:

- Read volumes in order
- Verify all present before extraction
- Handle missing volumes gracefully

## SFX Compatibility

### Stubs

7-Zip provides Windows stubs. For other platforms:

- Linux: Community-created stubs
- macOS: Manual stub creation

### Config Block

7-Zip's config format is Windows-centric. Cross-platform SFX:

- May not support all options
- May ignore GUI settings on non-Windows

## Future Compatibility

### Reserved Fields

- Reserved flag bits: Always write as 0
- Unknown properties: Skip by size
- Unknown methods: Report and skip

### Version Handling

- Major version > 0: Reject
- Minor version > 4: Warn but attempt

## Community Resources

### Test Archives

- py7zr test data: Various edge cases
- 7-Zip test suite: Official tests
- libarchive tests: Multi-format tests

### Implementation References

- 7-Zip source: Authoritative
- py7zr: Well-documented Python
- sevenz-rust2: Another Rust implementation

## See Also

- [Philosophy](/7z/00-philosophy) - Compatibility stance
- [Method IDs](/7z/appendix/b-method-ids) - Method support matrix
- [Error Conditions](/7z/18-error-conditions) - Error handling
