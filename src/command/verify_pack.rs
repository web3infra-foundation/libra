//! Implements `verify-pack` for validating `.idx` files against their pack.
//!
//! 实现 `verify-pack` 以验证 `.idx` 文件与其 pack 的对应关系。

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use clap::Parser;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind, set_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        object::types::ObjectType,
        pack::{Pack, entry::Entry, pack_index::IndexEntry},
    },
    utils::HashAlgorithm,
};
use serde::Serialize;
use sha1::{Digest, Sha1};

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
};

const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
const FANOUT_LEN: usize = 256 * 4;

const VERIFY_PACK_EXAMPLES: &str = "\
EXAMPLES:
    libra verify-pack objects/pack/pack-abc123.idx                   Verify an index against its sibling .pack
    libra verify-pack --pack pack.pack pack.idx                      Verify with an explicit pack path
    libra verify-pack -v pack-abc123.idx                             Print every indexed object hash and offset
    libra verify-pack pack-abc123.idx --json                         Structured JSON output for agents";

/// Command-line options for `libra verify-pack`.
#[derive(Parser, Debug)]
#[command(after_help = VERIFY_PACK_EXAMPLES)]
pub struct VerifyPackArgs {
    /// Pack index file to verify
    #[arg(value_name = "IDX_FILE")]
    pub idx_file: PathBuf,

    /// Pack file to verify against. Defaults to IDX_FILE with `.pack` extension.
    #[arg(long, value_name = "PACK_FILE")]
    pub pack: Option<PathBuf>,

    /// Print every indexed object hash and offset
    #[arg(short, long)]
    pub verbose: bool,
}

/// Full verification result used by human and JSON renderers.
#[derive(Debug, Clone, Serialize)]
struct VerifyPackOutput {
    /// Pack index path shown to the caller.
    idx_file: String,
    /// Pack data path verified against the index.
    pack_file: String,
    /// Parsed pack-index format version.
    index_version: u8,
    /// Number of objects listed by the index.
    object_count: usize,
    /// Pack checksum recorded in the index trailer.
    pack_hash: String,
    /// Index checksum recorded in the index trailer.
    index_hash: String,
    /// Whether the pack/index pair passed validation.
    verified: bool,
    /// Per-object details emitted in verbose mode.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    objects: Vec<VerifyPackObjectOutput>,
}

/// Verbose verification details for one packed object.
#[derive(Debug, Clone, Serialize)]
struct VerifyPackObjectOutput {
    /// Object ID listed in the index.
    oid: String,
    /// Decoded object type from the pack entry.
    object_type: String,
    /// Uncompressed object size.
    size: usize,
    /// Number of bytes occupied by this entry in the pack file.
    size_in_pack: u64,
    /// Offset of the entry inside the pack file.
    offset: u64,
    /// Optional CRC32 stored by version 2 pack indexes.
    #[serde(skip_serializing_if = "Option::is_none")]
    crc32: Option<u32>,
}

/// Parsed representation of a pack index file.
#[derive(Debug, Clone)]
struct ParsedIndex {
    /// Pack-index version.
    version: u8,
    /// Sorted index entries.
    entries: Vec<ParsedIndexEntry>,
    /// Pack checksum recorded by the index.
    pack_hash: ObjectHash,
    /// Index checksum bytes recorded by the index.
    index_hash: Vec<u8>,
}

/// One object entry parsed from a pack index.
#[derive(Debug, Clone)]
struct ParsedIndexEntry {
    /// Object ID.
    hash: ObjectHash,
    /// Pack-file offset.
    offset: u64,
    /// Optional CRC32 for version 2 indexes.
    crc32: Option<u32>,
}

/// Pack decoding result keyed by object ID.
#[derive(Debug, Clone)]
struct DecodedPack {
    /// Pack checksum computed while decoding.
    pack_hash: ObjectHash,
    /// Pack file length in bytes.
    pack_len: u64,
    /// Decoded entries by object ID.
    entries: BTreeMap<ObjectHash, DecodedPackEntry>,
}

