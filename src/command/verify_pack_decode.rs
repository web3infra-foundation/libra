use std::{
    collections::BTreeMap,
    fs, io,
    path::Path,
    sync::{Arc, Mutex},
};

use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        pack::{Pack, entry::Entry, pack_index::IndexEntry},
    },
};

use super::{
    verify_pack_support::format_io_error,
    verify_pack_types::{DecodedPack, DecodedPackEntry, ParsedIndex},
};
use crate::utils::error::{CliError, CliResult, StableErrorCode};

pub(crate) fn decode_pack(pack_file: &Path) -> CliResult<DecodedPack> {
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
                    chain_len: entry.inner.chain_len,
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

pub(crate) fn pack_entry_sizes(
    index: &ParsedIndex,
    pack_len: u64,
) -> CliResult<BTreeMap<ObjectHash, u64>> {
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

fn take_mutex<T>(arc: Arc<Mutex<T>>, label: &str) -> Result<T, String> {
    let mutex =
        Arc::try_unwrap(arc).map_err(|_| format!("{label} still has outstanding references"))?;
    mutex
        .into_inner()
        .map_err(|_| format!("{label} mutex poisoned"))
}

pub(crate) fn validate_index_against_pack(
    index: &ParsedIndex,
    pack: &DecodedPack,
) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use git_internal::{
        hash::{HashKind, set_hash_kind_for_test},
        internal::object::types::ObjectType,
    };

    use super::*;

    fn decoded_entry(hash: ObjectHash) -> DecodedPackEntry {
        DecodedPackEntry {
            index: IndexEntry {
                hash,
                crc32: 0,
                offset: 12,
            },
            object_type: ObjectType::Blob,
            size: 5,
            chain_len: 0,
        }
    }

    #[test]
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
