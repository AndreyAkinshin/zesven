//! Independent structural validation of the archives we write.
//!
//! Every writer test in this suite checks its output by reading it back through
//! our own reader. That cannot detect a writer and a reader that diverge from
//! the 7z format in the same direction: the round trip succeeds and the archive
//! is unreadable everywhere else. Exactly that hid an encrypted-header layout
//! bug and a transposed coder size list through a suite of over a thousand
//! tests.
//!
//! So this module parses archives with its own minimal decoder, written from the
//! format rules rather than from `zesven::format`, and asserts the invariants
//! those rules impose. It deliberately shares no code with the crate under test.
//! Where it is unsure - an unknown property, a shape the rules permit but we
//! never emit - it skips rather than fails: a validator that rejects legal
//! archives is worse than no validator.
//!
//! The reference implementation is the arbiter for anything this cannot settle;
//! see `tests/interop_7zip.rs`. The point of this file is that the checks it
//! does make run everywhere, on every writer test, with no external binary.

#![allow(dead_code)]

/// Offset every position in the header is measured from.
const SIGNATURE_HEADER_SIZE: u64 = 32;

const SIGNATURE: &[u8] = &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C];

// Property ids used below.
const K_END: u8 = 0x00;
const K_HEADER: u8 = 0x01;
const K_MAIN_STREAMS_INFO: u8 = 0x04;
const K_FILES_INFO: u8 = 0x05;
const K_PACK_INFO: u8 = 0x06;
const K_UNPACK_INFO: u8 = 0x07;
const K_SUBSTREAMS_INFO: u8 = 0x08;
const K_SIZE: u8 = 0x09;
const K_CRC: u8 = 0x0A;
const K_FOLDER: u8 = 0x0B;
const K_CODERS_UNPACK_SIZE: u8 = 0x0C;
const K_NUM_UNPACK_STREAM: u8 = 0x0D;
const K_ENCODED_HEADER: u8 = 0x17;

const METHOD_AES: &[u8] = &[0x06, 0xF1, 0x07, 0x01];

/// Asserts that an archive we produced is structurally well formed.
///
/// Panics with a description of the violated rule, naming the byte offset where
/// the problem is, so a failure points at the writer rather than at this file.
pub fn assert_archive_well_formed(bytes: &[u8]) {
    if let Err(problem) = check_archive(bytes) {
        panic!("archive is not well formed: {problem}");
    }
}

fn check_archive(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < SIGNATURE_HEADER_SIZE as usize {
        return Err(format!(
            "file is {} bytes, shorter than the signature header",
            bytes.len()
        ));
    }
    if &bytes[0..6] != SIGNATURE {
        return Err("signature bytes are not 7z".into());
    }

    let start_header = &bytes[12..32];
    let recorded_crc = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let actual_crc = crc32(start_header);
    if recorded_crc != actual_crc {
        return Err(format!(
            "StartHeaderCRC is {recorded_crc:#010x}, but the 20 bytes it covers hash to {actual_crc:#010x}"
        ));
    }

    let next_header_offset = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let next_header_size = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    let next_header_crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());

    // An empty archive records no header at all.
    if next_header_size == 0 {
        return Ok(());
    }

    let start = SIGNATURE_HEADER_SIZE
        .checked_add(next_header_offset)
        .ok_or("NextHeaderOffset overflows")?;
    let end = start
        .checked_add(next_header_size)
        .ok_or("NextHeaderSize overflows")?;
    if end != bytes.len() as u64 {
        return Err(format!(
            "the next header spans {start}..{end} but the file is {} bytes; \
             the header must be the last thing in the file and must not include \
             the packed streams it describes",
            bytes.len()
        ));
    }

    let header = &bytes[start as usize..end as usize];
    let actual = crc32(header);
    if next_header_crc != actual {
        return Err(format!(
            "NextHeaderCRC is {next_header_crc:#010x}, but the {next_header_size} bytes it \
             covers hash to {actual:#010x}"
        ));
    }

    let mut r = Reader::new(header);
    match r.u8()? {
        K_ENCODED_HEADER => check_encoded_header(&mut r, next_header_offset, bytes.len() as u64),
        K_HEADER => check_plain_header(&mut r, next_header_offset),
        other => Err(format!(
            "header starts with {other:#04x}, expected 0x01 or 0x17"
        )),
    }
}

