---
title: WASM/Browser Support
description: Use zesven in web browsers
---

# WASM/Browser Support

zesven can run in web browsers via WebAssembly.

## Feature Flag

Enable WASM support:

```toml
[dependencies]
zesven = { version = "1.0", default-features = false, features = ["wasm-default"] }
```

The `wasm-default` feature includes compression codecs but excludes parallel processing (WASM is single-threaded).

## Basic Usage

### Reading Archives

```rust
use zesven::wasm::WasmArchive;
use js_sys::Uint8Array;

#[wasm_bindgen]
pub fn extract_archive(data: Uint8Array) -> Result<JsValue, JsError> {
    let archive = WasmArchive::new(data)?;

    let entries: Vec<_> = archive.entries()
        .map(|e| e.path.as_str().to_string())
        .collect();

    Ok(serde_wasm_bindgen::to_value(&entries)?)
}
```

### Creating Archives

```rust
use zesven::wasm::{WasmWriter, WasmWriteOptions};

#[wasm_bindgen]
pub fn create_archive(files: JsValue) -> Result<Vec<u8>, JsError> {
    let mut writer = WasmWriter::new(None)?;

    // Add files from JavaScript
    // ...

    Ok(writer.finish()?)
}
```

## Memory Configuration

Configure memory limits for browser environments:

```rust
use zesven::wasm::{WasmArchive, WasmMemoryConfig};
use js_sys::Uint8Array;

let mut config = WasmMemoryConfig::new();
config.set_max_buffer_size(64 * 1024 * 1024);  // 64 MB limit

// Note: WasmMemoryConfig is used with streaming APIs
let archive = WasmArchive::new(data)?;
```

## JavaScript API

The WASM build exports JavaScript functions:

```javascript
import init, { extract_archive, create_archive } from "zesven";

await init();

// Extract
const file = await fetch("archive.7z");
const data = new Uint8Array(await file.arrayBuffer());
const entries = await extract_archive(data);
console.log("Files:", entries);

// Create
const archive = create_archive([
  { path: "hello.txt", data: new TextEncoder().encode("Hello") },
]);
```

## Streaming in Browsers

Use browser streams:

```rust
use zesven::wasm::{WasmArchive, extract_as_stream};
use web_sys::ReadableStream;
use js_sys::Uint8Array;

#[wasm_bindgen]
pub fn extract_to_stream(data: Uint8Array, entry_path: &str) -> Result<ReadableStream, JsError> {
    let mut archive = WasmArchive::new(data)?;
    Ok(extract_as_stream(&mut archive, entry_path)?)
}
```

## File API Integration

Work with browser File API:

```javascript
const input = document.querySelector('input[type="file"]');
input.addEventListener("change", async (e) => {
  const file = e.target.files[0];
  const data = new Uint8Array(await file.arrayBuffer());
  const entries = await extract_archive(data);
  console.log(entries);
});
```

## Limitations

| Feature              | Browser Support                |
| -------------------- | ------------------------------ |
| Parallel compression | No (WASM is single-threaded)   |
| File system access   | No (use ArrayBuffer)           |
| Large files          | Limited by browser memory      |
| Async API            | Yes (via wasm-bindgen-futures) |

## Building for WASM

```bash
# Install wasm-pack
cargo install wasm-pack

# Build
wasm-pack build --target web --features wasm-default
```

## See Also

- [Feature Flags](../reference/feature-flags) - Available features
- [Memory Management](../streaming/memory-management) - Memory configuration
