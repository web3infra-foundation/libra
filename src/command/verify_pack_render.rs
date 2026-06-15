use std::collections::BTreeMap;

use super::{
    verify_pack_decode::pack_entry_sizes,
    verify_pack_types::{
        DecodedPack, ParsedIndex, VerifyPackObjectOutput, VerifyPackOutput, VerifyPackStats,
    },
};
use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::{OutputConfig, emit_json_data},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifyPackRenderMode {
    Summary,
    Verbose,
    StatOnly,
}

pub(crate) fn build_object_outputs(
    index: &ParsedIndex,
    pack: &DecodedPack,
) -> CliResult<Vec<VerifyPackObjectOutput>> {
    let size_in_pack_by_hash = pack_entry_sizes(index, pack.pack_len)?;
    index
        .entries
        .iter()
        .map(|entry| {
            let decoded_entry = pack.entries.get(&entry.hash).ok_or_else(|| {
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
        .collect::<CliResult<Vec<_>>>()
}

pub(crate) fn build_stats(pack: &DecodedPack) -> VerifyPackStats {
    let mut non_delta = 0usize;
    let mut chain_lengths = BTreeMap::new();
    for entry in pack.entries.values() {
        if entry.chain_len == 0 {
            non_delta += 1;
        } else {
            *chain_lengths.entry(entry.chain_len).or_insert(0) += 1;
        }
    }

    VerifyPackStats {
        non_delta,
        chain_lengths,
    }
}

pub(crate) fn render_verify_pack_output(
    result: &VerifyPackOutput,
    mode: VerifyPackRenderMode,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("verify-pack", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match mode {
        VerifyPackRenderMode::Summary => println!("{}: ok", result.idx_file),
        VerifyPackRenderMode::Verbose => {
            for object in &result.objects {
                println!(
                    "{} {} {} {} {}",
                    object.oid, object.object_type, object.size, object.size_in_pack, object.offset
                );
            }
            println!("{}: ok", result.idx_file);
        }
        VerifyPackRenderMode::StatOnly => {
            let Some(stats) = result.stats.as_ref() else {
                return Err(CliError::fatal(
                    "verify-pack stat-only output was requested without computed statistics",
                )
                .with_stable_code(StableErrorCode::InternalInvariant));
            };
            print_stats(stats);
        }
    }
    Ok(())
}

fn print_stats(stats: &VerifyPackStats) {
    println!("non delta: {} objects", stats.non_delta);
    for (chain_len, count) in &stats.chain_lengths {
        println!("chain length = {chain_len}: {count} objects");
    }
}
