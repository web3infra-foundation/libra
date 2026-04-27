//! Implementation of `fsck` command for verifying repository integrity.
//!
//! This command checks the integrity of objects, refs, and index in a Libra repository.
//! It verifies:
//! - Object hash integrity (SHA1/SHA256)
//! - Object format validity
//! - Ref consistency (refs point to valid objects)
//! - Index file integrity
//! - Cross-reference validation (trees reference valid blobs/trees)
//!
//! ## Exit codes (bitmask)
//! - 0: All checks passed
//! - 1 (bit 0): Object corruption
//! - 2 (bit 1): Broken refs
//! - 4 (bit 2): Index corruption
//!   Bits are OR'd when multiple categories fail (e.g. 5 = objects + index)

use std::{fs, io};

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
    internal::{db, head::Head, model::reference},
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

This command checks:
  - Object hash integrity: verifies each object's hash matches its content
  - Object format validity: ensures objects can be parsed correctly
  - Ref consistency: all refs point to existing, valid objects
  - Index integrity: the staging index is valid and consistent
  - Cross-reference validation: trees reference valid child objects

Exit codes (bitmask, OR'd when multiple fail):
  0 - All checks passed
  1 (bit 0) - Object corruption
  2 (bit 1) - Broken refs
  4 (bit 2) - Index corruption";

const FSCK_AFTER_HELP: &str = "Examples:
  libra fsck
  libra fsck --verbose
  libra fsck --json
  libra fsck --no-cross-ref-check";

/// Verify repository integrity by checking objects, refs, and index
#[derive(Parser, Debug)]
#[command(
    about = "Verify the integrity of objects, refs, and index",
    long_about = FSCK_LONG_ABOUT,
    after_help = FSCK_AFTER_HELP,
)]
pub struct FsckArgs {
    /// Verbose output - print each object as it's verified
    #[arg(short, long)]
    pub verbose: bool,

    /// Skip cross-reference validation (faster but less thorough)
    #[arg(long)]
    pub no_cross_ref_check: bool,

    /// Skip index validation
    #[arg(long)]
    pub no_index_check: bool,

    /// Only check objects, skip refs and index
    #[arg(long)]
    pub objects_only: bool,

    /// Fix detected issues automatically (where possible)
    #[arg(long)]
    pub fix: bool,

    /// Object ID to check (optional - checks all objects if not provided)
    #[arg(value_name = "OBJECT")]
    pub object: Option<String>,
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

/// Detailed issue report
#[derive(Debug, Clone, Serialize)]
pub struct IssueReport {
    pub issue_type: String,
    pub severity: String,
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
            if fsck_result.failure_mask == exit_code::OK {
                if !args.verbose {
                    println!(
                        "Integrity check passed: {} objects verified",
                        fsck_result.objects_checked
                    );
                } else {
                    print_verbose_result(&fsck_result);
                }
            } else {
                print_issues(&fsck_result);
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
            "object_corruption".to_string(),
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
            "missing_object".to_string(),
            "Object may have been deleted or never created.".to_string(),
        ),
        (CheckStatus::Ok, _) => unreachable!("should not build issue for Ok status"),
    };

    IssueReport {
        issue_type,
        severity: "error".to_string(),
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
        cross_ref_issues: 0,
        overall_status: CheckStatus::Ok,
        issues: Vec::new(),
        failure_mask: exit_code::OK,
        failure_categories: Vec::new(),
    };

    // Get all object hashes
    let all_hashes = list_all_objects_in_storage(storage)
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?;

    let total = all_hashes.len();
    if total == 0 {
        println!("No objects to check");
        return Ok(result);
    }

    if args.verbose {
        println!("Checking {} objects...", total);
    }

    check_objects(&all_hashes, args, storage, &mut result).await?;

    if !args.objects_only {
        check_and_fix_refs(args, storage, &mut result).await?;
    }

    if !args.no_index_check && !args.objects_only {
        check_and_fix_index(args, storage, &mut result).await?;
    }

    if !args.no_cross_ref_check && !args.objects_only {
        check_cross_references(storage, &mut result).await?;
    }

    compute_failure_mask(&mut result);

    Ok(result)
}

/// Verify all objects in storage, updating `result` with per-object outcomes.
async fn check_objects(
    all_hashes: &[ObjectHash],
    args: &FsckArgs,
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    for (i, hash) in all_hashes.iter().enumerate() {
        if args.verbose {
            println!("Checking object {}/{}: {}", i + 1, all_hashes.len(), hash);
        }

        let check_result = verify_object(hash, storage).await?;
        result.objects_checked += 1;

        match check_result.status {
            CheckStatus::Ok => {
                result.objects_ok += 1;
            }
            _ => {
                result.objects_corrupted += 1;
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = check_result.status.clone();
                }
                result.issues.push(build_issue_report(
                    &check_result,
                    &hash.to_string(),
                    IssueContext::FullScan,
                ));
            }
        }
    }
    Ok(())
}

