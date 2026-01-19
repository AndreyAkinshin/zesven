//! JavaScript-exposed Archive API for WASM.
//!
//! Provides the `WasmArchive` struct that wraps the native Archive type
//! and exposes it to JavaScript via wasm-bindgen.

use js_sys::{Array, Map, Object, Reflect, Uint8Array};
use std::io::Cursor;
use wasm_bindgen::prelude::*;

use super::file::{uint8_array_to_vec, vec_to_uint8_array};
use crate::read::{Archive, Entry};

#[cfg(feature = "aes")]
use crate::Password;

/// A 7z archive reader exposed to JavaScript.
///
/// # JavaScript Example
///
/// ```javascript
/// // Open an archive from Uint8Array data
/// const archive = new WasmArchive(archiveData);
///
/// // Get archive info
/// const info = archive.getInfo();
/// console.log(`Entries: ${info.entryCount}`);
///
/// // List all entries
/// for (const entry of archive.getEntries()) {
///     console.log(`${entry.name}: ${entry.size} bytes`);
/// }
///
/// // Extract a single file
/// const content = archive.extractEntry('path/to/file.txt');
///
/// // Extract all files
/// const allFiles = archive.extractAll();
/// ```
#[wasm_bindgen]
pub struct WasmArchive {
    inner: Archive<Cursor<Vec<u8>>>,
    /// Keep the data buffer for potential re-reads
    #[allow(dead_code)] // Data buffer owned to keep Cursor valid
    data: Vec<u8>,
}

#[wasm_bindgen]
impl WasmArchive {
    /// Open an archive from a Uint8Array.
    ///
    /// @param data - Archive data as Uint8Array
    /// @returns A new WasmArchive instance
    /// @throws Error if the archive is invalid
    #[wasm_bindgen(constructor)]
    pub fn new(data: Uint8Array) -> Result<WasmArchive, JsValue> {
        let buffer = uint8_array_to_vec(&data);
        let cursor = Cursor::new(buffer.clone());

        let archive = Archive::open(cursor).map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(Self {
            inner: archive,
            data: buffer,
        })
    }

    /// Open an encrypted archive from a Uint8Array with a password.
    ///
    /// @param data - Archive data as Uint8Array
    /// @param password - Password for decryption
    /// @returns A new WasmArchive instance
    /// @throws Error if the archive is invalid or password is incorrect
    #[cfg(feature = "aes")]
    #[wasm_bindgen(js_name = "openWithPassword")]
    pub fn open_with_password(data: Uint8Array, password: &str) -> Result<WasmArchive, JsValue> {
        let buffer = uint8_array_to_vec(&data);
        let cursor = Cursor::new(buffer.clone());

        let archive = Archive::open_with_password(cursor, Password::new(password))
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(Self {
            inner: archive,
            data: buffer,
        })
    }

    /// Get archive info.
    ///
    /// @returns An object with archive information:
    /// - entryCount: number - Number of entries
    /// - totalSize: number - Total uncompressed size
    /// - packedSize: number - Total compressed size
    /// - isSolid: boolean - Whether archive uses solid compression
    /// - hasEncryptedEntries: boolean - Whether any entries are encrypted
    /// - folderCount: number - Number of folders
    #[wasm_bindgen(js_name = "getInfo")]
    pub fn get_info(&self) -> Result<JsValue, JsValue> {
        let info = self.inner.info();

        let obj = Object::new();

        Reflect::set(
            &obj,
            &"entryCount".into(),
            &(info.entry_count as f64).into(),
        )?;
        Reflect::set(&obj, &"totalSize".into(), &(info.total_size as f64).into())?;
        Reflect::set(
            &obj,
            &"packedSize".into(),
            &(info.packed_size as f64).into(),
        )?;
        Reflect::set(&obj, &"isSolid".into(), &info.is_solid.into())?;
        Reflect::set(
            &obj,
            &"hasEncryptedEntries".into(),
            &info.has_encrypted_entries.into(),
        )?;
        Reflect::set(
            &obj,
            &"hasEncryptedHeader".into(),
            &info.has_encrypted_header.into(),
        )?;
        Reflect::set(
            &obj,
            &"folderCount".into(),
            &(info.folder_count as f64).into(),
        )?;

        // Add compression methods as array
        let methods = Array::new();
        for method in &info.compression_methods {
            methods.push(&JsValue::from_str(&format!("{:?}", method)));
        }
        Reflect::set(&obj, &"compressionMethods".into(), &methods)?;

        Ok(obj.into())
    }

