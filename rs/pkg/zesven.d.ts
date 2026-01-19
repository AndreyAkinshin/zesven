/**
 * zesven TypeScript Type Definitions
 *
 * A pure Rust implementation of the 7z archive format, compiled to WebAssembly.
 *
 * @packageDocumentation
 */

// ============================================================================
// Entry Types
// ============================================================================

/**
 * Information about an archive entry (file or directory).
 */
export interface ArchiveEntry {
  /** Full path within the archive */
  name: string;
  /** Uncompressed size in bytes */
  size: number;
  /** Whether this entry is a directory */
  isDirectory: boolean;
  /** CRC32 checksum (if available) */
  crc?: number;
  /** Modification time as Windows FILETIME (100ns intervals since 1601) */
  mtime?: number;
  /** Creation time as Windows FILETIME */
  ctime?: number;
  /** Access time as Windows FILETIME */
  atime?: number;
  /** Windows file attributes */
  attributes?: number;
  /** Whether this entry is encrypted */
  isEncrypted: boolean;
}

/**
 * Archive information and statistics.
 */
export interface ArchiveInfo {
  /** Number of entries in the archive */
  entryCount: number;
  /** Total uncompressed size of all entries */
  totalSize: number;
  /** Total compressed (packed) size */
  packedSize: number;
  /** Whether the archive uses solid compression */
  isSolid: boolean;
  /** Whether any entries are encrypted */
  hasEncryptedEntries: boolean;
  /** Whether the header is encrypted */
  hasEncryptedHeader: boolean;
  /** Number of folders in the archive */
  folderCount: number;
  /** List of compression methods used */
  compressionMethods: string[];
}

// ============================================================================
// WasmArchive Class
// ============================================================================

/**
 * A 7z archive reader.
 *
 * @example
 * ```typescript
 * // Open an archive from Uint8Array
 * const archive = new WasmArchive(archiveData);
 *
 * // List all entries
 * for (const entry of archive.getEntries()) {
 *   console.log(`${entry.name}: ${entry.size} bytes`);
 * }
 *
 * // Extract a single file
 * const content = archive.extractEntry('path/to/file.txt');
 * const text = new TextDecoder().decode(content);
 *
 * // Clean up
 * archive.free();
 * ```
 */
export class WasmArchive {
  /**
   * Open an archive from a Uint8Array.
   * @param data - Archive data as Uint8Array
   * @throws Error if the archive is invalid
   */
  constructor(data: Uint8Array);

  /**
   * Open an encrypted archive with a password.
   * @param data - Archive data as Uint8Array
   * @param password - Password for decryption
   * @throws Error if the archive is invalid or password is incorrect
   */
  static openWithPassword(data: Uint8Array, password: string): WasmArchive;

  /** Number of entries in the archive */
  readonly length: number;

  /** Get archive information and statistics */
  getInfo(): ArchiveInfo;

  /** Get all entries in the archive */
  getEntries(): ArchiveEntry[];

  /** Check if the archive is empty */
  isEmpty(): boolean;

  /**
   * Find an entry by path.
   * @param path - The path to search for
   * @returns The entry or undefined if not found
   */
  getEntry(path: string): ArchiveEntry | undefined;

  /**
   * Extract a single entry.
   * @param name - Path of the entry to extract
   * @returns File content as Uint8Array
   * @throws Error if entry not found or extraction fails
   */
  extractEntry(name: string): Uint8Array;

  /**
   * Extract all file entries.
   * @returns Map of path to content
   */
  extractAll(): Map<string, Uint8Array>;

  /**
   * Test archive integrity.
   * @returns true if the archive passes integrity checks
   */
  test(): boolean;

  /**
   * Find entries matching a glob pattern.
   * @param pattern - Glob pattern (supports * and ?)
   * @returns Array of matching paths
   */
  findEntries(pattern: string): string[];

  /**
   * Get entries in a specific directory.
   * @param dir - Directory path (empty string for root)
   * @param recursive - Whether to include subdirectories
   */
  getEntriesInDirectory(dir: string, recursive: boolean): ArchiveEntry[];

  /** Free the archive resources */
  free(): void;
}

// ============================================================================
// WasmWriteOptions Class
// ============================================================================

/**
 * Options for archive creation.
 *
 * @example
 * ```typescript
 * const options = new WasmWriteOptions();
 * options.method = 'lzma2';
 * options.level = 7;
 * options.solid = true;
 * ```
 */
export class WasmWriteOptions {
  constructor();

  /** Enable solid compression */
  solid: boolean;

  /** Compression method: "copy", "lzma", "lzma2", "deflate", "bzip2" */
  method: string;

  /** Compression level (0-9) */
  level: number;

  /** Password for encryption (optional) */
  password: string | undefined;

  /** Whether to encrypt file names in header */
  encryptHeader: boolean;

  free(): void;
}

// ============================================================================
// WasmWriter Class
// ============================================================================

/**
 * A 7z archive writer.
 *
 * @example
 * ```typescript
 * const options = new WasmWriteOptions();
 * options.method = 'lzma2';
 *
 * const writer = new WasmWriter(options);
 * writer.addFile('hello.txt', new TextEncoder().encode('Hello, World!'));
 * writer.addDirectory('subdir');
 *
 * const archiveData = writer.finish();
 * ```
 */
export class WasmWriter {
  /**
   * Create a new archive writer.
   * @param options - Optional write options
   */
  constructor(options?: WasmWriteOptions);

  /** Number of pending entries */
  readonly entryCount: number;

  /** Whether the writer has been finished */
  readonly isFinished: boolean;

  /**
   * Add a file from Uint8Array.
   * @param name - Path within the archive
   * @param data - File content
   */
  addFile(name: string, data: Uint8Array): void;

  /**
   * Add a file from a string (UTF-8 encoded).
   * @param name - Path within the archive
   * @param content - String content
   */
  addFileFromString(name: string, content: string): void;

  /**
   * Add an empty directory.
   * @param name - Directory path
   */
  addDirectory(name: string): void;

  /**
   * Remove a pending entry.
   * @param name - Path of entry to remove
   * @returns true if an entry was removed
   */
  removeEntry(name: string): boolean;

  /** Get list of pending entry names */
  getEntryNames(): string[];

  /**
   * Finalize and get the archive data.
   * @returns Archive data as Uint8Array
   */
  finish(): Uint8Array;

  /** Cancel the writer and discard entries */
  cancel(): void;

  free(): void;
}

// ============================================================================
// WasmMemoryConfig Class
// ============================================================================

/**
 * Configuration for memory-constrained operations.
 *
 * @example
 * ```typescript
 * const config = new WasmMemoryConfig();
 * config.chunkSize = 64 * 1024;  // 64KB chunks
 * config.lowMemoryMode = true;
 *
 * extractWithMemoryLimit(archive, 'large.bin', config, (chunk) => {
 *   // Process each chunk
 * });
 * ```
 */
export class WasmMemoryConfig {
  constructor();

  /** Chunk size in bytes for streaming operations */
  chunkSize: number;

  /** Maximum buffer size in bytes */
  maxBufferSize: number;

  /** Enable low memory mode */
  lowMemoryMode: boolean;

  /** Auto-detect appropriate settings for current environment */
  static autoDetect(): WasmMemoryConfig;

  free(): void;
}

// ============================================================================
// Stream Options
// ============================================================================

/**
 * Options for stream reading operations.
 */
export class StreamReadOptions {
  constructor();

  /** Chunk size for reading */
  chunkSize: number;

  /** Maximum buffer size */
  maxBufferSize: number;

  free(): void;
}

// ============================================================================
// Entry Iterator
// ============================================================================

/**
 * Memory-efficient iterator for archive entries.
 */
export class EntryIterator {
  constructor(archive: WasmArchive);

  /** Check if there are more entries */
  hasNext(): boolean;

  /** Get the next entry */
  next(): ArchiveEntry | undefined;

  /** Reset to the beginning */
  reset(): void;

  /** Total number of entries */
  readonly count: number;

  /** Current position */
  readonly position: number;

  free(): void;
}

// ============================================================================
// Standalone Functions
// ============================================================================

// --- Module Info ---

/** Get the library version */
export function getVersion(): string;

/** Check if a compression method is supported */
export function isMethodSupported(method: string): boolean;

/** Get list of supported compression methods */
export function getSupportedMethods(): string[];

/** Check if encryption is supported */
export function isEncryptionSupported(): boolean;

// --- sevenz-rust2 Compatible API ---

/**
 * Decompress a 7z archive (sevenz-rust2 compatible).
 * @param src - Archive data
 * @param password - Password (empty string for none)
 * @param callback - Called for each extracted file
 */
export function decompress(
  src: Uint8Array,
  password: string,
  callback: (path: string, data: Uint8Array) => void
): void;

/**
 * Compress files into a 7z archive (sevenz-rust2 compatible).
 * @param entries - Array of file paths
 * @param datas - Array of file contents
 * @returns Archive data
 */
export function compress(entries: string[], datas: Uint8Array[]): Uint8Array;

/**
 * Compress files with options.
 * @param entries - Array of file paths
 * @param datas - Array of file contents
 * @param method - Compression method
 * @param level - Compression level (0-9)
 */