/// Checks the structure that describes a compressed or encrypted header.
fn check_encoded_header(
    r: &mut Reader<'_>,
    next_header_offset: u64,
    file_len: u64,
) -> Result<(), String> {
    let streams = read_streams_info(r)?;
    let pack = streams
        .pack
        .as_ref()
        .ok_or("encoded header has no PackInfo")?;

    let total: u64 = pack.sizes.iter().sum();
    let end = pack.pos + total;

    if end != next_header_offset {
        return Err(format!(
            "the packed header stream covers {}..{} (relative to offset {SIGNATURE_HEADER_SIZE}) \
             but the header structure begins at {next_header_offset}; the stream must sit in the \
             data area immediately before the structure that points at it, and PackPos is measured \
             from the end of the signature header, not from the structure",
            pack.pos, end
        ));
    }

    if SIGNATURE_HEADER_SIZE + end > file_len {
        return Err(format!(
            "the packed header stream ends past the end of the file ({} > {file_len})",
            SIGNATURE_HEADER_SIZE + end
        ));
    }

    check_folders(&streams, pack)?;
    Ok(())
}

/// Checks a plain header and the main streams it describes.
fn check_plain_header(r: &mut Reader<'_>, next_header_offset: u64) -> Result<(), String> {
    let mut streams: Option<StreamsInfo> = None;

    loop {
        match r.u8()? {
            K_END => break,
            K_MAIN_STREAMS_INFO => streams = Some(read_streams_info(r)?),
            K_FILES_INFO => {
                // FilesInfo carries no invariant this validator can check
                // independently; its properties are skipped by their declared
                // size, which is itself the rule readers must follow.
                skip_files_info(r)?;
            }
            // Anything else is either a property we do not emit or one added
            // after this was written. Not our business to police.
            _ => return Ok(()),
        }
    }

    let streams = match streams {
        Some(s) => s,
        None => return Ok(()),
    };
    let pack = match streams.pack.as_ref() {
        Some(p) => p,
        None => return Ok(()),
    };

    let total: u64 = pack.sizes.iter().sum();
    if pack.pos + total > next_header_offset {
        return Err(format!(
            "packed streams cover {}..{} but the header starts at {next_header_offset}: \
             the data would run into the header",
            pack.pos,
            pack.pos + total
        ));
    }

    check_folders(&streams, pack)?;

    if let Some(sub) = streams.substreams.as_ref() {
        let declared: u64 = sub.counts.iter().sum();
        if declared == 0 && !streams.folders.is_empty() {
            return Err("SubStreamsInfo declares no substreams at all".into());
        }
    }

    Ok(())
}

