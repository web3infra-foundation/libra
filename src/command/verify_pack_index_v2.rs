use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    utils::HashAlgorithm,
};
use sha1::{Digest, Sha1};

use super::{
    verify_pack_index_common::{
        FANOUT_LEN, IDX_MAGIC, parse_fanout, validate_fanout_matches_entries,
        validate_fanout_monotonic, validate_sorted_entries,
    },
    verify_pack_types::{ParsedIndex, ParsedIndexEntry},
};

pub(crate) fn infer_idx_v2_hash_kind(bytes: &[u8]) -> Result<Option<HashKind>, String> {
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

pub(super) fn parse_idx_v2(bytes: &[u8]) -> Result<ParsedIndex, String> {
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

    let large_count = offset_table
        .chunks_exact(4)
        .filter(|raw| u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) & 0x8000_0000 != 0)
        .count();
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

fn hash_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut hash = HashAlgorithm::new();
    hash.update(bytes);
    hash.finalize()
}

fn sha1_bytes(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}
