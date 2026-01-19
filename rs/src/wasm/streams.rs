//! Web Streams API integration for WASM.
//!
//! Provides ReadableStream and WritableStream support for
//! streaming archive operations in browsers.

use js_sys::{Object, Reflect, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStream, ReadableStreamDefaultReader};

use super::archive::WasmArchive;
use super::file::{uint8_array_to_vec, vec_to_uint8_array};

/// Open an archive from a ReadableStream.
///
/// This function reads the entire stream into memory before parsing,
/// which is necessary for archive formats that require random access.
///
/// @param stream - A ReadableStream containing archive data
/// @param password - Optional password for encrypted archives
/// @returns A Promise that resolves to a WasmArchive
///
/// # JavaScript Example
///
/// ```javascript
/// // From fetch response
/// const response = await fetch('archive.7z');
/// const archive = await openFromStream(response.body);
///
/// // From File API
/// const file = document.querySelector('input').files[0];
/// const archive = await openFromStream(file.stream());
/// ```
#[wasm_bindgen(js_name = "openFromStream")]
pub async fn open_from_stream(
    stream: ReadableStream,
    password: Option<String>,
) -> Result<WasmArchive, JsValue> {
    // Get a reader from the stream
    let reader = stream.get_reader();
    let reader: ReadableStreamDefaultReader = reader.dyn_into()?;

    // Collect all chunks into a buffer
    let mut buffer = Vec::new();

    loop {
        let result = JsFuture::from(reader.read()).await?;

        let done = Reflect::get(&result, &"done".into())?
            .as_bool()
            .unwrap_or(true);

        if done {
            break;
        }

        let value = Reflect::get(&result, &"value".into())?;
        if !value.is_undefined() {
            let chunk: Uint8Array = value.dyn_into()?;
            let mut chunk_data = vec![0u8; chunk.length() as usize];
            chunk.copy_to(&mut chunk_data);
            buffer.extend(chunk_data);
        }
    }

    // Create archive from buffer
    let uint8_array = vec_to_uint8_array(&buffer);

    if let Some(pwd) = password {
        #[cfg(feature = "aes")]
        {
            WasmArchive::open_with_password(uint8_array, &pwd)
        }
        #[cfg(not(feature = "aes"))]
        {
            let _ = pwd;
            Err(JsValue::from_str("AES encryption support not compiled"))
        }
    } else {
        WasmArchive::new(uint8_array)
    }
}

/// Extract an archive entry as a ReadableStream.
///
/// This creates a stream that yields the decompressed content of the entry.
/// Useful for large files where you want to process data as it's available.
///
/// @param archive - The archive to extract from
/// @param entryName - The path of the entry to extract
/// @returns A ReadableStream of the decompressed content
///
/// # JavaScript Example
///
/// ```javascript
/// const stream = extractAsStream(archive, 'large-file.bin');
///
/// // Pipe to Response for download
/// const response = new Response(stream);
/// const blob = await response.blob();
///
/// // Or read chunks manually
/// const reader = stream.getReader();
/// while (true) {
///     const { done, value } = await reader.read();
///     if (done) break;
///     processChunk(value);
/// }
/// ```
#[wasm_bindgen(js_name = "extractAsStream")]
pub fn extract_as_stream(
    archive: &mut WasmArchive,
    entry_name: &str,
) -> Result<ReadableStream, JsValue> {
    // Extract the data first
    let data = archive.extract_entry(entry_name)?;
    let data_vec = uint8_array_to_vec(&data);

    // Create a ReadableStream that yields the data
    create_readable_stream_from_data(data_vec)
}

/// Create a ReadableStream from a Vec<u8>.
fn create_readable_stream_from_data(data: Vec<u8>) -> Result<ReadableStream, JsValue> {
    use wasm_bindgen::closure::Closure;
    use web_sys::ReadableStreamDefaultController;

    // Wrap data in Rc<RefCell> for sharing with closure
    let data = std::rc::Rc::new(std::cell::RefCell::new(Some(data)));
    let data_clone = data.clone();

    // Create the start function for the underlying source
    let start = Closure::wrap(Box::new(
        move |controller: ReadableStreamDefaultController| -> Result<(), JsValue> {
            if let Some(bytes) = data_clone.borrow_mut().take() {
                // Enqueue the entire buffer at once
                // For true streaming, this should be chunked
                let array = vec_to_uint8_array(&bytes);
                controller.enqueue_with_chunk(&array)?;
            }
            controller.close()?;
            Ok(())
        },
    )
        as Box<dyn FnMut(ReadableStreamDefaultController) -> Result<(), JsValue>>);

    // Create underlying source object
    let source = Object::new();
    Reflect::set(&source, &"start".into(), start.as_ref())?;

    // Keep the closure alive
    start.forget();

    ReadableStream::new_with_underlying_source(&source)
}

/// Options for chunked stream reading.
#[wasm_bindgen]
pub struct StreamReadOptions {
    /// Chunk size in bytes
    chunk_size: usize,
    /// Maximum buffer size
    max_buffer_size: usize,
}

#[wasm_bindgen]
impl StreamReadOptions {
    /// Create default stream read options.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set chunk size (default: 64KB).
    #[wasm_bindgen(setter, js_name = "chunkSize")]
    pub fn set_chunk_size(&mut self, size: usize) {
        self.chunk_size = size;
    }

