//! JavaScript-exposed Writer API for WASM.
//!
//! Provides the `WasmWriter` struct that wraps the native Writer type
//! and exposes it to JavaScript via wasm-bindgen.

use js_sys::Uint8Array;
use std::io::Cursor;
use wasm_bindgen::prelude::*;

use super::file::{uint8_array_to_vec, vec_to_uint8_array};
use crate::ArchivePath;
use crate::codec::CodecMethod;
use crate::write::{EntryMeta, WriteOptions, Writer};

#[cfg(feature = "aes")]
use crate::Password;

/// Write options for WASM archive creation.
///
/// # JavaScript Example
///
/// ```javascript
/// const options = new WasmWriteOptions();
/// options.solid = true;
/// options.method = 'lzma2';
/// options.level = 7;
/// options.password = 'secret';
///
/// const writer = new WasmWriter(options);
/// ```
#[wasm_bindgen]
pub struct WasmWriteOptions {
    /// Enable solid compression
    solid: bool,
    /// Compression method name
    method: String,
    /// Compression level (0-9)
    level: u8,
    /// Password for encryption (optional)
    password: Option<String>,
    /// Encrypt file names in header
    encrypt_header: bool,
}

#[wasm_bindgen]
impl WasmWriteOptions {
    /// Create default write options.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set solid compression mode.
    #[wasm_bindgen(setter)]
    pub fn set_solid(&mut self, solid: bool) {
        self.solid = solid;
    }

    /// Get solid compression mode.
    #[wasm_bindgen(getter)]
    pub fn solid(&self) -> bool {
        self.solid
    }

    /// Set compression method.
    ///
    /// Supported methods: "copy", "lzma", "lzma2", "deflate", "bzip2"
    #[wasm_bindgen(setter)]
    pub fn set_method(&mut self, method: String) {
        self.method = method;
    }

    /// Get compression method.
    #[wasm_bindgen(getter)]
    pub fn method(&self) -> String {
        self.method.clone()
    }

    /// Set compression level (0-9).
    #[wasm_bindgen(setter)]
    pub fn set_level(&mut self, level: u8) {
        self.level = level.min(9);
    }

    /// Get compression level.
    #[wasm_bindgen(getter)]
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Set password for encryption.
    #[wasm_bindgen(setter)]
    pub fn set_password(&mut self, password: Option<String>) {
        self.password = password;
    }

    /// Get password.
    #[wasm_bindgen(getter)]
    pub fn password(&self) -> Option<String> {
        self.password.clone()
    }

    /// Set whether to encrypt file names in header.
    #[wasm_bindgen(setter, js_name = "encryptHeader")]
    pub fn set_encrypt_header(&mut self, encrypt: bool) {
        self.encrypt_header = encrypt;
    }

    /// Get encrypt header setting.
    #[wasm_bindgen(getter, js_name = "encryptHeader")]
    pub fn encrypt_header(&self) -> bool {
        self.encrypt_header
    }
}

impl Default for WasmWriteOptions {
    fn default() -> Self {
        Self {
            solid: false,
            method: "lzma2".to_string(),
            level: 5,
            password: None,
            encrypt_header: false,
        }
    }
}

impl WasmWriteOptions {
    /// Convert to internal WriteOptions.
    fn to_write_options(&self) -> Result<WriteOptions, JsValue> {
        let method = match self.method.to_lowercase().as_str() {
            "copy" => CodecMethod::Copy,
            "lzma" => {
                #[cfg(feature = "lzma")]
                {
                    CodecMethod::Lzma
                }
                #[cfg(not(feature = "lzma"))]
                return Err(JsValue::from_str("LZMA support not compiled"));
            }
            "lzma2" => {
                #[cfg(feature = "lzma2")]
                {
                    CodecMethod::Lzma2
                }
                #[cfg(not(feature = "lzma2"))]
                return Err(JsValue::from_str("LZMA2 support not compiled"));
            }
            "deflate" => {
                #[cfg(feature = "deflate")]
                {
                    CodecMethod::Deflate
                }
                #[cfg(not(feature = "deflate"))]
                return Err(JsValue::from_str("Deflate support not compiled"));
            }
            "bzip2" => {
                #[cfg(feature = "bzip2")]
                {
                    CodecMethod::BZip2
                }
                #[cfg(not(feature = "bzip2"))]
                return Err(JsValue::from_str("BZip2 support not compiled"));
            }
            other => return Err(JsValue::from_str(&format!("Unknown method: {}", other))),
        };

        let mut opts = WriteOptions::new()
            .method(method)
            .level_clamped(self.level as u32);

        if self.solid {
            opts = opts.solid();
        }

        // Configure encryption if password is set
        #[cfg(feature = "aes")]
        if let Some(ref password) = self.password {
            opts = opts.password(Password::new(password));
        }

        Ok(opts)
    }
}

/// Pending entry for the writer.
struct PendingWasmEntry {
    name: String,
    data: Vec<u8>,
    is_directory: bool,
}

/// A 7z archive writer exposed to JavaScript.
///
/// # JavaScript Example
///
/// ```javascript
/// // Create a new writer with options
/// const options = new WasmWriteOptions();
/// options.method = 'lzma2';
/// options.level = 7;
///
/// const writer = new WasmWriter(options);
///
/// // Add files
/// writer.addFile('hello.txt', new TextEncoder().encode('Hello, World!'));
/// writer.addFile('data.json', new TextEncoder().encode('{"key": "value"}'));
///
/// // Add a directory
/// writer.addDirectory('subdir');
///
/// // Finalize and get the archive data
/// const archiveData = writer.finish();
///
/// // Download or save the archive
/// downloadBlob(archiveData, 'archive.7z');
/// ```
#[wasm_bindgen]
pub struct WasmWriter {
    /// Pending entries (collected before finish)
    entries: Vec<PendingWasmEntry>,
    /// Write options
    options: WasmWriteOptions,
    /// Whether the writer has been finished
    finished: bool,
}

