# Timestamps and Attributes

This document specifies the format of timestamps and file attributes in 7z archives.

## Timestamps

### FILETIME Format

7z uses Windows FILETIME format for all timestamps:

- **Size:** 64 bits (8 bytes)
- **Unit:** 100-nanosecond intervals
- **Epoch:** January 1, 1601 00:00:00 UTC
- **Byte order:** Little-endian

### Time Properties

| Property ID | Name  | Description            |
| ----------- | ----- | ---------------------- |
| 0x12        | CTime | Creation time          |
| 0x13        | ATime | Last access time       |
| 0x14        | MTime | Last modification time |

### Epoch Conversion

**FILETIME to Unix timestamp:**

```
UNIX_EPOCH_FILETIME = 116444736000000000  # 1970-01-01 in FILETIME
unix_seconds = (filetime - UNIX_EPOCH_FILETIME) / 10_000_000
unix_nanos = ((filetime - UNIX_EPOCH_FILETIME) % 10_000_000) * 100
```

**Unix timestamp to FILETIME:**

```
filetime = unix_seconds * 10_000_000 + UNIX_EPOCH_FILETIME
filetime += unix_nanos / 100
```

### Value Ranges

| Description | FILETIME Value                  |
| ----------- | ------------------------------- |
| Minimum     | 0 (1601-01-01)                  |
| Unix epoch  | 116444736000000000 (1970-01-01) |
| Year 2000   | 125911584000000000              |
| Year 2038   | 137919572000000000              |
| Maximum     | 2^63 - 1 (year ~30828)          |

**Note:** While FILETIME is an unsigned 64-bit integer (full range 0 to 2^64 - 1), 7-Zip and most implementations treat values >= 2^63 as invalid or interpret them as signed negative values. Writers MUST NOT produce values >= 2^63. Readers encountering such values SHOULD treat them as undefined timestamps.

### Example Values

```
# 2024-01-15 12:00:00 UTC
# Unix: 1705320000
# FILETIME: 1705320000 * 10000000 + 116444736000000000
#         = 133497936000000000
#         = 0x01DA47AA5D872000

Bytes: 00 20 87 5D AA 47 DA 01
```

### Missing Timestamps

When a timestamp is not defined:

- BooleanList indicates absence
- Implementations SHOULD use a sensible default:
  - Current time for creation
  - File's actual mtime for modification

### Unencodable Timestamps

Some timestamps cannot be represented in FILETIME format:

- **Pre-1601 dates:** Dates before January 1, 1601 cannot be encoded. Writers SHOULD either use FILETIME value 0 or mark the timestamp as undefined in the BooleanList.
- **Unknown timestamps:** When a file's timestamp is unknown or unavailable, mark it as undefined in the BooleanList rather than storing an arbitrary value.

## File Attributes

### Storage Format

Attributes are stored as UINT32 (4 bytes, little-endian):

```
┌────────────────────────────────┐
│ Bits 31-16 │    Bits 15-0     │
│ Unix mode  │ Windows attrs    │
└────────────────────────────────┘
```

### Windows Attributes (Bits 0-15)

| Bit | Value  | Constant            | Description                  |
| --- | ------ | ------------------- | ---------------------------- |
| 0   | 0x0001 | READONLY            | Read-only file               |
| 1   | 0x0002 | HIDDEN              | Hidden file                  |
| 2   | 0x0004 | SYSTEM              | System file                  |
| 3   | -      | Reserved            | -                            |
| 4   | 0x0010 | DIRECTORY           | Directory                    |
| 5   | 0x0020 | ARCHIVE             | Archive flag                 |
| 6   | 0x0040 | DEVICE              | Device (reserved)            |
| 7   | 0x0080 | NORMAL              | Normal file (no other attrs) |
| 8   | 0x0100 | TEMPORARY           | Temporary file               |
| 9   | 0x0200 | SPARSE_FILE         | Sparse file                  |
| 10  | 0x0400 | REPARSE_POINT       | Symbolic link/junction       |
| 11  | 0x0800 | COMPRESSED          | NTFS compressed              |
| 12  | 0x1000 | OFFLINE             | Offline storage              |
| 13  | 0x2000 | NOT_CONTENT_INDEXED | Not indexed                  |
| 14  | 0x4000 | ENCRYPTED           | EFS encrypted                |
| 15  | 0x8000 | UNIX_EXTENSION      | Unix mode in high bits       |

### Unix Mode (Bits 16-31)

When bit 15 (UNIX_EXTENSION) is set, bits 16-31 contain the Unix mode:

```
Bits 16-18: Other permissions (rwx)
Bits 19-21: Group permissions (rwx)
Bits 22-24: Owner permissions (rwx)
Bit 25:     Sticky bit
Bit 26:     Set GID
Bit 27:     Set UID
Bits 28-31: File type
```

### Unix Permission Bits

