//! Implements `verify-pack` for validating `.idx` files against their pack.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use clap::Parser;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
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
Examples:
  libra verify-pack objects/pack/pack-abc123.idx
  libra verify-pack --pack objects/pack/pack-abc123.pack objects/pack/pack-abc123.idx
  libra verify-pack -v pack-abc123.idx
  libra verify-pack pack-abc123.idx --json
";

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

#[derive(Debug, Clone, Serialize)]
struct VerifyPackOutput {
    idx_file: String,
    pack_file: String,
    index_version: u8,
    object_count: usize,
    pack_hash: String,
    index_hash: String,
    verified: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    objects: Vec<VerifyPackObjectOutput>,
}

#[derive(Debug, Clone, Serialize)]
struct VerifyPackObjectOutput {
    oid: String,
    offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    crc32: Option<u32>,
}

#[derive(Debug, Clone)]
struct ParsedIndex {
    version: u8,
    entries: Vec<ParsedIndexEntry>,
    pack_hash: ObjectHash,
    index_hash: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ParsedIndexEntry {
    hash: ObjectHash,
    offset: u64,
    crc32: Option<u32>,
}

#[derive(Debug, Clone)]
struct DecodedPack {
    pack_hash: ObjectHash,
    entries: BTreeMap<ObjectHash, IndexEntry>,
}

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

fn verify_pack(args: &VerifyPackArgs) -> CliResult<VerifyPackOutput> {
    let idx_file = args.idx_file.clone();
    let pack_file = args
        .pack
        .clone()
        .unwrap_or_else(|| idx_file.with_extension("pack"));

    let idx_bytes = read_file(&idx_file, "pack index")?;
    let parsed = parse_index(&idx_bytes).map_err(|detail| invalid_index(&idx_file, detail))?;
    let decoded = decode_pack(&pack_file)?;
    validate_index_against_pack(&parsed, &decoded)
        .map_err(|detail| verification_failed(&idx_file, &pack_file, detail))?;

    let objects = if args.verbose {
        parsed
            .entries
            .iter()
            .map(|entry| VerifyPackObjectOutput {
                oid: entry.hash.to_string(),
                offset: entry.offset,
                crc32: entry.crc32,
            })
            .collect()
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

fn parse_index(bytes: &[u8]) -> Result<ParsedIndex, String> {
    if bytes.starts_with(&IDX_MAGIC) {
        parse_idx_v2(bytes)
    } else {
        parse_idx_v1(bytes)
    }
}

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
    let expected_len = trailer_start + hash_len * 2;
    if expected_len != bytes.len() {
        return Err(
            "pack index v2 length does not match fanout and large-offset tables".to_string(),
        );
    }

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
    let index_hash = bytes[trailer_start + hash_len..expected_len].to_vec();

    let computed_git_hash = hash_bytes(&bytes[..expected_len - hash_len]);
    let computed_libra_hash = hash_bytes(&bytes[..trailer_start]);
    if index_hash != computed_git_hash && index_hash != computed_libra_hash {
        return Err("pack index v2 checksum mismatch".to_string());
    }

    Ok(ParsedIndex {
        version: 2,
        entries,
        pack_hash,
        index_hash,
    })
}

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

fn validate_fanout_monotonic(fanout: &[u32; 256]) -> Result<(), String> {
    for pair in fanout.windows(2) {
        if pair[0] > pair[1] {
            return Err("pack index fanout table is not monotonic".to_string());
        }
    }
    Ok(())
}

fn validate_sorted_entries(entries: &[ParsedIndexEntry]) -> Result<(), String> {
    for pair in entries.windows(2) {
        if pair[0].hash > pair[1].hash {
            return Err(format!(
                "pack index object hashes are not sorted: {} > {}",
                pair[0].hash, pair[1].hash
            ));
        }
    }
    Ok(())
}

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

fn decode_pack(pack_file: &Path) -> CliResult<DecodedPack> {
    let file = fs::File::open(pack_file).map_err(|error| {
        CliError::fatal(format!(
            "could not open pack file '{}' for reading: {}",
            pack_file.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
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
            if let Ok(mut guard) = entries_clone.lock() {
                guard.push(entry);
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
        let index_entry = IndexEntry::try_from(&entry).map_err(|error| {
            CliError::fatal(format!(
                "failed to derive index metadata from pack '{}': {error}",
                pack_file.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        decoded_entries.insert(index_entry.hash, index_entry);
    }

    Ok(DecodedPack {
        pack_hash: pack.signature,
        entries: decoded_entries,
    })
}

fn take_mutex<T>(arc: Arc<Mutex<T>>, label: &str) -> Result<T, String> {
    let mutex =
        Arc::try_unwrap(arc).map_err(|_| format!("{label} still has outstanding references"))?;
    mutex
        .into_inner()
        .map_err(|_| format!("{label} mutex poisoned"))
}

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
        if entry.offset != pack_entry.offset {
            return Err(format!(
                "offset mismatch for {}: index has {}, pack has {}",
                entry.hash, entry.offset, pack_entry.offset
            ));
        }
        if let Some(crc32) = entry.crc32
            && crc32 != pack_entry.crc32
        {
            return Err(format!(
                "crc32 mismatch for {}: index has {crc32:#010x}, pack has {:#010x}",
                entry.hash, pack_entry.crc32
            ));
        }
    }

    Ok(())
}

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
            match object.crc32 {
                Some(crc32) => println!("{} {} {crc32:#010x}", object.oid, object.offset),
                None => println!("{} {}", object.oid, object.offset),
            }
        }
    }
    println!("{}: ok", result.idx_file);
    Ok(())
}

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

fn invalid_index(path: &Path, detail: String) -> CliError {
    CliError::fatal(format!("invalid pack index '{}': {detail}", path.display()))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

fn verification_failed(idx_file: &Path, pack_file: &Path, detail: String) -> CliError {
    CliError::fatal(format!(
        "pack verification failed for '{}' against '{}': {detail}",
        idx_file.display(),
        pack_file.display()
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}

fn hash_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut hash = HashAlgorithm::new();
    hash.update(bytes);
    hash.finalize()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn format_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => err.to_string(),
    }
}
