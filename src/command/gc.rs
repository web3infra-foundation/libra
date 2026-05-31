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

async fn collect_reachability(storage: &ClientStorage) -> CliResult<Reachability> {
    let loose = list_loose_objects(storage.base_path())?;
    let roots = collect_roots_from_database().await?;
    Ok(Reachability {
        loose,
        roots,
        reachable: HashSet::new(),
    })
}

fn list_loose_objects(objects_dir: &Path) -> CliResult<Vec<LooseObject>> {
    if !objects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut objects = Vec::new();
    for entry in fs::read_dir(objects_dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read object directory '{}': {}",
            objects_dir.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let entry = entry.map_err(|error| {
            CliError::fatal(format!("failed to read object directory entry: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let prefix_path = entry.path();
        if !prefix_path.is_dir() {
            continue;
        }
        let Some(prefix) = prefix_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_hex_prefix(prefix) {
            continue;
        }
        for object_entry in fs::read_dir(&prefix_path).map_err(|error| {
            CliError::fatal(format!(
                "failed to read loose object directory '{}': {}",
                prefix_path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })? {
            let object_entry = object_entry.map_err(|error| {
                CliError::fatal(format!("failed to read loose object entry: {error}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            let path = object_entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(suffix) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let candidate = format!("{prefix}{suffix}");
            let Ok(hash) = ObjectHash::from_str(&candidate) else {
                continue;
            };
            objects.push(LooseObject { hash, path });
        }
    }
    objects.sort_by_key(|object| object.hash.to_string());
    Ok(objects)
}

fn is_hex_prefix(prefix: &str) -> bool {
    prefix.len() == 2 && prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn collect_roots_from_database() -> CliResult<HashSet<ObjectHash>> {
    let db = get_db_conn_instance().await;
    let mut roots = HashSet::new();

    let refs = reference::Entity::find().all(&db).await.map_err(|error| {
        CliError::fatal(format!("failed to load references: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for entry in refs {
        if let Some(raw) = entry.commit.as_deref() {
            roots.insert(parse_stored_hash(raw, "reference")?);
        }
    }

    let reflogs = reflog::Entity::find().all(&db).await.map_err(|error| {
        CliError::fatal(format!("failed to load reflogs: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for entry in reflogs {
        for raw in [entry.old_oid.as_str(), entry.new_oid.as_str()] {
            if !is_null_oid(raw) {
                roots.insert(parse_stored_hash(raw, "reflog")?);
            }
        }
    }

    roots.extend(index_roots()?);
    Ok(roots)
}

fn parse_stored_hash(raw: &str, source: &str) -> CliResult<ObjectHash> {
    ObjectHash::from_str(raw).map_err(|error| {
        CliError::fatal(format!("invalid {source} object id '{raw}': {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

fn is_null_oid(raw: &str) -> bool {
    !raw.is_empty() && raw.bytes().all(|byte| byte == b'0')
}

fn index_roots() -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    let index_path = path::index();
    if !index_path.exists() {
        return Ok(roots);
    }
    let index = git_internal::internal::index::Index::load(&index_path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read index '{}': {error}",
            index_path.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    for entry in index.tracked_entries(0) {
        roots.insert(entry.hash);
    }
    Ok(roots)
}

fn trace_reachable(storage: &ClientStorage, reachability: &mut Reachability) -> CliResult<()> {
    let mut queue = VecDeque::from_iter(reachability.roots.iter().copied());
    while let Some(hash) = queue.pop_front() {
        if !reachability.reachable.insert(hash) {
            continue;
        }
        for child in object_children(storage, &hash)? {
            if !reachability.reachable.contains(&child) {
                queue.push_back(child);
            }
        }
    }
    Ok(())
}

fn object_children(storage: &ClientStorage, hash: &ObjectHash) -> CliResult<Vec<ObjectHash>> {
    let object_type = storage.get_object_type(hash).map_err(|error| {
        CliError::fatal(format!(
            "failed to inspect reachable object {hash}: {error}"
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    match object_type {
        ObjectType::Commit => {
            let commit: Commit = load_object(hash).map_err(|error| corrupt_object(hash, error))?;
            let mut children = Vec::with_capacity(commit.parent_commit_ids.len() + 1);
            children.push(commit.tree_id);
            children.extend(commit.parent_commit_ids);
            Ok(children)
        }
        ObjectType::Tree => {
            let tree: Tree = load_object(hash).map_err(|error| corrupt_object(hash, error))?;
            Ok(tree.tree_items.iter().map(|item| item.id).collect())
        }
        ObjectType::Tag => {
            let tag: GitTag = load_object(hash).map_err(|error| corrupt_object(hash, error))?;
            Ok(vec![tag.object_hash])
        }
        ObjectType::Blob => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}

fn corrupt_object(hash: &ObjectHash, error: git_internal::errors::GitError) -> CliError {
    CliError::fatal(format!("failed to load reachable object {hash}: {error}"))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

fn prune_unreachable_loose_objects(
    storage: &ClientStorage,
    reachability: &Reachability,
    policy: PrunePolicy,
    dry_run: bool,
) -> CliResult<Vec<GcObjectAction>> {
    let mut actions = Vec::new();
    let reachable = &reachability.reachable;
    for loose in &reachability.loose {
        if reachable.contains(&loose.hash) {
            continue;
        }

        let object_type = storage
            .get_object_type(&loose.hash)
            .map(|kind| kind.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        if should_prune(&loose.path, policy)? {
            let action = if dry_run {
                GcAction::WouldPrune
            } else {
                remove_file(&loose.path)?;
                remove_empty_parent_dir(&loose.path)?;
                GcAction::Pruned
            };
            actions.push(GcObjectAction {
                oid: loose.hash.to_string(),
                object_type,
                action,
                reason: "unreachable loose object matched prune policy".to_string(),
            });
        } else {
            actions.push(GcObjectAction {
                oid: loose.hash.to_string(),
                object_type,
                action: GcAction::Retained,
                reason: "unreachable object is newer than prune cutoff or pruning is disabled"
                    .to_string(),
            });
        }
    }
    Ok(actions)
}

fn clean_pack_directory(
    storage: &ClientStorage,
    policy: PrunePolicy,
    dry_run: bool,
) -> CliResult<PackStats> {
    let pack_dir = storage.base_path().join("pack");
    let mut stats = PackStats {
        directory_exists: pack_dir.exists(),
        ..Default::default()
    };
    if !pack_dir.exists() {
        return Ok(stats);
    }

    let groups = collect_pack_groups(&pack_dir)?;
    for (stem, group) in groups {
        let has_keep = group.keep.is_some();
        match (&group.pack, &group.idx) {
            (Some(pack), Some(idx)) => {
                let inspection = verify_pack::inspect_pack_files(idx, pack).map_err(|error| {
                    CliError::fatal(format!(
                        "failed to verify pack group '{}': {}",
                        stem,
                        error.render()
                    ))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
                })?;
                stats.packs_verified += 1;
                stats.objects_in_packs += inspection.object_count;
            }
            (Some(pack), None) => {
                stats.stale_files.push(handle_pack_file(
                    pack,
                    policy,
                    dry_run,
                    has_keep,
                    "pack file has no matching .idx",
                )?);
            }
            (None, Some(idx)) => {
                stats.stale_files.push(handle_pack_file(
                    idx,
                    policy,
                    dry_run,
                    has_keep,
                    "pack index has no matching .pack",
                )?);
            }
            (None, None) => {}
        }

        for other in group.others {
            stats.stale_files.push(handle_pack_file(
                &other,
                policy,
                dry_run,
                has_keep,
                "stale pack temporary or sidecar file",
            )?);
        }
    }

    Ok(stats)
}

fn collect_pack_groups(pack_dir: &Path) -> CliResult<BTreeMap<String, PackGroup>> {
    let mut groups = BTreeMap::<String, PackGroup>::new();
    for entry in fs::read_dir(pack_dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read pack directory '{}': {}",
            pack_dir.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let entry = entry.map_err(|error| {
            CliError::fatal(format!("failed to read pack directory entry: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(stem) = pack_stem(&path) else {
            continue;
        };
        let group = groups.entry(stem).or_default();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("pack") => group.pack = Some(path),
            Some("idx") => group.idx = Some(path),
            Some("keep") => group.keep = Some(path),
            _ => group.others.push(path),
        }
    }
    Ok(groups)
}

fn pack_stem(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if !file_name.starts_with("pack-") {
        return None;
    }
    if let Some(stem) = file_name.strip_suffix(".pack") {
        return Some(stem.to_string());
    }
    if let Some(stem) = file_name.strip_suffix(".idx") {
        return Some(stem.to_string());
    }
    if let Some(stem) = file_name.strip_suffix(".keep") {
        return Some(stem.to_string());
    }
    file_name
        .split_once('.')
        .map(|(stem, _)| stem.to_string())
        .or_else(|| Some(file_name.to_string()))
}

fn handle_pack_file(
    path: &Path,
    policy: PrunePolicy,
    dry_run: bool,
    has_keep: bool,
    reason: &str,
) -> CliResult<PackFileAction> {
    if has_keep {
        return Ok(PackFileAction {
            path: display_path(path),
            action: PackAction::Retained,
            reason: format!("{reason}; matching .keep file is present"),
        });
    }
    if !should_prune(path, policy)? {
        return Ok(PackFileAction {
            path: display_path(path),
            action: PackAction::Retained,
            reason: format!("{reason}; file is newer than prune cutoff or pruning is disabled"),
        });
    }
    let action = if dry_run {
        PackAction::WouldPrune
    } else {
        remove_file(path)?;
        PackAction::Pruned
    };
    Ok(PackFileAction {
        path: display_path(path),
        action,
        reason: reason.to_string(),
    })
}

fn remove_file(path: &Path) -> CliResult<()> {
    fs::remove_file(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to remove '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })
}

fn remove_empty_parent_dir(path: &Path) -> CliResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    match fs::remove_dir(parent) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::fatal(format!(
            "failed to remove empty object directory '{}': {}",
            parent.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)),
    }
}

fn format_io_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => error.to_string(),
    }
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use git_internal::{
        hash::{ObjectHash, get_hash_kind},
        internal::object::{
            blob::Blob,
            commit::Commit,
            signature::{Signature, SignatureType},
            tree::{Tree, TreeItem, TreeItemMode},
        },
    };
    use tempfile::tempdir;

    use super::*;
    use crate::{
        command::save_object,
        utils::{output::JsonFormat, test, util},
    };

    fn test_hash(byte: u8) -> ObjectHash {
        ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
            .expect("hash bytes should match active hash kind")
    }

    fn signature() -> Signature {
        Signature {
            signature_type: SignatureType::Author,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp: 1,
            timezone: "+0000".to_string(),
        }
    }

    fn commit_with_tree(tree_id: ObjectHash, parents: Vec<ObjectHash>) -> Commit {
        Commit {
            id: test_hash(9),
            tree_id,
            parent_commit_ids: parents,
            author: signature(),
            committer: Signature {
                signature_type: SignatureType::Committer,
                ..signature()
            },
            message: "commit".to_string(),
        }
    }

    #[test]
    fn parse_prune_date_accepts_never_and_now() {
        assert_eq!(parse_prune_date("never").unwrap(), PrunePolicy::Never);
        assert!(matches!(
            parse_prune_date("now").unwrap(),
            PrunePolicy::OlderThan(_)
        ));
    }

    #[test]
    fn parse_prune_date_accepts_relative_weeks() {
        let PrunePolicy::OlderThan(cutoff) = parse_prune_date("2.weeks.ago").unwrap() else {
            panic!("expected cutoff");
        };
        assert!(cutoff < SystemTime::now());
    }

    #[test]
    fn parse_prune_date_accepts_supported_relative_units() {
        for value in [
            "1.second.ago",
            "2.seconds.ago",
            "1.minute.ago",
            "2.minutes.ago",
            "1.hour.ago",
            "2.hours.ago",
            "1.day.ago",
            "2.days.ago",
            "1.week.ago",
            "2.weeks.ago",
            "all",
        ] {
            assert!(
                matches!(parse_prune_date(value).unwrap(), PrunePolicy::OlderThan(_)),
                "{value} should parse"
            );
        }
    }

    #[test]
    fn parse_prune_date_rejects_unknown_values() {
        let error = parse_prune_date("yesterday").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }

    #[test]
    fn parse_prune_date_rejects_bad_amount_and_unit() {
        for value in ["x.days.ago", "2.months.ago"] {
            let error = parse_prune_date(value).unwrap_err();
            assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
        }
    }

    #[test]
    fn is_hex_prefix_requires_two_hex_digits() {
        assert!(is_hex_prefix("ab"));
        assert!(is_hex_prefix("09"));
        assert!(!is_hex_prefix("abc"));
        assert!(!is_hex_prefix("zz"));
    }

    #[test]
    fn pack_stem_groups_standard_pack_files() {
        assert_eq!(
            pack_stem(Path::new("pack-abc.pack")).as_deref(),
            Some("pack-abc")
        );
        assert_eq!(
            pack_stem(Path::new("pack-abc.idx")).as_deref(),
            Some("pack-abc")
        );
        assert_eq!(
            pack_stem(Path::new("pack-abc.keep")).as_deref(),
            Some("pack-abc")
        );
    }

    #[test]
    fn pack_stem_ignores_non_pack_prefixes() {
        assert!(pack_stem(Path::new("tmp.pack")).is_none());
        assert!(pack_stem(Path::new("README")).is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn list_loose_objects_skips_pack_directory() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let blob = Blob::from_content("hello");
        save_object(&blob, &blob.id).unwrap();
        fs::create_dir_all(path::objects().join("pack")).unwrap();
        fs::write(path::objects().join("pack").join("pack-x.pack"), b"bad").unwrap();

        let objects = list_loose_objects(&path::objects()).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].hash, blob.id);
    }

    #[test]
    fn list_loose_objects_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let objects = list_loose_objects(&dir.path().join("missing")).unwrap();
        assert!(objects.is_empty());
    }

    #[test]
    fn list_loose_objects_skips_non_files_and_invalid_names() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("loose-file"), b"x").unwrap();
        fs::create_dir(dir.path().join("zz")).unwrap();
        fs::create_dir(dir.path().join("ab")).unwrap();
        fs::create_dir(dir.path().join("ab").join("nested")).unwrap();
        fs::write(dir.path().join("ab").join("not-a-hash"), b"x").unwrap();

        let objects = list_loose_objects(dir.path()).unwrap();
        assert!(objects.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn trace_reachable_walks_commit_tree_and_blob() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("content");
        save_object(&blob, &blob.id).unwrap();
        let tree = Tree {
            id: test_hash(2),
            tree_items: vec![TreeItem {
                mode: TreeItemMode::Blob,
                id: blob.id,
                name: "file.txt".to_string(),
            }],
        };
        save_object(&tree, &tree.id).unwrap();
        let commit = commit_with_tree(tree.id, Vec::new());
        save_object(&commit, &commit.id).unwrap();

        let mut reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([commit.id]),
            reachable: HashSet::new(),
        };
        trace_reachable(&storage, &mut reachability).unwrap();

        assert!(reachability.reachable.contains(&commit.id));
        assert!(reachability.reachable.contains(&tree.id));
        assert!(reachability.reachable.contains(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn trace_reachable_skips_already_seen_roots() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("content");
        save_object(&blob, &blob.id).unwrap();
        let mut reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([blob.id]),
            reachable: HashSet::from([blob.id]),
        };
        trace_reachable(&storage, &mut reachability).unwrap();

        assert_eq!(reachability.reachable.len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prune_unreachable_loose_objects_respects_dry_run() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("garbage");
        save_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            reachable: HashSet::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            true,
        )
        .unwrap();
        assert_eq!(actions[0].action, GcAction::WouldPrune);
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prune_unreachable_loose_objects_removes_matching_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("garbage");
        save_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            reachable: HashSet::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();
        assert_eq!(actions[0].action, GcAction::Pruned);
        assert!(!storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prune_unreachable_loose_objects_keeps_reachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("reachable");
        save_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([blob.id]),
            reachable: HashSet::from([blob.id]),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();
        assert!(actions.is_empty());
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn clean_pack_directory_prunes_orphan_idx() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let idx = pack_dir.join("pack-deadbeef.idx");
        fs::write(&idx, b"orphan").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files.len(), 1);
        assert_eq!(stats.stale_files[0].action, PackAction::Pruned);
        assert!(!idx.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn clean_pack_directory_keeps_files_when_keep_exists() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let idx = pack_dir.join("pack-deadbeef.idx");
        let keep = pack_dir.join("pack-deadbeef.keep");
        fs::write(&idx, b"orphan").unwrap();
        fs::write(&keep, b"keep").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files[0].action, PackAction::Retained);
        assert!(idx.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn clean_pack_directory_returns_empty_when_directory_missing() {
        let dir = tempdir().unwrap();
        let storage = ClientStorage::init(dir.path().join("objects"));

        let stats = clean_pack_directory(&storage, PrunePolicy::Never, false).unwrap();

        assert!(!stats.directory_exists);
        assert!(stats.stale_files.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn clean_pack_directory_prunes_orphan_pack_and_sidecar() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let pack = pack_dir.join("pack-deadbeef.pack");
        let sidecar = pack_dir.join("pack-feedface.tmp");
        fs::write(&pack, b"orphan").unwrap();
        fs::write(&sidecar, b"tmp").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files.len(), 2);
        assert!(
            stats
                .stale_files
                .iter()
                .all(|file| file.action == PackAction::Pruned)
        );
        assert!(!pack.exists());
        assert!(!sidecar.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn run_gc_prune_never_reports_retained_unreachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let blob = Blob::from_content("unreachable");
        save_object(&blob, &blob.id).unwrap();

        let output = run_gc(&GcArgs {
            dry_run: false,
            prune: "never".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.unreachable, 1);
        assert_eq!(output.unreachable_objects[0].action, GcAction::Retained);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn run_gc_prune_now_removes_unreachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("unreachable");
        save_object(&blob, &blob.id).unwrap();

        let output = run_gc(&GcArgs {
            dry_run: false,
            prune: "now".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.pruned, 1);
        assert!(!storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn run_gc_warns_for_compatibility_flags() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let output = run_gc(&GcArgs {
            dry_run: true,
            prune: "never".to_string(),
            no_prune: false,
            aggressive: true,
            auto: true,
            force: true,
        })
        .await
        .unwrap();

        assert_eq!(output.warnings.len(), 3);
    }

    #[test]
    fn render_gc_output_prints_human_dry_run_summary() {
        let result = GcOutput {
            prune: "now".into(),
            dry_run: true,
            loose_objects: LooseObjectStats {
                scanned: 2,
                reachable: 1,
                unreachable: 1,
                pruned: 0,
                retained: 0,
            },
            reachable_objects: 1,
            unreachable_objects: vec![GcObjectAction {
                oid: "abc".into(),
                object_type: "blob".into(),
                action: GcAction::WouldPrune,
                reason: "old".into(),
            }],
            pack_files: PackStats {
                directory_exists: true,
                packs_verified: 1,
                objects_in_packs: 3,
                stale_files: vec![PackFileAction {
                    path: "pack/tmp".into(),
                    action: PackAction::WouldPrune,
                    reason: "tmp".into(),
                }],
            },
            warnings: vec!["compat warning".into()],
        };

        render_gc_output(&result, &OutputConfig::default()).unwrap();
    }

    #[test]
    fn render_gc_output_prints_human_pruned_summary() {
        let result = GcOutput {
            prune: "now".into(),
            dry_run: false,
            loose_objects: LooseObjectStats {
                scanned: 1,
                reachable: 0,
                unreachable: 1,
                pruned: 1,
                retained: 0,
            },
            reachable_objects: 0,
            unreachable_objects: Vec::new(),
            pack_files: PackStats {
                directory_exists: true,
                packs_verified: 0,
                objects_in_packs: 0,
                stale_files: vec![PackFileAction {
                    path: "pack/tmp".into(),
                    action: PackAction::Pruned,
                    reason: "tmp".into(),
                }],
            },
            warnings: Vec::new(),
        };

        render_gc_output(&result, &OutputConfig::default()).unwrap();
    }

    #[test]
    fn render_gc_output_respects_quiet_and_json_modes() {
        let result = GcOutput {
            prune: "never".into(),
            dry_run: false,
            loose_objects: LooseObjectStats::default(),
            reachable_objects: 0,
            unreachable_objects: Vec::new(),
            pack_files: PackStats::default(),
            warnings: Vec::new(),
        };
        let quiet = OutputConfig {
            quiet: true,
            ..Default::default()
        };
        render_gc_output(&result, &quiet).unwrap();

        let json = OutputConfig {
            json_format: Some(JsonFormat::Compact),
            ..Default::default()
        };
        render_gc_output(&result, &json).unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn execute_maps_errors_to_strings() {
        let dir = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(dir.path());
        let error = execute(GcArgs {
            dry_run: false,
            prune: "now".into(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap_err();
        assert!(error.contains("not a libra repository"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn execute_safe_reports_missing_repository() {
        let dir = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(dir.path());
        let error = execute_safe(
            GcArgs {
                dry_run: false,
                prune: "now".into(),
                no_prune: false,
                aggressive: false,
                auto: false,
                force: false,
            },
            &OutputConfig::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::RepoNotFound);
    }

    #[test]
    fn display_path_uses_path_display() {
        assert!(display_path(Path::new("objects/pack")).contains("objects"));
    }

    #[test]
    fn is_null_oid_requires_non_empty_zero_string() {
        assert!(is_null_oid("0000"));
        assert!(!is_null_oid(""));
        assert!(!is_null_oid("0001"));
    }

    #[test]
    fn format_io_error_normalizes_not_found() {
        let error = io::Error::new(io::ErrorKind::NotFound, "missing");
        assert_eq!(format_io_error(&error), "No such file or directory");
    }

    #[test]
    fn loose_object_stats_default_is_zero() {
        let stats = LooseObjectStats::default();
        assert_eq!(stats.scanned, 0);
        assert_eq!(stats.pruned, 0);
    }

    #[test]
    fn pack_stats_default_has_no_directory() {
        let stats = PackStats::default();
        assert!(!stats.directory_exists);
        assert!(stats.stale_files.is_empty());
    }

    #[test]
    fn prune_policy_obeys_no_prune() {
        let args = GcArgs {
            dry_run: false,
            prune: "now".into(),
            no_prune: true,
            aggressive: false,
            auto: false,
            force: false,
        };
        assert_eq!(prune_policy(&args).unwrap(), PrunePolicy::Never);
    }

    #[test]
    fn parse_stored_hash_rejects_invalid_hash() {
        let error = parse_stored_hash("not-a-hash", "reference").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn collect_roots_includes_index_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        fs::write(repo.path().join("file.txt"), "indexed").unwrap();
        util::working_dir();
        let add = crate::command::add::AddArgs {
            pathspec: vec!["file.txt".into()],
            all: false,
            update: false,
            verbose: false,
            dry_run: false,
            refresh: false,
            ignore_errors: false,
            force: false,
        };
        crate::command::add::execute_safe(add, &OutputConfig::default())
            .await
            .unwrap();

        let roots = collect_roots_from_database().await.unwrap();
        assert!(!roots.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn execute_safe_rejects_invalid_prune() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let error = execute_safe(
            GcArgs {
                dry_run: false,
                prune: "bad-date".to_string(),
                no_prune: false,
                aggressive: false,
                auto: false,
                force: false,
            },
            &OutputConfig::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }
