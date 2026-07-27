# Changelog

Notable changes to zesven. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

This release began as an investigation into a report that multivolume writing
was slow, and ended up finding that several write options did not reach the
encoder at all. Archives written by earlier versions remain readable; nothing
here changes how archives are read.

### Breaking

- Removed `codec::fast_lzma2` and `codec::fast_lzma2_encode`. The encoder
  produced LZMA2 streams that no decoder can read - the smallest input that
  reproduces it is 117 bytes - and its tests missed this because every one of
  them compressed strictly periodic data. Use `codec::lzma::Lzma2Encoder` with
  a low preset for fast compression.
- Removed `write::Lzma2Variant`, `WriteOptions::lzma2_variant` and
  `WriteOptions::fast_lzma2`. Nothing ever read them: an archive was always
  written by the standard encoder whatever they were set to, so removing the
  calls changes no output.
- Removed the `fast-lzma2` feature.
- Removed the `async_codec` module and its exports: `AsyncDecoder`,
  `AsyncEncoder`,
  `build_async_decoder`, `build_async_encoder`. The LZMA2 decoder it handed out
  was an LZMA decoder, which fails on the first byte of any LZMA2 stream, and
  it reported LZMA's method ID for it. Nothing in the crate used it: the async
  reader decodes through the blocking codecs. Wrap those in `spawn_blocking` if
  you need the same thing.
- `WriteOptions` is `#[non_exhaustive]`. It gained `threads` and `memory_limit`,
  and a struct literal listing every field no longer compiles; build one from
  `WriteOptions::new` and the setters.
- `read::Threads` now lives at the crate root as `zesven::Threads`. It is still
  re-exported from `read`, so `zesven::read::Threads` continues to work.
- Adding entries to a `Writer`, and `ArchiveEditor::apply`, now require the
  destination to be `Send`. Compressing an entry as it is read hands the sink to
  an encoder, and the encoders require it. Every sink in ordinary use - a file,
  a `BufWriter`, a `Cursor`, a multivolume writer - already satisfies this.

### Fixed

- `CodecMethod::Lzma2` reports the feature it actually needs. It claimed to be
  available whenever `lzma` was, while its encoder is compiled under `lzma2`,
  so a build with only `lzma` accepted the configuration and failed on the
  first entry. `required_feature` said "lzma" for the same reason.
- The password is fixed when a key is first derived from it, not when an entry
  is offered: an entry turned away for an unrelated reason, or one that is
  never encrypted such as a directory, leaves the archive free to be keyed on
  another password.
- The password cannot be changed once an entry has been encrypted. Entry data
  was encrypted with the password in force when the entry was accepted while
  the header took whichever was set at `finish`, so changing it produced an
  archive no single password opens: the first password fails on the header, the
  second on the entry.
- An entry is written with the options it was accepted under. A small entry is
  buffered and compressed later, and reading the options as they stand at that
  point applied settings it was never offered: a file added with `password()`
  and `encrypt_data(true)` went into the archive in the clear once the options
  were switched to an unencrypted method, and switching to BCJ2 reshaped
  entries accepted under an ordinary method into a folder this crate's own
  reader rejects. Each buffered entry carries the options it was accepted
  under - the whole of them, including the nonce policy encryption draws its
  salt and IV from - and anything held under options that have since changed
  is written out before the next entry is taken.
- A method the build does not carry is refused before an entry is accepted
  rather than when the buffer is compressed. The batch was dropped at that
  point while the writer carried on, so `finish` produced an archive that
  opened correctly and was missing files the caller had been told were
  written. A compression failure now leaves the writer unusable.
- Entries keep the order they were added in a solid archive. Directories and
  anti-items were recorded immediately while files waited in the solid buffer,
  so an entry that skipped the buffer overtook the ones still in it - `a.txt`,
  `dir`, `z.txt` came back as `dir`, `a.txt`, `z.txt`, `deterministic` or not.
- Changing the options after entries have been written no longer corrupts them.
  The header described every folder with whichever compression method was set
  last, so an entry written with LZMA2 and followed by a switch to `Copy` was
  declared as stored: it failed its checksum on extraction. Each folder now
  records the method it was written with, as it already did for the coder
  properties. Both writers had this, separately.
- A failure while writing to the sink poisons the writer on every path, not
  only the streaming one, and in the async writer as well. A batch that failed
  to flush, or a transient error in an async sink, left the writer usable and
  produced an archive with folders at the wrong offsets.
- A rejected entry no longer reserves its position under `deterministic`. The
  order was recorded before the entry was accepted, so a failed `add_path` for
  a late-sorting name then rejected a perfectly good earlier one.
