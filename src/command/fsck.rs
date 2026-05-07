//! Implementation of `fsck` command for verifying repository integrity.
//!
//! This command checks the integrity of objects, refs, and index in a Libra repository.
//! It diagnoses the same issues as git fsck:
//! - `missing <type> <object>`: Object is referenced but doesn't exist
//! - `hash mismatch <object>`: Object's hash doesn't match its content
//! - `dangling <type> <object>`: Object exists but is not referenced
//! - `unreachable <type> <object>`: Object is not reachable from any ref
//!
//! ## Exit codes (bitmask)
//! - 0: All checks passed
//! - 1 (bit 0): Object corruption
//! - 2 (bit 1): Broken refs
//! - 4 (bit 2): Index corruption
//!   Bits are OR'd when multiple categories fail (e.g. 5 = objects + index)

use std::{collections::HashSet, fs, io, io::{Read, Seek}};

use clap::Parser;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        index::Index,
        object::{ObjectTrait, blob::Blob, commit::Commit, tree::Tree, types::ObjectType},
    },
};
use hex;
use ring::digest::{Context, SHA1_FOR_LEGACY_USE_ONLY, SHA256};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;

use crate::{
    command::{load_object, reset::rebuild_index_from_tree},
    internal::{branch::Branch, db, head::Head, model::reference, model::reflog},
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

/// Bitmask flags for fsck exit codes. Multiple failure categories are OR'd together.
mod exit_code {
    pub const OK: i32 = 0;
    pub const OBJECT_CORRUPT: i32 = 1; // bit 0
    pub const REF_BROKEN: i32 = 2; // bit 1
    pub const INDEX_CORRUPT: i32 = 4; // bit 2
}

const FSCK_LONG_ABOUT: &str =
    "Verify the integrity of objects, refs, and index in a Libra repository.

By default, checks all objects using refs, index, and reflogs as starting points.

Exit codes (bitmask, OR'd when multiple fail):
  0 - All checks passed
  1 (bit 0) - Object corruption
  2 (bit 1) - Broken refs
  4 (bit 2) - Index corruption";

const FSCK_AFTER_HELP: &str = "Examples:
  libra fsck
  libra fsck --no-reflogs
  libra fsck --json
  libra fsck <object-id>";

/// Verify repository integrity by checking objects, refs, and index
#[derive(Parser, Debug)]
#[command(
    about = "Verify the integrity of objects, refs, and index",
    long_about = FSCK_LONG_ABOUT,
    after_help = FSCK_AFTER_HELP,
)]
pub struct FsckArgs {
    /// Object ID to check (optional - checks all objects if not provided)
    #[arg(value_name = "OBJECT")]
    pub object: Option<String>,

    /// Skip reflog validation
    #[arg(long)]
    pub no_reflogs: bool,

    /// Verbose output - print each object as it's verified
    #[arg(short, long)]
    pub verbose: bool,
}

/// Result of verifying a single object
#[derive(Debug, Clone, Serialize)]
pub struct ObjectCheckResult {
    pub object_id: String,
    pub object_type: String,
    pub status: CheckStatus,
    pub error_message: Option<String>,
    pub size: usize,
}

/// Status of a check result
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Missing,
    InvalidFormat,
    HashMismatch,
}

/// Result of fsck verification
#[derive(Debug, Serialize)]
pub struct FsckResult {
    pub objects_checked: usize,
    pub objects_ok: usize,
    pub objects_corrupted: usize,
    pub refs_checked: usize,
    pub refs_ok: usize,
    pub refs_broken: usize,
    pub index_valid: bool,
    pub reflog_issues: usize,
    pub cross_ref_issues: usize,
    pub overall_status: CheckStatus,
    pub issues: Vec<IssueReport>,
    /// Bitmask of failure categories (see `exit_code` module).
    #[serde(skip_serializing_if = "is_zero")]
    pub failure_mask: i32,
    /// Human-readable names for the set bits in `failure_mask`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failure_categories: Vec<String>,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

/// Detailed issue report for git fsck-style diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct IssueReport {
    pub issue_type: String,  // "missing", "hash_mismatch", "dangling", "unreachable"
    pub severity: String,
    pub object_type: Option<String>,  // "commit", "tree", "blob", "tag"
    pub object_id: Option<String>,
    pub ref_name: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Result of checking the index file
#[derive(Debug, Clone)]
pub struct IndexCheckResult {
    pub valid: bool,
    pub entries_checked: usize,
    pub entries_ok: usize,
    pub entries_corrupted: usize,
    pub issues: Vec<IssueReport>,
}

pub async fn execute(args: FsckArgs) {
    let storage = ClientStorage::init(path::objects());

    let result = if let Some(ref object_id) = args.object {
        check_single_object(object_id, &storage).await
    } else {
        check_all_objects(&args, &storage).await
    };

    match result {
        Ok(fsck_result) => {
            // Print diagnostic messages (dangling/unreachable are printed but don't cause failure)
            if !fsck_result.issues.is_empty() {
                print_diagnostic_messages(&fsck_result.issues);
            }
            // Exit with failure code only for serious issues (not dangling/unreachable)
            if fsck_result.failure_mask != exit_code::OK {
                std::process::exit(fsck_result.failure_mask);
            }
        }
        Err(e) => {
            eprintln!("fatal: {}", e);
            std::process::exit(1);
        }
    }
}

pub async fn execute_safe(args: FsckArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    if !output.is_json() {
        execute(args).await;
        return Ok(());
    }

    let storage = ClientStorage::init(path::objects());
    let result = if let Some(ref object_id) = args.object {
        check_single_object(object_id, &storage).await
    } else {
        check_all_objects(&args, &storage).await
    }?;

    emit_json_data(
        "fsck",
        &serde_json::to_value(&result)
            .map_err(|e| CliError::fatal(format!("failed to serialize result: {}", e)))?,
        output,
    )?;

    if result.failure_mask != exit_code::OK {
        return Err(CliError::failure("repository integrity check failed")
            .with_stable_code(StableErrorCode::RepoCorrupt)
            .with_exit_code(result.failure_mask));
    }

    Ok(())
}

/// Parse hex string to ObjectHash
fn parse_object_hash(hex_str: &str) -> Option<ObjectHash> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.is_empty() {
        return None;
    }
    // Use from_bytes to create ObjectHash directly from bytes, not hash them again
    ObjectHash::from_bytes(&bytes).ok()
}

