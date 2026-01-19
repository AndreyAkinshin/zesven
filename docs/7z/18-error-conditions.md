# Error Conditions

This document specifies error conditions and handling requirements for 7z archive processing.

## Error Categories

### Fatal Errors

Errors that MUST abort processing:

| Error                 | Condition                                     | Action              |
| --------------------- | --------------------------------------------- | ------------------- |
| InvalidSignature      | Signature bytes don't match                   | Abort               |
| UnsupportedVersion    | Major version > 0                             | Abort               |
| HeaderCrcMismatch     | StartHeaderCRC or NextHeaderCRC invalid       | Abort               |
| HeaderTruncated       | NextHeaderOffset + NextHeaderSize > file size | Abort               |
| PathTraversal         | Path contains `..` or is absolute             | Abort or skip entry |
| ResourceLimitExceeded | Any resource limit violated                   | Abort               |

### Recoverable Errors

Errors that MAY allow continued processing:

| Error             | Condition                  | Options                     |
| ----------------- | -------------------------- | --------------------------- |
| DataCrcMismatch   | File CRC doesn't match     | Warn and continue, or abort |
| UnsupportedMethod | Unknown compression method | Skip entry                  |
| UnknownProperty   | Unknown property ID        | Skip property               |
| WrongPassword     | Decryption fails           | Request new password        |
| CorruptEntry      | Single entry corrupted     | Skip entry                  |

### Warnings

Conditions that SHOULD be reported but don't require action:

| Warning              | Condition                    |
| -------------------- | ---------------------------- |
| MinorVersionUnknown  | Minor version > 4            |
| UnusedProperty       | Property with no effect      |
| NonCanonicalEncoding | NUMBER not minimally encoded |
| DeprecatedMethod     | Using obsolete compression   |

## Error Codes

Numeric error codes for programmatic handling:

| Code  | Name              | Description               |
| ----- | ----------------- | ------------------------- |
| 0     | OK                | Success                   |
| 1     | ERROR_DATA        | Corrupted data            |
| 2     | ERROR_MEM         | Memory allocation failed  |
| 3     | ERROR_CRC         | Checksum mismatch         |
| 4     | ERROR_UNSUPPORTED | Unsupported feature       |
| 5     | ERROR_PARAM       | Invalid parameter         |
| 6     | ERROR_INPUT_EOF   | Unexpected end of file    |
| 7     | ERROR_OUTPUT_EOF  | Output buffer overflow    |
| 8     | ERROR_READ        | Read failure              |
| 9     | ERROR_WRITE       | Write failure             |
| 10    | ERROR_PROGRESS    | Progress callback error   |
| 11    | ERROR_FAIL        | General failure           |
| 12    | ERROR_THREAD      | Threading error           |
| 13-15 | Reserved          | Reserved for future use   |
| 16    | ERROR_ARCHIVE     | Invalid archive format    |
| 17    | ERROR_NO_ARCHIVE  | Not a 7z archive          |
| 18    | ERROR_PASSWORD    | Wrong or missing password |
| 19    | ERROR_PATH        | Path validation failed    |
| 20    | ERROR_LIMIT       | Resource limit exceeded   |

## Error Context

Errors SHOULD include context information:

```
struct Error {
    code: ErrorCode,
    message: String,
    context: ErrorContext,
}

struct ErrorContext {
    offset: Option<u64>,      // File offset where error occurred
    entry_index: Option<u32>, // Entry being processed
    entry_name: Option<String>, // Entry name if known
    property_id: Option<u8>,  // Property being parsed
    method_id: Option<u64>,   // Method causing error
}
```

### Example Error Messages

```
Error: CRC mismatch
  Expected: 0x12345678
  Actual:   0x87654321
  Offset:   0x1000
  Entry:    "documents/report.pdf"

Error: Unsupported compression method
  Method ID: 0x04F71199
  Entry:     "data.bin"

Error: Path traversal detected
  Path:     "../../../etc/passwd"
  Entry:    3
```

## Validation Sequence

### Opening Archive