/// Metadata extracted from one decoded pack entry.
#[derive(Debug, Clone)]
struct DecodedPackEntry {
    /// Index metadata derived from the decoded entry.
    index: IndexEntry,
    /// Decoded object type.
    object_type: ObjectType,
    /// Uncompressed object size.
    size: usize,
}

/// Run `verify-pack` with default human-output configuration.
pub async fn execute(args: VerifyPackArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// # Side Effects
///
/// This command is read-only. It reads the requested `.idx` file and matching
/// `.pack` file, decodes the pack, and reports whether the index is consistent.
///
/// # Errors
///
/// Returns structured CLI errors for unreadable files and repository-corruption
/// errors for malformed indexes, malformed packs, or index/pack mismatches.
pub async fn execute_safe(args: VerifyPackArgs, output: &OutputConfig) -> CliResult<()> {
    let result = verify_pack(&args)?;
    render_verify_pack_output(&result, args.verbose, output)
}

/// Minimal pack/index verification summary reused by maintenance commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackInspection {
    /// Number of objects recorded in the pack index.
    pub object_count: usize,
    /// Pack index format version.
    pub index_version: u8,
    /// Hash of the verified pack payload stored in the index trailer.
    pub pack_hash: String,
}

/// Verify an explicit `.idx` / `.pack` pair and return reusable pack metadata.
pub(crate) fn inspect_pack_files(idx_file: &Path, pack_file: &Path) -> CliResult<PackInspection> {
    let idx_bytes = read_file(idx_file, "pack index")?;
    if let Some(hash_kind) =
        infer_idx_v2_hash_kind(&idx_bytes).map_err(|detail| invalid_index(idx_file, detail))?
    {
        set_hash_kind(hash_kind);
    }
    let parsed = parse_index(&idx_bytes).map_err(|detail| invalid_index(idx_file, detail))?;
    let decoded = decode_pack(pack_file)?;
    validate_index_against_pack(&parsed, &decoded)
        .map_err(|detail| verification_failed(idx_file, pack_file, detail))?;
    Ok(PackInspection {
        object_count: parsed.entries.len(),
        index_version: parsed.version,
        pack_hash: parsed.pack_hash.to_string(),
    })
}

/// Verify command arguments and build the full verification output.
fn verify_pack(args: &VerifyPackArgs) -> CliResult<VerifyPackOutput> {
    let idx_file = args.idx_file.clone();
    let pack_file = args
        .pack
        .clone()
        .unwrap_or_else(|| idx_file.with_extension("pack"));

    let idx_bytes = read_file(&idx_file, "pack index")?;
    if let Some(hash_kind) =
        infer_idx_v2_hash_kind(&idx_bytes).map_err(|detail| invalid_index(&idx_file, detail))?
    {
        set_hash_kind(hash_kind);
    }
    let parsed = parse_index(&idx_bytes).map_err(|detail| invalid_index(&idx_file, detail))?;
    let decoded = decode_pack(&pack_file)?;
    validate_index_against_pack(&parsed, &decoded)
        .map_err(|detail| verification_failed(&idx_file, &pack_file, detail))?;

    let objects = if args.verbose {
        let size_in_pack_by_hash = pack_entry_sizes(&parsed, decoded.pack_len)?;
        parsed
            .entries
            .iter()
            .map(|entry| {
                let decoded_entry = decoded.entries.get(&entry.hash).ok_or_else(|| {
                    CliError::fatal(format!(
                        "decoded pack metadata for indexed object {} is missing",
                        entry.hash
                    ))
                    .with_stable_code(StableErrorCode::InternalInvariant)
                })?;
                let size_in_pack = *size_in_pack_by_hash.get(&entry.hash).ok_or_else(|| {
                    CliError::fatal(format!(
                        "pack size metadata for indexed object {} is missing",
                        entry.hash
                    ))
                    .with_stable_code(StableErrorCode::InternalInvariant)
                })?;
                Ok(VerifyPackObjectOutput {
                    oid: entry.hash.to_string(),
                    object_type: decoded_entry.object_type.to_string(),
                    size: decoded_entry.size,
                    size_in_pack,
                    offset: entry.offset,
                    crc32: entry.crc32,
                })
            })
            .collect::<CliResult<Vec<_>>>()?
    } else {
        Vec::new()
    };

    Ok(VerifyPackOutput {
        idx_file: path_string(&idx_file),
        pack_file: path_string(&pack_file),
        index_version: parsed.version,
        object_count: parsed.entries.len(),
        pack_hash: parsed.pack_hash.to_string(),
        index_hash: bytes_to_hex(&parsed.index_hash),
        verified: true,
        objects,
    })
}