/// Checks the invariants that tie a folder's coders, bind pairs and sizes together.
fn check_folders(streams: &StreamsInfo, pack: &PackInfo) -> Result<(), String> {
    if streams.folders.is_empty() {
        return Ok(());
    }

    let mut pack_index = 0usize;

    for (i, folder) in streams.folders.iter().enumerate() {
        let total_out: usize = folder.coders.iter().map(|c| c.out_streams).sum();
        let total_in: usize = folder.coders.iter().map(|c| c.in_streams).sum();

        if folder.bind_pairs.len() + 1 != total_out {
            return Err(format!(
                "folder {i} has {} output streams and {} bind pairs; the format requires \
                 exactly one fewer bind pair than output streams",
                total_out,
                folder.bind_pairs.len()
            ));
        }

        if folder.sizes.len() != total_out {
            return Err(format!(
                "folder {i} records {} coder unpack sizes for {} output streams; \
                 kCodersUnpackSize holds one size per output stream",
                folder.sizes.len(),
                total_out
            ));
        }

        // Every output must be consumed by at most one bind pair, and exactly
        // one output must be left over: that one is the folder's result.
        let mut consumed = vec![false; total_out];
        for &(_, out_index) in &folder.bind_pairs {
            let out_index = out_index as usize;
            if out_index >= total_out {
                return Err(format!(
                    "folder {i} binds output {out_index}, which does not exist"
                ));
            }
            if consumed[out_index] {
                return Err(format!("folder {i} binds output {out_index} twice"));
            }
            consumed[out_index] = true;
        }
        if consumed.iter().filter(|c| !**c).count() != 1 {
            return Err(format!(
                "folder {i} does not have exactly one unbound output stream, so it has no \
                 well-defined result"
            ));
        }

        // Same for inputs: whatever is not bound is fed by a packed stream.
        let mut bound_in = vec![false; total_in];
        for &(in_index, _) in &folder.bind_pairs {
            let in_index = in_index as usize;
            if in_index >= total_in {
                return Err(format!(
                    "folder {i} binds input {in_index}, which does not exist"
                ));
            }
            if bound_in[in_index] {
                return Err(format!("folder {i} binds input {in_index} twice"));
            }
            bound_in[in_index] = true;
        }
        let num_packed = bound_in.iter().filter(|b| !**b).count();

        if num_packed == 0 {
            return Err(format!("folder {i} has no packed input stream"));
        }
        if num_packed > 1 && folder.packed_indices.len() != num_packed {
            return Err(format!(
                "folder {i} takes {num_packed} packed streams but lists {} indices; \
                 the indices are written when and only when there is more than one",
                folder.packed_indices.len()
            ));
        }
        if num_packed == 1 && !folder.packed_indices.is_empty() {
            return Err(format!(
                "folder {i} lists packed stream indices although it takes only one; \
                 that index is implicit and must not be written"
            ));
        }

        // A declared CRC states something about the data. Zero over a non-empty
        // folder means a placeholder was declared as a real checksum: the odds of
        // a genuine CRC32 landing on zero are one in four billion.
        let output_size = folder
            .sizes
            .iter()
            .zip(0..)
            .find(|(_, index)| !consumed.get(*index as usize).copied().unwrap_or(true))
            .map(|(size, _)| *size)
            .unwrap_or(0);
        if folder.crc == Some(0) && output_size > 0 {
            return Err(format!(
                "folder {i} declares a CRC of zero over {output_size} bytes of output; \
                 a folder with no meaningful checksum must be left undeclared instead"
            ));
        }

        // A stream fed to AES is ciphertext, and CBC works in whole blocks.
        for (coder_index, coder) in folder.coders.iter().enumerate() {
            if coder.method != METHOD_AES {
                continue;
            }
            let is_packed_input = !bound_in.get(coder_index).copied().unwrap_or(false);
            if !is_packed_input {
                continue;
            }
            let size = pack.sizes.get(pack_index).copied().unwrap_or(0);
            if size % 16 != 0 {
                return Err(format!(
                    "folder {i} feeds {size} bytes into AES, which is not a multiple of the \
                     16-byte block size"
                ));
            }
        }

        pack_index += num_packed;
    }

    if pack_index != pack.sizes.len() {
        return Err(format!(
            "folders consume {pack_index} packed streams but PackInfo declares {}",
            pack.sizes.len()
        ));
    }

    Ok(())
}

// ============================================================================
// Minimal 7z header decoding
// ============================================================================

struct Coder {
    method: Vec<u8>,
    in_streams: usize,
    out_streams: usize,
}

struct Folder {
    coders: Vec<Coder>,
    /// `(in_index, out_index)` pairs.
    bind_pairs: Vec<(u64, u64)>,
    packed_indices: Vec<u64>,
    sizes: Vec<u64>,
    /// CRC of the folder's output, when the header declares one.
    crc: Option<u32>,
}

struct PackInfo {
    pos: u64,
    sizes: Vec<u64>,
}

struct SubStreamsInfo {
    counts: Vec<u64>,
}

struct StreamsInfo {
    pack: Option<PackInfo>,
    folders: Vec<Folder>,
    substreams: Option<SubStreamsInfo>,
}

fn read_streams_info(r: &mut Reader<'_>) -> Result<StreamsInfo, String> {
    let mut info = StreamsInfo {
        pack: None,
        folders: Vec::new(),
        substreams: None,
    };

    loop {
        match r.u8()? {
            K_END => break,
            K_PACK_INFO => info.pack = Some(read_pack_info(r)?),
            K_UNPACK_INFO => info.folders = read_unpack_info(r)?,
            K_SUBSTREAMS_INFO => {
                info.substreams = Some(read_substreams_info(r, info.folders.len())?)
            }
            other => return Err(format!("unexpected property {other:#04x} in StreamsInfo")),
        }
    }

    Ok(info)
}

