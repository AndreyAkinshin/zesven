---
title: 7z Archive Format Specification
description: Comprehensive specification for the 7z archive format
---

# 7z Archive Format Specification

This section contains the comprehensive specification for the 7z archive format as implemented by zesven.

## Purpose

This specification provides a precise, unambiguous definition of the 7z archive format suitable for:

- Implementing compliant readers and writers
- Validating archive correctness
- Understanding format internals
- Ensuring interoperability with 7-Zip and other implementations

## Scope

**In scope:**

- Binary format structure and layout
- Data encoding schemes
- Compression and filter method interfaces
- Encryption scheme
- File metadata representation
- Error conditions and handling

**Out of scope:**

- Compression algorithm internals (LZMA, LZMA2, etc.)
- Cryptographic primitive implementations
- Platform-specific extraction behaviors
- User interface considerations

## Document Conventions

### Requirement Keywords

This specification uses requirement level keywords as defined in RFC 2119:

| Keyword        | Meaning                            |
| -------------- | ---------------------------------- |
| **MUST**       | Absolute requirement               |
| **MUST NOT**   | Absolute prohibition               |
| **SHOULD**     | Recommended but not required       |
| **SHOULD NOT** | Not recommended but not prohibited |
| **MAY**        | Optional                           |

### Byte Order

All multi-byte integers are little-endian unless explicitly stated otherwise.

### Binary Notation

- Hexadecimal bytes: `0x7A` or `7A`
- Byte sequences: `37 7A BC AF 27 1C`
- Binary literals: `0b10110100`
- Bit ranges: bits 0-3 (inclusive, 0 is LSB)

### Range Notation

All numeric ranges in this specification are inclusive on both ends unless otherwise noted:

- `0-255` means values from 0 to 255 inclusive (256 total values)
- `0x00-0x04` means values from 0x00 to 0x04 inclusive (5 total values)

For loop bounds in pseudocode:

- `for i in 0..N` means i takes values 0, 1, 2, ..., N-1 (exclusive upper bound, Python/Rust style)
- `for i in 0 to N - 1` means the same range explicitly

### Size Units

- All sizes are in bytes unless otherwise specified
- 1 KiB = 1024 bytes
- 1 MiB = 1024 KiB
- 1 GiB = 1024 MiB

### Grammar Notation

Structure definitions use a BNF-like notation:

```
Structure ::= Field1 Field2 [OptionalField] Field3*
Field1 ::= BYTE
Field2 ::= NUMBER
OptionalField ::= 0x01 Data
Field3 ::= UINT32
```

Where:

- `[...]` denotes optional elements
- `*` denotes zero or more repetitions
- `+` denotes one or more repetitions
- `|` denotes alternatives

## Document Map

### Foundation (00-04)

| Document                                    | Description                          |
| ------------------------------------------- | ------------------------------------ |
| [Philosophy](./00-philosophy)               | Design principles, goals, invariants |
| [Glossary](./01-glossary)                   | Canonical terminology definitions    |
| [Archive Structure](./02-archive-structure) | High-level archive layout            |
| [Signature Header](./03-signature-header)   | 32-byte start header format          |
| [Data Encoding](./04-data-encoding)         | NUMBER, BitField, and type encodings |

### Header Format (05-09)

| Document                                  | Description                          |
| ----------------------------------------- | ------------------------------------ |
| [Header Structure](./05-header-structure) | Main and encoded header organization |
| [Pack Info](./06-pack-info)               | Compressed stream information        |
| [Unpack Info](./07-unpack-info)           | Folder and coder definitions         |
| [Substreams Info](./08-substreams-info)   | Per-file data within folders         |
| [Files Info](./09-files-info)             | File metadata and properties         |

### Codecs and Filters (10-12)

| Document                                        | Description                         |
| ----------------------------------------------- | ----------------------------------- |
| [Compression Methods](./10-compression-methods) | Compression algorithm interfaces    |
| [Filters](./11-filters)                         | BCJ, Delta, and other preprocessors |
| [Encryption](./12-encryption)                   | AES-256-SHA256 encryption scheme    |

### Special Features (13-15)

| Document                              | Description              |
| ------------------------------------- | ------------------------ |
| [Solid Archives](./13-solid-archives) | Solid block compression  |
| [Multi-Volume](./14-multi-volume)     | Split archive handling   |
| [SFX Archives](./15-sfx-archives)     | Self-extracting archives |

### Metadata and Safety (16-18)

| Document                                              | Description                   |
| ----------------------------------------------------- | ----------------------------- |
| [Timestamps & Attributes](./16-timestamps-attributes) | Time and attribute formats    |
| [Security](./17-security)                             | Safety constraints and limits |
| [Error Conditions](./18-error-conditions)             | Error handling requirements   |

### Reference Appendices

| Document                                       | Description                          |
| ---------------------------------------------- | ------------------------------------ |
| [A: Property IDs](./appendix/a-property-ids)   | Complete property ID table           |
| [B: Method IDs](./appendix/b-method-ids)       | Complete compression method ID table |
| [C: CRC Algorithm](./appendix/c-crc-algorithm) | CRC-32 specification                 |
| [D: Compatibility](./appendix/d-compatibility) | Interoperability notes               |

## Reading Order

For implementers new to the 7z format:

1. Start with [Philosophy](./00-philosophy) for context
2. Read [Glossary](./01-glossary) to understand terminology
3. Continue with [Archive Structure](./02-archive-structure) for the big picture
4. Then proceed sequentially through the remaining documents

For quick reference:

- Property IDs: [Appendix A](./appendix/a-property-ids)
- Method IDs: [Appendix B](./appendix/b-method-ids)
- CRC algorithm: [Appendix C](./appendix/c-crc-algorithm)

## Version History

| Version | Date    | Changes               |
| ------- | ------- | --------------------- |
| 1.0.0   | 2025-01 | Initial specification |

## Acknowledgments

This specification is derived from:

- Official 7-Zip source code by Igor Pavlov
- py7zr documentation by Hiroshi Miura
- Analysis of multiple open-source implementations

## License

This specification is part of the zesven project and is licensed under MIT OR Apache-2.0.
