# Security

This document specifies security requirements and constraints for 7z archive processing.

## Overview

Security considerations fall into three categories:

1. **Path safety:** Preventing file system escape
2. **Resource limits:** Preventing denial of service
3. **Cryptographic safety:** Protecting encrypted content

## Path Traversal Prevention

### Forbidden Patterns

Implementations MUST reject paths containing:

| Pattern          | Example          | Risk               |
| ---------------- | ---------------- | ------------------ |
| `..` component   | `../etc/passwd`  | Directory escape   |
| Absolute paths   | `/etc/passwd`    | Arbitrary write    |
| Windows absolute | `C:\Windows\...` | Arbitrary write    |
| UNC paths        | `\\server\share` | Network access     |
| Null bytes       | `file\x00.txt`   | Truncation attacks |

### Path Normalization

Before checking, paths SHOULD be normalized:

```
function normalize_path(path):
    # Convert backslash to forward slash
    path = path.replace('\\', '/')

    # Remove leading slash
    path = path.lstrip('/')

    # Collapse consecutive slashes
    while '//' in path:
        path = path.replace('//', '/')

    # Check for .. components
    for component in path.split('/'):
        if component == '..':
            reject("Path traversal detected")

    return path
```

**Additional considerations:**

- **Trailing dots/spaces (Windows):** Paths like `file.txt.` or `file.txt ` may be silently truncated on Windows. Implementations SHOULD warn about or reject paths with trailing dots or spaces.
- **Windows reserved names:** Names like `CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9` are reserved on Windows. Implementations MAY warn when extracting archives containing such names.
- **Empty components:** Paths like `foo//bar` (consecutive separators) should be normalized to `foo/bar`.

### Validation Algorithm

```
function validate_path(path):
    # Reject null bytes
    if '\x00' in path:
        reject("Null byte in path")

    # Reject absolute paths
    if path.startswith('/'):
        reject("Absolute path")
    if len(path) >= 2 and path[1] == ':':
        reject("Windows absolute path")
    if path.startswith('\\\\'):
        reject("UNC path")

    # Normalize and check
    normalized = normalize_path(path)

    # Verify result is under extraction root
    if not is_under_root(normalized):
        reject("Path escape detected")

    return normalized
```

### Symbolic Link Safety

Symbolic links present additional risks:

| Risk             | Mitigation                      |
| ---------------- | ------------------------------- |
| Link to `../`    | Resolve links, check final path |
| Link to `/etc`   | Reject absolute link targets    |
| Link chain loops | Limit resolution depth          |

**Recommended:** Extract links last, after verifying targets exist within extraction directory.

## Resource Limits

### Limits and Recommended Defaults

| Resource              | Type   | Value     | Rationale                   |
| --------------------- | ------ | --------- | --------------------------- |
| Max key iterations    | MUST   | 2^30      | Prevent DoS via crypto      |
| Max recursion depth   | MUST   | 4         | Prevent stack overflow      |
| Max property size     | MUST   | 2^32      | Prevent allocation overflow |
| Max entries           | SHOULD | 1,000,000 | Prevent memory exhaustion   |
| Max header size       | SHOULD | 64 MiB    | Limit parsing memory        |
| Max total unpack      | SHOULD | 1 TiB     | Prevent disk exhaustion     |
| Max entry size        | SHOULD | 64 GiB    | Single file limit           |
| Max compression ratio | SHOULD | 1000:1    | Detect zip bombs            |

**Type meanings:**

- **MUST**: Hard limit; implementations MUST enforce this limit
- **SHOULD**: Recommended default; implementations SHOULD enforce but MAY allow configuration

### Compression Bomb Detection

**Compression bombs** are archives that expand to extremely large sizes.

**Detection methods:**

1. **Ratio monitoring:**

```
function check_ratio(compressed, decompressed):
    if compressed == 0 and decompressed > 0:
        reject("Infinite compression ratio")
    ratio = decompressed / compressed
    if ratio > MAX_RATIO:
        reject("Compression ratio exceeds limit")
```

2. **Progressive checking:**

```
function decompress_with_limit(stream, max_output):
    total_output = 0
    while data = stream.read():
        total_output += len(data)
        if total_output > max_output:
            reject("Output size exceeds limit")
        yield data
```