/// Parse a pack index in either version 1 or version 2 format.
fn parse_index(bytes: &[u8]) -> Result<ParsedIndex, String> {
    if bytes.starts_with(&IDX_MAGIC) {
        parse_idx_v2(bytes)
    } else {
        parse_idx_v1(bytes)
    }
}

/// Infer SHA-1 or SHA-256 object format from a version 2 index layout.
fn infer_idx_v2_hash_kind(bytes: &[u8]) -> Result<Option<HashKind>, String> {
    if !bytes.starts_with(&IDX_MAGIC) {
        return Ok(None);
    }

    let version = u32::from_be_bytes(
        bytes
            .get(4..8)
            .ok_or_else(|| "truncated v2 version".to_string())?
            .try_into()
            .map_err(|_| "truncated v2 version".to_string())?,
    );
    if version != 2 {
        return Ok(None);
    }

    let fanout = parse_fanout(bytes, 8)?;
    validate_fanout_monotonic(&fanout)?;
    let object_count = fanout[255] as usize;
    let mut candidates = [HashKind::Sha1, HashKind::Sha256]
        .into_iter()
        .filter(|kind| idx_v2_layout_matches_hash_kind(bytes, object_count, *kind))
        .collect::<Vec<_>>();

    match candidates.len() {
        0 => Err("pack index v2 layout does not match sha1 or sha256".to_string()),
        1 => Ok(candidates.pop()),
        _ => {
            let current = get_hash_kind();
            if candidates.contains(&current) {
                Ok(Some(current))
            } else {
                Ok(candidates.into_iter().next())
            }
        }
    }
}

/// Return whether a version 2 index can fit the requested hash length.
fn idx_v2_layout_matches_hash_kind(bytes: &[u8], object_count: usize, kind: HashKind) -> bool {
    let hash_len = kind.size();
    let Some(mut cursor) = (8 + FANOUT_LEN).checked_add(object_count.saturating_mul(hash_len))
    else {
        return false;
    };
    if cursor > bytes.len() {
        return false;
    }

    let Some(crc_end) = cursor.checked_add(object_count.saturating_mul(4)) else {
        return false;
    };
    if crc_end > bytes.len() {
        return false;
    }
    cursor = crc_end;

    let Some(offsets_end) = cursor.checked_add(object_count.saturating_mul(4)) else {
        return false;
    };
    if offsets_end > bytes.len() {
        return false;
    }
    let offset_table = &bytes[cursor..offsets_end];
    cursor = offsets_end;

    let large_count = offset_table
        .chunks_exact(4)
        .filter(|raw| {
            let offset = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
            offset & 0x8000_0000 != 0
        })
        .count();
    let Some(trailer_start) = cursor.checked_add(large_count.saturating_mul(8)) else {
        return false;
    };
    if trailer_start > bytes.len() {
        return false;
    }

    let remaining = bytes.len() - trailer_start;
    remaining == hash_len * 2 || remaining == hash_len + 20
}

