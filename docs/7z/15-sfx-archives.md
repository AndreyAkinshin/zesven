# Self-Extracting Archives (SFX)

This document specifies self-extracting archive format and detection.

## Overview

A self-extracting archive (SFX) is an executable that contains:

1. Extraction stub (executable code)
2. Optional configuration block
3. Standard 7z archive data

When executed, the stub extracts the embedded archive.

## Structure

```
┌─────────────────────────────┐  Offset 0x00
│     Executable Stub         │  Platform-specific executable
│     (PE, ELF, Mach-O)       │
├─────────────────────────────┤
│     Configuration Block     │  Optional installer settings
│     (text format)           │
├─────────────────────────────┤  SFX Offset
│     7z Signature Header     │  Standard 32-byte header
├─────────────────────────────┤
│     Pack Data               │  Compressed content
├─────────────────────────────┤
│     Next Header             │  Archive metadata
└─────────────────────────────┘
```

## Platform Stubs

### Windows PE

**Magic:** `4D 5A` ("MZ") at offset 0

```
MZ Header → PE Header → Sections → [Config] → 7z Archive
```

**File extension:** `.exe`

### Linux ELF

**Magic:** `7F 45 4C 46` ("\x7FELF") at offset 0

```
ELF Header → Program Headers → Sections → [Config] → 7z Archive
```

**File extension:** none (or `.run`, `.bin`)

### macOS Mach-O

**Magic:** `CF FA ED FE` or `FE ED FA CF` at offset 0

```
Mach-O Header → Load Commands → Segments → [Config] → 7z Archive
```

**File extension:** none (or `.app` bundle)

## Configuration Block

Optional text block between stub and archive:

```
;!@Install@!UTF-8!
Title="Application Installer"
BeginPrompt="Install Application?"
RunProgram="setup.exe"
Directory="%%T"
Progress="yes"
;!@InstallEnd@!
```

### Configuration Fields

| Field               | Description                     |
| ------------------- | ------------------------------- |
| `Title`             | Window title                    |
| `BeginPrompt`       | Confirmation prompt             |
| `RunProgram`        | Program to run after extraction |
| `Directory`         | Extraction target (%%T = temp)  |
| `Progress`          | Show progress ("yes"/"no")      |
| `GUIFlags`          | GUI behavior flags              |
| `ExtractTitle`      | Title during extraction         |
| `ExtractDialogText` | Text during extraction          |
| `ExtractCancelText` | Cancel button text              |

### Block Delimiters

- Start: `;!@Install@!UTF-8!`
- End: `;!@InstallEnd@!`

## Signature Detection

### Search Limit Requirements

Implementations MUST impose a finite search limit to prevent denial-of-service attacks when scanning large files.

Implementations SHOULD search at least the first 1 MiB (1,048,576 bytes) of the file.

Implementations SHOULD NOT search beyond 16 MiB without first analyzing the executable format to determine expected stub boundaries. Unbounded searching enables DoS attacks via large non-archive files.

Implementations MAY search beyond 1 MiB based on heuristics (e.g., executable section analysis, platform-specific PE/ELF/Mach-O parsing to determine expected stub size).

### Algorithm

```
function find_sfx_archive(file):
    # Implementation-defined search limit (MUST be finite)
    # Recommended minimum: 1 MiB
    SEARCH_LIMIT = implementation_defined_limit()

    signature = bytes([0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])
    search_end = min(file.size, SEARCH_LIMIT)

    position = 0
    while position < search_end:
        # Search for signature bytes
        index = file.find(signature, position, search_end)
        if index == -1:
            return None

        # Verify it's a valid archive header
        file.seek(index)
        header = file.read(32)

        if validate_signature_header(header):
            return index

        # Continue searching after false positive
        position = index + 1

    return None
```

### Validation Checks

After finding signature bytes:

1. **Version check:** Major version must be 0
2. **Minor version:** Should be ≤ 4
3. **CRC check:** StartHeaderCRC must be valid
4. **Offset check:** NextHeaderOffset must be reasonable

### False Positive Prevention

The 6-byte signature could appear in:

- Executable code
- Resource data
- Coincidental byte patterns

Full header validation ensures correct detection.

## Offset Adjustment

All archive offsets are relative to the SFX offset:

```
Actual position = SFX_offset + archive_offset
```

**Examples:**

- Pack data at `SFX_offset + 32`
- Next header at `SFX_offset + 32 + NextHeaderOffset`

### Implementation

```
struct SfxArchive {
    file: File,
    sfx_offset: u64,
}

impl SfxArchive {
    fn translate_offset(&self, archive_offset: u64) -> u64 {
        self.sfx_offset + archive_offset
    }

    fn read_pack_data(&self, pack_pos: u64, size: u64) -> Vec<u8> {
        let actual_pos = self.translate_offset(32 + pack_pos);
        self.file.seek(actual_pos);
        self.file.read(size)
    }
}
```

## Creating SFX Archives

### Process

1. Select appropriate stub for target platform
2. Optionally create configuration block
3. Create standard 7z archive
4. Concatenate: stub + [config] + archive

### Stub Sources

7-Zip provides stubs:

- `7z.sfx` - GUI extractor (Windows)
- `7zCon.sfx` - Console extractor (Windows)
- Custom stubs for other platforms

### Concatenation

```bash
# Windows
copy /b 7z.sfx + config.txt + archive.7z output.exe

# Unix
cat 7z.sfx config.txt archive.7z > output.run
chmod +x output.run
```

## Security Considerations

### Execution Risk

SFX archives are executables:

- May trigger security warnings
- May be blocked by email filters
- Should be signed for distribution

### Code Signing

Windows SFX should be signed:

- Authenticode signature on final `.exe`
- Signature covers entire file including archive

### Extraction Safety

Same protections as regular archives:

- Path traversal prevention
- Resource limits
- CRC verification

## Detection Heuristics

### Quick Detection

```
function is_likely_sfx(file):
    header = file.read(4)

    # Windows PE
    if header[0:2] == "MZ":
        return scan_for_7z(file)

    # Linux ELF
    if header == "\x7FELF":
        return scan_for_7z(file)

    # macOS Mach-O
    if header in ["\xCF\xFA\xED\xFE", "\xFE\xED\xFA\xCF"]:
        return scan_for_7z(file)

    return False
```

### File Extension

SFX archives may have:

- `.exe` (Windows)
- `.run`, `.bin` (Linux)
- `.app` (macOS bundle)
- No extension (Unix convention)

Extension alone is not reliable for detection.

## Compatibility Notes

- All 7-Zip versions support SFX extraction
- Creating SFX requires stub files
- Cross-platform SFX requires platform-specific stubs
- Some antivirus may flag SFX files

## Example Analysis

Windows SFX archive:

```
Offset 0x00000: 4D 5A ...           # MZ header (PE)
Offset 0x01000: 50 45 00 00 ...     # PE signature
...
Offset 0x10000: ;!@Install@!UTF-8!  # Config start
Offset 0x10100: ;!@InstallEnd@!     # Config end
Offset 0x10120: 37 7A BC AF 27 1C   # 7z signature
Offset 0x10120: 00 04               # Version 0.4
Offset 0x10128: [CRC + offsets]     # Start header
Offset 0x10140: [Pack data...]
```

SFX offset = 0x10120

## See Also

- [Archive Structure](/7z/02-archive-structure) - Standard archive layout
- [Signature Header](/7z/03-signature-header) - Signature header format
- [Security](/7z/17-security) - Security considerations