    /// Get all entries in the archive.
    ///
    /// @returns An array of entry objects, each containing:
    /// - name: string - Full path within the archive
    /// - size: number - Uncompressed size in bytes
    /// - isDirectory: boolean - Whether this is a directory
    /// - crc: number | undefined - CRC32 checksum
    /// - mtime: number | undefined - Modification time (Windows FILETIME)
    /// - ctime: number | undefined - Creation time (Windows FILETIME)
    /// - atime: number | undefined - Access time (Windows FILETIME)
    /// - isEncrypted: boolean - Whether this entry is encrypted
    #[wasm_bindgen(js_name = "getEntries")]
    pub fn get_entries(&self) -> Result<Array, JsValue> {
        let entries = self.inner.entries();
        let arr = Array::new();

        for entry in entries {
            let obj = entry_to_js_object(entry)?;
            arr.push(&obj);
        }

        Ok(arr)
    }

    /// Get the number of entries in the archive.
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.inner.len()
    }

    /// Check if the archive is empty.
    #[wasm_bindgen(js_name = "isEmpty")]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Find an entry by path.
    ///
    /// @param path - The path to search for
    /// @returns The entry object or undefined if not found
    #[wasm_bindgen(js_name = "getEntry")]
    pub fn get_entry(&self, path: &str) -> Result<JsValue, JsValue> {
        match self.inner.entry(path) {
            Some(entry) => Ok(entry_to_js_object(entry)?),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Extract a single entry by name.
    ///
    /// @param name - The path of the entry to extract
    /// @returns The file content as Uint8Array
    /// @throws Error if the entry is not found or extraction fails
    #[wasm_bindgen(js_name = "extractEntry")]
    pub fn extract_entry(&mut self, name: &str) -> Result<Uint8Array, JsValue> {
        let entry = self
            .inner
            .entry(name)
            .ok_or_else(|| JsValue::from_str(&format!("Entry not found: {}", name)))?;

        if entry.is_directory {
            return Err(JsValue::from_str("Cannot extract directory"));
        }

        // Extract to memory
        let output = self
            .inner
            .extract_to_vec(name)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(vec_to_uint8_array(&output))
    }

    /// Extract all file entries.
    ///
    /// @returns A Map where keys are entry paths and values are Uint8Array contents
    /// @throws Error if extraction fails
    #[wasm_bindgen(js_name = "extractAll")]
    pub fn extract_all(&mut self) -> Result<Map, JsValue> {
        let map = Map::new();

        let entry_names: Vec<(String, bool)> = self
            .inner
            .entries()
            .iter()
            .map(|e| (e.path.as_str().to_string(), e.is_directory))
            .collect();

        for (name, is_dir) in entry_names {
            if !is_dir {
                let data = self.extract_entry(&name)?;
                map.set(&JsValue::from_str(&name), &data);
            }
        }

        Ok(map)
    }

    /// Test archive integrity.
    ///
    /// @returns true if the archive passes integrity checks
    /// @throws Error if the test fails
    #[wasm_bindgen]
    pub fn test(&mut self) -> Result<bool, JsValue> {
        let result = self
            .inner
            .test((), &crate::read::TestOptions::default())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(result.entries_failed == 0)
    }

    /// Get entry names matching a glob pattern.
    ///
    /// @param pattern - A simple glob pattern (supports * and ?)
    /// @returns Array of matching entry paths
    #[wasm_bindgen(js_name = "findEntries")]
    pub fn find_entries(&self, pattern: &str) -> Array {
        let arr = Array::new();

        for entry in self.inner.entries() {
            if glob_match(pattern, entry.path.as_str()) {
                arr.push(&JsValue::from_str(entry.path.as_str()));
            }
        }

        arr
    }

    /// Get entries in a specific directory.
    ///
    /// @param dir - The directory path (empty string for root)
    /// @param recursive - Whether to include subdirectories
    /// @returns Array of entry objects in the directory
    #[wasm_bindgen(js_name = "getEntriesInDirectory")]
    pub fn get_entries_in_directory(&self, dir: &str, recursive: bool) -> Result<Array, JsValue> {
        let arr = Array::new();
        let dir_prefix = if dir.is_empty() || dir.ends_with('/') {
            dir.to_string()
        } else {
            format!("{}/", dir)
        };

        for entry in self.inner.entries() {
            let path = entry.path.as_str();

            if dir.is_empty() {
                // Root directory
                if recursive || !path.contains('/') {
                    arr.push(&entry_to_js_object(entry)?);
                }
            } else if path.starts_with(&dir_prefix) {
                let relative = &path[dir_prefix.len()..];
                if recursive || !relative.contains('/') {
                    arr.push(&entry_to_js_object(entry)?);
                }
            }
        }

        Ok(arr)
    }
}

/// Convert an Entry to a JavaScript object.
fn entry_to_js_object(entry: &Entry) -> Result<JsValue, JsValue> {
    let obj = Object::new();

    Reflect::set(&obj, &"name".into(), &entry.path.as_str().into())?;
    Reflect::set(&obj, &"size".into(), &(entry.size as f64).into())?;
    Reflect::set(&obj, &"isDirectory".into(), &entry.is_directory.into())?;
    Reflect::set(&obj, &"isEncrypted".into(), &entry.is_encrypted.into())?;

    if let Some(crc) = entry.crc32 {
        Reflect::set(&obj, &"crc32".into(), &(crc as f64).into())?;
    }

    if let Some(mtime) = entry.modification_time {
        Reflect::set(&obj, &"mtime".into(), &(mtime as f64).into())?;
    }

    if let Some(ctime) = entry.creation_time {
        Reflect::set(&obj, &"ctime".into(), &(ctime as f64).into())?;
    }

    if let Some(atime) = entry.access_time {
        Reflect::set(&obj, &"atime".into(), &(atime as f64).into())?;
    }

    if let Some(attrs) = entry.attributes {
        Reflect::set(&obj, &"attributes".into(), &(attrs as f64).into())?;
    }

    Ok(obj.into())
}

/// Simple glob pattern matching (supports * and ?).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    glob_match_recursive(&pattern_chars, &text_chars)
}