export function compressWithOptions(
  entries: string[],
  datas: Uint8Array[],
  method: string,
  level: number
): Uint8Array;

/**
 * List entries without extracting.
 */
export function listEntries(src: Uint8Array, password: string): ArchiveEntry[];

/**
 * Extract a single file.
 */
export function extractFile(
  src: Uint8Array,
  password: string,
  path: string
): Uint8Array;

/**
 * Test if an archive is valid.
 */
export function testArchive(src: Uint8Array, password: string): boolean;

/**
 * Get archive information.
 */
export function getArchiveInfo(src: Uint8Array, password: string): ArchiveInfo;

/**
 * Compress a single file.
 */
export function compressFile(name: string, data: Uint8Array): Uint8Array;

/**
 * Extract all files to a Map.
 */
export function extractAll(
  src: Uint8Array,
  password: string
): Map<string, Uint8Array>;

// --- Async/Promise API ---

/**
 * Open an archive asynchronously.
 */
export function openArchiveAsync(
  data: Uint8Array,
  password?: string
): Promise<WasmArchive>;

/**
 * Extract an entry asynchronously.
 */
export function extractEntryAsync(
  archive: WasmArchive,
  name: string
): Promise<Uint8Array>;

/**
 * Extract all entries asynchronously.
 */
export function extractAllAsync(
  archive: WasmArchive
): Promise<Map<string, Uint8Array>>;

/**
 * Test archive integrity asynchronously.
 */
export function testArchiveAsync(archive: WasmArchive): Promise<boolean>;

/**
 * Create an archive asynchronously.
 * @param entries - Array of {name, data} objects
 * @param options - Optional write options
 */
export function createArchiveAsync(
  entries: Array<{ name: string; data: Uint8Array }>,
  options?: WasmWriteOptions
): Promise<Uint8Array>;

/**
 * Process entries with an async callback.
 */
export function processEntriesAsync(
  archive: WasmArchive,
  callback: (entry: ArchiveEntry) => void | Promise<void>
): Promise<void>;

/**
 * Batch extract with progress callback.
 */
export function batchExtractAsync(
  archive: WasmArchive,
  entryNames: string[],
  onProgress?: (extracted: number, total: number) => void
): Promise<Map<string, Uint8Array>>;

/** Check if async operations are supported */
export function supportsAsync(): boolean;

/** Delay for milliseconds */
export function delay(ms: number): Promise<void>;

// --- Web Streams API ---

/**
 * Open an archive from a ReadableStream.
 */
export function openFromStream(
  stream: ReadableStream<Uint8Array>,
  password?: string
): Promise<WasmArchive>;

/**
 * Extract an entry as a ReadableStream.
 */
export function extractAsStream(
  archive: WasmArchive,
  entryName: string
): ReadableStream<Uint8Array>;

/**
 * Extract as a chunked ReadableStream.
 */
export function extractAsChunkedStream(
  archive: WasmArchive,
  entryName: string,
  options: StreamReadOptions
): ReadableStream<Uint8Array>;

/**
 * Read a stream to Uint8Array with size limit.
 */
export function readStreamToArray(
  stream: ReadableStream<Uint8Array>,
  maxSize: number
): Promise<Uint8Array>;

// --- Memory Management ---

/**
 * Extract with memory limit, calling callback for each chunk.
 */
export function extractWithMemoryLimit(
  archive: WasmArchive,
  entryName: string,
  config: WasmMemoryConfig,
  onChunk: (chunk: Uint8Array) => void
): void;

/**
 * Extract multiple entries with total memory limit.
 */
export function extractMultipleWithLimit(
  archive: WasmArchive,
  entryNames: string[],
  config: WasmMemoryConfig,
  onEntry: (name: string, data: Uint8Array) => void
): void;

/**
 * Get memory usage statistics (Chrome only).
 */
export function getMemoryStats(): {
  usedHeapSize?: number;
  totalHeapSize?: number;
  heapLimit?: number;
};

/**
 * Check if an extraction would fit in memory.
 */
export function canExtractSafely(
  archive: WasmArchive,
  entryName: string,
  config: WasmMemoryConfig
): boolean;

/**
 * Request garbage collection (if available).
 */
export function requestGC(): void;

// ============================================================================
// Module Initialization
// ============================================================================

/**
 * Initialize the WASM module.
 *
 * @example
 * ```typescript
 * import init, { WasmArchive } from 'zesven';
 *
 * async function main() {
 *   await init();
 *   const archive = new WasmArchive(data);
 * }
 * ```
 */
export default function init(
  input?: RequestInfo | URL | Response | BufferSource
): Promise<void>;
