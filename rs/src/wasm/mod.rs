//! WASM/Browser support for zesven
//!
//! This module provides WebAssembly bindings for the zesven library,
//! enabling browser-based archive operations through JavaScript.
//!
//! # Features
//!
//! - **WasmArchive**: JavaScript-exposed archive reader API
//! - **WasmWriter**: JavaScript-exposed archive writer API
//! - **Web Streams API**: ReadableStream/WritableStream integration
//! - **Async/Promise API**: JavaScript Promise-based async operations
//! - **TypeScript definitions**: Full TypeScript type support
//!
//! # Example (JavaScript)
//!
//! ```javascript
//! import { WasmArchive, WasmWriter } from 'zesven';
//!
//! // Reading an archive
//! const archive = new WasmArchive(uint8ArrayData);
//! const entries = archive.getEntries();
//! const content = archive.extractEntry('file.txt');
//!
//! // Creating an archive
//! const writer = new WasmWriter();
//! writer.addFile('hello.txt', new TextEncoder().encode('Hello, World!'));
//! const archiveData = writer.finish();
//! ```

mod archive;
mod async_api;
mod file;
mod memory;
mod streams;
mod writer;

// Re-export main types for wasm-bindgen
pub use archive::WasmArchive;
pub use async_api::{extract_entry_async, open_archive_async};
pub use memory::WasmMemoryConfig;
pub use streams::{extract_as_stream, open_from_stream};
pub use writer::{WasmWriteOptions, WasmWriter};

use wasm_bindgen::prelude::*;

/// Initialize the WASM module (called automatically by wasm-bindgen)
#[wasm_bindgen(start)]
pub fn init() {
    // Panic hook for better error messages would go here
    // Currently no-op as console_error_panic_hook is not included
}

/// Get the library version
#[wasm_bindgen(js_name = "getVersion")]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check if a compression method is supported
#[wasm_bindgen(js_name = "isMethodSupported")]
pub fn is_method_supported(method: &str) -> bool {
    match method.to_lowercase().as_str() {
        "copy" => true,
        #[cfg(feature = "lzma")]
        "lzma" => true,
        #[cfg(not(feature = "lzma"))]
        "lzma" => false,
        #[cfg(feature = "lzma2")]
        "lzma2" => true,
        #[cfg(not(feature = "lzma2"))]
        "lzma2" => false,
        #[cfg(feature = "deflate")]
        "deflate" => true,
        #[cfg(not(feature = "deflate"))]
        "deflate" => false,
        #[cfg(feature = "bzip2")]
        "bzip2" => true,
        #[cfg(not(feature = "bzip2"))]
        "bzip2" => false,
        #[cfg(feature = "ppmd")]
        "ppmd" => true,
        #[cfg(not(feature = "ppmd"))]
        "ppmd" => false,
        #[cfg(feature = "aes")]
        "aes" | "aes256" => true,
        #[cfg(not(feature = "aes"))]
        "aes" | "aes256" => false,
        _ => false,
    }
}

/// Get list of supported compression methods
#[wasm_bindgen(js_name = "getSupportedMethods")]
pub fn get_supported_methods() -> js_sys::Array {
    let methods = js_sys::Array::new();

    methods.push(&JsValue::from_str("copy"));

    #[cfg(feature = "lzma")]
    methods.push(&JsValue::from_str("lzma"));

    #[cfg(feature = "lzma2")]
    methods.push(&JsValue::from_str("lzma2"));

    #[cfg(feature = "deflate")]
    methods.push(&JsValue::from_str("deflate"));

    #[cfg(feature = "bzip2")]
    methods.push(&JsValue::from_str("bzip2"));

    #[cfg(feature = "ppmd")]
    methods.push(&JsValue::from_str("ppmd"));

    methods
}

/// Check if encryption is supported
#[wasm_bindgen(js_name = "isEncryptionSupported")]
pub fn is_encryption_supported() -> bool {
    #[cfg(feature = "aes")]
    {
        true
    }
    #[cfg(not(feature = "aes"))]
    {
        false
    }
}
