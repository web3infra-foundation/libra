//! `libra cache info` — report the resolved tiered-storage / LRU cache tunables
//! (storage type, small/large threshold, LRU disk budget), exposing the existing
//! `LIBRA_STORAGE_*` knobs for inspection (lore.md §0.10). Pure inspection of the
//! resolved storage configuration; needs no repository.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::utils::{
    client_storage::{CacheConfig, resolve_cache_config},
    error::{CliError, CliResult},
    output::{OutputConfig, emit_json_data},
};

pub const CACHE_EXAMPLES: &str = "\
EXAMPLES:
    libra cache info                       Show the resolved storage/cache tunables
    LIBRA_STORAGE_TYPE=r2 LIBRA_STORAGE_CACHE_SIZE=536870912 libra cache info
    libra --json cache info                Structured { storage_type, tiered, threshold_bytes, cache_size_bytes }";

/// Inspect the tiered-storage / LRU cache configuration.
#[derive(Parser, Debug)]
#[command(after_help = CACHE_EXAMPLES)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Show the resolved storage/cache tunables (type, threshold, LRU budget).
    Info,
}

#[derive(Debug, Serialize)]
struct CacheInfo {
    /// The raw `LIBRA_STORAGE_TYPE` value (`local` only when unset), e.g. `s3`/`r2`.
    storage_type: String,
    /// Whether the config statically selects a durable tier (`s3`/`r2` + valid
    /// bucket/endpoint/keys) — the tunables only apply then; a local-only repo
    /// caches nothing. A real connection also needs valid credentials.
    tiered: bool,
    /// Small/large object threshold in bytes (`LIBRA_STORAGE_THRESHOLD`).
    threshold_bytes: usize,
    /// Local LRU disk budget in bytes (`LIBRA_STORAGE_CACHE_SIZE`).
    cache_size_bytes: usize,
}

pub async fn execute_safe(args: CacheArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        CacheCommand::Info => info(output),
    }
}

fn info(output: &OutputConfig) -> CliResult<()> {
    let CacheConfig {
        storage_type,
        tiered,
        threshold_bytes,
        cache_size_bytes,
    } = resolve_cache_config().map_err(|message| {
        CliError::fatal(format!(
            "failed to resolve storage/cache configuration: {message}"
        ))
    })?;
    let report = CacheInfo {
        storage_type,
        tiered,
        threshold_bytes,
        cache_size_bytes,
    };

    if output.is_json() {
        return emit_json_data("cache", &report, output);
    }

    println!("storage:   {}", report.storage_type);
    if report.tiered {
        println!("tier:      durable tier active (cache tunables apply)");
    } else {
        println!("tier:      local-only (no durable tier; cache tunables are inert)");
    }
    println!(
        "threshold: {} bytes (objects >= this size are LRU-cached, not stored permanently)",
        report.threshold_bytes
    );
    println!(
        "cache:     {} bytes (LRU disk budget for large cached objects)",
        report.cache_size_bytes
    );
    println!(
        "(configure via LIBRA_STORAGE_TYPE / LIBRA_STORAGE_THRESHOLD / LIBRA_STORAGE_CACHE_SIZE)"
    );
    Ok(())
}