| Bit | Mask         | Description       |
| --- | ------------ | ----------------- |
| 16  | 0x0001 << 16 | Other execute     |
| 17  | 0x0002 << 16 | Other write       |
| 18  | 0x0004 << 16 | Other read        |
| 19  | 0x0008 << 16 | Group execute     |
| 20  | 0x0010 << 16 | Group write       |
| 21  | 0x0020 << 16 | Group read        |
| 22  | 0x0040 << 16 | Owner execute     |
| 23  | 0x0080 << 16 | Owner write       |
| 24  | 0x0100 << 16 | Owner read        |
| 25  | 0x0200 << 16 | Sticky (S_ISVTX)  |
| 26  | 0x0400 << 16 | Set GID (S_ISGID) |
| 27  | 0x0800 << 16 | Set UID (S_ISUID) |

### Unix File Types (Bits 28-31)

| Value | Mask     | Type              |
| ----- | -------- | ----------------- |
| 0x1   | S_IFIFO  | FIFO (named pipe) |
| 0x2   | S_IFCHR  | Character device  |
| 0x4   | S_IFDIR  | Directory         |
| 0x6   | S_IFBLK  | Block device      |
| 0x8   | S_IFREG  | Regular file      |
| 0xA   | S_IFLNK  | Symbolic link     |
| 0xC   | S_IFSOCK | Socket            |

### Attribute Extraction

```
function parse_attributes(attr: u32):
    windows_attrs = attr & 0xFFFF
    has_unix = (attr & 0x8000) != 0

    if has_unix:
        unix_mode = (attr >> 16) & 0xFFFF
        unix_type = (attr >> 28) & 0x0F
        unix_perms = (attr >> 16) & 0x0FFF
    else:
        unix_mode = None

    return (windows_attrs, unix_mode)
```

## Symbolic Links

### Storage

Symbolic links are stored as:

1. File with data (target path)
2. Special attributes indicating symlink type

### Unix Symbolic Links

**Attributes:** `UNIX_EXTENSION | (S_IFLNK << 16) | permissions`

Example: `0x8000 | 0xA0000000 | 0x01FF0000` = `0xA1FF8000`

**Data content:** UTF-8 encoded target path (no null terminator)

### Windows Reparse Points

**Attributes:** `REPARSE_POINT` (0x0400)

**Data content:** Target path in UTF-8

### Target Path Format

- Relative paths: `../other/file.txt`
- Absolute paths: `/usr/local/bin/tool` (Unix) or `C:\Windows\...` (Windows)

## Directory Handling

### Detection

A directory is identified by:

- `EmptyStream = true` AND `EmptyFile = false`
- OR `DIRECTORY` attribute (0x0010) set

### Attributes

Directories typically have:

- Windows: `DIRECTORY` (0x0010)
- Unix: `S_IFDIR` (0x4) in type bits + permissions

### Path Format

Directory paths SHOULD end with `/`:

- `src/` - directory
- `src` - could be file or directory (check attributes)

## Special Files (Unix)

### Device Files

Stored with appropriate type bits:

- Character device: S_IFCHR (0x2)
- Block device: S_IFBLK (0x6)

**Data content:** Major/minor device numbers (implementation-defined)

### FIFO and Sockets

Stored with type bits:

- FIFO: S_IFIFO (0x1)
- Socket: S_IFSOCK (0xC)

Typically stored as empty files with type attributes.

## Cross-Platform Considerations

### Windows → Unix

| Windows Attribute | Unix Handling                |
| ----------------- | ---------------------------- |
| READONLY          | Clear write bits             |
| HIDDEN            | Prefix with `.` (convention) |
| DIRECTORY         | Set S_IFDIR                  |
| ARCHIVE           | Ignore                       |

### Unix → Windows

| Unix Feature | Windows Handling            |
| ------------ | --------------------------- |
| Symlink      | REPARSE_POINT               |
| Execute bit  | Ignore (use extension)      |
| Permissions  | Map to READONLY if no write |
| Device files | Cannot represent            |

## Example Attributes

### Regular File (644)

```
Unix: rw-r--r-- (0644)
Attribute: 0x81A48020
  Windows: 0x0020 (ARCHIVE)
  Unix ext: 0x8000
  Mode: 0x81A4 << 16
    Type: 0x8 (S_IFREG)
    Perms: 0x1A4 (rw-r--r--)
```

### Executable (755)

```
Unix: rwxr-xr-x (0755)
Attribute: 0x81ED8020
  Windows: 0x0020 (ARCHIVE)
  Unix ext: 0x8000
  Mode: 0x81ED << 16
    Type: 0x8 (S_IFREG)
    Perms: 0x1ED (rwxr-xr-x)
```

### Directory (755)

```
Unix: drwxr-xr-x
Attribute: 0x41ED8010
  Windows: 0x0010 (DIRECTORY)
  Unix ext: 0x8000
  Mode: 0x41ED << 16
    Type: 0x4 (S_IFDIR)
    Perms: 0x1ED (rwxr-xr-x)
```

### Symbolic Link

```
Unix: lrwxrwxrwx
Attribute: 0xA1FF8000
  Windows: 0x0000 (or 0x0400 for reparse)
  Unix ext: 0x8000
  Mode: 0xA1FF << 16
    Type: 0xA (S_IFLNK)
    Perms: 0x1FF (rwxrwxrwx)
```

## See Also

- [Files Info](/7z/09-files-info) - File metadata structure
- [Property IDs](/7z/appendix/a-property-ids) - Time property IDs