```
function open_archive(file):
    # 1. Check file size
    if file.size < 32:
        error(ERROR_NO_ARCHIVE, "File too small")

    # 2. Check signature
    signature = file.read(6)
    if signature != SIGNATURE:
        error(ERROR_NO_ARCHIVE, "Invalid signature")

    # 3. Check version
    major = file.read_byte()
    minor = file.read_byte()
    if major > 0:
        error(ERROR_UNSUPPORTED, "Unsupported major version")
    if minor > 4:
        warn("Unknown minor version")

    # 4. Validate start header CRC
    start_crc = file.read_u32()
    header_data = file.read(20)
    if crc32(header_data) != start_crc:
        error(ERROR_CRC, "Start header CRC mismatch")

    # 5. Parse header offsets
    next_offset = parse_u64(header_data[0:8])
    next_size = parse_u64(header_data[8:16])
    next_crc = parse_u32(header_data[16:20])

    # 6. Validate bounds
    if 32 + next_offset + next_size > file.size:
        error(ERROR_ARCHIVE, "Header offset out of bounds")

    # 7. Read and validate next header
    file.seek(32 + next_offset)
    header_bytes = file.read(next_size)
    if crc32(header_bytes) != next_crc:
        error(ERROR_CRC, "Next header CRC mismatch")

    return parse_header(header_bytes)
```

### Extracting Entry

```
function extract_entry(archive, entry, output):
    try:
        # Validate path
        if not is_safe_path(entry.path):
            error(ERROR_PATH, "Unsafe path")

        # Check method support
        for coder in entry.folder.coders:
            if not is_supported(coder.method_id):
                error(ERROR_UNSUPPORTED, "Unsupported method")

        # Decompress
        data = decompress(entry)

        # Verify CRC
        if entry.crc is not None:
            if crc32(data) != entry.crc:
                error(ERROR_CRC, "Entry CRC mismatch")

        # Write output
        write_file(output, data, entry.attributes)

    except PasswordRequired:
        # Request password and retry
        raise

    except Exception as e:
        # Log and optionally continue
        log_error(e, entry)
        if strict_mode:
            raise
```

## Password Error Handling

### Detection Methods

1. **PKCS#7 padding validation:**
   - After AES decryption, check padding bytes
   - Invalid padding → likely wrong password

2. **Decompression failure:**
   - After decryption, decompression fails
   - Invalid compressed data → wrong password

3. **CRC mismatch:**
   - Decompression succeeds but CRC fails
   - Data corruption or wrong password

### Response

```
function handle_password_error(error_type):
    # Don't reveal which check failed
    if error_type in [PADDING_INVALID, DECOMPRESS_FAIL, CRC_MISMATCH]:
        raise PasswordError("Wrong password or corrupted data")

    raise error_type  # Other errors pass through
```

## Recovery Strategies

### Partial Extraction

For large archives with some corrupted entries:

```
function extract_with_recovery(archive, output_dir):
    results = []

    for entry in archive.entries:
        try:
            extract_entry(entry, output_dir)
            results.append((entry, SUCCESS))
        except CrcMismatch as e:
            # Extract anyway, mark as potentially corrupt
            results.append((entry, EXTRACTED_WITH_WARNING))
            log_warning(e)
        except UnsupportedMethod:
            # Skip entry
            results.append((entry, SKIPPED))
            log_info("Skipped unsupported entry")
        except FatalError as e:
            # Cannot continue
            raise

    return results
```

### Archive Recovery

For severely corrupted archives:

1. Scan for valid signature
2. Attempt to parse header
3. Extract files with valid CRCs
4. Report unrecoverable entries

## Cleanup on Error

When errors occur during extraction:

1. Close open file handles
2. Delete partially written files
3. Remove empty directories created
4. Report cleanup actions

```
function safe_extract(archive, output_dir):
    created_files = []
    created_dirs = []

    try:
        for entry in archive:
            if entry.is_directory:
                mkdir(output_dir / entry.path)
                created_dirs.append(entry.path)
            else:
                extract_file(entry, output_dir)
                created_files.append(entry.path)

    except Exception as e:
        # Cleanup on failure
        for file in reversed(created_files):
            try_delete(output_dir / file)
        for dir in reversed(created_dirs):
            try_rmdir(output_dir / dir)
        raise
```

## See Also

- [Philosophy](/7z/00-philosophy) - Error handling philosophy
- [Security](/7z/17-security) - Security-related errors
- [CRC Algorithm](/7z/appendix/c-crc-algorithm) - CRC calculation for validation