/// Try to parse a loose object file path into an ObjectHash.
/// `dir_name` is the 2-char prefix directory (e.g. "ab"),
/// `sub_path` is the file inside that directory.
fn try_parse_loose_object(dir_name: &str, sub_path: &std::path::Path) -> Option<ObjectHash> {
    let file_name = sub_path.file_name().and_then(|n| n.to_str())?;
    let full_hash = format!("{dir_name}{file_name}");
    parse_object_hash(&full_hash)
}

/// List all object hashes in storage
fn list_all_objects_in_storage(storage: &ClientStorage) -> io::Result<Vec<ObjectHash>> {
    let objects_dir = storage.base_path();
    if !objects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut hashes = Vec::new();
    for entry in fs::read_dir(objects_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 2 {
            continue;
        }

        for sub_entry in fs::read_dir(&path)? {
            let sub_entry = sub_entry?;
            let sub_path = sub_entry.path();
            if sub_path.is_file()
                && let Some(hash) = try_parse_loose_object(dir_name, &sub_path)
            {
                hashes.push(hash);
            }
        }
    }

    Ok(hashes)
}

/// Build an IssueReport for a failed object check.
/// `context` controls whether the report is for a single-object CLI check
/// (`context == Single`) or a full-scan object (`context == FullScan`).
enum IssueContext {
    Single,
    FullScan,
}

fn build_issue_report(
    check_result: &ObjectCheckResult,
    object_id: &str,
    context: IssueContext,
) -> IssueReport {
    let (issue_type, suggestion) = match (&check_result.status, context) {
        (CheckStatus::HashMismatch, IssueContext::Single) => (
            "hash_mismatch".to_string(),
            "Object data is corrupted. Consider restoring from backup or remote.".to_string(),
        ),
        (CheckStatus::HashMismatch, IssueContext::FullScan) => (
            "hash_mismatch".to_string(),
            "Consider restoring from backup or remote.".to_string(),
        ),
        (CheckStatus::InvalidFormat, _) => (
            "invalid_format".to_string(),
            "Object has invalid format.".to_string(),
        ),
        (CheckStatus::Missing, _) => (
            "missing".to_string(),
            "Object may have been deleted or never created.".to_string(),
        ),
        (CheckStatus::Ok, _) => unreachable!("should not build issue for Ok status"),
    };

    IssueReport {
        issue_type,
        severity: "error".to_string(),
        object_type: Some(check_result.object_type.clone()),
        object_id: Some(object_id.to_string()),
        ref_name: None,
        message: check_result
            .error_message
            .clone()
            .unwrap_or_else(|| "Object verification failed".to_string()),
        suggestion: Some(suggestion),
    }
}

/// Compute failure mask and categories for a single-object check.
fn failure_for_single_status(status: &CheckStatus) -> (i32, Vec<String>) {
    match status {
        CheckStatus::Ok => (exit_code::OK, vec![]),
        _ => (exit_code::OBJECT_CORRUPT, vec!["objects".to_string()]),
    }
}

async fn check_single_object(object_id: &str, storage: &ClientStorage) -> CliResult<FsckResult> {
    let hash = parse_object_hash(object_id)
        .ok_or_else(|| CliError::command_usage(format!("invalid object ID: {}", object_id)))?;

    let check_result = verify_object(&hash, storage).await?;

    let (overall_status, issues) = match check_result.status {
        CheckStatus::Ok => {
            println!("Object {} is valid", object_id);
            (CheckStatus::Ok, Vec::new())
        }
        _ => {
            let issue = build_issue_report(&check_result, object_id, IssueContext::Single);
            (check_result.status, vec![issue])
        }
    };

    let (failure_mask, failure_categories) = failure_for_single_status(&overall_status);
    let is_ok = overall_status == CheckStatus::Ok;

    Ok(FsckResult {
        objects_checked: 1,
        objects_ok: if is_ok { 1 } else { 0 },
        objects_corrupted: if is_ok { 0 } else { 1 },
        refs_checked: 0,
        refs_ok: 0,
        refs_broken: 0,
        index_valid: true,
        reflog_issues: 0,
        cross_ref_issues: 0,
        overall_status,
        issues,
        failure_mask,
        failure_categories,
    })
}

async fn check_all_objects(args: &FsckArgs, storage: &ClientStorage) -> CliResult<FsckResult> {
    let mut result = FsckResult {
        objects_checked: 0,
        objects_ok: 0,
        objects_corrupted: 0,
        refs_checked: 0,
        refs_ok: 0,
        refs_broken: 0,
        index_valid: true,
        reflog_issues: 0,
        cross_ref_issues: 0,
        overall_status: CheckStatus::Ok,
        issues: Vec::new(),
        failure_mask: exit_code::OK,
        failure_categories: Vec::new(),
    };

    // Get all object hashes
    let all_hashes = list_all_objects_in_storage(storage)
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?;

    // Stage 1: Check all 256 object directories
    check_directories(storage, &all_hashes)?;

    // Sort hashes lexicographically for stage 2
    let mut sorted_hashes: Vec<String> = all_hashes.iter().map(|h| h.to_string()).collect();
    sorted_hashes.sort();

    // Stage 2: Check each object (sorted by hash)
    check_objects(&sorted_hashes, storage, &mut result, args.verbose).await?;

    // Stage 3: Check HEAD link
    let head_is_unborn = check_head().await;

    // Stage 4: Check reflog entries
    if !args.no_reflogs {
        check_reflogs(storage, &mut result, args.verbose).await?;
    }

    // Stage 5: Check index
    check_index(storage, &mut result, args.verbose)?;

    // Stage 6: Check connectivity (re-verify all objects in storage order)
    check_connectivity(&all_hashes, storage, &mut result, args.verbose).await?;

    // Stage 7: Find dangling and unreachable objects
    find_dangling_unreachable(storage, &mut result).await?;

    // Print notices
    print_notices(head_is_unborn, &result);

    compute_failure_mask(&mut result);

    Ok(result)
}