### Memory Limits

Large archives can exhaust memory:

| Operation        | Memory Risk       |
| ---------------- | ----------------- |
| Header parsing   | Large entry count |
| Name storage     | Many long paths   |
| Decompression    | Large dictionary  |
| Solid extraction | Buffering data    |

**Mitigations:**

- Stream data instead of buffering
- Limit dictionary sizes
- Process entries incrementally
- Use memory-mapped I/O

## Cryptographic Safety

### Key Derivation Limits

The `num_cycles_power` parameter controls iteration count:

| Value | Iterations | Risk                |
| ----- | ---------- | ------------------- |
| 19    | 524K       | Typical, safe       |
| 24    | 16M        | Slow but acceptable |
| 30    | 1B         | Maximum allowed     |
| > 30  | > 1B       | MUST reject         |

**Rationale:** Prevents DoS via archives requiring years of computation.

### Implementation

```
const MAX_CYCLES_POWER: u8 = 30;

function derive_key(password, salt, cycles_power):
    if cycles_power > MAX_CYCLES_POWER:
        reject("Key derivation cycles exceed maximum")

    iterations = 1 << cycles_power
    # ... key derivation
```

### Password Handling

- Never log passwords
- Zero password memory after use
- Use secure memory if available
- Implement timeout for cached keys

## Input Validation

### Header Validation

| Field         | Validation                         |
| ------------- | ---------------------------------- |
| Signature     | Exact match to `37 7A BC AF 27 1C` |
| Version major | Must be 0                          |
| CRCs          | Must match calculated values       |
| Offsets       | Must be within file bounds         |
| Sizes         | Must not cause overflow            |
| Counts        | Must match actual data             |

### Integer Overflow

When calculating sizes and offsets:

```
function safe_add(a, b):
    if a > MAX_VALUE - b:
        reject("Integer overflow")
    return a + b

function safe_multiply(a, b):
    if b != 0 and a > MAX_VALUE / b:
        reject("Integer overflow")
    return a * b
```

### Malformed Data Handling

| Condition        | Action                       |
| ---------------- | ---------------------------- |
| Truncated file   | Abort with clear error       |
| Invalid CRC      | Abort or warn (configurable) |
| Unknown property | Skip (size-prefixed)         |
| Unknown method   | Abort with method ID         |
| Invalid UTF-16   | Replace or reject            |

## Error Information Leakage

### Password Detection

Don't distinguish between:

- Wrong password
- Corrupted encrypted data
- Missing password

Generic error prevents oracle attacks.

### Timing Attacks

- Key derivation time is constant (iterations-dependent only)
- Don't short-circuit on password mismatch
- Use constant-time comparison for hashes

## Implementation Checklist

### Required (MUST)

- [ ] Reject paths with `..` components
- [ ] Reject absolute paths
- [ ] Validate all CRCs
- [ ] Enforce header size limit
- [ ] Limit key derivation iterations to 2^30
- [ ] Check integer overflow in size calculations
- [ ] Verify offsets are within file bounds

### Recommended (SHOULD)

- [ ] Enforce entry count limit
- [ ] Detect compression bombs
- [ ] Limit total decompression size
- [ ] Resolve symbolic links safely
- [ ] Use secure memory for passwords
- [ ] Implement extraction timeout

### Optional (MAY)

- [ ] Sandbox extraction directory
- [ ] Verify digital signatures
- [ ] Quarantine suspicious archives
- [ ] Log security events

## Security Test Cases

Test archives SHOULD include:

1. Path traversal attempts (`../../../etc/passwd`)
2. Absolute paths (`/tmp/evil`)
3. Very long paths (> 4096 characters)
4. Compression bomb (1 KB → 1 TB)
5. Nested encoded headers (depth > 4)
6. Extreme iteration count (2^63)
7. Malformed headers
8. Truncated archives
9. Invalid CRCs
10. Symbolic link loops

## See Also

- [Philosophy](/7z/00-philosophy) - Security-first design
- [Encryption](/7z/12-encryption) - Cryptographic details
- [Error Conditions](/7z/18-error-conditions) - Error handling
