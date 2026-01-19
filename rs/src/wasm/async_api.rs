//! Async/Promise-based API for WASM.
//!
//! Provides JavaScript Promise-based async operations for archive handling.
//! These functions allow non-blocking archive operations in the browser.

use js_sys::{Array, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use super::archive::WasmArchive;
use super::writer::{WasmWriteOptions, WasmWriter};

/// Open an archive asynchronously.
///
/// This wraps the synchronous open operation in a Promise, allowing
/// the browser's event loop to remain responsive during parsing.
///
/// @param data - Archive data as Uint8Array
/// @param password - Optional password for encrypted archives
/// @returns A Promise that resolves to a WasmArchive
///
/// # JavaScript Example
///
/// ```javascript
/// const archive = await openArchiveAsync(archiveData);
/// console.log(`Opened archive with ${archive.length} entries`);
/// ```
#[wasm_bindgen(js_name = "openArchiveAsync")]
pub fn open_archive_async(data: Uint8Array, password: Option<String>) -> Promise {
    future_to_promise(async move {
        // Yield to event loop
        yield_to_event_loop().await;

        let archive = if let Some(pwd) = password {
            #[cfg(feature = "aes")]
            {
                WasmArchive::open_with_password(data, &pwd)?
            }
            #[cfg(not(feature = "aes"))]
            {
                let _ = pwd;
                return Err(JsValue::from_str("AES encryption support not compiled"));
            }
        } else {
            WasmArchive::new(data)?
        };

        Ok(JsValue::from(archive))
    })
}

/// Extract a single entry asynchronously.
///
/// @param archive - The archive to extract from
/// @param name - Path of the entry to extract
/// @returns A Promise that resolves to Uint8Array content
///
/// # JavaScript Example
///
/// ```javascript
/// const content = await extractEntryAsync(archive, 'file.txt');
/// const text = new TextDecoder().decode(content);
/// ```
#[wasm_bindgen(js_name = "extractEntryAsync")]
pub fn extract_entry_async(archive: &mut WasmArchive, name: String) -> Promise {
    // Note: We need to handle archive borrowing carefully
    // For now, extract synchronously but wrap in promise
    let result = archive.extract_entry(&name);

    future_to_promise(async move {
        yield_to_event_loop().await;

        match result {
            Ok(data) => Ok(data.into()),
            Err(e) => Err(e),
        }
    })
}

/// Extract all entries asynchronously.
///
/// This extracts all non-directory entries from the archive.
///
/// @param archive - The archive to extract from
/// @returns A Promise that resolves to a Map of path -> Uint8Array
///
/// # JavaScript Example
///
/// ```javascript
/// const files = await extractAllAsync(archive);
/// for (const [path, content] of files) {
///     console.log(`${path}: ${content.length} bytes`);
/// }
/// ```
#[wasm_bindgen(js_name = "extractAllAsync")]
pub fn extract_all_async(archive: &mut WasmArchive) -> Promise {
    let result = archive.extract_all();

    future_to_promise(async move {
        yield_to_event_loop().await;
        result.map(JsValue::from)
    })
}

/// Test archive integrity asynchronously.
///
/// @param archive - The archive to test
/// @returns A Promise that resolves to true if the archive is valid
#[wasm_bindgen(js_name = "testArchiveAsync")]
pub fn test_archive_async(archive: &mut WasmArchive) -> Promise {
    let result = archive.test();

    future_to_promise(async move {
        yield_to_event_loop().await;
        result.map(JsValue::from)
    })
}

/// Create an archive asynchronously from entries.
///
/// @param entries - Array of {name: string, data: Uint8Array} objects
/// @param options - Optional write options
/// @returns A Promise that resolves to the archive data as Uint8Array
///
/// # JavaScript Example
///
/// ```javascript
/// const entries = [
///     { name: 'file.txt', data: new TextEncoder().encode('Hello') },
///     { name: 'data.json', data: new TextEncoder().encode('{}') }
/// ];
/// const archive = await createArchiveAsync(entries);
/// ```
#[wasm_bindgen(js_name = "createArchiveAsync")]
pub fn create_archive_async(entries: Array, options: Option<WasmWriteOptions>) -> Promise {
    future_to_promise(async move {
        yield_to_event_loop().await;

        let mut writer = WasmWriter::new(options);

        for i in 0..entries.length() {
            yield_to_event_loop().await;

            let entry = entries.get(i);
            let name = js_sys::Reflect::get(&entry, &"name".into())?
                .as_string()
                .ok_or_else(|| JsValue::from_str("Entry missing 'name' property"))?;

            let data_val = js_sys::Reflect::get(&entry, &"data".into())?;
            let data: Uint8Array = data_val
                .dyn_into()
                .map_err(|_| JsValue::from_str("Entry 'data' must be Uint8Array"))?;

            writer.add_file(&name, data)?;
        }

        let result = writer.finish()?;
        Ok(result.into())
    })
}

/// Process archive entries asynchronously with a callback.
///
/// @param archive - The archive to process
/// @param callback - A function(entry) => Promise<void> called for each entry
/// @returns A Promise that resolves when all entries are processed
///
/// # JavaScript Example
///
/// ```javascript
/// await processEntriesAsync(archive, async (entry) => {
///     console.log(`Processing: ${entry.name}`);
///     await doSomethingWith(entry);
/// });
/// ```
#[wasm_bindgen(js_name = "processEntriesAsync")]
pub fn process_entries_async(archive: &WasmArchive, callback: &js_sys::Function) -> Promise {
    let entries = match archive.get_entries() {
        Ok(e) => e,
        Err(e) => return Promise::reject(&e),
    };

    let callback = callback.clone();

    future_to_promise(async move {
        for i in 0..entries.length() {
            let entry = entries.get(i);

            // Call the callback and await if it returns a promise
            let result = callback.call1(&JsValue::NULL, &entry)?;

            if let Ok(promise) = result.dyn_into::<Promise>() {
                wasm_bindgen_futures::JsFuture::from(promise).await?;
            }

            // Yield periodically
            if i % 10 == 0 {
                yield_to_event_loop().await;
            }
        }

        Ok(JsValue::UNDEFINED)
    })
}

/// Batch extract entries asynchronously with progress callback.
///
/// @param archive - The archive to extract from
/// @param entryNames - Array of entry paths to extract
/// @param onProgress - Optional callback(extracted, total) called after each entry
/// @returns A Promise that resolves to a Map of path -> Uint8Array
///
/// # JavaScript Example
///
/// ```javascript
/// const names = ['file1.txt', 'file2.txt', 'data/file3.json'];
/// const files = await batchExtractAsync(archive, names, (done, total) => {
///     console.log(`Progress: ${done}/${total}`);
/// });
/// ```
#[wasm_bindgen(js_name = "batchExtractAsync")]
pub fn batch_extract_async(
    archive: &mut WasmArchive,
    entry_names: Array,
    on_progress: Option<js_sys::Function>,
) -> Promise {
    let total = entry_names.length();
    let names: Vec<String> = (0..total)
        .filter_map(|i| entry_names.get(i).as_string())
        .collect();

    // Pre-extract all entries synchronously but wrap in promise
    let mut results: Vec<(String, Result<Uint8Array, JsValue>)> = Vec::new();
    for name in &names {
        let result = archive.extract_entry(name);
        results.push((name.clone(), result));
    }

    future_to_promise(async move {
        let map = js_sys::Map::new();
        let total = results.len() as u32;

        for (i, (name, result)) in results.into_iter().enumerate() {
            yield_to_event_loop().await;

            match result {
                Ok(data) => {
                    map.set(&JsValue::from_str(&name), &data);
                }
                Err(e) => {
                    // Log error but continue with other entries
                    web_sys::console::warn_2(
                        &JsValue::from_str(&format!("Failed to extract {}: ", name)),
                        &e,
                    );
                }
            }

            // Report progress
            if let Some(ref callback) = on_progress {
                let _ = callback.call2(
                    &JsValue::NULL,
                    &JsValue::from((i + 1) as u32),
                    &JsValue::from(total),
                );
            }
        }

        Ok(JsValue::from(map))
    })
}

/// Helper to yield to the browser's event loop.
///
/// This allows long-running operations to not block the UI.
async fn yield_to_event_loop() {
    let promise = Promise::new(&mut |resolve, _| {
        // Use setTimeout(0) to yield to event loop
        let window = web_sys::window();
        if let Some(win) = window {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        } else {
            // If no window (e.g., in a worker), just resolve immediately
            let _ = resolve.call0(&JsValue::NULL);
        }
    });

    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Check if the current context supports async operations.
///
/// @returns true if Promises and async operations are available
#[wasm_bindgen(js_name = "supportsAsync")]
pub fn supports_async() -> bool {
    // Check if we have access to Promise
    js_sys::Reflect::get(&js_sys::global(), &"Promise".into())
        .map(|v| !v.is_undefined())
        .unwrap_or(false)
}

/// Delay for a specified number of milliseconds.
///
/// Useful for throttling or adding delays in async operations.
///
/// @param ms - Number of milliseconds to delay
/// @returns A Promise that resolves after the delay
#[wasm_bindgen(js_name = "delay")]
pub fn delay(ms: u32) -> Promise {
    Promise::new(&mut |resolve, _| {
        let window = web_sys::window();
        if let Some(win) = window {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
        } else {
            // Fallback: resolve immediately if no window
            let _ = resolve.call0(&JsValue::NULL);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_async() {
        // This will be false in non-WASM tests
        // Just ensure it doesn't panic
        let _ = supports_async();
    }
}