fn glob_match_recursive(pattern: &[char], text: &[char]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // * matches zero or more characters
            glob_match_recursive(&pattern[1..], text)
                || (!text.is_empty() && glob_match_recursive(pattern, &text[1..]))
        }
        (Some('?'), Some(_)) => {
            // ? matches exactly one character
            glob_match_recursive(&pattern[1..], &text[1..])
        }
        (Some(p), Some(t)) if *p == *t => glob_match_recursive(&pattern[1..], &text[1..]),
        (Some(_), Some(_)) => false,
        (Some(_), None) => {
            // Pattern remaining but no text - only match if all remaining pattern is *
            pattern.iter().all(|c| *c == '*')
        }
        (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.txt", "file.txt"));
        assert!(glob_match("*.txt", "path/to/file.txt"));
        assert!(!glob_match("*.txt", "file.rs"));

        assert!(glob_match("file?", "file1"));
        assert!(glob_match("file?", "filea"));
        assert!(!glob_match("file?", "file"));
        assert!(!glob_match("file?", "file12"));

        assert!(glob_match("*", "anything"));
        assert!(glob_match("**", "anything"));
        assert!(glob_match("a*b", "ab"));
        assert!(glob_match("a*b", "aXXXb"));

        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.rs.bak"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("file.txt", "file.txt"));
        assert!(!glob_match("file.txt", "other.txt"));
    }
}