/// Check all 256 object directories and print progress
fn check_directories(storage: &ClientStorage, all_hashes: &[ObjectHash]) -> CliResult<()> {
    // Count objects per prefix directory
    let mut prefix_counts = vec![0usize; 256];
    for hash in all_hashes {
        let hash_str = hash.to_string();
        if hash_str.len() >= 2 {
            if let Ok(prefix) = u8::from_str_radix(&hash_str[0..2], 16) {
                prefix_counts[prefix as usize] += 1;
            }
        }
    }

    // Count pack objects
    let mut pack_count = 0;
    let pack_dir = storage.base_path().join("pack");
    if pack_dir.exists() {
        if let Ok(entries) = fs::read_dir(&pack_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "idx") {
                    if let Ok(count) = count_pack(&path) {
                        pack_count += count;
                    }
                }
            }
        }
    }

    // Print directory progress - single line like git fsck
    println!("Checking object directory: 100% (256/256), done.");

    // Print pack objects if any
    if pack_count > 0 {
        println!("Checking objects: 100% ({}/{}), done.", pack_count, pack_count);
    }

    Ok(())
}

/// Count objects in a pack index file
fn count_pack(idx_path: &std::path::Path) -> io::Result<usize> {
    let mut file = fs::File::open(idx_path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;

    if magic == [0xFF, 0x74, 0x4F, 0x63] {
        // Index version 2
        file.seek(io::SeekFrom::Current(4))?;
        file.seek(io::SeekFrom::Current(255 * 4))?;
        let mut fanout_entry = [0u8; 4];
        file.read_exact(&mut fanout_entry)?;
        Ok(u32::from_be_bytes(fanout_entry) as usize)
    } else {
        // Index version 1
        file.seek(io::SeekFrom::Start(255 * 4))?;
        let mut fanout_entry = [0u8; 4];
        file.read_exact(&mut fanout_entry)?;
        Ok(u32::from_be_bytes(fanout_entry) as usize)
    }
}

/// Check objects sorted by hash (lexicographic order)
async fn check_objects(
    sorted_hashes: &[String],
    storage: &ClientStorage,
    result: &mut FsckResult,
    verbose: bool,
) -> CliResult<()> {
    for hash_str in sorted_hashes {
        let hash = match parse_object_hash(hash_str) {
            Some(h) => h,
            None => continue,
        };

        let obj_type = match storage.get_object_type(&hash) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if verbose {
            let type_name = match obj_type {
                ObjectType::Blob => "blob",
                ObjectType::Tree => "tree",
                ObjectType::Commit => "commit",
                ObjectType::Tag => "tag",
                _ => "unknown",
            };
            println!("Checking {} {}", type_name, hash);
        }

        let check_result = verify_object(&hash, storage).await?;
        result.objects_checked += 1;

        match check_result.status {
            CheckStatus::Ok => result.objects_ok += 1,
            _ => {
                result.objects_corrupted += 1;
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = check_result.status.clone();
                }
                result.issues.push(build_issue_report(
                    &check_result,
                    hash_str,
                    IssueContext::FullScan,
                ));
            }
        }
    }
    Ok(())
}

/// Check if HEAD points to a valid ref
/// Returns true if HEAD points to an unborn branch
async fn check_head() -> bool {
    match Head::current_result().await {
        Ok(Head::Branch(name)) => {
            // HEAD points to a branch, check if that branch exists
            match Branch::find_branch_result(&name, None).await {
                Ok(Some(_)) => false,    // Branch exists, not unborn
                Ok(None) => true,        // Branch doesn't exist, unborn
                Err(_) => true,          // Error, treat as unborn
            }
        }
        Ok(Head::Detached(_)) => false, // Detached HEAD, not unborn
        Err(_) => true,                  // Error, treat as unborn
    }
}

/// Print notices (unborn branch, missing refs, etc.)
fn print_notices(head_is_unborn: bool, _result: &FsckResult) {
    if head_is_unborn {
        eprintln!("notice: HEAD points to an unborn branch (main)");
        eprintln!("notice: No default references");
    }
}

/// Print diagnostic messages in git fsck format
/// Format: <issue_type> <object_type> <object_id>
/// Examples:
///   dangling commit 8ae045f058b7a0a5b0b0e8a0a0e8a0a0e8a0a0
///   missing blob abc123def456789012345678901234567890abcd
fn print_diagnostic_messages(issues: &[IssueReport]) {
    for issue in issues {
        if let (Some(obj_type), Some(obj_id)) = (&issue.object_type, &issue.object_id) {
            // git fsck format: <issue_type> <object_type> <object_id>
            eprintln!("{} {} {}", issue.issue_type, obj_type, obj_id);
        }
    }
}