/// Parse a version 1 pack index.
fn parse_idx_v1(bytes: &[u8]) -> Result<ParsedIndex, String> {
    const HASH_LEN: usize = 20;
    const ENTRY_LEN: usize = 4 + HASH_LEN;
    const TRAILER_LEN: usize = HASH_LEN * 2;

    if get_hash_kind() != HashKind::Sha1 {
        return Err("pack index v1 only supports sha1 repositories".to_string());
    }
    if bytes.len() < FANOUT_LEN + TRAILER_LEN {
        return Err("pack index v1 is too short".to_string());
    }

    let fanout = parse_fanout(bytes, 0)?;
    validate_fanout_monotonic(&fanout)?;
    let object_count = fanout[255] as usize;
    let entries_start = FANOUT_LEN;
    let entries_end = entries_start + object_count * ENTRY_LEN;
    let expected_len = entries_end + TRAILER_LEN;
    if bytes.len() != expected_len {
        return Err(format!(
            "pack index v1 length {} does not match fanout object count {}",
            bytes.len(),
            object_count
        ));
    }

    let mut entries = Vec::with_capacity(object_count);
    for i in 0..object_count {
        let start = entries_start + i * ENTRY_LEN;
        let offset = u32::from_be_bytes(
            bytes[start..start + 4]
                .try_into()
                .map_err(|_| "truncated v1 offset".to_string())?,
        ) as u64;
        let hash = ObjectHash::from_bytes(&bytes[start + 4..start + ENTRY_LEN])
            .map_err(|error| format!("invalid v1 object hash: {error}"))?;
        entries.push(ParsedIndexEntry {
            hash,
            offset,
            crc32: None,
        });
    }

    validate_sorted_entries(&entries)?;
    validate_fanout_matches_entries(&fanout, &entries)?;

    let pack_hash = ObjectHash::from_bytes(&bytes[entries_end..entries_end + HASH_LEN])
        .map_err(|error| format!("invalid v1 pack hash: {error}"))?;
    let index_hash = bytes[entries_end + HASH_LEN..expected_len].to_vec();
    let computed_hash: [u8; HASH_LEN] = Sha1::digest(&bytes[..expected_len - HASH_LEN]).into();
    if index_hash != computed_hash {
        return Err("pack index v1 checksum mismatch".to_string());
    }

    Ok(ParsedIndex {
        version: 1,
        entries,
        pack_hash,
        index_hash,
    })
}

/// Parse a version 2 pack index.
fn parse_idx_v2(bytes: &[u8]) -> Result<ParsedIndex, String> {
    let hash_len = get_hash_kind().size();
    if bytes.len() < 8 + FANOUT_LEN + hash_len * 2 {
        return Err("pack index v2 is too short".to_string());
    }
    if bytes[0..4] != IDX_MAGIC {
        return Err("pack index v2 magic mismatch".to_string());
    }
    let version = u32::from_be_bytes(
        bytes[4..8]
            .try_into()
            .map_err(|_| "truncated v2 version".to_string())?,
    );
    if version != 2 {
        return Err(format!("unsupported pack index version {version}"));
    }

    let fanout = parse_fanout(bytes, 8)?;
    validate_fanout_monotonic(&fanout)?;
    let object_count = fanout[255] as usize;
    let mut cursor = 8 + FANOUT_LEN;

    let names_end = cursor + object_count * hash_len;
    if names_end > bytes.len() {
        return Err("pack index v2 object names are truncated".to_string());
    }
    let names = &bytes[cursor..names_end];
    cursor = names_end;

    let crc_end = cursor + object_count * 4;
    if crc_end > bytes.len() {
        return Err("pack index v2 crc32 table is truncated".to_string());
    }
    let crc_table = &bytes[cursor..crc_end];
    cursor = crc_end;

    let offsets_end = cursor + object_count * 4;
    if offsets_end > bytes.len() {
        return Err("pack index v2 offset table is truncated".to_string());
    }
    let offset_table = &bytes[cursor..offsets_end];
    cursor = offsets_end;

    let mut large_count = 0usize;
    for raw in offset_table.chunks_exact(4) {
        let offset = u32::from_be_bytes(
            raw.try_into()
                .map_err(|_| "truncated v2 offset".to_string())?,
        );
        if offset & 0x8000_0000 != 0 {
            large_count += 1;
        }
    }
    let large_offsets_end = cursor + large_count * 8;
    let trailer_start = large_offsets_end;
    let remaining = bytes.len().saturating_sub(trailer_start);
    if remaining != hash_len * 2 && remaining != hash_len + 20 {
        return Err(
            "pack index v2 length does not match fanout and large-offset tables".to_string(),
        );
    }
    let index_hash_len = remaining - hash_len;

    let mut large_offsets = Vec::with_capacity(large_count);
    for chunk in bytes[cursor..large_offsets_end].chunks_exact(8) {
        large_offsets.push(u64::from_be_bytes(
            chunk
                .try_into()
                .map_err(|_| "truncated v2 large offset".to_string())?,
        ));
    }

    let mut entries = Vec::with_capacity(object_count);
    for i in 0..object_count {
        let hash_start = i * hash_len;
        let hash_end = hash_start + hash_len;
        let hash = ObjectHash::from_bytes(&names[hash_start..hash_end])
            .map_err(|error| format!("invalid v2 object hash: {error}"))?;

        let crc_start = i * 4;
        let crc32 = u32::from_be_bytes(
            crc_table[crc_start..crc_start + 4]
                .try_into()
                .map_err(|_| "truncated v2 crc32".to_string())?,
        );

        let offset_start = i * 4;
        let raw_offset = u32::from_be_bytes(
            offset_table[offset_start..offset_start + 4]
                .try_into()
                .map_err(|_| "truncated v2 offset".to_string())?,
        );
        let offset = if raw_offset & 0x8000_0000 == 0 {
            raw_offset as u64
        } else {
            let large_index = (raw_offset & 0x7FFF_FFFF) as usize;
            *large_offsets
                .get(large_index)
                .ok_or_else(|| format!("v2 large-offset index {large_index} is out of range"))?
        };

        entries.push(ParsedIndexEntry {
            hash,
            offset,
            crc32: Some(crc32),
        });
    }

    validate_sorted_entries(&entries)?;
    validate_fanout_matches_entries(&fanout, &entries)?;

    let pack_hash = ObjectHash::from_bytes(&bytes[trailer_start..trailer_start + hash_len])
        .map_err(|error| format!("invalid v2 pack hash: {error}"))?;
    let index_hash = bytes[trailer_start + hash_len..].to_vec();

    let computed_git_hash = hash_bytes(&bytes[..bytes.len() - index_hash_len]);
    let computed_libra_hash = hash_bytes(&bytes[..trailer_start]);
    let computed_legacy_sha1_libra_hash = sha1_bytes(&bytes[..trailer_start]);
    if index_hash != computed_git_hash
        && index_hash != computed_libra_hash
        && index_hash != computed_legacy_sha1_libra_hash
    {
        return Err("pack index v2 checksum mismatch".to_string());
    }

    Ok(ParsedIndex {
        version: 2,
        entries,
        pack_hash,
        index_hash,
    })
}

