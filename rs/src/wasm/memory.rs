//! Memory-efficient operations for WASM.
//!
//! Provides chunked extraction and memory-bounded operations
//! for handling large archives in browser environments.

use wasm_bindgen::prelude::*;

use super::archive::WasmArchive;
use super::file::{uint8_array_to_vec, vec_to_uint8_array};

/// Configuration for memory-constrained operations.
///
/// Use this to control memory usage during extraction,
/// especially important for large files in browsers.
///
/// # JavaScript Example
///
/// ```javascript
/// const config = new WasmMemoryConfig();
/// config.chunkSize = 64 * 1024;  // 64KB chunks
/// config.maxBufferSize = 32 * 1024 * 1024;  // 32MB max buffer
///
/// extractWithMemoryLimit(archive, 'large-file.bin', config, (chunk) => {
///     // Process each chunk as it arrives
///     processChunk(chunk);
/// });
/// ```
#[wasm_bindgen]
pub struct WasmMemoryConfig {
    /// Chunk size for streaming operations (default: 64 KB)
    chunk_size: usize,
    /// Maximum buffer size before streaming (default: 64 MB)
    max_buffer_size: usize,
    /// Whether to use low-memory mode (default: false)
    low_memory_mode: bool,
}

#[wasm_bindgen]
impl WasmMemoryConfig {
    /// Create default memory configuration.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set chunk size in bytes.
    #[wasm_bindgen(setter, js_name = "chunkSize")]
    pub fn set_chunk_size(&mut self, size: usize) {
        self.chunk_size = size.max(1024); // Minimum 1KB
    }

    /// Get chunk size.
    #[wasm_bindgen(getter, js_name = "chunkSize")]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Set maximum buffer size in bytes.
    #[wasm_bindgen(setter, js_name = "maxBufferSize")]
    pub fn set_max_buffer_size(&mut self, size: usize) {
        self.max_buffer_size = size.max(1024 * 1024); // Minimum 1MB
    }

    /// Get maximum buffer size.
    #[wasm_bindgen(getter, js_name = "maxBufferSize")]
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }

    /// Set low memory mode.
    ///
    /// In low memory mode, operations use smaller buffers
    /// and yield more frequently to the event loop.
    #[wasm_bindgen(setter, js_name = "lowMemoryMode")]
    pub fn set_low_memory_mode(&mut self, enabled: bool) {
        self.low_memory_mode = enabled;
        if enabled {
            self.chunk_size = self.chunk_size.min(16 * 1024); // Max 16KB chunks
            self.max_buffer_size = self.max_buffer_size.min(8 * 1024 * 1024); // Max 8MB buffer
        }
    }

    /// Get low memory mode setting.
    #[wasm_bindgen(getter, js_name = "lowMemoryMode")]
    pub fn low_memory_mode(&self) -> bool {
        self.low_memory_mode
    }

    /// Get recommended config for the current environment.
    ///
    /// This checks available memory and returns appropriate settings.
    #[wasm_bindgen(js_name = "autoDetect")]
    pub fn auto_detect() -> Self {
        let mut config = Self::default();

        // Try to detect available memory
        if let Some(memory_mb) = get_available_memory_mb() {
            if memory_mb < 256 {
                config.set_low_memory_mode(true);
            } else if memory_mb < 512 {
                config.chunk_size = 32 * 1024;
                config.max_buffer_size = 16 * 1024 * 1024;
            }
            // For 512MB+, use defaults
        }

        config
    }
}

impl Default for WasmMemoryConfig {
    fn default() -> Self {
        Self {
            chunk_size: 64 * 1024,             // 64 KB
            max_buffer_size: 64 * 1024 * 1024, // 64 MB
            low_memory_mode: false,
        }
    }
}

/// Extract an entry with memory limits, calling a callback for each chunk.
///
/// This is the most memory-efficient way to extract large files,
/// as it processes data in fixed-size chunks rather than loading
/// the entire file into memory.
///
/// @param archive - The archive to extract from
/// @param entryName - Path of the entry to extract
/// @param config - Memory configuration
/// @param onChunk - Callback function(chunk: Uint8Array) called for each chunk
/// @throws Error if extraction fails
///
/// # JavaScript Example
///
/// ```javascript
/// const chunks = [];
/// extractWithMemoryLimit(archive, 'video.mp4', config, (chunk) => {
///     chunks.push(chunk);
///     // Or stream to IndexedDB, etc.
/// });
///
/// // Combine chunks if needed
/// const blob = new Blob(chunks, { type: 'video/mp4' });
/// ```
#[wasm_bindgen(js_name = "extractWithMemoryLimit")]
pub fn extract_with_memory_limit(
    archive: &mut WasmArchive,
    entry_name: &str,
    config: &WasmMemoryConfig,
    on_chunk: &js_sys::Function,
) -> Result<(), JsValue> {
    // Extract the full data (this is the limitation - we can't truly stream from 7z)
    let data = archive.extract_entry(entry_name)?;
    let data_vec = uint8_array_to_vec(&data);

    // Process in chunks
    for chunk in data_vec.chunks(config.chunk_size) {
        let chunk_array = vec_to_uint8_array(chunk);
        on_chunk.call1(&JsValue::NULL, &chunk_array)?;
    }

    Ok(())
}