/// Check refs and optionally fix broken ones.
async fn check_and_fix_refs(
    args: &FsckArgs,
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
        if args.fix {
            apply_fix_broken_refs(&ref_result.broken_ref_names, result).await?;
        }
    }
    Ok(())
}

/// Delete broken refs and update result counters.
async fn apply_fix_broken_refs(
    broken_ref_names: &[String],
    result: &mut FsckResult,
) -> CliResult<()> {
    let fixed = fix_broken_refs(broken_ref_names).await?;
    if fixed > 0 {
        println!("Fixed: deleted {} broken ref(s)", fixed);
        result.refs_broken = 0;
        result.issues.retain(|i| {
            i.ref_name
                .as_deref()
                .is_none_or(|n| !broken_ref_names.iter().any(|bn| bn == n))
        });
    }
    Ok(())
}

/// Check index and optionally fix corruption.
async fn check_and_fix_index(
    args: &FsckArgs,
    storage: &ClientStorage,
    result: &mut FsckResult,
) -> CliResult<()> {
    let index_result = check_index(storage)?;
    result.index_valid = index_result.valid;
    result.issues.extend(index_result.issues);

    if !index_result.valid {
        if result.overall_status == CheckStatus::Ok {
            result.overall_status = CheckStatus::InvalidFormat;
        }
        if args.fix {
            apply_fix_corrupted_index(result).await?;
        }
    }
    Ok(())
}

/// Rebuild corrupted index and update result.
async fn apply_fix_corrupted_index(result: &mut FsckResult) -> CliResult<()> {
    let fixed = fix_corrupted_index().await?;
    if fixed {
        println!("Fixed: rebuilt corrupted index");
        result.index_valid = true;
        result
            .issues
            .retain(|i| i.severity != "error" || !i.issue_type.contains("index"));
    }
    Ok(())
}

/// Validate cross-references and update result.
async fn check_cross_references(storage: &ClientStorage, result: &mut FsckResult) -> CliResult<()> {
    let cross_ref_issues = validate_cross_references(storage).await?;
    let issue_count = cross_ref_issues.len();
    result.cross_ref_issues = issue_count;
    result.issues.extend(cross_ref_issues);

    if issue_count > 0 && result.overall_status == CheckStatus::Ok {
        result.overall_status = CheckStatus::InvalidFormat;
    }

    Ok(())
}