/// Read the 256-entry fanout table from a pack index.
fn parse_fanout(bytes: &[u8], offset: usize) -> Result<[u32; 256], String> {
    if bytes.len() < offset + FANOUT_LEN {
        return Err("pack index fanout table is truncated".to_string());
    }

    let mut fanout = [0u32; 256];
    for (slot, chunk) in fanout
        .iter_mut()
        .zip(bytes[offset..offset + FANOUT_LEN].chunks_exact(4))
    {
        *slot = u32::from_be_bytes(
            chunk
                .try_into()
                .map_err(|_| "truncated fanout entry".to_string())?,
        );
    }
    Ok(fanout)
}

/// Validate that the fanout table is monotonically increasing.
fn validate_fanout_monotonic(fanout: &[u32; 256]) -> Result<(), String> {
    for pair in fanout.windows(2) {
        if pair[0] > pair[1] {
            return Err("pack index fanout table is not monotonic".to_string());
        }
    }
    Ok(())
}

/// Validate that pack-index object names are strictly sorted.
fn validate_sorted_entries(entries: &[ParsedIndexEntry]) -> Result<(), String> {
    for pair in entries.windows(2) {
        if pair[0].hash >= pair[1].hash {
            return Err(format!(
                "pack index object hashes are not strictly sorted: {} >= {}",
                pair[0].hash, pair[1].hash
            ));
        }
    }
    Ok(())
}

/// Validate fanout bucket counts against parsed index entries.
fn validate_fanout_matches_entries(
    fanout: &[u32; 256],
    entries: &[ParsedIndexEntry],
) -> Result<(), String> {
    let mut computed = [0u32; 256];
    for entry in entries {
        computed[entry.hash.as_ref()[0] as usize] += 1;
    }
    for i in 1..computed.len() {
        computed[i] += computed[i - 1];
    }
    if &computed != fanout {
        return Err("pack index fanout table does not match object names".to_string());
    }
    Ok(())
}

