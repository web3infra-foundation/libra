use super::verify_pack_types::ParsedIndexEntry;

pub(super) const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
pub(super) const FANOUT_LEN: usize = 256 * 4;

pub(super) fn parse_fanout(bytes: &[u8], offset: usize) -> Result<[u32; 256], String> {
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

pub(super) fn validate_fanout_monotonic(fanout: &[u32; 256]) -> Result<(), String> {
    for pair in fanout.windows(2) {
        if pair[0] > pair[1] {
            return Err("pack index fanout table is not monotonic".to_string());
        }
    }
    Ok(())
}

pub(super) fn validate_sorted_entries(entries: &[ParsedIndexEntry]) -> Result<(), String> {
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

pub(super) fn validate_fanout_matches_entries(
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
