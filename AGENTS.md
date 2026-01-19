# Agent Instructions

Instructions for LLM agents working with this repository.

## Repository Overview

**zesven** is a pure-Rust 7z archive library. The repository contains:

| Directory  | Description                      |
| ---------- | -------------------------------- |
| `rs/`      | Rust library and CLI source code |
| `docs/`    | VitePress documentation site     |
| `.github/` | CI/CD workflows                  |

## Development Guides

| Component        | Guide                                                                                                          |
| ---------------- | -------------------------------------------------------------------------------------------------------------- |
| **Rust library** | See [`rs/AGENTS.md`](rs/AGENTS.md) for Rust-specific commands, CI checks, feature flags, and project structure |

## Quick Reference

### Rust Development

```bash
mise run check    # fmt, clippy
mise run test     # run all tests
mise run ci       # full CI pipeline
```

See [`rs/AGENTS.md`](rs/AGENTS.md) for complete details.