fn read_pack_info(r: &mut Reader<'_>) -> Result<PackInfo, String> {
    let pos = r.number()?;
    let count = r.number()? as usize;
    let mut sizes = Vec::new();

    loop {
        match r.u8()? {
            K_END => break,
            K_SIZE => {
                for _ in 0..count {
                    sizes.push(r.number()?);
                }
            }
            K_CRC => skip_digests(r, count)?,
            other => return Err(format!("unexpected property {other:#04x} in PackInfo")),
        }
    }

    if sizes.len() != count {
        return Err(format!(
            "PackInfo declares {count} streams but records {} sizes",
            sizes.len()
        ));
    }

    Ok(PackInfo { pos, sizes })
}

fn read_unpack_info(r: &mut Reader<'_>) -> Result<Vec<Folder>, String> {
    let mut folders = Vec::new();

    loop {
        match r.u8()? {
            K_END => break,
            K_FOLDER => {
                let count = r.number()? as usize;
                let external = r.u8()?;
                if external != 0 {
                    // Folders stored in a separate stream; nothing to check here.
                    return Ok(Vec::new());
                }
                for _ in 0..count {
                    folders.push(read_folder(r)?);
                }
            }
            K_CODERS_UNPACK_SIZE => {
                for folder in folders.iter_mut() {
                    let total_out: usize = folder.coders.iter().map(|c| c.out_streams).sum();
                    for _ in 0..total_out {
                        folder.sizes.push(r.number()?);
                    }
                }
            }
            K_CRC => {
                let digests = read_digests(r, folders.len())?;
                for (folder, crc) in folders.iter_mut().zip(digests) {
                    folder.crc = crc;
                }
            }
            other => return Err(format!("unexpected property {other:#04x} in UnpackInfo")),
        }
    }

    Ok(folders)
}

fn read_folder(r: &mut Reader<'_>) -> Result<Folder, String> {
    let num_coders = r.number()? as usize;
    let mut coders = Vec::with_capacity(num_coders);

    for _ in 0..num_coders {
        let flags = r.u8()?;
        let id_size = (flags & 0x0F) as usize;
        let is_complex = flags & 0x10 != 0;
        let has_attributes = flags & 0x20 != 0;

        if flags & 0xC0 != 0 {
            return Err(format!(
                "coder flags {flags:#04x} set bits the format reserves"
            ));
        }

        let method = r.bytes(id_size)?.to_vec();
        let (in_streams, out_streams) = if is_complex {
            (r.number()? as usize, r.number()? as usize)
        } else {
            (1, 1)
        };

        if has_attributes {
            let len = r.number()? as usize;
            r.bytes(len)?;
        }

        coders.push(Coder {
            method,
            in_streams,
            out_streams,
        });
    }

    let total_in: usize = coders.iter().map(|c| c.in_streams).sum();
    let total_out: usize = coders.iter().map(|c| c.out_streams).sum();

    let mut bind_pairs = Vec::new();
    for _ in 0..total_out.saturating_sub(1) {
        let in_index = r.number()?;
        let out_index = r.number()?;
        bind_pairs.push((in_index, out_index));
    }

    let num_packed = total_in.saturating_sub(bind_pairs.len());
    let mut packed_indices = Vec::new();
    if num_packed > 1 {
        for _ in 0..num_packed {
            packed_indices.push(r.number()?);
        }
    }

    Ok(Folder {
        coders,
        bind_pairs,
        packed_indices,
        sizes: Vec::new(),
        crc: None,
    })
}

fn read_substreams_info(r: &mut Reader<'_>, num_folders: usize) -> Result<SubStreamsInfo, String> {
    let mut counts = vec![1u64; num_folders];

    loop {
        match r.u8()? {
            K_END => break,
            K_NUM_UNPACK_STREAM => {
                for slot in counts.iter_mut() {
                    *slot = r.number()?;
                }
            }
            K_SIZE => {
                // One size per substream except the last of each folder, which
                // is implied by the folder's total.
                for &count in &counts {
                    for _ in 0..count.saturating_sub(1) {
                        r.number()?;
                    }
                }
            }
            K_CRC => {
                let total: u64 = counts.iter().sum();
                skip_digests(r, total as usize)?;
            }
            other => {
                return Err(format!(
                    "unexpected property {other:#04x} in SubStreamsInfo"
                ));
            }
        }
    }

    Ok(SubStreamsInfo { counts })
}

