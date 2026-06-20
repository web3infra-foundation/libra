use git_internal::hash::{HashKind, ObjectHash, get_hash_kind};
use sha1::{Digest, Sha1};

pub(crate) use super::verify_pack_index_v2::infer_idx_v2_hash_kind;
use super::{
    verify_pack_index_common::{
        FANOUT_LEN, IDX_MAGIC, parse_fanout, validate_fanout_matches_entries,
        validate_fanout_monotonic, validate_sorted_entries,
    },
    verify_pack_index_v2::parse_idx_v2,
    verify_pack_types::{ParsedIndex, ParsedIndexEntry},
};

pub(crate) fn parse_index(bytes: &[u8]) -> Result<ParsedIndex, String> {
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