#[wasm_bindgen]
impl WasmWriter {
    /// Create a new archive writer.
    ///
    /// @param options - Optional write options
    #[wasm_bindgen(constructor)]
    pub fn new(options: Option<WasmWriteOptions>) -> Self {
        Self {
            entries: Vec::new(),
            options: options.unwrap_or_default(),
            finished: false,
        }
    }

    /// Add a file from a Uint8Array.
    ///
    /// @param name - Path within the archive
    /// @param data - File content as Uint8Array
    /// @throws Error if the writer has already been finished
    #[wasm_bindgen(js_name = "addFile")]
    pub fn add_file(&mut self, name: &str, data: Uint8Array) -> Result<(), JsValue> {
        self.ensure_not_finished()?;

        let buffer = uint8_array_to_vec(&data);
        self.entries.push(PendingWasmEntry {
            name: name.to_string(),
            data: buffer,
            is_directory: false,
        });

        Ok(())
    }

    /// Add a file from a string.
    ///
    /// @param name - Path within the archive
    /// @param content - File content as string (will be encoded as UTF-8)
    /// @throws Error if the writer has already been finished
    #[wasm_bindgen(js_name = "addFileFromString")]
    pub fn add_file_from_string(&mut self, name: &str, content: &str) -> Result<(), JsValue> {
        self.ensure_not_finished()?;

        self.entries.push(PendingWasmEntry {
            name: name.to_string(),
            data: content.as_bytes().to_vec(),
            is_directory: false,
        });

        Ok(())
    }

    /// Add an empty directory.
    ///
    /// @param name - Directory path within the archive
    /// @throws Error if the writer has already been finished
    #[wasm_bindgen(js_name = "addDirectory")]
    pub fn add_directory(&mut self, name: &str) -> Result<(), JsValue> {
        self.ensure_not_finished()?;

        let dir_name = if name.ends_with('/') {
            name.to_string()
        } else {
            format!("{}/", name)
        };

        self.entries.push(PendingWasmEntry {
            name: dir_name,
            data: Vec::new(),
            is_directory: true,
        });

        Ok(())
    }

    /// Get the number of pending entries.
    #[wasm_bindgen(getter, js_name = "entryCount")]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if the writer has been finished.
    #[wasm_bindgen(getter, js_name = "isFinished")]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Finalize the archive and return the data.
    ///
    /// @returns The archive data as Uint8Array
    /// @throws Error if the archive creation fails
    #[wasm_bindgen]
    pub fn finish(&mut self) -> Result<Uint8Array, JsValue> {
        self.ensure_not_finished()?;
        self.finished = true;

        let write_options = self.options.to_write_options()?;
        let buffer = Cursor::new(Vec::new());

        let mut writer = Writer::create(buffer).map_err(|e| JsValue::from_str(&e.to_string()))?;
        writer = writer.options(write_options);

        // Add all entries
        for entry in &self.entries {
            let archive_path =
                ArchivePath::new(&entry.name).map_err(|e| JsValue::from_str(&e.to_string()))?;

            if entry.is_directory {
                writer
                    .add_directory(archive_path, EntryMeta::directory())
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            } else {
                writer
                    .add_bytes(archive_path, &entry.data)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
        }

        // Use finish_into_inner to get the buffer back
        let (_result, cursor) = writer
            .finish_into_inner()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let output = cursor.into_inner();
        Ok(vec_to_uint8_array(&output))
    }

    /// Cancel the writer and discard all pending entries.
    #[wasm_bindgen]
    pub fn cancel(&mut self) {
        self.entries.clear();
        self.finished = true;
    }

    /// Remove a pending entry by name.
    ///
    /// @param name - The path of the entry to remove
    /// @returns true if an entry was removed
    #[wasm_bindgen(js_name = "removeEntry")]
    pub fn remove_entry(&mut self, name: &str) -> Result<bool, JsValue> {
        self.ensure_not_finished()?;

        let len_before = self.entries.len();
        self.entries.retain(|e| e.name != name);
        Ok(self.entries.len() < len_before)
    }

    /// Get list of pending entry names.
    #[wasm_bindgen(js_name = "getEntryNames")]
    pub fn get_entry_names(&self) -> js_sys::Array {
        let arr = js_sys::Array::new();
        for entry in &self.entries {
            arr.push(&JsValue::from_str(&entry.name));
        }
        arr
    }

    fn ensure_not_finished(&self) -> Result<(), JsValue> {
        if self.finished {
            return Err(JsValue::from_str("Writer has already been finished"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_write_options_default() {
        let opts = WasmWriteOptions::default();
        assert!(!opts.solid);
        assert_eq!(opts.method, "lzma2");
        assert_eq!(opts.level, 5);
        assert!(opts.password.is_none());
    }

    #[test]
    fn test_wasm_write_options_setters() {
        let mut opts = WasmWriteOptions::new();
        opts.set_solid(true);
        opts.set_method("deflate".to_string());
        opts.set_level(9);

        assert!(opts.solid());
        assert_eq!(opts.method(), "deflate");
        assert_eq!(opts.level(), 9);
    }
}