    /// Get chunk size.
    #[wasm_bindgen(getter, js_name = "chunkSize")]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Set maximum buffer size (default: 64MB).
    #[wasm_bindgen(setter, js_name = "maxBufferSize")]
    pub fn set_max_buffer_size(&mut self, size: usize) {
        self.max_buffer_size = size;
    }

    /// Get maximum buffer size.
    #[wasm_bindgen(getter, js_name = "maxBufferSize")]
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }
}

impl Default for StreamReadOptions {
    fn default() -> Self {
        Self {
            chunk_size: 64 * 1024,             // 64 KB
            max_buffer_size: 64 * 1024 * 1024, // 64 MB
        }
    }
}

/// Extract an archive entry as a chunked ReadableStream.
///
/// This version yields data in fixed-size chunks, which is more memory-efficient
/// for very large files.
///
/// @param archive - The archive to extract from
/// @param entryName - The path of the entry to extract
/// @param options - Stream options (chunk size, etc.)
/// @returns A ReadableStream yielding chunks of the decompressed content
#[wasm_bindgen(js_name = "extractAsChunkedStream")]
pub fn extract_as_chunked_stream(
    archive: &mut WasmArchive,
    entry_name: &str,
    options: &StreamReadOptions,
) -> Result<ReadableStream, JsValue> {
    let data = archive.extract_entry(entry_name)?;
    let data_vec = uint8_array_to_vec(&data);

    create_chunked_readable_stream(data_vec, options.chunk_size)
}

/// Create a ReadableStream that yields data in chunks.
fn create_chunked_readable_stream(
    data: Vec<u8>,
    chunk_size: usize,
) -> Result<ReadableStream, JsValue> {
    use wasm_bindgen::closure::Closure;
    use web_sys::ReadableStreamDefaultController;

    // State for the pull function
    let state = std::rc::Rc::new(std::cell::RefCell::new(StreamState {
        data,
        position: 0,
        chunk_size,
    }));

    let state_clone = state.clone();

    // Create the pull function
    let pull = Closure::wrap(Box::new(
        move |controller: ReadableStreamDefaultController| -> Result<(), JsValue> {
            let mut state = state_clone.borrow_mut();

            if state.position >= state.data.len() {
                controller.close()?;
                return Ok(());
            }

            let end = (state.position + state.chunk_size).min(state.data.len());
            let chunk = &state.data[state.position..end];
            let array = vec_to_uint8_array(chunk);

            controller.enqueue_with_chunk(&array)?;
            state.position = end;

            Ok(())
        },
    )
        as Box<dyn FnMut(ReadableStreamDefaultController) -> Result<(), JsValue>>);

    // Create underlying source object
    let source = Object::new();
    Reflect::set(&source, &"pull".into(), pull.as_ref())?;

    // Keep the closure alive
    pull.forget();

    ReadableStream::new_with_underlying_source(&source)
}

/// Internal state for chunked streaming.
struct StreamState {
    data: Vec<u8>,
    position: usize,
    chunk_size: usize,
}

/// Read a ReadableStream into a Uint8Array with size limit.
///
/// @param stream - The stream to read
/// @param maxSize - Maximum number of bytes to read (0 for unlimited)
/// @returns A Promise that resolves to a Uint8Array
#[wasm_bindgen(js_name = "readStreamToArray")]
pub async fn read_stream_to_array(
    stream: ReadableStream,
    max_size: usize,
) -> Result<Uint8Array, JsValue> {
    let reader = stream.get_reader();
    let reader: ReadableStreamDefaultReader = reader.dyn_into()?;

    let mut buffer = Vec::new();

    loop {
        if max_size > 0 && buffer.len() >= max_size {
            // Cancel the stream and return what we have
            let _ = JsFuture::from(reader.cancel()).await;
            break;
        }

        let result = JsFuture::from(reader.read()).await?;

        let done = Reflect::get(&result, &"done".into())?
            .as_bool()
            .unwrap_or(true);

        if done {
            break;
        }

        let value = Reflect::get(&result, &"value".into())?;
        if !value.is_undefined() {
            let chunk: Uint8Array = value.dyn_into()?;
            let mut chunk_data = vec![0u8; chunk.length() as usize];
            chunk.copy_to(&mut chunk_data);

            // Respect max size
            if max_size > 0 {
                let remaining = max_size - buffer.len();
                if chunk_data.len() > remaining {
                    buffer.extend_from_slice(&chunk_data[..remaining]);
                    let _ = JsFuture::from(reader.cancel()).await;
                    break;
                }
            }

            buffer.extend(chunk_data);
        }
    }

    Ok(vec_to_uint8_array(&buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_read_options_default() {
        let opts = StreamReadOptions::default();
        assert_eq!(opts.chunk_size, 64 * 1024);
        assert_eq!(opts.max_buffer_size, 64 * 1024 * 1024);
    }

    #[test]
    fn test_stream_read_options_setters() {
        let mut opts = StreamReadOptions::new();
        opts.set_chunk_size(1024);
        opts.set_max_buffer_size(1024 * 1024);

        assert_eq!(opts.chunk_size(), 1024);
        assert_eq!(opts.max_buffer_size(), 1024 * 1024);
    }
}