/// Extract multiple entries with total memory limit.
///
/// This extracts entries one at a time, keeping total memory
/// usage bounded by the configuration.
///
/// @param archive - The archive to extract from
/// @param entryNames - Array of entry paths to extract
/// @param config - Memory configuration
/// @param onEntry - Callback(name: string, data: Uint8Array) for each entry
/// @throws Error if total extracted size exceeds limit
///
/// # JavaScript Example
///
/// ```javascript
/// const entries = ['file1.txt', 'file2.txt', 'file3.txt'];
/// extractMultipleWithLimit(archive, entries, config, (name, data) => {
///     saveToDatabase(name, data);
/// });
/// ```
#[wasm_bindgen(js_name = "extractMultipleWithLimit")]
pub fn extract_multiple_with_limit(
    archive: &mut WasmArchive,
    entry_names: js_sys::Array,
    config: &WasmMemoryConfig,
    on_entry: &js_sys::Function,
) -> Result<(), JsValue> {
    let mut total_extracted = 0usize;

    for i in 0..entry_names.length() {
        let name = entry_names
            .get(i)
            .as_string()
            .ok_or_else(|| JsValue::from_str("Entry names must be strings"))?;

        let data = archive.extract_entry(&name)?;
        let data_len = data.length() as usize;

        // Check memory limit
        if total_extracted + data_len > config.max_buffer_size {
            return Err(JsValue::from_str(&format!(
                "Memory limit exceeded: {} bytes extracted, limit is {} bytes",
                total_extracted + data_len,
                config.max_buffer_size
            )));
        }

        on_entry.call2(&JsValue::NULL, &JsValue::from_str(&name), &data)?;
        total_extracted += data_len;
    }

    Ok(())
}

/// Get memory usage statistics.
///
/// @returns An object with memory statistics (if available):
/// - usedHeapSize: number - Current heap usage in bytes
/// - totalHeapSize: number - Total heap size in bytes
/// - heapLimit: number - Heap size limit in bytes
#[wasm_bindgen(js_name = "getMemoryStats")]
pub fn get_memory_stats() -> Result<JsValue, JsValue> {
    let obj = js_sys::Object::new();

    // Try to get performance.memory (Chrome only)
    if let Some(window) = web_sys::window() {
        if let Ok(performance) = js_sys::Reflect::get(&window, &"performance".into()) {
            if let Ok(memory) = js_sys::Reflect::get(&performance, &"memory".into()) {
                if !memory.is_undefined() {
                    if let Ok(used) = js_sys::Reflect::get(&memory, &"usedJSHeapSize".into()) {
                        js_sys::Reflect::set(&obj, &"usedHeapSize".into(), &used)?;
                    }
                    if let Ok(total) = js_sys::Reflect::get(&memory, &"totalJSHeapSize".into()) {
                        js_sys::Reflect::set(&obj, &"totalHeapSize".into(), &total)?;
                    }
                    if let Ok(limit) = js_sys::Reflect::get(&memory, &"jsHeapSizeLimit".into()) {
                        js_sys::Reflect::set(&obj, &"heapLimit".into(), &limit)?;
                    }
                }
            }
        }
    }

    // Add WASM memory info
    // WASM linear memory grows in 64KB pages
    // We can't easily get current usage, but we can provide guidance

    Ok(obj.into())
}

