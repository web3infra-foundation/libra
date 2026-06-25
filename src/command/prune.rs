//! Implementation of `prune` command for removing unreachable loose objects.
//!
//! This command scans loose objects, determines reachability from refs and
//! additional heads, and removes unreachable objects that are eligible for
//! expiration.

use std::{
    collections::{HashSet, VecDeque},
    fs, io,
    io::{Read, Seek},
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use byteorder::{BigEndian, ReadBytesExt};
use clap::Parser;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind, set_hash_kind},
    internal::{
        index::Index,
        object::{commit::Commit, tag::Tag, tree::Tree, types::ObjectType},
    },
    utils::read_sha,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;

use crate::{
    command::{gc, load_object, verify_pack},
    internal::{
        db,
        head::Head,
        log::date_parser::parse_date,
        model::{reference, reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
const FANOUT_LEN: u64 = 256 * 4;
const TAG_REF_PREFIX: &str = "refs/tags/";

const PRUNE_LONG_ABOUT: &str =
    "Prune unreachable loose objects from the repository.
    
By default, objects reachable from refs (and any provided heads) and do not already exist in any packfile are kept.
When --expire is provided, only loose objects older than the given time are removed.";

const PRUNE_AFTER_HELP: &str = "Examples:
  libra prune
  libra prune -n
  libra prune -v --expire \"2 weeks ago\"
  libra prune --expire 2024-01-01
  libra prune HEAD~2";

/// Prune unreachable loose objects.
#[derive(Parser, Debug)]
#[command(
    about = "Prune unreachable loose objects",
    long_about = PRUNE_LONG_ABOUT,
    after_help = PRUNE_AFTER_HELP,
)]
pub struct PruneArgs {
    /// Do not remove anything; just report what would be removed.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Report all removed objects.
    #[arg(short, long)]
    pub verbose: bool,

    /// Only expire loose objects older than this time.
    #[arg(long, value_name = "TIME")]
    pub expire: Option<String>,

    /// Additional heads to keep reachable objects from.
    #[arg(value_name = "HEAD")]
    pub heads: Vec<String>,
}

/// Summary of a prune plan.
#[derive(Debug, Clone)]
struct PrunePlan {
    prunable: Vec<LooseObjectInfo>,
}

#[derive(Debug, Clone, Copy)]
enum PrunePolicy {
    Always,
    Never,
    ExpiredBy(SystemTime),
}

#[derive(Debug, Serialize)]
struct PruneObjectInfo {
    object_id: String,
    object_type: String,
}

#[derive(Debug, Serialize)]
struct PruneOutput {
    objects: Vec<PruneObjectInfo>,
    expire: Option<String>,
    heads: Vec<String>,
    dry_run: bool,
    verbose: bool,
}

/// Metadata for a loose object on disk.
#[derive(Debug, Clone)]
struct LooseObjectInfo {
    hash: ObjectHash,
    obj_type: ObjectType,
    path: PathBuf,
    modified: Option<SystemTime>,
}

/// Entry point for `libra prune`.
pub async fn execute(args: PruneArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// Safe entry point returning structured errors.
///
/// # Side Effects
///
/// - Scans loose object directories under `.libra/objects`.
/// - Deletes unreachable loose objects unless `--dry-run` is set.
///
/// # Errors
///
/// Returns `CliError` for invalid arguments, repository corruption, or IO
/// failures while scanning or deleting objects.
pub async fn execute_safe(args: PruneArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let _gc_lock = if args.dry_run {
        None
    } else {
        Some(gc::acquire_gc_lock(false)?)
    };

    let objects_dir = path::objects();
    let objects_dir_existed = objects_dir.exists();
    let expire_before = parse_expire_cutoff(args.expire.as_deref())?;
    if args.dry_run && !objects_dir_existed {
        render_prune_json(&PrunePlan { prunable: vec![] }, &args, output)?;
        return Ok(());
    }

    let storage = if args.dry_run {
        ClientStorage::init_local(objects_dir.clone())
    } else {
        ClientStorage::init(objects_dir.clone())
    };

    let (reachable, protected) = collect_reachable_objects(&storage, &args.heads).await?;
    let packed = collect_packed_objects(&storage).await?;
    let needs_mtime = matches!(expire_before, PrunePolicy::ExpiredBy(_));
    let loose_objects = list_loose_objects(&storage, needs_mtime)?;
    let plan = build_prune_plan(
        loose_objects,
        &reachable,
        &packed,
        &protected,
        expire_before,
    );

    let should_report = (args.verbose || args.dry_run) && (!output.is_json() && !output.quiet);
    apply_prune_plan(&plan, &storage, args.dry_run, should_report).await?;

    render_prune_json(&plan, &args, output)?;

    cleanup_dry_run_objects_dir(args.dry_run, objects_dir_existed, &objects_dir)?;

    Ok(())
}

fn render_prune_json(plan: &PrunePlan, args: &PruneArgs, output: &OutputConfig) -> CliResult<()> {
    if !output.is_json() {
        return Ok(());
    }

    let prune_output = PruneOutput {
        objects: plan
            .prunable
            .iter()
            .map(|info| PruneObjectInfo {
                object_id: info.hash.to_string(),
                object_type: info.obj_type.to_string(),
            })
            .collect(),
        expire: args.expire.clone(),
        heads: args.heads.clone(),
        dry_run: args.dry_run,
        verbose: args.verbose,
    };
    emit_json_data("prune", &prune_output, output)
}

fn cleanup_dry_run_objects_dir(
    dry_run: bool,
    objects_dir_existed: bool,
    objects_dir: &Path,
) -> CliResult<()> {
    if !dry_run || objects_dir_existed || !objects_dir.exists() {
        return Ok(());
    }
    if is_dir_empty(objects_dir)? {
        fs::remove_dir(objects_dir).map_err(|error| {
            CliError::fatal(format!(
                "failed to remove empty dry-run objects directory '{}': {error}",
                objects_dir.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    }
    Ok(())
}

/// Parse the `--expire` argument into a concrete cutoff time.
fn parse_expire_cutoff(expire: Option<&str>) -> CliResult<PrunePolicy> {
    let Some(value) = expire else {
        return Ok(PrunePolicy::Always);
    };

    parse_expire_date(value)
}

/// Parse expire dates.
fn parse_expire_date(raw: &str) -> CliResult<PrunePolicy> {
    let value = raw.trim();

    if value.eq_ignore_ascii_case("never") {
        return Ok(PrunePolicy::Never);
    }
    if value.eq_ignore_ascii_case("all") {
        return Ok(PrunePolicy::Always);
    }
    if value.eq_ignore_ascii_case("now") {
        return Ok(PrunePolicy::ExpiredBy(SystemTime::now()));
    }
    let value = value.replace('.', " ");
    let timestamp = parse_date(&value).map_err(|error| {
        CliError::command_usage(error.to_string())
            .with_stable_code(StableErrorCode::CliInvalidArguments)
            .with_hint(
                r#"supported formats: now, never, all, YYYY-MM-DD, "N days ago", unix timestamp"#,
            )
    })?;
    Ok(PrunePolicy::ExpiredBy(system_time_from_unix_seconds(
        timestamp,
    )))
}

/// Convert signed Unix seconds into a `SystemTime` cutoff.
fn system_time_from_unix_seconds(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

/// Collect all objects reachable from refs, HEAD, and user-supplied heads.
async fn collect_reachable_objects(
    storage: &ClientStorage,
    heads: &[String],
) -> CliResult<(HashSet<ObjectHash>, HashSet<ObjectHash>)> {
    let mut starting_points = collect_starting_points(storage, heads).await?;
    let (gc_roots, protected) = gc::collect_roots_from_database().await?;
    starting_points.extend(gc_roots);
    let reachable = bfs_mark_reachable(&starting_points, storage)?;
    Ok((reachable, protected))
}

/// Collect objects already in packfiles.
async fn collect_packed_objects(storage: &ClientStorage) -> CliResult<HashSet<ObjectHash>> {
    let mut packed_objects = HashSet::new();
    let pack_dir = storage.base_path().join("pack");
    if pack_dir.exists() {
        let entries = fs::read_dir(&pack_dir).map_err(|error| {
            CliError::fatal(format!(
                "failed to read pack directory '{}': {error}",
                pack_dir.display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "idx") {
                let pack_path = path.with_extension("pack");
                if inspect_pack_files_preserving_hash_kind(&path, &pack_path).is_err() {
                    continue;
                }
                let packed = list_idx_objects(&path).map_err(|error| {
                    CliError::fatal(format!(
                        "failed to read pack index '{}': {error}",
                        path.display()
                    ))
                    .with_stable_code(StableErrorCode::IoReadFailed)
                })?;
                packed_objects.extend(packed);
            }
        }
    }

    Ok(packed_objects)
}

struct HashKindRestoreGuard {
    previous: HashKind,
}

impl HashKindRestoreGuard {
    fn preserve() -> Self {
        Self {
            previous: get_hash_kind(),
        }
    }
}

impl Drop for HashKindRestoreGuard {
    fn drop(&mut self) {
        set_hash_kind(self.previous);
    }
}

fn inspect_pack_files_preserving_hash_kind(idx_path: &Path, pack_path: &Path) -> CliResult<()> {
    let _guard = HashKindRestoreGuard::preserve();
    verify_pack::inspect_pack_files(idx_path, pack_path).map(|_| ())
}

/// List all objects contained in a pack index file.
fn list_idx_objects(idx_path: &Path) -> io::Result<Vec<ObjectHash>> {
    let hash_size = get_hash_kind().size() as u64;
    let mut idx_file = fs::File::open(idx_path)?;
    let mut magic = [0u8; 4];
    idx_file.read_exact(&mut magic)?;
    if magic == IDX_MAGIC {
        // Index v2
        idx_file.seek(io::SeekFrom::Start(FANOUT_LEN + 8))?;
        idx_file.seek(io::SeekFrom::Current(-4))?;
        let mut fanout_entry = [0u8; 4];
        idx_file.read_exact(&mut fanout_entry)?;

        let object_count = u32::from_be_bytes(fanout_entry) as usize;
        let mut objs = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            let hash = read_sha(&mut idx_file)?;
            objs.push(hash);
        }
        Ok(objs)
    } else {
        // Index v1
        if hash_size != 20 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pack index v1 only supports sha1",
            ));
        }
        idx_file.seek(io::SeekFrom::Start(FANOUT_LEN))?;
        idx_file.seek(io::SeekFrom::Current(-4))?;
        let mut fanout_entry = [0u8; 4];
        idx_file.read_exact(&mut fanout_entry)?;
        let object_count = u32::from_be_bytes(fanout_entry) as usize;
        let mut objs = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            let _offset = idx_file.read_u32::<BigEndian>()?;
            let hash = read_sha(&mut idx_file)?;
            objs.push(hash);
        }
        Ok(objs)
    }
}

/// Gather starting points for reachability from references and explicit heads.
async fn collect_starting_points(
    storage: &ClientStorage,
    heads: &[String],
) -> CliResult<HashSet<ObjectHash>> {
    let mut starting_points = HashSet::new();
    let db_conn = db::get_db_conn_instance().await;

    // References
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load refs: {error}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

    for ref_entry in refs {
        let Some(commit_hash) = &ref_entry.commit else {
            continue;
        };
        let Some(hash) = parse_object_hash(commit_hash) else {
            let ref_name = ref_entry.name.as_deref().unwrap_or("<unknown>");
            return Err(CliError::fatal(format!(
                "invalid ref oid '{commit_hash}' in '{ref_name}'"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        };

        if !storage.exist(&hash) {
            let ref_name = ref_entry.name.as_deref().unwrap_or("<unknown>");
            return Err(CliError::fatal(format!(
                "reference '{ref_name}' points to missing object {hash}"
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        starting_points.insert(hash);
    }

    // Reflogs
    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load reflogs: {error}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

    for reflog_entry in reflogs {
        let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');
        if !is_null_oid(&reflog_entry.old_oid)
            && let Some(hash) = parse_object_hash(&reflog_entry.old_oid)
        {
            starting_points.insert(hash);
        }
        if !is_null_oid(&reflog_entry.new_oid)
            && let Some(hash) = parse_object_hash(&reflog_entry.new_oid)
        {
            starting_points.insert(hash);
        }
    }

    // Index
    let index_path = path::index();
    if index_path.exists() {
        let index = Index::load(&index_path).map_err(|error| {
            CliError::fatal(format!("failed to load index: {error}"))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;
        for stage in [0, 1, 2, 3] {
            for entry in index.tracked_entries(stage) {
                starting_points.insert(entry.hash);
            }
        }
    }

    // Current head
    let head = Head::current_result().await.map_err(|error| {
        CliError::fatal(format!("failed to read HEAD: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    if let Head::Detached(hash) = head {
        if !storage.exist(&hash) {
            return Err(
                CliError::fatal(format!("HEAD points to missing object {hash}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt),
            );
        }
        starting_points.insert(hash);
    }

    // User-specified heads
    for head in heads {
        let hash = resolve_head_object(head, storage).await?;
        if !storage.exist(&hash) {
            return Err(
                CliError::fatal(format!("head '{head}' points to missing object {hash}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt),
            );
        }
        starting_points.insert(hash);
    }

    Ok(starting_points)
}

/// Resolve a user-provided head argument to an object hash.
async fn resolve_head_object(head: &str, storage: &ClientStorage) -> CliResult<ObjectHash> {
    // If the head is a valid object hash, ignore any reference.
    if let Ok(hash) = ObjectHash::from_str(head) {
        return Ok(hash);
    }

    if let Some(hash) = resolve_tag_object_ref(head).await {
        return Ok(hash);
    }

    if let Ok(hash) = util::get_commit_base(head).await {
        return Ok(hash);
    }

    let results = storage.search_result(head).await.map_err(|error| {
        CliError::fatal(format!(
            "failed to search objects while resolving '{head}': {error}"
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    if results.len() == 1 {
        return Ok(results[0]);
    }
    if results.len() > 1 {
        return Err(CliError::command_usage(format!(
            "ambiguous argument '{}': matched {} objects",
            head,
            results.len()
        ))
        .with_stable_code(StableErrorCode::CliInvalidArguments));
    }

    Err(CliError::fatal(format!("Not a valid object name {}", head))
        .with_stable_code(StableErrorCode::CliInvalidTarget))
}

fn normalize_tag_ref_name(object_ref: &str) -> String {
    if object_ref.starts_with(TAG_REF_PREFIX) {
        object_ref.to_string()
    } else {
        format!("{TAG_REF_PREFIX}{object_ref}")
    }
}

/// Resolve a tag reference to the object hash it points to, if it exists.
async fn resolve_tag_object_ref(object_ref: &str) -> Option<ObjectHash> {
    let full_ref_name = normalize_tag_ref_name(object_ref);
    let db_conn = db::get_db_conn_instance().await;
    let tag_ref = reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Tag))
        .filter(reference::Column::Name.eq(full_ref_name))
        .one(&db_conn)
        .await
        .ok()
        .flatten()?;

    let target_hash = tag_ref.commit?;
    ObjectHash::from_str(&target_hash).ok()
}

/// Build a prune plan by filtering unreachable loose objects.
fn build_prune_plan(
    loose_objects: Vec<LooseObjectInfo>,
    reachable: &HashSet<ObjectHash>,
    packed: &HashSet<ObjectHash>,
    protected: &HashSet<ObjectHash>,
    expire_before: PrunePolicy,
) -> PrunePlan {
    let prunable = loose_objects
        .into_iter()
        .filter(|info| {
            (reachable.contains(&info.hash) == packed.contains(&info.hash))
                && !protected.contains(&info.hash)
                && is_expired(info.modified, expire_before)
        })
        .collect();

    PrunePlan { prunable }
}

/// Apply the prune plan by removing loose objects (or reporting in dry-run mode).
async fn apply_prune_plan(
    plan: &PrunePlan,
    storage: &ClientStorage,
    dry_run: bool,
    report: bool,
) -> CliResult<()> {
    let objects_dir = &fs::canonicalize(storage.base_path()).map_err(|error| {
        CliError::fatal(format!("failed to resolve objects directory: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for info in &plan.prunable {
        if report {
            println!("{} {}", info.hash, info.obj_type);
        }

        if dry_run {
            continue;
        }

        remove_loose_object(info, objects_dir)?;
        if !storage.exist_local(&info.hash) {
            gc::remove_object_index_rows(&info.hash).await?;
        }
    }

    Ok(())
}

/// Remove a loose object file and prune empty parent directories.
fn remove_loose_object(info: &LooseObjectInfo, objects_dir: &Path) -> CliResult<()> {
    if !info.path.starts_with(objects_dir) {
        return Err(CliError::fatal(format!(
            "refusing to prune object outside objects dir: {}",
            info.path.display()
        ))
        .with_stable_code(StableErrorCode::InternalInvariant));
    }

    match fs::remove_file(&info.path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to remove object '{}': {error}",
                info.path.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed));
        }
    }

    let Some(parent) = info.path.parent() else {
        return Ok(());
    };

    if should_prune_object_dir(parent, objects_dir) && is_dir_empty(parent)? {
        fs::remove_dir(parent).map_err(|error| {
            CliError::fatal(format!(
                "failed to remove empty object directory '{}': {error}",
                parent.display()
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    }

    Ok(())
}

/// Determine whether a directory is a loose object prefix directory.
fn should_prune_object_dir(dir: &Path, objects_dir: &Path) -> bool {
    if !dir.starts_with(objects_dir) {
        return false;
    }
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.len() == 2 && u8::from_str_radix(name, 16).is_ok()
}

/// Check whether a directory is empty.
fn is_dir_empty(dir: &Path) -> CliResult<bool> {
    let mut entries = fs::read_dir(dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read directory '{}': {error}",
            dir.display()
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    Ok(entries.next().is_none())
}

/// Check whether a loose object is expired relative to the cutoff time.
fn is_expired(modified: Option<SystemTime>, expire_before: PrunePolicy) -> bool {
    match expire_before {
        PrunePolicy::Always => true,
        PrunePolicy::Never => false,
        PrunePolicy::ExpiredBy(cutoff) => modified.is_some_and(|mtime| mtime < cutoff),
    }
}

/// List all loose objects under `.libra/objects`.
fn list_loose_objects(
    storage: &ClientStorage,
    needs_mtime: bool,
) -> CliResult<Vec<LooseObjectInfo>> {
    let objects_dir = &fs::canonicalize(storage.base_path()).map_err(|error| {
        CliError::fatal(format!("failed to resolve objects directory: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    if !objects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut objects = Vec::new();
    for entry in fs::read_dir(objects_dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read objects directory '{}': {error}",
            objects_dir.display()
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let entry = entry.map_err(|error| {
            CliError::fatal(format!(
                "failed to read objects directory entry in '{}': {error}",
                objects_dir.display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let path = fs::canonicalize(entry.path()).map_err(|error| {
            CliError::fatal(format!(
                "failed to resolve object directory '{}': {error}",
                entry.path().display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        if !path.is_dir() || !path.starts_with(objects_dir) {
            continue;
        }

        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 2 || u8::from_str_radix(dir_name, 16).is_err() {
            continue;
        }

        for sub_entry in fs::read_dir(&path).map_err(|error| {
            CliError::fatal(format!(
                "failed to read object subdirectory '{}': {error}",
                path.display()
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })? {
            let sub_entry = sub_entry.map_err(|error| {
                CliError::fatal(format!(
                    "failed to read object entry in '{}': {error}",
                    path.display()
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            let sub_path = sub_entry.path();
            if !sub_path.is_file() {
                continue;
            }

            let Some(hash) = try_parse_loose_object(dir_name, &sub_path) else {
                continue;
            };

            let obj_type = storage.get_object_type(&hash).map_err(|error| {
                CliError::fatal(format!("could not resolve object type for {hash}: {error}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;

            let modified = if needs_mtime {
                let metadata = sub_entry.metadata().map_err(|error| {
                    CliError::fatal(format!(
                        "failed to read metadata for '{}': {error}",
                        sub_path.display()
                    ))
                    .with_stable_code(StableErrorCode::IoReadFailed)
                })?;
                Some(metadata.modified().map_err(|error| {
                    CliError::fatal(format!(
                        "failed to read modified time for '{}': {error}",
                        sub_path.display()
                    ))
                    .with_stable_code(StableErrorCode::IoReadFailed)
                })?)
            } else {
                None
            };

            objects.push(LooseObjectInfo {
                hash,
                obj_type,
                path: sub_path,
                modified,
            });
        }
    }

    Ok(objects)
}

/// Parse a hex string into an `ObjectHash`.
fn parse_object_hash(hex_str: &str) -> Option<ObjectHash> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.is_empty() {
        return None;
    }
    ObjectHash::from_bytes(&bytes).ok()
}

/// Try to parse a loose object file path into an `ObjectHash`.
fn try_parse_loose_object(dir_name: &str, sub_path: &Path) -> Option<ObjectHash> {
    let file_name = sub_path.file_name().and_then(|n| n.to_str())?;
    let full_hash = format!("{dir_name}{file_name}");
    parse_object_hash(&full_hash)
}

/// Walk object references: returns objects referenced by the given object.
/// For commits: returns tree and parent commits. For trees: returns child blobs and subtrees.
fn walk_object_refs(hash: &ObjectHash, storage: &ClientStorage) -> CliResult<Vec<ObjectHash>> {
    let mut refs = Vec::new();

    let obj_type = storage.get_object_type(hash).map_err(|error| {
        CliError::fatal(format!("could not resolve object type for {hash}: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    match obj_type {
        ObjectType::Commit => {
            let commit = load_object::<Commit>(hash).map_err(|error| {
                CliError::fatal(format!("failed to load commit {hash}: {error}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            refs.push(commit.tree_id);
            refs.extend(commit.parent_commit_ids.iter().copied());
        }
        ObjectType::Tree => {
            let tree = load_object::<Tree>(hash).map_err(|error| {
                CliError::fatal(format!("failed to load tree {hash}: {error}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            for item in &tree.tree_items {
                refs.push(item.id);
            }
        }
        ObjectType::Tag => {
            let tag = load_object::<Tag>(hash).map_err(|error| {
                CliError::fatal(format!("failed to load tag {hash}: {error}"))
                    .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            refs.push(tag.object_hash);
        }
        _ => {}
    }

    Ok(refs)
}

/// BFS to mark all objects reachable from starting points.
fn bfs_mark_reachable(
    starting_points: &HashSet<ObjectHash>,
    storage: &ClientStorage,
) -> CliResult<HashSet<ObjectHash>> {
    let mut reachable = HashSet::new();
    let mut queue: VecDeque<ObjectHash> = starting_points.iter().copied().collect();

    while let Some(current) = queue.pop_front() {
        if reachable.contains(&current) {
            continue;
        }
        reachable.insert(current);

        let children = walk_object_refs(&current, storage)?;
        for child in children {
            if !reachable.contains(&child) {
                queue.push_back(child);
            }
        }
    }

    Ok(reachable)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use git_internal::hash::{HashKind, set_hash_kind_for_test};
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test;

    #[test]
    fn list_idx_objects_reads_v2_hashes() {
        let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
        let temp = tempdir().expect("tempdir");
        let idx_path = temp.path().join("pack-test.idx");

        let hashes = vec![
            ObjectHash::from_bytes(&[0x11; 20]).expect("hash1"),
            ObjectHash::from_bytes(&[0x22; 20]).expect("hash2"),
        ];

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&IDX_MAGIC);
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&vec![0u8; 255 * 4]);
        bytes.extend_from_slice(&(hashes.len() as u32).to_be_bytes());
        for hash in &hashes {
            bytes.extend_from_slice(hash.as_ref());
        }

        let mut file = fs::File::create(&idx_path).expect("create idx");
        file.write_all(&bytes).expect("write idx");

        let read = list_idx_objects(&idx_path).expect("read idx objects");
        assert_eq!(read, hashes);
    }

    #[cfg(unix)]
    #[test]
    fn list_loose_objects_skips_symlinked_outside_dirs() {
        let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
        let temp = tempdir().expect("tempdir");

        let objects_dir = temp.path().join("objects");
        let storage = ClientStorage::init(objects_dir.clone());

        let inside_hash = ObjectHash::from_bytes(&[0xBB; 20]).expect("inside hash");
        storage
            .put(&inside_hash, b"inside", ObjectType::Blob)
            .expect("write inside object");

        let outside_dir = temp.path().join("outside");
        let outside_storage = ClientStorage::init(outside_dir.clone());
        let outside_hash = ObjectHash::from_bytes(&[0xAA; 20]).expect("outside hash");
        outside_storage
            .put(&outside_hash, b"outside", ObjectType::Blob)
            .expect("write outside object");

        let outside_prefix = outside_dir.join("aa");
        let link_path = objects_dir.join("aa");
        std::os::unix::fs::symlink(&outside_prefix, &link_path)
            .expect("create symlink to outside dir");

        let objects = list_loose_objects(&storage, false).expect("list loose objects");
        let has_inside = objects.iter().any(|info| info.hash == inside_hash);
        let has_outside = objects.iter().any(|info| info.hash == outside_hash);

        assert!(has_inside, "expected inside object to be listed");
        assert!(
            !has_outside,
            "symlinked outside object should not be listed"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers dry-run prune inspecting a missing object directory without creating it.
    async fn execute_safe_dry_run_does_not_create_missing_object_directory() {
        let repo = tempdir().expect("tempdir");
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let objects = path::objects();
        fs::remove_dir_all(&objects).expect("remove objects directory");

        execute_safe(
            PruneArgs {
                dry_run: true,
                verbose: false,
                expire: None,
                heads: Vec::new(),
            },
            &OutputConfig::default(),
        )
        .await
        .expect("prune dry-run should succeed");

        assert!(!objects.exists());
    }
}