- The async writer runs the same validation as the blocking one before reading
  its input. A method this build does not carry was reported only after the
  whole source had been consumed.
- The async writer refuses the options it does not implement - encryption,
  filters, solid mode and comments - rather than writing an archive without
  them. `AsyncWriter::add_bytes` reached the internal add without the check
  the other entry points perform, so on the path most callers use nothing was
  refused at all, including adds after `finish`. Encryption in particular was
  tested for a password rather than for the flag, so `encrypt_data(true)` on
  its own wrote the data in the clear. The entry order is now checked for
  directories too, not just files.
- `MemoryLimit::Auto` no longer hands a machine under memory pressure a larger
  budget than a comfortable one. A quarter of the free memory was used only if
  that came to at least 16 MiB, and otherwise the fixed 512 MiB default: 63 MiB
  free yielded a 512 MiB budget, 64 MiB free yielded 16 MiB. The default is now
  for machines that cannot be asked at all.
- `AsyncWriter` counts anti-items the way the blocking writer does. They are
  removals rather than files, and the blocking writer leaves them out of
  `entries_written`; the async writer counted them, so the two disagreed about
  the same archive. The archives themselves were correct either way.
- A finished archive is flushed to its sink. The signature header is written
  last, over the start of the archive, so a buffered sink still held it when
  `finish_into_inner` handed the sink back: the file was correct only once the
  caller dropped it, and any error in that final write was discarded by
  `BufWriter::drop` rather than returned.
- An archive may be written into a sink positioned after something else. Every
  offset in the header, and the seek back for the signature, assumed the
  archive began at the start of the sink.
- `WriteResult::volume_sizes` reports the size of the archive. It was read from
  the sink after the signature header had been written, which happens last and
  seeks back to the start, so every single-file archive claimed to be 32 bytes
  long. The async writer reported no sizes at all.
- The default compression method covers every optional codec. A build with only
  Zstd, Brotli or LZ4 fell through to storing entries uncompressed.
- `deterministic(true)` no longer pairs entries with the wrong contents. It
  sorted the file list after the streams had been written, so `a.txt` came back
  holding what was written for `z.txt` - with a matching checksum and no error.
  An entry's position in the list is what binds it to its stream, so the setting
  now requires entries to arrive sorted and reports the one that breaks the
  order, rather than rearranging names over data.
- Data encryption through the BCJ2 filter is refused instead of silently
  skipped. BCJ2 builds its own four-stream coder chain with nowhere to put an
  AES coder, so archives written with `password()` and `encrypt_data(true)`
  opened and extracted with no password at all.
- The BCJ2 filter in a solid archive is refused. The combination wrote an
  archive that this crate's own reader rejects with "BCJ2 coder requires
  exactly 4 input streams, found 1".
- A write that fails partway through an entry now poisons the writer. Entries
  past the streaming threshold reach the sink as they are compressed, so a read
  error leaves bytes belonging to no folder; the writer used to accept more
  entries and finish successfully, producing an archive that opened and then
  failed on a later entry.
- `AsyncWriter::create_path` produces a readable archive. The signature header
  is written last, after seeking back to the start, and a buffered async sink
  has no drop that flushes it: the file was left with 32 zero bytes where its
  signature belongs, and neither 7-Zip nor this crate could open it. Every
  async test wrote to an in-memory cursor, which has no buffer to lose.
- The async writer no longer loses the entry after an empty one. It gave every
  entry a folder, but an empty entry carries no stream, so the extra folder
  shifted the pairing and the following file came back empty.
- The async writer honours `threads` and `memory_limit`, and shares the
  blocking writer's codec dispatch and header encoder rather than a second copy
  of each. Its own encoder wrote no substream information and no anti-items,
  and had to be fixed separately for every defect found in the other one -
  which is how it kept the folder-method and poisoning defects after the
  blocking writer was fixed.
- `aes` and `cli` build on their own. Encrypted headers are stored compressed,
  so `aes` needs a codec, and the command-line tool handles passwords, so it
  needs encryption; neither dependency was declared and neither combination was
  ever built.
- The compression level now reaches the encoder. Options were built from a
  fixed preset with only the dictionary overridden, so the match finder, encoder
  mode and nice length never moved: across all ten levels, one sample compressed
  to byte-identical output. Requesting the fastest level now genuinely is
  fastest, and requesting the slowest genuinely compresses harder.
- The header now declares the dictionary the data was compressed with. It was
  derived a second time when writing the header, from a copy of the same formula
  missing a clamp, so at levels 8 and 9 an archive declared a dictionary larger
  than the one used - and every reader allocated for it.
- The dictionary is capped by the size of the data. A dictionary larger than the
  input cannot help, and a reader allocates whatever the archive declares.
- `BZip2` no longer panics at level 0. BZip2 has no level 0 of its own, and
  passing this crate's lowest level through reached an assertion inside the
  bzip2 crate.
- The async writer carried its own copy of the level and dictionary defects and
  is fixed the same way.

### Added

- `WriteOptions::threads` bounds the parallelism used for compression. Any
  explicit setting - `Threads::Single` or `Threads::Count` - produces the same
  archive on every machine, which is what reproducible builds need.
  `Threads::Single` also compresses a solid block as one stream, which is the
  smallest a level can produce.
- `WriteOptions::memory_limit` bounds how many encoders run at once and how much
  data waits between them. Each reserves a match finder several times the size
  of its dictionary, so this is what stops a large machine from being asked for
  gigabytes. It changes how fast an archive is written, never its contents, and
  it is not a ceiling on the process: an entry compressed in memory costs what
  it costs.
- `zesven::MemoryLimit`, alongside `zesven::Threads`.
- Large entries are compressed straight into the sink instead of being read
  into memory first, so archiving a file no longer costs as much memory as the
  file is long. Writing a 300 MB file peaks at 98 MB rather than 924 MB, and
  that figure no longer grows with the file: 200 MB, 400 MB and 800 MB inputs
  all peak at the same 98 MB. This covers `Copy`, LZMA and LZMA2, whose
  encoders write through; Deflate, BZip2, PPMd, Zstd, LZ4 and Brotli hand back
  a buffer and so keep the buffered path, as do entries that need the
  compressed bytes in hand - those with a filter ahead of the codec, encryption
  behind it, or a solid block around them.

### Performance

Measured on 24 cores against 1.2.0, writing ten 8 MB files into 17 MB volumes:

| case | 1.2.0 | now |
|------|-------|-----|
| already-compressed input | 9.8 s | 5.3 s |
| source-like input | 9.3 s | 2.3 s |
| source-like input, level 1 | 8.0 s | 0.3 s |

- Entries of a non-solid archive are compressed across the available cores.
  Each entry is its own folder and so independent of the others; this costs
  nothing in compression ratio.
- Solid blocks are cut into chunks that are compressed in parallel. A chunk
  cannot match against the one before it, so this costs a little ratio -
  `threads(Threads::Single)` declines the trade.
- A solid archive now compresses considerably better regardless, because the
  dictionary is no longer smaller than the entries: on duplicate-heavy input the
  packed size fell from 6.6 MB to 3.3 MB, and to 0.6 MB with one thread.

### Changed

- Peak memory during writing is higher, because compressing on several cores
  means several match finders at once. `memory_limit` bounds how many run
  concurrently, and lowering it or `threads` brings the peak back down. It
  bounds the concurrency rather than the total: an entry compressed in memory
  costs what it costs, so one entry larger than the limit exceeds it.
- The default level compresses slightly less on some inputs than 1.2.0 did.
  1.2.0 effectively always used one particular preset; level 5 now means level
  5. Callers who want the old ratio should ask for level 6.
- With `Threads::Auto`, a machine with a single core writes a different (also
  valid, and slightly smaller) archive than a multi-core one, since there is no
  parallelism to arrange for. Ask for an explicit thread count where
  byte-for-byte reproducibility matters.

- Archives written by the async writer now carry substream information and
  anti-items, because it builds its header the same way the blocking writer
  does. Archives it wrote before are still readable; new ones simply say more.

### Internal

- Added `tests/options_have_effect.rs`, which asserts that every public write
  option changes what gets written - for anything affecting encoding, that the
  packed streams differ rather than merely the header.
- Added `tests/codec_roundtrip_props.rs`, which round-trips every exposed
  encoder through its decoder on generated inputs, including blocks repeated at
  varying distances.
- Added `benches/write_throughput.rs` and `mise run bench`, which hold
  compression ratios to exact bounds and check that parallel writing really is
  faster than sequential. Built without the `parallel` feature, every scenario
  fails that check.
- `mise run msrv` now checks against the version declared in the manifest
  instead of running `cargo check` on whatever toolchain is current.
- The feature matrix disables default features for every combination. Building
  `--features aes` also built every default, so a feature that could not stand
  on its own still passed - which is how `aes` and `cli` came to not compile
  alone while CI reported success.
- `mise run version:sync` updates the version in the install snippets, which
  otherwise kept telling readers to depend on the previous major. It finds them
  rather than working from a list - the list missed two - and the release commit
  stages what it rewrote, which it previously did not.
- The interoperability test for multi-threaded streams writes a solid archive,
  which is the only shape that gets chunked; written non-solid it had been
  testing the ordinary single-stream path. The multivolume streaming test now
  requires the entry to actually span volumes.