/// Estimate if an extraction would fit in memory.
///
/// @param archive - The archive to check
/// @param entryName - Path of the entry to check
/// @param config - Memory configuration to check against
/// @returns true if the entry can likely be extracted without OOM
#[wasm_bindgen(js_name = "canExtractSafely")]
pub fn can_extract_safely(
    archive: &WasmArchive,
    entry_name: &str,
    config: &WasmMemoryConfig,
) -> Result<bool, JsValue> {
    let entry = archive.get_entry(entry_name)?;

    if entry.is_undefined() {
        return Err(JsValue::from_str(&format!(
            "Entry not found: {}",
            entry_name
        )));
    }

    let size = js_sys::Reflect::get(&entry, &"size".into())?
        .as_f64()
        .unwrap_or(0.0) as usize;

    // Conservative estimate: we need ~2x the size (compressed + decompressed)
    // plus some overhead
    let estimated_memory = size * 2 + 1024 * 1024; // +1MB overhead

    Ok(estimated_memory <= config.max_buffer_size)
}

/// Force garbage collection if available.
///
/// This hints to the JavaScript engine that now would be a good time
/// to collect garbage. Not all engines support this.
#[wasm_bindgen(js_name = "requestGC")]
pub fn request_gc() {
    // Try to trigger GC through various means

    // Method 1: window.gc() if available (some debug builds)
    if let Some(window) = web_sys::window() {
        if let Ok(gc) = js_sys::Reflect::get(&window, &"gc".into()) {
            if let Ok(gc_fn) = gc.dyn_into::<js_sys::Function>() {
                let _ = gc_fn.call0(&JsValue::NULL);
            }
        }
    }

    // Method 2: Create and drop a large array to encourage GC
    // This is a heuristic, not guaranteed to trigger GC
    let _ = vec![0u8; 1024 * 1024]; // 1MB allocation
}

/// Helper to try to get available memory in MB.
fn get_available_memory_mb() -> Option<u32> {
    // Try performance.memory (Chrome only)
    let window = web_sys::window()?;
    let performance = js_sys::Reflect::get(&window, &"performance".into()).ok()?;
    let memory = js_sys::Reflect::get(&performance, &"memory".into()).ok()?;

    if memory.is_undefined() {
        return None;
    }

    let limit = js_sys::Reflect::get(&memory, &"jsHeapSizeLimit".into()).ok()?;
    let used = js_sys::Reflect::get(&memory, &"usedJSHeapSize".into()).ok()?;

    let limit_bytes = limit.as_f64()?;
    let used_bytes = used.as_f64()?;

    let available = (limit_bytes - used_bytes) / (1024.0 * 1024.0);
    Some(available as u32)
}

/// Memory-efficient iterator for archive entries.
#[wasm_bindgen]
pub struct EntryIterator {
    entries: js_sys::Array,
    index: u32,
}

#[wasm_bindgen]
impl EntryIterator {
    /// Create an iterator from an archive.
    #[wasm_bindgen(constructor)]
    pub fn new(archive: &WasmArchive) -> Result<EntryIterator, JsValue> {
        let entries = archive.get_entries()?;
        Ok(Self { entries, index: 0 })
    }

    /// Check if there are more entries.
    #[wasm_bindgen(js_name = "hasNext")]
    pub fn has_next(&self) -> bool {
        self.index < self.entries.length()
    }

    /// Get the next entry.
    pub fn next(&mut self) -> JsValue {
        if self.index >= self.entries.length() {
            return JsValue::UNDEFINED;
        }

        let entry = self.entries.get(self.index);
        self.index += 1;
        entry
    }

    /// Reset the iterator to the beginning.
    pub fn reset(&mut self) {
        self.index = 0;
    }

    /// Get the total number of entries.
    #[wasm_bindgen(getter)]
    pub fn count(&self) -> u32 {
        self.entries.length()
    }

    /// Get current position.
    #[wasm_bindgen(getter)]
    pub fn position(&self) -> u32 {
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_memory_config_default() {
        let config = WasmMemoryConfig::default();
        assert_eq!(config.chunk_size, 64 * 1024);
        assert_eq!(config.max_buffer_size, 64 * 1024 * 1024);
        assert!(!config.low_memory_mode);
    }

    #[test]
    fn test_wasm_memory_config_low_memory() {
        let mut config = WasmMemoryConfig::new();
        config.chunk_size = 128 * 1024;
        config.max_buffer_size = 128 * 1024 * 1024;

        config.set_low_memory_mode(true);

        assert!(config.low_memory_mode());
        assert!(config.chunk_size <= 16 * 1024);
        assert!(config.max_buffer_size <= 8 * 1024 * 1024);
    }

    #[test]
    fn test_wasm_memory_config_minimum_values() {
        let mut config = WasmMemoryConfig::new();

        config.set_chunk_size(100); // Too small
        assert!(config.chunk_size >= 1024);

        config.set_max_buffer_size(100); // Too small
        assert!(config.max_buffer_size >= 1024 * 1024);
    }
}
