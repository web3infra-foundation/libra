//! Implements `gc` by tracing reachable objects, pruning old unreachable loose objects,
//! and cleaning stale pack sidecar files without rewriting valid packs.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use clap::Parser;
use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tag::Tag as GitTag, tree::Tree, types::ObjectType},
};
use sea_orm::EntityTrait;
use serde::Serialize;

use crate::{
    command::{load_object, verify_pack},
    internal::{
        db::get_db_conn_instance,
        model::{reference, reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

const GC_EXAMPLES: &str = "\
EXAMPLES:
    libra gc                         Trace reachable objects and prune old unreachable loose objects
    libra gc --dry-run --prune=now   Report every object and stale pack file that would be removed
    libra gc --prune=now             Remove unreachable loose objects immediately
    libra gc --prune=never --json    Inspect reachability and pack hygiene without deleting objects";

const DEFAULT_PRUNE: &str = "2.weeks.ago";
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const SECONDS_PER_WEEK: u64 = 7 * SECONDS_PER_DAY;

#[derive(Parser, Debug)]
#[command(after_help = GC_EXAMPLES)]
pub struct GcArgs {
    /// Do not remove anything; print/report planned actions only.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Prune unreachable loose objects older than DATE (`now`, `never`, `N.days.ago`, `N.weeks.ago`).
    #[arg(long, default_value = DEFAULT_PRUNE, value_name = "DATE", conflicts_with = "no_prune")]
    pub prune: String,

    /// Do not prune unreachable loose objects or stale pack sidecars.
    #[arg(long)]
    pub no_prune: bool,

    /// Accepted for Git compatibility. Libra gc currently does not repack or delta-compress.
    #[arg(long)]
    pub aggressive: bool,

    /// Accepted for Git compatibility. Libra still performs a deterministic local pass.
    #[arg(long)]
    pub auto: bool,

    /// Accepted for Git compatibility. The current implementation has no gc lock.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
struct GcOutput {
    prune: String,
    dry_run: bool,
    loose_objects: LooseObjectStats,
    reachable_objects: usize,
    unreachable_objects: Vec<GcObjectAction>,
    pack_files: PackStats,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct LooseObjectStats {
    scanned: usize,
    reachable: usize,
    unreachable: usize,
    pruned: usize,
    retained: usize,
}

#[derive(Debug, Clone, Serialize)]
struct GcObjectAction {
    oid: String,
    object_type: String,
    action: GcAction,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GcAction {
    Pruned,
    WouldPrune,
    Retained,
}

#[derive(Debug, Clone, Default, Serialize)]
struct PackStats {
    directory_exists: bool,
    packs_verified: usize,
    objects_in_packs: usize,
    stale_files: Vec<PackFileAction>,
}

#[derive(Debug, Clone, Serialize)]
struct PackFileAction {
    path: String,
    action: PackAction,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PackAction {
    Pruned,
    WouldPrune,
    Retained,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrunePolicy {
    Never,
    OlderThan(SystemTime),
}

#[derive(Debug, Clone)]
struct LooseObject {
    hash: ObjectHash,
    path: PathBuf,
}

#[derive(Debug, Clone, Default)]
struct Reachability {
    loose: Vec<LooseObject>,
    roots: HashSet<ObjectHash>,
    reachable: HashSet<ObjectHash>,
}

#[derive(Debug, Clone, Default)]
struct PackGroup {
    pack: Option<PathBuf>,
    idx: Option<PathBuf>,
    keep: Option<PathBuf>,
    others: Vec<PathBuf>,
}

pub async fn execute(args: GcArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// # Side Effects
///
/// Reads refs, reflogs, the index, loose objects, and pack sidecar files. When
/// pruning is enabled and `--dry-run` is absent, deletes only unreachable loose
/// object files and stale pack sidecars; valid pack/index pairs are never
/// rewritten or removed.
///
/// # Errors
///
/// Returns structured CLI errors for invalid prune dates, unreadable object
/// storage, corrupt reachable objects, malformed pack/index pairs, and failed
/// deletion attempts.
pub async fn execute_safe(args: GcArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_gc(&args).await?;
    render_gc_output(&result, output)
}

async fn run_gc(args: &GcArgs) -> CliResult<GcOutput> {
    let policy = prune_policy(args)?;
    let storage = ClientStorage::init(path::objects());
    let mut reachability = collect_reachability(&storage).await?;
    trace_reachable(&storage, &mut reachability)?;

    let loose = prune_unreachable_loose_objects(&storage, &reachability, policy, args.dry_run)?;
    let pack_files = clean_pack_directory(&storage, policy, args.dry_run)?;

    let loose_stats = LooseObjectStats {
        scanned: reachability.loose.len(),
        reachable: reachability
            .loose
            .iter()
            .filter(|object| reachability.reachable.contains(&object.hash))
            .count(),
        unreachable: loose.len(),
        pruned: loose
            .iter()
            .filter(|action| action.action == GcAction::Pruned)
            .count(),
        retained: loose
            .iter()
            .filter(|action| action.action == GcAction::Retained)
            .count(),
    };

    let mut warnings = Vec::new();
    if args.aggressive {
        warnings.push(
            "--aggressive is accepted for compatibility; Libra gc does not repack objects yet"
                .to_string(),
        );
    }
    if args.auto {
        warnings
            .push("--auto is accepted for compatibility; Libra still runs one local pass".into());
    }
    if args.force {
        warnings.push("--force is accepted for compatibility; no gc lock is used".into());
    }

    Ok(GcOutput {
        prune: if args.no_prune {
            "never".to_string()
        } else {
            args.prune.clone()
        },
        dry_run: args.dry_run,
        reachable_objects: reachability.reachable.len(),
        loose_objects: loose_stats,
        unreachable_objects: loose,
        pack_files,
        warnings,
    })
}

fn render_gc_output(result: &GcOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("gc", result, output);
    }

    for warning in &result.warnings {
        emit_warning(warning.clone());
    }

    if output.quiet {
        return Ok(());
    }

    println!(
        "Enumerating loose objects: {} scanned, {} reachable, {} unreachable.",
        result.loose_objects.scanned,
        result.loose_objects.reachable,
        result.loose_objects.unreachable
    );

    if result.dry_run {
        let would_prune = result
            .unreachable_objects
            .iter()
            .filter(|object| object.action == GcAction::WouldPrune)
            .count();
        println!("Would prune {would_prune} loose object(s).");
    } else {
        println!("Pruned {} loose object(s).", result.loose_objects.pruned);
    }

    if result.pack_files.directory_exists {
        println!(
            "Checked {} pack(s), containing {} indexed object(s).",
            result.pack_files.packs_verified, result.pack_files.objects_in_packs
        );
        let pack_pruned = result
            .pack_files
            .stale_files
            .iter()
            .filter(|file| matches!(file.action, PackAction::Pruned | PackAction::WouldPrune))
            .count();
        if result.dry_run {
            println!("Would clean {pack_pruned} stale pack file(s).");
        } else {
            println!("Cleaned {pack_pruned} stale pack file(s).");
        }
    }

    Ok(())
}

fn prune_policy(args: &GcArgs) -> CliResult<PrunePolicy> {
    if args.no_prune {
        return Ok(PrunePolicy::Never);
    }
    parse_prune_date(&args.prune)
}

fn parse_prune_date(raw: &str) -> CliResult<PrunePolicy> {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("never") {
        return Ok(PrunePolicy::Never);
    }
    if value.eq_ignore_ascii_case("now") || value.eq_ignore_ascii_case("all") {
        return Ok(PrunePolicy::OlderThan(SystemTime::now()));
    }

    let Some((amount, unit)) = value.split_once('.') else {
        return Err(invalid_prune_date(value));
    };
    let amount = amount
        .parse::<u64>()
        .map_err(|_| invalid_prune_date(value))?;
    let seconds = match unit {
        "seconds.ago" | "second.ago" => amount,
        "minutes.ago" | "minute.ago" => amount.saturating_mul(60),
        "hours.ago" | "hour.ago" => amount.saturating_mul(60 * 60),
        "days.ago" | "day.ago" => amount.saturating_mul(SECONDS_PER_DAY),
        "weeks.ago" | "week.ago" => amount.saturating_mul(SECONDS_PER_WEEK),
        _ => return Err(invalid_prune_date(value)),
    };

    Ok(PrunePolicy::OlderThan(
        SystemTime::now()
            .checked_sub(Duration::from_secs(seconds))
            .unwrap_or(SystemTime::UNIX_EPOCH),
    ))
}

fn invalid_prune_date(value: &str) -> CliError {
    CliError::fatal(format!("invalid prune date '{value}'"))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("use 'now', 'never', or a relative value like '2.weeks.ago'.")
}

fn should_prune(path: &Path, policy: PrunePolicy) -> CliResult<bool> {
    match policy {
        PrunePolicy::Never => Ok(false),
        PrunePolicy::OlderThan(cutoff) => {
            let modified = fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| {
                    CliError::fatal(format!(
                        "failed to read metadata for '{}': {}",
                        path.display(),
                        format_io_error(&error)
                    ))
                    .with_stable_code(StableErrorCode::IoReadFailed)
                })?;
            Ok(modified <= cutoff)
        }
    }
}