/// Skips FilesInfo by honouring each property's declared size.
///
/// That skipping rule is itself part of the format: a reader must step over
/// properties it does not know rather than refusing the archive.
fn skip_files_info(r: &mut Reader<'_>) -> Result<(), String> {
    let _num_files = r.number()?;

    loop {
        let property = r.u8()?;
        if property == K_END {
            break;
        }
        let size = r.number()? as usize;
        r.bytes(size)?;
    }

    Ok(())
}

/// Reads a digest vector, returning one entry per item.
fn read_digests(r: &mut Reader<'_>, count: usize) -> Result<Vec<Option<u32>>, String> {
    let all_defined = r.u8()?;
    let defined: Vec<bool> = if all_defined == 0 {
        let bits = r.bytes(count.div_ceil(8))?;
        (0..count)
            .map(|i| bits[i / 8] & (0x80 >> (i % 8)) != 0)
            .collect()
    } else {
        vec![true; count]
    };

    let mut digests = Vec::with_capacity(count);
    for is_defined in defined {
        if is_defined {
            let bytes = r.bytes(4)?;
            digests.push(Some(u32::from_le_bytes(bytes.try_into().unwrap())));
        } else {
            digests.push(None);
        }
    }

    Ok(digests)
}

fn skip_digests(r: &mut Reader<'_>, count: usize) -> Result<(), String> {
    let all_defined = r.u8()?;
    let defined = if all_defined == 0 {
        let bits = r.bytes(count.div_ceil(8))?;
        (0..count)
            .filter(|i| bits[i / 8] & (0x80 >> (i % 8)) != 0)
            .count()
    } else {
        count
    };
    r.bytes(defined * 4)?;
    Ok(())
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn u8(&mut self) -> Result<u8, String> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or_else(|| format!("header ends at byte {}, more was expected", self.pos))?;
        self.pos += 1;
        Ok(byte)
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(len).ok_or("length overflows")?;
        if end > self.data.len() {
            return Err(format!(
                "header wants {len} bytes at offset {} but only {} remain",
                self.pos,
                self.data.len() - self.pos
            ));
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Reads a 7z variable-length number.
    ///
    /// The leading byte's high bits say how many further bytes follow, and its
    /// remaining low bits supply the value's high part.
    fn number(&mut self) -> Result<u64, String> {
        let first = self.u8()?;
        let mut mask = 0x80u8;
        let mut value = 0u64;

        for i in 0..8 {
            if first & mask == 0 {
                let high = u64::from(first & (mask.wrapping_sub(1)));
                return Ok(value | (high << (8 * i)));
            }
            let byte = self.u8()?;
            value |= u64::from(byte) << (8 * i);
            mask >>= 1;
        }

        Ok(value)
    }
}

/// CRC-32 (IEEE), computed here so the check does not lean on the crate's copy.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn test_number_decoding() {
        let cases: &[(&[u8], u64)] = &[
            (&[0x00], 0),
            (&[0x50], 0x50),
            (&[0x7F], 0x7F),
            (&[0x80, 0x90], 0x90),
            (&[0x80, 0x84], 0x84),
            (&[0x81, 0x20], 0x120),
            (&[0x83, 0x3A], 0x33A),
            (&[0xE0, 0x80, 0xC7, 0x2D], 0x2D_C780),
        ];

        for (bytes, expected) in cases {
            let mut r = Reader::new(bytes);
            assert_eq!(r.number().unwrap(), *expected, "decoding {bytes:02x?}");
        }
    }

    /// The layout zesven 1.1.0 produced for header-encrypted archives.
    ///
    /// Reading it back through our own reader used to succeed, which is the
    /// whole reason this validator exists; it must not succeed here.
    #[test]
    fn test_rejects_header_encrypted_layout_from_1_1_0() {
        // Captured from an archive written by 1.1.0: PackPos = 0 and the
        // ciphertext carried inline behind the ENCODED_HEADER structure.
        const ARCHIVE: &[u8] = include_bytes!("fixtures/legacy_header_encrypted.7z");

        let problem = check_archive(ARCHIVE).expect_err("the 1.1.0 layout must be rejected");
        assert!(
            problem.contains("packed header stream"),
            "expected a complaint about the packed header stream, got: {problem}"
        );
    }

    #[test]
    fn test_rejects_truncated_input() {
        assert!(check_archive(&[0u8; 8]).is_err());
        assert!(check_archive(SIGNATURE).is_err());
    }
}