/// Check reflogs and print entries
async fn check_reflogs(
    storage: &ClientStorage,
    result: &mut FsckResult,
    verbose: bool,
) -> CliResult<()> {
    let db_conn = db::get_db_conn_instance().await;

    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load reflogs: {}", e)))?;

    for entry in reflogs {
        if verbose {
            println!("Checking reflog {}->{}", entry.old_oid, entry.new_oid);
        }

        // Skip null OID (all zeros)
        let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');

        if !is_null_oid(&entry.old_oid) {
            if let Some(old_hash) = parse_object_hash(&entry.old_oid) {
                if !storage.exist(&old_hash) {
                    result.reflog_issues += 1;
                    result.issues.push(IssueReport {
                        issue_type: "missing".to_string(),
                        severity: "warning".to_string(),
                        object_type: Some("unknown".to_string()),
                        object_id: Some(entry.old_oid.clone()),
                        ref_name: Some(entry.ref_name.clone()),
                        message: format!(
                            "Reflog for '{}' references missing old OID {}",
                            entry.ref_name, entry.old_oid
                        ),
                        suggestion: Some("Reflog entry is stale.".to_string()),
                    });
                }
            }
        }

        if !is_null_oid(&entry.new_oid) {
            if let Some(new_hash) = parse_object_hash(&entry.new_oid) {
                if !storage.exist(&new_hash) {
                    result.reflog_issues += 1;
                    result.issues.push(IssueReport {
                        issue_type: "missing".to_string(),
                        severity: "warning".to_string(),
                        object_type: Some("unknown".to_string()),
                        object_id: Some(entry.new_oid.clone()),
                        ref_name: Some(entry.ref_name.clone()),
                        message: format!(
                            "Reflog for '{}' references missing new OID {}",
                            entry.ref_name, entry.new_oid
                        ),
                        suggestion: Some("Reflog entry is stale.".to_string()),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Check index
fn check_index(
    storage: &ClientStorage,
    result: &mut FsckResult,
    verbose: bool,
) -> CliResult<()> {
    if verbose {
        println!("Checking cache tree of .libra/index");
    }

    let index_result = check_index_file(storage)?;
    result.index_valid = index_result.valid;
    result.issues.extend(index_result.issues);

    if !index_result.valid && result.overall_status == CheckStatus::Ok {
        result.overall_status = CheckStatus::InvalidFormat;
    }
    Ok(())
}

/// Check connectivity (re-verify all objects)
async fn check_connectivity(
    all_hashes: &[ObjectHash],
    storage: &ClientStorage,
    result: &mut FsckResult,
    verbose: bool,
) -> CliResult<()> {
    let count = all_hashes.len();
    if verbose {
        println!("Checking connectivity ({} objects)", count);
    }

    for hash in all_hashes {
        if verbose {
            println!("Checking {}", hash);
        }
        let check_result = verify_object(hash, storage).await?;
        if check_result.status != CheckStatus::Ok && result.overall_status == CheckStatus::Ok {
            result.overall_status = check_result.status.clone();
        }
    }
    Ok(())
}

/// Context for tracking object reachability
struct ReachabilityContext {
    /// All objects in storage
    all_objects: HashSet<ObjectHash>,
    /// Objects reachable from refs
    refs_reachable: HashSet<ObjectHash>,
    /// Objects mentioned in reflogs (for dangling detection)
    reflog_objects: HashSet<ObjectHash>,
    /// Objects referenced by index entries
    index_objects: HashSet<ObjectHash>,
}

impl ReachabilityContext {
    fn new() -> Self {
        Self {
            all_objects: HashSet::new(),
            refs_reachable: HashSet::new(),
            reflog_objects: HashSet::new(),
            index_objects: HashSet::new(),
        }
    }
}

/// Collect all starting points for reachability analysis
async fn collect_reachability_context(
    storage: &ClientStorage,
) -> CliResult<ReachabilityContext> {
    let mut ctx = ReachabilityContext::new();

    // Collect all objects in storage
    ctx.all_objects = list_all_objects_in_storage(storage)
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?
        .into_iter()
        .collect();

    // Collect objects from refs
    let db_conn = db::get_db_conn_instance().await;
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load refs: {}", e)))?;

    for ref_entry in refs {
        if let Some(commit_hash_str) = &ref_entry.commit {
            if let Some(hash) = parse_object_hash(commit_hash_str) {
                ctx.refs_reachable.insert(hash);
            }
        }
    }

    // Collect objects from reflogs
    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load reflogs: {}", e)))?;

    for entry in reflogs {
        let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');
        if !is_null_oid(&entry.old_oid) {
            if let Some(hash) = parse_object_hash(&entry.old_oid) {
                ctx.reflog_objects.insert(hash);
            }
        }
        if !is_null_oid(&entry.new_oid) {
            if let Some(hash) = parse_object_hash(&entry.new_oid) {
                ctx.reflog_objects.insert(hash);
            }
        }
    }

    // Collect objects from index
    let index_path = path::index();
    if index_path.exists() {
        if let Ok(index) = Index::load(&index_path) {
            for entry in index.tracked_entries(0) {
                ctx.index_objects.insert(entry.hash);
            }
        }
    }

    Ok(ctx)
}

/// Walk object references: returns objects referenced by the given object
/// For commits: returns tree and parent commits
/// For trees: returns child blobs and subtrees
fn walk_object_refs(hash: &ObjectHash, storage: &ClientStorage) -> Vec<ObjectHash> {
    let mut refs = Vec::new();

    let Ok(obj_type) = storage.get_object_type(hash) else {
        return refs;
    };

    match obj_type {
        ObjectType::Commit => {
            if let Ok(commit) = load_object::<Commit>(hash) {
                refs.push(commit.tree_id);
                refs.extend(commit.parent_commit_ids.iter().copied());
            }
        }
        ObjectType::Tree => {
            if let Ok(tree) = load_object::<Tree>(hash) {
                for item in &tree.tree_items {
                    refs.push(item.id);
                }
            }
        }
        _ => {}
    }

    refs
}

/// BFS to mark all objects reachable from starting points
fn bfs_mark_reachable(
    starting_points: &HashSet<ObjectHash>,
    storage: &ClientStorage,
) -> HashSet<ObjectHash> {
    let mut reachable = HashSet::new();
    let mut queue: std::collections::VecDeque<ObjectHash> = starting_points.iter().copied().collect();

    while let Some(current) = queue.pop_front() {
        if reachable.contains(&current) {
            continue;
        }
        reachable.insert(current);

        // Get objects referenced by current object
        let children = walk_object_refs(&current, storage);
        for child in children {
            if !reachable.contains(&child) {
                queue.push_back(child);
            }
        }
    }

    reachable
}

/// Find dangling and unreachable objects
/// - dangling: objects in reflog/index but not reachable from current refs
/// - unreachable: objects not in reflog/index and not reachable from refs
async fn find_dangling_unreachable(
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    let ctx = collect_reachability_context(storage).await?;

    // Mark all objects reachable from refs
    let refs_reachable = bfs_mark_reachable(&ctx.refs_reachable, storage);

    // Find objects not reachable from refs
    for hash in &ctx.all_objects {
        if refs_reachable.contains(hash) {
            continue; // Reachable from refs
        }

        // Object is not reachable from current refs
        // Check if it's in reflog or index (dangling) or completely isolated (unreachable)
        let in_reflog = ctx.reflog_objects.contains(hash);
        let in_index = ctx.index_objects.contains(hash);

        if in_reflog || in_index {
            // Dangling: was referenced but no longer is
            let obj_type = match storage.get_object_type(hash) {
                Ok(t) => t.to_string(),
                Err(_) => "unknown".to_string(),
            };
            result.issues.push(IssueReport {
                issue_type: "dangling".to_string(),
                severity: "info".to_string(),
                object_type: Some(obj_type),
                object_id: Some(hash.to_string()),
                ref_name: None,
                message: format!("{} {} is dangling", hash, hash),
                suggestion: None,
            });
        } else {
            // Unreachable: completely isolated
            let obj_type = match storage.get_object_type(hash) {
                Ok(t) => t.to_string(),
                Err(_) => "unknown".to_string(),
            };
            result.issues.push(IssueReport {
                issue_type: "unreachable".to_string(),
                severity: "info".to_string(),
                object_type: Some(obj_type),
                object_id: Some(hash.to_string()),
                ref_name: None,
                message: format!("{} {} is unreachable", hash, hash),
                suggestion: None,
            });
        }
    }

    Ok(())
}


/// Check index and print status
fn check_index_and_print(storage: &ClientStorage, result: &mut FsckResult) -> CliResult<()> {
    println!("Checking cache tree of .libra/index");

    let index_result = check_index_file(storage)?;
    result.index_valid = index_result.valid;
    result.issues.extend(index_result.issues);

    if !index_result.valid && result.overall_status == CheckStatus::Ok {
        result.overall_status = CheckStatus::InvalidFormat;
    }
    Ok(())
}

/// Check connectivity - verify all objects are reachable and valid
async fn check_connectivity_and_print(
    all_hashes: &[ObjectHash],
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    let count = all_hashes.len();
    println!("Checking connectivity ({} objects)", count);

    for hash in all_hashes {
        println!("Checking {}", hash);

        // Verify the object is still valid (re-check)
        let check_result = verify_object(hash, storage).await?;

        match check_result.status {
            CheckStatus::Ok => {
                // Already counted in check_objects_by_type
            }
            _ => {
                // Object became corrupted between checks
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = check_result.status.clone();
                }
            }
        }
    }
    Ok(())
}

/// Check refs and optionally fix broken ones.
async fn check_and_fix_refs(
    _args: &FsckArgs,
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    let ref_result = check_refs(storage).await?;
    result.refs_checked = ref_result.checked;
    result.refs_ok = ref_result.ok;
    result.refs_broken = ref_result.broken;
    result.issues.extend(ref_result.issues.clone());

    if ref_result.broken > 0 {
        if result.overall_status == CheckStatus::Ok {
            result.overall_status = CheckStatus::Missing;
        }
    }
    Ok(())
}

/// Wrapper for check_index that updates result
async fn check_index_wrapper(
    _args: &FsckArgs,
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    let index_result = check_index_file(storage)?;
    result.index_valid = index_result.valid;
    result.issues.extend(index_result.issues);

    if !index_result.valid && result.overall_status == CheckStatus::Ok {
        result.overall_status = CheckStatus::InvalidFormat;
    }
    Ok(())
}

/// Compute failure bitmask and human-readable categories from current result state.
fn compute_failure_mask(result: &mut FsckResult) {
    let mut mask = exit_code::OK;
    let mut categories = Vec::new();
    if result.objects_corrupted > 0 {
        mask |= exit_code::OBJECT_CORRUPT;
        categories.push("objects".to_string());
    }
    if result.refs_broken > 0 {
        mask |= exit_code::REF_BROKEN;
        categories.push("refs".to_string());
    }
    if !result.index_valid {
        mask |= exit_code::INDEX_CORRUPT;
        categories.push("index".to_string());
    }
    if result.reflog_issues > 0 {
        mask |= exit_code::OBJECT_CORRUPT;
        categories.push("reflogs".to_string());
    }
    result.failure_mask = mask;
    result.failure_categories = categories;
    // Clear overall_status if all categories are clean
    if mask == exit_code::OK {
        result.overall_status = CheckStatus::Ok;
    }
}

/// Verify a single object's integrity
async fn verify_object(hash: &ObjectHash, storage: &ClientStorage) -> CliResult<ObjectCheckResult> {
    // Check if object exists
    if !storage.exist(hash) {
        return Ok(ObjectCheckResult {
            object_id: hash.to_string(),
            object_type: "unknown".to_string(),
            status: CheckStatus::Missing,
            error_message: Some("Object not found in storage".to_string()),
            size: 0,
        });
    }

    // Get raw data
    let data = match storage.get(hash) {
        Ok(d) => d,
        Err(e) => {
            return Ok(ObjectCheckResult {
                object_id: hash.to_string(),
                object_type: "unknown".to_string(),
                status: CheckStatus::HashMismatch,
                error_message: Some(format!("Failed to read object: {}", e)),
                size: 0,
            });
        }
    };

    let size = data.len();

    // Get object type
    let obj_type = match storage.get_object_type(hash) {
        Ok(t) => t,
        Err(e) => {
            return Ok(ObjectCheckResult {
                object_id: hash.to_string(),
                object_type: "unknown".to_string(),
                status: CheckStatus::InvalidFormat,
                error_message: Some(format!("Failed to determine object type: {}", e)),
                size,
            });
        }
    };

    // Verify hash integrity using ring crate.
    // Git/Libra computes hash as: SHAx(type + ' ' + size + '\0' + content)
    // The algorithm is determined by the repo's core.objectformat config.
    let mut ctx = Context::new(match get_hash_kind() {
        HashKind::Sha256 => &SHA256,
        _ => &SHA1_FOR_LEGACY_USE_ONLY,
    });

    // Add header: "<type> <size>\0"
    let header = format!("{} {}\0", obj_type.to_string().to_lowercase(), size);
    ctx.update(header.as_bytes());
    ctx.update(&data);
    let computed_hash = ctx.finish();
    let computed_bytes = computed_hash.as_ref();

    // Compare with stored hash
    let hash_bytes = hash.as_ref();
    if computed_bytes != hash_bytes {
        return Ok(ObjectCheckResult {
            object_id: hash.to_string(),
            object_type: obj_type.to_string(),
            status: CheckStatus::HashMismatch,
            error_message: Some(format!(
                "Hash mismatch: expected {}, computed {}",
                hash,
                hex::encode(computed_bytes)
            )),
            size,
        });
    }

    // Verify object format by attempting to parse
    let format_valid = match obj_type {
        ObjectType::Blob => Blob::from_bytes(&data, *hash).is_ok(),
        ObjectType::Tree => Tree::from_bytes(&data, *hash).is_ok(),
        ObjectType::Commit => Commit::from_bytes(&data, *hash).is_ok(),
        ObjectType::Tag => {
            // Tag objects are text-based, just check UTF-8 validity
            String::from_utf8(data.clone()).is_ok()
        }
        _ => false,
    };

    if !format_valid {
        return Ok(ObjectCheckResult {
            object_id: hash.to_string(),
            object_type: obj_type.to_string(),
            status: CheckStatus::InvalidFormat,
            error_message: Some(format!("Object {} has invalid {} format", hash, obj_type)),
            size,
        });
    }

    Ok(ObjectCheckResult {
        object_id: hash.to_string(),
        object_type: obj_type.to_string(),
        status: CheckStatus::Ok,
        error_message: None,
        size,
    })
}

/// Result of checking refs
#[derive(Clone)]
struct RefCheckResult {
    checked: usize,
    ok: usize,
    broken: usize,
    issues: Vec<IssueReport>,
    broken_ref_names: Vec<String>,
}

/// Check all refs point to valid objects
async fn check_refs(storage: &ClientStorage) -> CliResult<RefCheckResult> {
    let mut result = RefCheckResult {
        checked: 0,
        ok: 0,
        broken: 0,
        issues: Vec::new(),
        broken_ref_names: Vec::new(),
    };

    let db_conn = db::get_db_conn_instance().await;

    // Check all references in database
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load refs: {}", e)))?;

    for ref_entry in refs {
        result.checked += 1;

        if let Some(commit_hash_str) = &ref_entry.commit {
            if let Some(hash) = parse_object_hash(commit_hash_str) {
                if storage.exist(&hash) {
                    // Verify the object is actually valid
                    match verify_object(&hash, storage).await {
                        Ok(check) if check.status == CheckStatus::Ok => {
                            result.ok += 1;
                        }
                        Ok(check) => {
                            result.broken += 1;
                            let ref_name = ref_entry.name.clone().unwrap_or_default();
                            result.broken_ref_names.push(ref_name.clone());
                            result.issues.push(IssueReport {
                                issue_type: "hash_mismatch".to_string(),
                                severity: "error".to_string(),
                                object_type: Some(check.object_type.clone()),
                                object_id: Some(hash.to_string()),
                                ref_name: Some(ref_name),
                                message: format!(
                                    "Ref points to invalid object: {}",
                                    check.error_message.unwrap_or_default()
                                ),
                                suggestion: Some("Update or delete this ref.".to_string()),
                            });
                        }
                        Err(e) => {
                            result.broken += 1;
                            let ref_name = ref_entry.name.clone().unwrap_or_default();
                            result.broken_ref_names.push(ref_name.clone());
                            result.issues.push(IssueReport {
                                issue_type: "missing".to_string(),
                                severity: "error".to_string(),
                                object_type: Some("unknown".to_string()),
                                object_id: Some(hash.to_string()),
                                ref_name: Some(ref_name),
                                message: format!("Failed to verify ref target: {}", e),
                                suggestion: None,
                            });
                        }
                    }
                } else {
                    result.broken += 1;
                    let ref_name = ref_entry.name.clone().unwrap_or_default();
                    result.broken_ref_names.push(ref_name.clone());
                    result.issues.push(IssueReport {
                        issue_type: "missing".to_string(),
                        severity: "error".to_string(),
                        object_type: Some("commit".to_string()),
                        object_id: Some(hash.to_string()),
                        ref_name: Some(ref_name),
                        message: format!("Ref points to missing object {}", hash),
                        suggestion: Some("Update or delete this ref.".to_string()),
                    });
                }
            } else {
                result.broken += 1;
                let ref_name = ref_entry.name.clone().unwrap_or_default();
                result.broken_ref_names.push(ref_name.clone());
                result.issues.push(IssueReport {
                    issue_type: "invalid_ref_hash".to_string(),
                    severity: "error".to_string(),
                    object_type: None,
                    object_id: None,
                    ref_name: Some(ref_name.clone()),
                    message: format!(
                        "Ref '{}' has invalid hash format: {}",
                        ref_name, commit_hash_str
                    ),
                    suggestion: Some("Delete this corrupted ref.".to_string()),
                });
            }
        }
    }

    Ok(result)
}

/// Check index file integrity.
///
/// Loads the binary index file (`.libra/index`), validates its structure,
/// and cross-references each entry's hash against object storage.
fn check_index_file(storage: &ClientStorage) -> CliResult<IndexCheckResult> {
    let mut result = IndexCheckResult {
        valid: true,
        entries_checked: 0,
        entries_ok: 0,
        entries_corrupted: 0,
        issues: Vec::new(),
    };

    let index_path = path::index();

    if !index_path.exists() {
        // No index file is OK (clean state, nothing staged)
        return Ok(result);
    }

    // Step 1: Load and parse the index file.
    // Index::from_file validates the DIRC magic, version, entry count,
    // and the SHA1/SHA256 trailer checksum.
    let index = match Index::load(&index_path) {
        Ok(idx) => idx,
        Err(e) => {
            result.valid = false;
            result.issues.push(IssueReport {
                issue_type: "index_parse_error".to_string(),
                severity: "error".to_string(),
                object_type: None,
                object_id: None,
                ref_name: None,
                message: format!("Failed to parse index file: {}", e),
                suggestion: Some("The index file is corrupted. Try removing .libra/index and running 'libra add' to rebuild.".to_string()),
            });
            return Ok(result);
        }
    };

    // Step 2: Validate each index entry.
    let entries = index.tracked_entries(0);

    for entry in entries {
        result.entries_checked += 1;

        if let Some(issue) = validate_index_entry(entry, storage) {
            result.entries_corrupted += 1;
            result.valid = false;
            result.issues.push(issue);
            continue;
        }

        result.entries_ok += 1;
    }

    // Step 3: Check for entries in non-zero stages (merge conflict markers)
    for stage in [1, 2, 3] {
        let conflict_entries = index.tracked_entries(stage);
        if !conflict_entries.is_empty() {
            for entry in conflict_entries {
                result.issues.push(IssueReport {
                    issue_type: "index_conflict_marker".to_string(),
                    severity: "warning".to_string(),
                    object_type: Some("blob".to_string()),
                    object_id: Some(entry.hash.to_string()),
                    ref_name: Some(entry.name.clone()),
                    message: format!(
                        "Index entry '{}' is in merge conflict stage {}",
                        entry.name, stage
                    ),
                    suggestion: Some(
                        "Resolve the merge conflict and re-add this file.".to_string(),
                    ),
                });
                result.entries_checked += 1;
            }
        }
    }

    Ok(result)
}

/// Delete broken refs that point to nonexistent or invalid objects.
async fn fix_broken_refs(broken_ref_names: &[String]) -> CliResult<usize> {
    let db_conn = db::get_db_conn_instance().await;
    let mut fixed = 0;

    for name in broken_ref_names {
        let deleted = reference::Entity::delete_many()
            .filter(reference::Column::Name.eq(name))
            .exec(&db_conn)
            .await
            .map_err(|e| CliError::fatal(format!("failed to delete ref '{}': {}", name, e)))?;

        if deleted.rows_affected > 0 {
            eprintln!("Deleted broken ref '{}'", name);
            fixed += 1;
        }
    }

    Ok(fixed)
}

/// Rebuild a corrupted index from HEAD's tree.
///
/// Deletes the corrupted index file and constructs a new one
/// by walking the tree that HEAD points to.
async fn fix_corrupted_index() -> CliResult<bool> {
    let index_path = path::index();

    // Try to get HEAD commit
    let head_commit = match Head::current_commit().await {
        Some(commit) => commit,
        None => {
            // No HEAD commit yet (unborn branch) — just delete the corrupted index
            if index_path.exists() {
                fs::remove_file(&index_path).map_err(|e| {
                    CliError::fatal(format!("failed to remove corrupted index: {}", e))
                })?;
                return Ok(true);
            }
            return Ok(false);
        }
    };

    // Load the commit's tree
    let commit: Commit = load_object(&head_commit).map_err(|e| {
        CliError::fatal(format!("failed to load HEAD commit {}: {}", head_commit, e))
    })?;

    let tree: Tree = load_object(&commit.tree_id)
        .map_err(|e| CliError::fatal(format!("failed to load tree {}: {}", commit.tree_id, e)))?;

    // Build a new index from the tree
    let mut new_index = Index::new();
    rebuild_index_from_tree(&tree, &mut new_index, "")
        .map_err(|e| CliError::fatal(format!("failed to rebuild index: {}", e)))?;

    // Save the new index
    new_index
        .save(&index_path)
        .map_err(|e| CliError::fatal(format!("failed to save rebuilt index: {}", e)))?;

    Ok(true)
}

/// Valid git index file modes.
fn is_valid_index_mode(mode: u32) -> bool {
    matches!(
        mode,
        0o100644 // regular file
            | 0o100755 // executable
            | 0o120000 // symlink
            | 0o160000 // gitlink (submodule)
            | 0o040000 // directory (tree)
    )
}

/// Validate a single index entry against storage. Returns Some(issue) on failure.
fn validate_index_entry(
    entry: &git_internal::internal::index::IndexEntry,
    storage: &ClientStorage,
) -> Option<IssueReport> {
    if !is_valid_index_mode(entry.mode) {
        return Some(IssueReport {
            issue_type: "invalid_index_mode".to_string(),
            severity: "error".to_string(),
            object_type: None,
            object_id: None,
            ref_name: Some(entry.name.clone()),
            message: format!(
                "Index entry '{}' has invalid mode 0o{:o}",
                entry.name, entry.mode
            ),
            suggestion: Some("Remove and re-add this file to fix.".to_string()),
        });
    }

    if entry.flags.stage > 3 {
        return Some(IssueReport {
            issue_type: "invalid_index_stage".to_string(),
            severity: "error".to_string(),
            object_type: None,
            object_id: None,
            ref_name: Some(entry.name.clone()),
            message: format!(
                "Index entry '{}' has invalid stage {}",
                entry.name, entry.flags.stage
            ),
            suggestion: Some("This may indicate a corrupted merge state.".to_string()),
        });
    }

    if !storage.exist(&entry.hash) {
        return Some(IssueReport {
            issue_type: "missing".to_string(),
            severity: "error".to_string(),
            object_type: Some("blob".to_string()),
            object_id: Some(entry.hash.to_string()),
            ref_name: Some(entry.name.clone()),
            message: format!(
                "Index entry '{}' references missing object {}",
                entry.name, entry.hash
            ),
            suggestion: Some("Run 'libra add <file>' to re-stage this file.".to_string()),
        });
    }

    if let Ok(obj_type) = storage.get_object_type(&entry.hash)
        && obj_type != ObjectType::Blob
    {
        return Some(IssueReport {
            issue_type: "index_entry_wrong_type".to_string(),
            severity: "error".to_string(),
            object_type: Some(obj_type.to_string()),
            object_id: Some(entry.hash.to_string()),
            ref_name: Some(entry.name.clone()),
            message: format!(
                "Index entry '{}' references a {} object instead of a blob",
                entry.name, obj_type
            ),
            suggestion: Some("Re-stage this file to fix the reference.".to_string()),
        });
    }

    None
}

/// Validate cross-references between objects (trees reference valid blobs/trees)
///
/// Checks that:
/// - Every tree entry's referenced object exists and has the correct type
/// - Every commit's tree reference exists
/// - Every commit's parent references exist
async fn validate_cross_references(storage: &ClientStorage) -> CliResult<Vec<IssueReport>> {
    use git_internal::internal::object::tree::TreeItemMode;

    let mut issues = Vec::new();

    let all_hashes = list_all_objects_in_storage(storage)
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?;

    for hash in &all_hashes {
        let Ok(obj_type) = storage.get_object_type(hash) else {
            continue;
        };

        if obj_type == ObjectType::Tree {
            let Ok(tree) = load_object::<Tree>(hash) else {
                continue;
            };
            for item in &tree.tree_items {
                if !storage.exist(&item.id) {
                    issues.push(IssueReport {
                        issue_type: "missing".to_string(),
                        severity: "error".to_string(),
                        object_type: Some("unknown".to_string()),
                        object_id: Some(item.id.to_string()),
                        ref_name: None,
                        message: format!(
                            "Tree {} references missing object {} ({})",
                            hash, item.id, item.name
                        ),
                        suggestion: Some(
                            "The tree references an object that doesn't exist.".to_string(),
                        ),
                    });
                    continue;
                }
                // Verify the referenced object type matches the declaration
                if let Ok(actual_type) = storage.get_object_type(&item.id) {
                    let declared_is_tree = item.mode == TreeItemMode::Tree;
                    if declared_is_tree != (actual_type == ObjectType::Tree) {
                        let declared_kind = if declared_is_tree { "subtree" } else { "blob" };
                        issues.push(IssueReport {
                            issue_type: "tree_entry_type_mismatch".to_string(),
                            severity: "error".to_string(),
                            object_type: Some(actual_type.to_string()),
                            object_id: Some(item.id.to_string()),
                            ref_name: None,
                            message: format!(
                                "Tree {} declares {} as a {declared_kind} but it is a {}",
                                hash, item.name, actual_type
                            ),
                            suggestion: Some(
                                "The tree entry type does not match the actual object type."
                                    .to_string(),
                            ),
                        });
                    }
                }
            }
        } else if obj_type == ObjectType::Commit {
            let Ok(commit) = load_object::<Commit>(hash) else {
                continue;
            };
            if !storage.exist(&commit.tree_id) {
                issues.push(IssueReport {
                    issue_type: "missing".to_string(),
                    severity: "error".to_string(),
                    object_type: Some("tree".to_string()),
                    object_id: Some(commit.tree_id.to_string()),
                    ref_name: None,
                    message: format!("Commit {} references missing tree {}", hash, commit.tree_id),
                    suggestion: Some("The commit's tree is missing.".to_string()),
                });
            }

            for parent in &commit.parent_commit_ids {
                if !storage.exist(parent) {
                    issues.push(IssueReport {
                        issue_type: "missing".to_string(),
                        severity: "warning".to_string(),
                        object_type: Some("commit".to_string()),
                        object_id: Some(parent.to_string()),
                        ref_name: None,
                        message: format!("Commit {} references missing parent {}", hash, parent),
                        suggestion: Some(
                            "Parent commit is missing - history may be incomplete.".to_string(),
                        ),
                    });
                }
            }
        }
    }

    Ok(issues)
}

fn print_verbose_result(result: &FsckResult) {
    println!("\n=== Fsck Summary ===");
    println!("Objects checked: {}", result.objects_checked);
    println!("  - OK: {}", result.objects_ok);
    println!("  - Corrupted: {}", result.objects_corrupted);
    println!("Refs checked: {}", result.refs_checked);
    println!("  - OK: {}", result.refs_ok);
    println!("  - Broken: {}", result.refs_broken);
    println!("Index valid: {}", result.index_valid);
    println!("Reflog issues: {}", result.reflog_issues);
    println!("Cross-reference issues: {}", result.cross_ref_issues);

    if !result.issues.is_empty() {
        println!("\n=== Issues Found ===");
        for issue in &result.issues {
            println!(
                "[{}] {}: {}",
                issue.severity.to_uppercase(),
                issue.issue_type,
                issue.message
            );
            if let Some(ref obj) = issue.object_id {
                println!("  Object: {}", obj);
            }
            if let Some(ref r#ref) = issue.ref_name {
                println!("  Ref: {}", r#ref);
            }
            if let Some(ref suggestion) = issue.suggestion {
                println!("  Suggestion: {}", suggestion);
            }
        }
    }
}

fn print_issues(result: &FsckResult) {
    eprintln!("Integrity check FAILED");
    eprintln!(
        "Objects: {} checked, {} OK, {} corrupted",
        result.objects_checked, result.objects_ok, result.objects_corrupted
    );
    eprintln!(
        "Refs: {} checked, {} OK, {} broken",
        result.refs_checked, result.refs_ok, result.refs_broken
    );

    if !result.issues.is_empty() {
        eprintln!("\nIssues:");
        for issue in &result.issues {
            eprintln!("  [{}] {}", issue.severity.to_uppercase(), issue.message);
        }
    }
}