/// Compute failure bitmask and human-readable categories from current result state.
fn compute_failure_mask(result: &mut FsckResult) {
    let mut mask = exit_code::OK;
    let mut categories = Vec::new();
    if result.objects_corrupted > 0 || result.cross_ref_issues > 0 {
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
    result.failure_mask = mask;
    result.failure_categories = categories;
    // After fixing, clear overall_status if all categories are clean
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
                                issue_type: "invalid_ref_target".to_string(),
                                severity: "error".to_string(),
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
                                issue_type: "ref_check_error".to_string(),
                                severity: "error".to_string(),
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
                        issue_type: "broken_ref".to_string(),
                        severity: "error".to_string(),
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
fn check_index(storage: &ClientStorage) -> CliResult<IndexCheckResult> {
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
            issue_type: "index_entry_missing_object".to_string(),
            severity: "error".to_string(),
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
                        issue_type: "missing_tree_entry".to_string(),
                        severity: "error".to_string(),
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
                    issue_type: "missing_commit_tree".to_string(),
                    severity: "error".to_string(),
                    object_id: Some(commit.tree_id.to_string()),
                    ref_name: None,
                    message: format!("Commit {} references missing tree {}", hash, commit.tree_id),
                    suggestion: Some("The commit's tree is missing.".to_string()),
                });
            }

            for parent in &commit.parent_commit_ids {
                if !storage.exist(parent) {
                    issues.push(IssueReport {
                        issue_type: "missing_parent_commit".to_string(),
                        severity: "warning".to_string(),
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

#[cfg(test)]
mod tests {
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test;

    #[tokio::test]
    #[serial]
    async fn test_fsck_empty_repo() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: false,
            objects_only: false,
            fix: false,
            object: None,
        };

        let storage = ClientStorage::init(path::objects());
        let result = check_all_objects(&args, &storage).await.unwrap();

        assert_eq!(result.objects_checked, 0);
        assert_eq!(result.overall_status, CheckStatus::Ok);
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_valid_blob() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let blob = Blob::from_content("test content");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let result = verify_object(&blob.id, &storage).await.unwrap();

        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.object_type, "blob");
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_missing_object() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let fake_hash = ObjectHash::new(&[0u8; 20]);

        let result = verify_object(&fake_hash, &storage).await.unwrap();

        assert_eq!(result.status, CheckStatus::Missing);
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_valid_commit() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a tree first
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "test.txt".to_string(),
            id: ObjectHash::new(&[1u8; 20]),
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        // Create a commit referencing the tree
        let commit = Commit::from_tree_id(tree.id, vec![], "Test commit");
        crate::command::save_object(&commit, &commit.id).unwrap();

        let result = verify_object(&commit.id, &storage).await.unwrap();

        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.object_type, "commit");
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_valid_tree() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a blob first, then a tree referencing it
        let blob = Blob::from_content("test");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "test.txt".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let result = verify_object(&tree.id, &storage).await.unwrap();

        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.object_type, "tree");
    }

    #[test]
    fn test_check_status_serialize() {
        let status = CheckStatus::Ok;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"ok\"");

        let status = CheckStatus::Missing;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"missing\"");
    }

    #[test]
    fn test_issue_report_serialize() {
        let issue = IssueReport {
            issue_type: "test".to_string(),
            severity: "error".to_string(),
            object_id: Some("abc123".to_string()),
            ref_name: None,
            message: "Test message".to_string(),
            suggestion: Some("Fix it".to_string()),
        };

        let json = serde_json::to_string(&issue).unwrap();
        assert!(json.contains("\"issue_type\":\"test\""));
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"object_id\":\"abc123\""));
    }

    #[test]
    fn test_fsck_args_parsing() {
        let args = FsckArgs::try_parse_from(["fsck"]).unwrap();
        assert!(!args.verbose);
        assert!(!args.no_cross_ref_check);
        assert!(!args.objects_only);
        assert!(!args.fix);
        assert!(args.object.is_none());
    }

    #[test]
    fn test_fsck_args_verbose() {
        let args = FsckArgs::try_parse_from(["fsck", "--verbose"]).unwrap();
        assert!(args.verbose);
    }

    #[test]
    fn test_fsck_args_with_object() {
        let args =
            FsckArgs::try_parse_from(["fsck", "abc123def456789012345678901234567890abcd"]).unwrap();
        assert_eq!(
            args.object,
            Some("abc123def456789012345678901234567890abcd".to_string())
        );
    }

    #[test]
    fn test_fsck_args_all_flags() {
        let args = FsckArgs::try_parse_from([
            "fsck",
            "-v",
            "--no-cross-ref-check",
            "--no-index-check",
            "--objects-only",
            "--fix",
        ])
        .unwrap();
        assert!(args.verbose);
        assert!(args.no_cross_ref_check);
        assert!(args.no_index_check);
        assert!(args.objects_only);
        assert!(args.fix);
    }

    #[tokio::test]
    #[serial]
    async fn test_fsck_single_object() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let blob = Blob::from_content("test");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let result = check_single_object(&blob.id.to_string(), &storage)
            .await
            .unwrap();

        assert_eq!(result.objects_checked, 1);
        assert_eq!(result.objects_ok, 1);
        assert_eq!(result.overall_status, CheckStatus::Ok);
    }

    #[tokio::test]
    #[serial]
    async fn test_fsck_with_commit_and_tree() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a blob
        let blob = Blob::from_content("file content");
        crate::command::save_object(&blob, &blob.id).unwrap();

        // Create a tree referencing the blob
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "test.txt".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        // Create a commit referencing the tree
        let commit = Commit::from_tree_id(tree.id, vec![], "Initial commit");
        crate::command::save_object(&commit, &commit.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: false,
            objects_only: true,
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert_eq!(result.objects_checked, 3);
        assert_eq!(result.objects_ok, 3);
        assert_eq!(result.overall_status, CheckStatus::Ok);
        assert_eq!(result.cross_ref_issues, 0);
    }

    #[tokio::test]
    #[serial]
    async fn test_cross_ref_detects_missing_tree_entry() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a tree referencing a non-existent blob
        let fake_blob_id = ObjectHash::new(&[0xff; 20]);
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "missing.txt".to_string(),
            id: fake_blob_id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false, // Must be false to enable cross-ref validation
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "missing_tree_entry")
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_cross_ref_detects_missing_commit_tree() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a commit referencing a non-existent tree
        let fake_tree_id = ObjectHash::new(&[0xfe; 20]);
        let commit = Commit::from_tree_id(fake_tree_id, vec![], "Bad commit");
        crate::command::save_object(&commit, &commit.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false, // Must be false to enable cross-ref validation
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "missing_commit_tree")
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_cross_ref_detects_missing_parent() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a commit referencing a non-existent parent
        let fake_parent_id = ObjectHash::new(&[0xfd; 20]);

        // Create a minimal tree (empty trees are not allowed)
        let blob = Blob::from_content("dummy");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: ".gitkeep".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let commit =
            Commit::from_tree_id(tree.id, vec![fake_parent_id], "Commit with missing parent");
        crate::command::save_object(&commit, &commit.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false, // Must be false to enable cross-ref validation
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "missing_parent_commit" && i.severity == "warning")
        );
    }

    #[test]
    fn test_object_check_result_structure() {
        let result = ObjectCheckResult {
            object_id: "test".to_string(),
            object_type: "blob".to_string(),
            status: CheckStatus::Ok,
            error_message: None,
            size: 100,
        };

        assert_eq!(result.object_id, "test");
        assert_eq!(result.size, 100);
        assert!(result.error_message.is_none());
    }

    #[test]
    fn test_fsck_result_structure() {
        let result = FsckResult {
            objects_checked: 10,
            objects_ok: 9,
            objects_corrupted: 1,
            refs_checked: 5,
            refs_ok: 5,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: exit_code::OK,
            failure_categories: vec![],
        };

        assert_eq!(result.objects_checked, 10);
        assert!(result.index_valid);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_ref_check_result_structure() {
        let result = RefCheckResult {
            checked: 3,
            ok: 2,
            broken: 1,
            issues: vec![],
            broken_ref_names: vec![],
        };

        assert_eq!(result.checked, 3);
        assert_eq!(result.broken, 1);
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_blob_hash_mismatch_detection() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let blob = Blob::from_content("test content for hash verification");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let result = verify_object(&blob.id, &storage).await.unwrap();

        assert_eq!(result.status, CheckStatus::Ok);
    }

    #[tokio::test]
    #[serial]
    async fn test_fsck_json_output_structure() {
        let result = FsckResult {
            objects_checked: 5,
            objects_ok: 5,
            objects_corrupted: 0,
            refs_checked: 2,
            refs_ok: 2,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: exit_code::OK,
            failure_categories: vec![],
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"objects_checked\": 5"));
        assert!(json.contains("\"overall_status\": \"ok\""));
        assert!(json.contains("\"index_valid\": true"));
    }

    #[test]
    fn test_check_status_all_variants() {
        let statuses = [
            CheckStatus::Ok,
            CheckStatus::Missing,
            CheckStatus::InvalidFormat,
            CheckStatus::HashMismatch,
        ];

        for status in &statuses {
            let serialized = serde_json::to_string(status).unwrap();
            assert!(!serialized.is_empty());
        }
    }

    #[test]
    fn test_parse_object_hash_valid() {
        let hash = parse_object_hash("a1b2c3d4e5f6789012345678901234567890abcd");
        assert!(hash.is_some());
    }

    #[test]
    fn test_parse_object_hash_invalid() {
        let hash = parse_object_hash("invalid_hex_string");
        assert!(hash.is_none());
    }

    #[test]
    fn test_parse_object_hash_empty() {
        let hash = parse_object_hash("");
        assert!(hash.is_none());
    }

    #[test]
    fn test_issue_report_all_fields() {
        let issue = IssueReport {
            issue_type: "hash_mismatch".to_string(),
            severity: "critical".to_string(),
            object_id: Some("abc123".to_string()),
            ref_name: Some("refs/heads/main".to_string()),
            message: "Hash does not match content".to_string(),
            suggestion: Some("Recreate object from source".to_string()),
        };

        let json = serde_json::to_string(&issue).unwrap();
        assert!(json.contains("hash_mismatch"));
        assert!(json.contains("critical"));
        assert!(json.contains("abc123"));
        assert!(json.contains("refs/heads/main"));
    }

    #[test]
    fn test_issue_report_optional_fields_none() {
        let issue = IssueReport {
            issue_type: "empty_repo".to_string(),
            severity: "info".to_string(),
            object_id: None,
            ref_name: None,
            message: "No objects to check".to_string(),
            suggestion: None,
        };

        let json = serde_json::to_string(&issue).unwrap();
        assert!(json.contains("empty_repo"));
    }

    #[test]
    fn test_object_check_result_with_error() {
        let result = ObjectCheckResult {
            object_id: "bad123".to_string(),
            object_type: "commit".to_string(),
            status: CheckStatus::HashMismatch,
            error_message: Some("Computed hash differs from stored hash".to_string()),
            size: 256,
        };

        assert_eq!(result.status, CheckStatus::HashMismatch);
        assert!(result.error_message.is_some());
        assert_eq!(result.size, 256);
    }

    #[test]
    fn test_fsck_args_object_with_short_hash() {
        let args = FsckArgs::try_parse_from(["fsck", "abc123"]).unwrap();
        assert_eq!(args.object, Some("abc123".to_string()));
    }

    #[test]
    fn test_check_status_display() {
        assert_eq!(serde_json::to_string(&CheckStatus::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&CheckStatus::Missing).unwrap(),
            "\"missing\""
        );
        // Note: serde renames to kebab-case, but InvalidFormat is rendered as "invalidformat"
        // due to how the rename works - this is expected behavior
        assert_eq!(
            serde_json::to_string(&CheckStatus::InvalidFormat).unwrap(),
            "\"invalidformat\""
        );
        assert_eq!(
            serde_json::to_string(&CheckStatus::HashMismatch).unwrap(),
            "\"hashmismatch\""
        );
    }

    #[test]
    fn test_fsck_result_default_values() {
        let result = FsckResult {
            objects_checked: 0,
            objects_ok: 0,
            objects_corrupted: 0,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: exit_code::OK,
            failure_categories: vec![],
        };

        assert_eq!(result.objects_checked, 0);
        assert!(result.issues.is_empty());
        assert_eq!(result.overall_status, CheckStatus::Ok);
    }

    #[test]
    fn test_ref_check_result_zero() {
        let result = RefCheckResult {
            checked: 0,
            ok: 0,
            broken: 0,
            issues: vec![],
            broken_ref_names: vec![],
        };

        assert_eq!(result.checked, 0);
        assert_eq!(result.broken, 0);
        assert!(result.issues.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_commit_empty_parents() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a minimal tree with at least one entry (empty trees are not allowed)
        let blob = Blob::from_content("dummy");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: ".gitkeep".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let commit = Commit::from_tree_id(tree.id, vec![], "Root commit");
        crate::command::save_object(&commit, &commit.id).unwrap();

        let result = verify_object(&commit.id, &storage).await.unwrap();
        assert_eq!(result.status, CheckStatus::Ok);
    }

    #[tokio::test]
    #[serial]
    async fn test_fsck_single_invalid_object() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        let fake_hash = ObjectHash::new(&[0xee; 20]);
        let result = check_single_object(&fake_hash.to_string(), &storage)
            .await
            .unwrap();

        assert_eq!(result.objects_checked, 1);
        assert_eq!(result.objects_ok, 0);
        assert_eq!(result.objects_corrupted, 1);
        assert!(result.overall_status != CheckStatus::Ok);
    }

    #[test]
    fn test_issue_severity_levels() {
        let severities = ["error", "warning", "info", "critical"];
        for sev in severities {
            let issue = IssueReport {
                issue_type: "test".to_string(),
                severity: sev.to_string(),
                object_id: None,
                ref_name: None,
                message: "Test".to_string(),
                suggestion: None,
            };
            let json = serde_json::to_string(&issue).unwrap();
            assert!(json.contains(sev));
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_tree_with_subtree() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("file");
        let subtree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "inner.txt".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&blob, &blob.id).unwrap();
        crate::command::save_object(&subtree, &subtree.id).unwrap();

        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Tree,
            name: "subdir".to_string(),
            id: subtree.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let result = verify_object(&tree.id, &storage).await.unwrap();
        assert_eq!(result.status, CheckStatus::Ok);
    }

    #[test]
    fn test_print_functions_exist() {
        let result = FsckResult {
            objects_checked: 0,
            objects_ok: 0,
            objects_corrupted: 0,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: exit_code::OK,
            failure_categories: vec![],
        };

        print_verbose_result(&result);
        print_issues(&result);
    }

    // -----------------------------------------------------------------------
    // Failure mask / exit code bitmask tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_exit_code_bitmask_objects_only() {
        assert_eq!(exit_code::OK, 0);
        assert_eq!(exit_code::OBJECT_CORRUPT, 1);
        assert_eq!(exit_code::REF_BROKEN, 2);
        assert_eq!(exit_code::INDEX_CORRUPT, 4);
    }

    #[test]
    fn test_exit_code_bitmask_combinations() {
        assert_eq!(exit_code::OBJECT_CORRUPT | exit_code::REF_BROKEN, 3);
        assert_eq!(exit_code::OBJECT_CORRUPT | exit_code::INDEX_CORRUPT, 5);
        assert_eq!(exit_code::REF_BROKEN | exit_code::INDEX_CORRUPT, 6);
        assert_eq!(
            exit_code::OBJECT_CORRUPT | exit_code::REF_BROKEN | exit_code::INDEX_CORRUPT,
            7
        );
    }

    #[test]
    fn test_failure_mask_serialization_skipped_when_zero() {
        let result = FsckResult {
            objects_checked: 1,
            objects_ok: 1,
            objects_corrupted: 0,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: 0,
            failure_categories: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            !json.contains("failure_mask"),
            "failure_mask should be skipped when zero: {json}"
        );
    }

    #[test]
    fn test_failure_categories_serialization_skipped_when_empty() {
        let result = FsckResult {
            objects_checked: 1,
            objects_ok: 1,
            objects_corrupted: 0,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::Ok,
            issues: vec![],
            failure_mask: 0,
            failure_categories: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            !json.contains("failure_categories"),
            "failure_categories should be skipped when empty: {json}"
        );
    }

    #[test]
    fn test_failure_mask_serialized_when_nonzero() {
        let result = FsckResult {
            objects_checked: 2,
            objects_ok: 1,
            objects_corrupted: 1,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::HashMismatch,
            issues: vec![],
            failure_mask: exit_code::OBJECT_CORRUPT,
            failure_categories: vec!["objects".to_string()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            json.contains("\"failure_mask\":1"),
            "failure_mask should appear when nonzero: {json}"
        );
        assert!(
            json.contains("\"failure_categories\":[\"objects\"]"),
            "failure_categories should appear: {json}"
        );
    }

    // -----------------------------------------------------------------------
    // Issue type / severity JSON serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_issue_report_issue_types() {
        let issue_types = [
            "hash_mismatch",
            "invalid_format",
            "missing_object",
            "missing_tree_entry",
            "missing_commit_tree",
            "missing_parent_commit",
            "broken_ref",
            "invalid_ref_hash",
            "index_parse_error",
            "invalid_index_mode",
            "invalid_index_stage",
            "index_entry_missing_object",
            "index_entry_wrong_type",
            "index_conflict_marker",
            "tree_entry_type_mismatch",
        ];
        for itype in issue_types {
            let issue = IssueReport {
                issue_type: itype.to_string(),
                severity: "error".to_string(),
                object_id: Some("abc123".to_string()),
                ref_name: None,
                message: format!("Test {itype}"),
                suggestion: None,
            };
            let json = serde_json::to_string(&issue).unwrap();
            assert!(
                json.contains(itype),
                "JSON should contain issue type '{itype}': {json}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // is_valid_index_mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_valid_index_mode_valid_modes() {
        assert!(is_valid_index_mode(0o100644)); // regular file
        assert!(is_valid_index_mode(0o100755)); // executable
        assert!(is_valid_index_mode(0o120000)); // symlink
        assert!(is_valid_index_mode(0o160000)); // gitlink
        assert!(is_valid_index_mode(0o040000)); // directory/tree
    }

    #[test]
    fn test_is_valid_index_mode_invalid_modes() {
        assert!(!is_valid_index_mode(0o000000));
        assert!(!is_valid_index_mode(0o010000));
        assert!(!is_valid_index_mode(0o077777));
        assert!(!is_valid_index_mode(0o100000)); // missing permission bits
        assert!(!is_valid_index_mode(0o777777));
    }

    // -----------------------------------------------------------------------
    // parse_object_hash edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_object_hash_odd_length() {
        // Odd-length hex strings should still be decodable by hex::decode? Actually no.
        let hash = parse_object_hash("abc");
        assert!(hash.is_none(), "odd-length hex should fail: {hash:?}");
    }

    #[test]
    fn test_parse_object_hash_non_hex() {
        let hash = parse_object_hash("zzzzzzzz");
        assert!(hash.is_none(), "non-hex should fail: {hash:?}");
    }

    #[test]
    fn test_parse_object_hash_with_uppercase() {
        // hex::decode accepts both uppercase and lowercase
        // Use a proper 40-char (even length) uppercase hex string
        let hash = parse_object_hash("ABCDEF0123456789ABCDEF0123456789ABCDEF01");
        assert!(hash.is_some(), "uppercase hex should be accepted: {hash:?}");
    }

    #[test]
    fn test_parse_object_hash_zero_hash() {
        let zero_40 = "0000000000000000000000000000000000000000";
        let hash = parse_object_hash(zero_40);
        assert!(
            hash.is_some(),
            "all-zero hash should parse (valid hex): {hash:?}"
        );
    }

    // -----------------------------------------------------------------------
    // list_all_objects_in_storage edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_list_all_objects_nonexistent_dir() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        // Remove the objects directory entirely
        let objects_dir = path::objects();
        if objects_dir.exists() {
            fs::remove_dir_all(&objects_dir).unwrap();
        }

        let storage = ClientStorage::init(path::objects());
        let hashes = list_all_objects_in_storage(&storage).unwrap();
        assert!(
            hashes.is_empty(),
            "should return empty list for nonexistent dir"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_list_all_objects_empty_dir() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let hashes = list_all_objects_in_storage(&storage).unwrap();
        assert!(
            hashes.is_empty(),
            "empty objects dir should return no hashes"
        );
    }

    // -----------------------------------------------------------------------
    // verify_object: corrupted object detection
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_verify_corrupted_blob_detected() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());
        let blob = Blob::from_content("original content");
        crate::command::save_object(&blob, &blob.id).unwrap();

        // Corrupt the object by overwriting with garbage
        let objects_dir = path::objects();
        let hash_str = blob.id.to_string();
        let object_path = objects_dir.join(&hash_str[..2]).join(&hash_str[2..]);
        fs::write(&object_path, b"garbage data!!!").unwrap();

        let result = verify_object(&blob.id, &storage).await.unwrap();
        assert!(
            result.status == CheckStatus::HashMismatch
                || result.status == CheckStatus::InvalidFormat,
            "corrupted blob should be detected: {:?}",
            result.status
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_verify_corrupted_tree_detected() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        let blob = Blob::from_content("file");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "file.txt".to_string(),
            id: blob.id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        // Corrupt the tree object
        let objects_dir = path::objects();
        let hash_str = tree.id.to_string();
        let object_path = objects_dir.join(&hash_str[..2]).join(&hash_str[2..]);
        fs::write(&object_path, b"corrupted tree!!!").unwrap();

        let result = verify_object(&tree.id, &storage).await.unwrap();
        assert!(
            result.status != CheckStatus::Ok,
            "corrupted tree should be detected: {:?}",
            result.status
        );
    }

    // -----------------------------------------------------------------------
    // validate_cross_references: tree entry type mismatch
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_cross_ref_detects_tree_entry_type_mismatch() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a blob
        let blob = Blob::from_content("content");
        crate::command::save_object(&blob, &blob.id).unwrap();

        // Create a tree that incorrectly declares the blob as a subtree
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Tree, // declares as tree
            name: "should_be_blob".to_string(),
            id: blob.id, // but is actually a blob
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false,
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "tree_entry_type_mismatch"),
            "should detect tree entry type mismatch: {:?}",
            result.issues
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_cross_ref_detects_blob_declared_as_tree_when_actual_is_tree() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a real subtree
        let inner_blob = Blob::from_content("inner");
        crate::command::save_object(&inner_blob, &inner_blob.id).unwrap();

        let subtree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "inner.txt".to_string(),
            id: inner_blob.id,
        }])
        .unwrap();
        crate::command::save_object(&subtree, &subtree.id).unwrap();

        // Create a parent tree that declares the subtree as a blob
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob, // declares as blob
            name: "should_be_tree".to_string(),
            id: subtree.id, // but is actually a tree
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false,
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "tree_entry_type_mismatch"),
            "should detect blob-declared-as-tree mismatch: {:?}",
            result.issues
        );
    }

    // -----------------------------------------------------------------------
    // check_index: corrupted index parse error
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_check_index_corrupted_file_parse_error() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Write garbage to the index file
        let index_path = path::index();
        fs::write(&index_path, b"not a valid index!!!").unwrap();

        let result = check_index(&storage).unwrap();

        assert!(
            !result.valid,
            "corrupted index should be detected as invalid"
        );
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.issue_type == "index_parse_error"),
            "should report index_parse_error: {:?}",
            result.issues
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_check_index_no_index_file() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Ensure no index file exists
        let index_path = path::index();
        if index_path.exists() {
            fs::remove_file(&index_path).unwrap();
        }

        let result = check_index(&storage).unwrap();

        assert!(result.valid, "no index file should be treated as valid");
        assert_eq!(result.entries_checked, 0);
    }

    // -----------------------------------------------------------------------
    // check_all_objects: --fix flow for broken refs
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_check_all_objects_fix_broken_refs() {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};

        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create at least one object so check_all_objects proceeds past the early return
        let blob = Blob::from_content("dummy");
        crate::command::save_object(&blob, &blob.id).unwrap();

        // Insert a broken ref pointing to a nonexistent object
        let fake_hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let db_conn = db::get_db_conn_instance().await;
        let broken_ref = reference::ActiveModel {
            name: Set(Some("refs/heads/broken-branch".to_string())),
            kind: Set(crate::internal::model::reference::ConfigKind::Branch),
            commit: Set(Some(fake_hash.to_string())),
            remote: Set(None),
            ..Default::default()
        };
        broken_ref.insert(&db_conn).await.unwrap();

        // Run check with fix=false first — should detect the broken ref
        let args_no_fix = FsckArgs {
            verbose: false,
            no_cross_ref_check: true,
            no_index_check: true,
            objects_only: false,
            fix: false,
            object: None,
        };
        let result_no_fix = check_all_objects(&args_no_fix, &storage).await.unwrap();
        assert!(
            result_no_fix.refs_broken > 0,
            "broken ref should be detected without fix: refs_broken={}",
            result_no_fix.refs_broken
        );

        // Run check with fix=true — should delete the broken ref
        let args_fix = FsckArgs {
            verbose: false,
            no_cross_ref_check: true,
            no_index_check: true,
            objects_only: false,
            fix: true,
            object: None,
        };
        let result_fix = check_all_objects(&args_fix, &storage).await.unwrap();
        assert!(
            result_fix.refs_broken == 0,
            "broken ref should be fixed (deleted): refs_broken={}",
            result_fix.refs_broken
        );
    }

    // -----------------------------------------------------------------------
    // check_all_objects: failure_mask computed correctly with multiple failures
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_failure_mask_objects_and_cross_ref() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a tree referencing a nonexistent blob (triggers cross_ref issue)
        let fake_blob_id = ObjectHash::new(&[0xaa; 20]);
        let tree = Tree::from_tree_items(vec![git_internal::internal::object::tree::TreeItem {
            mode: git_internal::internal::object::tree::TreeItemMode::Blob,
            name: "missing.txt".to_string(),
            id: fake_blob_id,
        }])
        .unwrap();
        crate::command::save_object(&tree, &tree.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: false,
            no_index_check: true,
            objects_only: false,
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert!(result.cross_ref_issues > 0);
        assert!(
            result.failure_mask & exit_code::OBJECT_CORRUPT != 0,
            "failure_mask should include OBJECT_CORRUPT for cross-ref issues: {}",
            result.failure_mask
        );
    }

    // -----------------------------------------------------------------------
    // print_issues output format
    // -----------------------------------------------------------------------

    #[test]
    fn test_print_issues_outputs_to_stderr() {
        let result = FsckResult {
            objects_checked: 1,
            objects_ok: 0,
            objects_corrupted: 1,
            refs_checked: 0,
            refs_ok: 0,
            refs_broken: 0,
            index_valid: true,
            cross_ref_issues: 0,
            overall_status: CheckStatus::HashMismatch,
            issues: vec![IssueReport {
                issue_type: "hash_mismatch".to_string(),
                severity: "error".to_string(),
                object_id: Some("abc123".to_string()),
                ref_name: None,
                message: "Object data corrupted".to_string(),
                suggestion: Some("Restore from backup.".to_string()),
            }],
            failure_mask: exit_code::OBJECT_CORRUPT,
            failure_categories: vec!["objects".to_string()],
        };

        print_issues(&result);
    }

    // -----------------------------------------------------------------------
    // check_all_objects: --objects-only skips refs and index
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_objects_only_skips_refs_and_index() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Create a blob
        let blob = Blob::from_content("test");
        crate::command::save_object(&blob, &blob.id).unwrap();

        let args = FsckArgs {
            verbose: false,
            no_cross_ref_check: true,
            no_index_check: false, // would normally check index
            objects_only: true,    // should skip both refs and index
            fix: false,
            object: None,
        };

        let result = check_all_objects(&args, &storage).await.unwrap();

        assert_eq!(result.objects_checked, 1);
        assert_eq!(result.refs_checked, 0, "--objects-only should skip refs");
        assert!(
            result.index_valid,
            "--objects-only should leave index_valid as default true"
        );
    }

    // -----------------------------------------------------------------------
    // check_single_object: parse error on object ID
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[serial]
    async fn test_check_single_object_invalid_hash() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        // Invalid hex should error
        let result = check_single_object("not-hex!!", &storage).await;
        assert!(result.is_err(), "invalid hex object ID should return error");
    }

    #[tokio::test]
    #[serial]
    async fn test_check_single_object_empty_hash() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let storage = ClientStorage::init(path::objects());

        let result = check_single_object("", &storage).await;
        assert!(result.is_err(), "empty object ID should return error");
    }
}
