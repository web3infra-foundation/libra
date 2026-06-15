//! Implements `verify-pack` for validating `.idx` files against their pack.

use std::path::PathBuf;

use clap::Parser;
use git_internal::hash::set_hash_kind;

use super::{
    verify_pack_decode::{decode_pack, validate_index_against_pack},
    verify_pack_index::{infer_idx_v2_hash_kind, parse_index},
    verify_pack_render::{
        VerifyPackRenderMode, build_object_outputs, build_stats, render_verify_pack_output,
    },
    verify_pack_support::{
        bytes_to_hex, invalid_index, path_string, read_file, verification_failed,
    },
    verify_pack_types::VerifyPackOutput,
};
use crate::utils::{error::CliResult, output::OutputConfig};

const VERIFY_PACK_EXAMPLES: &str = "\
EXAMPLES:
    libra verify-pack objects/pack/pack-abc123.idx                   Verify an index against its sibling .pack
    libra verify-pack --pack pack.pack pack.idx                      Verify with an explicit pack path
    libra verify-pack -v pack-abc123.idx                             Print every indexed object hash and offset
    libra verify-pack -s pack-abc123.idx                             Print only pack statistics
    libra verify-pack pack-abc123.idx --json                         Structured JSON output for agents";

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
    #[arg(short, long, conflicts_with = "stat_only")]
    pub verbose: bool,

    /// Show pack statistics only
    #[arg(short = 's', long = "stat-only", conflicts_with = "verbose")]
    pub stat_only: bool,
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
    let mode = render_mode(&args);
    let result = verify_pack(&args)?;
    render_verify_pack_output(&result, mode, output)
}

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
        build_object_outputs(&parsed, &decoded)?
    } else {
        Vec::new()
    };
    let stats = args.stat_only.then(|| build_stats(&decoded));

    Ok(VerifyPackOutput {
        idx_file: path_string(&idx_file),
        pack_file: path_string(&pack_file),
        index_version: parsed.version,
        object_count: parsed.entries.len(),
        pack_hash: parsed.pack_hash.to_string(),
        index_hash: bytes_to_hex(&parsed.index_hash),
        verified: true,
        stats,
        objects,
    })
}

const fn render_mode(args: &VerifyPackArgs) -> VerifyPackRenderMode {
    if args.stat_only {
        VerifyPackRenderMode::StatOnly
    } else if args.verbose {
        VerifyPackRenderMode::Verbose
    } else {
        VerifyPackRenderMode::Summary
    }
}