/// Decode a pack archive into objects and packed sizes.
fn decode_pack(pack_file: &Path) -> CliResult<DecodedPack> {
    let file = fs::File::open(pack_file).map_err(|error| {
        CliError::fatal(format!(
            "could not open pack file '{}' for reading: {}",
            pack_file.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    let pack_len = file
        .metadata()
        .map_err(|error| {
            CliError::fatal(format!(
                "could not inspect pack file '{}' metadata: {}",
                pack_file.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?
        .len();
    let mut reader = io::BufReader::new(file);
    let entries = Arc::new(Mutex::new(Vec::new()));
    let entries_clone = Arc::clone(&entries);
    let tmp_path = pack_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let mut pack = Pack::new(Some(8), Some(1024 * 1024 * 1024), Some(tmp_path), true);
    pack.decode(
        &mut reader,
        move |entry: MetaAttached<Entry, EntryMeta>| {
            let decoded_entry = IndexEntry::try_from(&entry)
                .map(|index| DecodedPackEntry {
                    index,
                    object_type: entry.inner.obj_type,
                    size: entry.inner.data.len(),
                })
                .map_err(|error| error.to_string());
            if let Ok(mut guard) = entries_clone.lock() {
                guard.push(decoded_entry);
            }
        },
        None::<fn(ObjectHash)>,
    )
    .map_err(|error| {
        CliError::fatal(format!(
            "failed to decode pack file '{}': {error}",
            pack_file.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let mut decoded_entries = BTreeMap::new();
    let entries = take_mutex(entries, "verify-pack entries").map_err(|detail| {
        CliError::fatal(format!("failed to collect decoded pack entries: {detail}"))
            .with_stable_code(StableErrorCode::InternalInvariant)
    })?;
    for entry in entries {
        let decoded_entry = entry.map_err(|error| {
            CliError::fatal(format!(
                "failed to derive index metadata from pack '{}': {error}",
                pack_file.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        insert_decoded_pack_entry(&mut decoded_entries, decoded_entry).map_err(|detail| {
            CliError::fatal(format!(
                "failed to decode pack file '{}': {detail}",
                pack_file.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
    }

    Ok(DecodedPack {
        pack_hash: pack.signature,
        pack_len,
        entries: decoded_entries,
    })
}

/// Insert a decoded pack entry while rejecting duplicate object IDs.
fn insert_decoded_pack_entry(
    entries: &mut BTreeMap<ObjectHash, DecodedPackEntry>,
    entry: DecodedPackEntry,
) -> Result<(), String> {
    let hash = entry.index.hash;
    if entries.insert(hash, entry).is_some() {
        return Err(format!("pack contains duplicate object ID {hash}"));
    }
    Ok(())
}

/// Compute packed entry sizes from index offsets and pack length.
fn pack_entry_sizes(index: &ParsedIndex, pack_len: u64) -> CliResult<BTreeMap<ObjectHash, u64>> {
    let trailer_start = pack_len
        .checked_sub(get_hash_kind().size() as u64)
        .ok_or_else(|| {
            CliError::fatal("pack file is shorter than its trailing checksum")
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

    let mut by_offset = index.entries.iter().collect::<Vec<_>>();
    by_offset.sort_by_key(|entry| entry.offset);

    let mut sizes = BTreeMap::new();
    for (idx, entry) in by_offset.iter().enumerate() {
        let next_offset = by_offset
            .get(idx + 1)
            .map(|next| next.offset)
            .unwrap_or(trailer_start);
        let size_in_pack = next_offset.checked_sub(entry.offset).ok_or_else(|| {
            CliError::fatal(format!(
                "pack entry offsets are not monotonically increasing near {}",
                entry.hash
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        sizes.insert(entry.hash, size_in_pack);
    }
    Ok(sizes)
}

/// Extract a value from an `Arc<Mutex<_>>` after pack decoding completes.
fn take_mutex<T>(arc: Arc<Mutex<T>>, label: &str) -> Result<T, String> {
    let mutex =
        Arc::try_unwrap(arc).map_err(|_| format!("{label} still has outstanding references"))?;
    mutex
        .into_inner()
        .map_err(|_| format!("{label} mutex poisoned"))
}

/// Compare parsed index metadata with decoded pack contents.
fn validate_index_against_pack(index: &ParsedIndex, pack: &DecodedPack) -> Result<(), String> {
    if index.pack_hash != pack.pack_hash {
        return Err(format!(
            "pack checksum mismatch: index has {}, pack has {}",
            index.pack_hash, pack.pack_hash
        ));
    }

    if index.entries.len() != pack.entries.len() {
        return Err(format!(
            "object count mismatch: index has {}, pack has {}",
            index.entries.len(),
            pack.entries.len()
        ));
    }

    for entry in &index.entries {
        let Some(pack_entry) = pack.entries.get(&entry.hash) else {
            return Err(format!(
                "indexed object {} is missing from pack",
                entry.hash
            ));
        };
        if entry.offset != pack_entry.index.offset {
            return Err(format!(
                "offset mismatch for {}: index has {}, pack has {}",
                entry.hash, entry.offset, pack_entry.index.offset
            ));
        }
        if let Some(crc32) = entry.crc32
            && crc32 != pack_entry.index.crc32
        {
            return Err(format!(
                "crc32 mismatch for {}: index has {crc32:#010x}, pack has {:#010x}",
                entry.hash, pack_entry.index.crc32
            ));
        }
    }

    Ok(())
}

/// Render verification results as human output or JSON.
fn render_verify_pack_output(
    result: &VerifyPackOutput,
    verbose: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("verify-pack", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    if verbose {
        for object in &result.objects {
            println!(
                "{} {} {} {} {}",
                object.oid, object.object_type, object.size, object.size_in_pack, object.offset
            );
        }
    }
    println!("{}: ok", result.idx_file);
    Ok(())
}

/// Read a file and convert failures into stable CLI I/O errors.
fn read_file(path: &Path, label: &str) -> CliResult<Vec<u8>> {
    fs::read(path).map_err(|error| {
        CliError::fatal(format!(
            "could not open {label} '{}' for reading: {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })
}

/// Build the structured error used for malformed index files.
fn invalid_index(path: &Path, detail: String) -> CliError {
    CliError::fatal(format!("invalid pack index '{}': {detail}", path.display()))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

/// Build the structured error used for pack/index verification failures.
fn verification_failed(idx_file: &Path, pack_file: &Path, detail: String) -> CliError {
    CliError::fatal(format!(
        "pack verification failed for '{}' against '{}': {detail}",
        idx_file.display(),
        pack_file.display()
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}

/// Hash bytes using the repository's active object format.
fn hash_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut hash = HashAlgorithm::new();
    hash.update(bytes);
    hash.finalize()
}

/// Compute a SHA-1 digest for version 1 pack-index checksums.
fn sha1_bytes(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}

/// Convert bytes to lowercase hexadecimal text.
fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Convert a path to a display string for command output.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Normalize common filesystem errors for human-readable messages.
fn format_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use git_internal::hash::{HashKind, set_hash_kind_for_test};

    use super::*;

    /// Build a decoded pack entry for unit tests.
    fn decoded_entry(hash: ObjectHash) -> DecodedPackEntry {
        DecodedPackEntry {
            index: IndexEntry {
                hash,
                crc32: 0,
                offset: 12,
            },
            object_type: ObjectType::Blob,
            size: 5,
        }
    }

    #[test]
    /// Covers duplicate object detection while inserting decoded entries.
    fn insert_decoded_pack_entry_rejects_duplicate_hashes() {
        let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
        let hash = ObjectHash::new(b"duplicate");
        let mut entries = BTreeMap::new();

        insert_decoded_pack_entry(&mut entries, decoded_entry(hash)).expect("first insert");
        let err = insert_decoded_pack_entry(&mut entries, decoded_entry(hash))
            .expect_err("duplicate hash should fail");

        assert!(err.contains("duplicate object ID"));
    }
}
