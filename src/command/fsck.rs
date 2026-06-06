//! Implementation of `fsck` command for verifying repository integrity.
//!
//! `fsck` 命令的实现，用于验证存储库完整性。
//!
//! This command checks the integrity of objects, refs, and index in a Libra repository.

use std::{
    collections::HashSet,
    fs, io,
    io::{Read, Seek},
};

use clap::Parser;
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        index::Index,
        object::{
            ObjectTrait,
            blob::Blob,
            commit::Commit,
            tag::Tag as GitTag,
            tree::{Tree, TreeItemMode},
            types::ObjectType,
        },
    },
};
use hex;
use ring::digest::{Context, SHA1_FOR_LEGACY_USE_ONLY, SHA256};
use sea_orm::EntityTrait;
use serde::Serialize;

use crate::{
    command::load_object,
    internal::{
        branch::Branch,
        db,
        head::Head,
        model::{reference, reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult},
        output::OutputConfig,
        path,
    },
};

/// Fsck message types - diagnostic messages go to stdout, errors to stderr
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsckMsgId {
    // ===== Diagnostic messages (stdout) =====
    Missing,
    HashMismatch,
    Dangling,
    Unreachable,
    // ===== Error messages (stderr) - Object integrity =====
    BadObjectSha1,
    BadTree,
    BadTreeSha1,
    UnknownType,
    // ===== Error messages - Commit validation =====
    MissingAuthor,
    MissingCommitter,
    MissingTree,
    BadDate,
    BadEmail,
    BadName,
    BadTimezone,
    MultipleAuthors,
    MissingEmail,
    // ===== Error messages - Tag validation =====
    MissingTagEntry,
    MissingType,
    MissingObject,
    MissingTaggerEntry,
    BadTagName,
    // ===== Error messages - Ref validation =====
    BadRefOid,
    BadRefContent,
    BadRefName,
    BadHeadTarget,
    // ===== Error messages - Index validation =====
    DuplicateEntries,
    NullSha1,
    TreeNotSorted,
    // ===== Error messages - Pathname checks =====
    HasDot,
    HasDotdot,
    HasDotlibra, // Libra-specific: renamed from hasDotgit
    EmptyName,
    FullPathname,
    // ===== Error messages - Libra specific =====
    IndexCorruption,
    InvalidIndexMode,
    InvalidIndexStage,
    IndexEntryWrongType,
}

impl FsckMsgId {
    /// Check if this message is an error (stderr) or diagnostic (stdout)
    /// All diagnostic messages (missing, hash_mismatch, dangling, unreachable) go to stdout
    pub fn is_error(&self) -> bool {
        !matches!(
            self,
            FsckMsgId::Missing
                | FsckMsgId::HashMismatch
                | FsckMsgId::Dangling
                | FsckMsgId::Unreachable
        )
    }

    /// Check if this message should cause non-zero exit code
    /// Only dangling and unreachable are informational; all others cause failure
    pub fn causes_failure(&self) -> bool {
        !matches!(self, FsckMsgId::Dangling | FsckMsgId::Unreachable)
    }

    /// Get the output format string for this message
    pub fn format(&self, obj_type: &str, obj_id: &str) -> String {
        match self {
            // Diagnostic messages - stdout
            FsckMsgId::Missing => format!("missing {} {}", obj_type, obj_id),
            FsckMsgId::HashMismatch => format!("hash mismatch {} {}", obj_type, obj_id),
            FsckMsgId::Dangling => format!("dangling {} {}", obj_type, obj_id),
            FsckMsgId::Unreachable => format!("unreachable {} {}", obj_type, obj_id),
            // Error messages - stderr
            FsckMsgId::BadObjectSha1 => format!("bad object sha1: {} {}", obj_type, obj_id),
            FsckMsgId::BadTree => format!("bad tree: {}", obj_id),
            FsckMsgId::BadTreeSha1 => format!("bad tree sha1: {}", obj_id),
            FsckMsgId::UnknownType => format!("unknown type: {} {}", obj_type, obj_id),
            FsckMsgId::MissingAuthor => format!("missing author: {}", obj_id),
            FsckMsgId::MissingCommitter => format!("missing committer: {}", obj_id),
            FsckMsgId::MissingTree => format!("missing tree: {}", obj_id),
            FsckMsgId::BadDate => format!("bad date: {}", obj_id),
            FsckMsgId::BadEmail => format!("bad email: {}", obj_id),
            FsckMsgId::MissingEmail => format!("missing email: {}", obj_id),
            FsckMsgId::BadName => format!("bad name: {}", obj_id),
            FsckMsgId::BadTimezone => format!("bad timezone: {}", obj_id),
            FsckMsgId::MultipleAuthors => format!("multiple authors: {}", obj_id),
            FsckMsgId::MissingTagEntry => format!("missing tag entry: {}", obj_id),
            FsckMsgId::MissingType => format!("missing type: {}", obj_id),
            FsckMsgId::MissingObject => format!("missing object: {}", obj_id),
            FsckMsgId::MissingTaggerEntry => format!("missing tagger: {}", obj_id),
            FsckMsgId::BadTagName => format!("bad tag name: {}", obj_id),
            FsckMsgId::BadRefOid => format!("bad ref oid: {}", obj_id),
            FsckMsgId::BadRefContent => format!("bad ref content: {}", obj_id),
            FsckMsgId::BadRefName => format!("bad ref name: {}", obj_id),
            FsckMsgId::BadHeadTarget => format!("bad head target: {}", obj_id),
            FsckMsgId::DuplicateEntries => format!("duplicate entries: {}", obj_id),
            FsckMsgId::NullSha1 => format!("null sha1: {}", obj_id),
            FsckMsgId::TreeNotSorted => format!("tree not sorted: {}", obj_id),
            FsckMsgId::HasDot => format!("has .: {}", obj_id),
            FsckMsgId::HasDotdot => format!("has ..: {}", obj_id),
            FsckMsgId::HasDotlibra => format!("has .libra: {}", obj_id),
            FsckMsgId::EmptyName => format!("empty name: {}", obj_id),
            FsckMsgId::FullPathname => format!("full pathname: {}", obj_id),
            FsckMsgId::IndexCorruption => format!("index corruption: {}", obj_id),
            FsckMsgId::InvalidIndexMode => format!("invalid index mode: {} {}", obj_type, obj_id),
            FsckMsgId::InvalidIndexStage => format!("invalid index stage: {} {}", obj_type, obj_id),
            FsckMsgId::IndexEntryWrongType => {
                format!("index entry wrong type: {} is {}", obj_id, obj_type)
            }
        }
    }
}

/// Report a fsck message
///
/// - Diagnostic messages (missing, hash_mismatch, dangling, unreachable) -> stdout
/// - Error messages -> stderr
///
/// Returns true if this is an error (for exit code tracking)
pub fn report(msg_id: FsckMsgId, obj_type: &str, obj_id: &str) -> bool {
    let output = msg_id.format(obj_type, obj_id);
    if msg_id.is_error() {
        eprintln!("{}", output);
    } else {
        println!("{}", output);
    }
    msg_id.causes_failure()
}

fn tag_parse_error_msg_id(error: &impl std::fmt::Display) -> FsckMsgId {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("missing object type")
        || message.contains("invalid object type")
        || message.contains("object type")
    {
        FsckMsgId::MissingType
    } else if message.contains("missing object hash")
        || message.contains("missing object")
        || message.contains("invalid object hash")
    {
        FsckMsgId::MissingObject
    } else if message.contains("missing tag name") {
        FsckMsgId::MissingTagEntry
    } else if message.contains("missing tagger") {
        FsckMsgId::MissingTaggerEntry
    } else if message.contains("tag name") {
        FsckMsgId::BadTagName
    } else {
        FsckMsgId::BadObjectSha1
    }
}

/// Convenience macro for reporting fsck messages
#[macro_export]
macro_rules! fsck_error {
    ($msg_id:expr, $obj_type:expr, $obj_id:expr) => {
        $crate::command::fsck::report($msg_id, $obj_type, $obj_id)
    };
}

const FSCK_LONG_ABOUT: &str =
    "Verify the integrity of objects, refs, and index in a Libra repository.

By default, checks all objects using refs, index, and reflogs as starting points.

Dangling objects are those that exist but are not referenced by any ref, index, or reflog.
By default, only dangling commits are reported (matching git fsck behavior).
Unreachable objects include all dangling objects plus those only reachable from other unreachable objects.";

const FSCK_AFTER_HELP: &str = "EXAMPLES:
    libra fsck                          Verify every object, ref, and reflog entry
    libra fsck --no-reflogs             Skip reflog validation (faster on large repos)
    libra fsck --unreachable            Report unreachable objects (not just dangling commits)
    libra fsck --no-dangling            Suppress the default dangling-commit report
    libra fsck --lost-found             Stage dangling objects under .libra/lost-found/
    libra fsck --root                   Print root commit ids in the report
    libra fsck --tags                   Print tag ids in the report
    libra fsck --connectivity-only      Skip blob content checks; verify graph only
    libra fsck --no-full                Verify only loose objects (skip packfiles)
    libra fsck --strict                 Apply stricter commit/tree format checks
    libra fsck <object-id>              Verify a single object by id";

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

    /// Print unreachable objects (not just dangling)
    #[arg(long)]
    pub unreachable: bool,

    /// Report dangling objects (default: dangling commits only)
    #[arg(long, default_value = "true", num_args = 0..=1, require_equals = false, value_name = "BOOL", overrides_with = "no_dangling")]
    pub dangling: Option<String>,

    /// Hide dangling objects in output
    #[arg(long, conflicts_with = "dangling")]
    pub no_dangling: bool,

    /// Show object names (e.g., refs/heads/master, HEAD@{1234567890}~2^1:src/) in verbose output
    #[arg(long)]
    pub name_objects: bool,

    /// Write dangling objects to .libra/lost-found/
    #[arg(long)]
    pub lost_found: bool,

    /// Report root commits (commits with no parents)
    #[arg(long)]
    pub root: bool,

    /// Report tagged commits
    #[arg(long)]
    pub tags: bool,

    /// Only check connectivity, not object contents
    #[arg(long)]
    pub connectivity_only: bool,

    /// Also verify objects stored in packfiles (default). Accepted explicitly;
    /// pass `--no-full` to restrict the check to loose objects.
    #[arg(long, conflicts_with = "no_full")]
    pub full: bool,

    /// Restrict the check to loose objects, skipping packed objects.
    #[arg(long)]
    pub no_full: bool,

    /// Enable stricter format checks: commit author/committer emails must
    /// contain `@` and carry a well-formed timezone within ±1400; a commit's
    /// tree/parents and a tree's entries must exist with matching object types;
    /// and tree entries must be in Git's canonical sort order.
    #[arg(long)]
    pub strict: bool,
}

impl FsckArgs {
    /// Returns whether dangling objects should be reported.
    /// Default is true (only dangling commits).
    /// Use --dangling or --dangling=true to enable, --no-dangling to disable.
    fn dangling_enabled(&self) -> bool {
        // --no-dangling alias takes precedence
        if self.no_dangling {
            return false;
        }
        match &self.dangling {
            None => true, // default
            Some(s) => s != "false" && s != "no" && s != "0",
        }
    }

    /// Returns whether packed objects should be enumerated and verified.
    /// `--full` is the default; `--no-full` restricts the check to loose objects.
    fn full_enabled(&self) -> bool {
        !self.no_full
    }
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
    pub has_errors: bool, // Track if any error was printed to stderr
}

/// Result of checking the index file
#[derive(Debug, Clone)]
pub struct IndexCheckResult {
    pub valid: bool,
    pub entries_checked: usize,
    pub entries_ok: usize,
    pub entries_corrupted: usize,
}

pub async fn execute(args: FsckArgs) {
    let exit_code = match run_fsck(&args).await {
        Ok(fsck_result) => {
            // Exit with failure code only for serious issues (not dangling/unreachable).
            if fsck_result.has_errors { 1 } else { 0 }
        }
        Err(error) => {
            error.print_stderr();
            error.exit_code()
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

async fn run_fsck(args: &FsckArgs) -> CliResult<FsckResult> {
    let storage = ClientStorage::init(path::objects());

    if let Some(ref object_id) = args.object {
        check_single_object(object_id, &storage, args.strict).await
    } else {
        check_all_objects(args, &storage).await
    }
}

pub async fn execute_safe(args: FsckArgs, _output: &OutputConfig) -> CliResult<()> {
    let fsck_result = run_fsck(&args).await?;
    if fsck_result.has_errors {
        return Err(CliError::failure("fsck found repository integrity issues").with_exit_code(1));
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

/// List the object hashes stored in a single pack index (`.idx`, v1 or v2),
/// reusing `verify_pack`'s validated index reader. A malformed/truncated index
/// surfaces as `io::ErrorKind::InvalidData`.
fn list_objects_in_pack_idx(idx_path: &std::path::Path) -> io::Result<Vec<ObjectHash>> {
    let bytes = fs::read(idx_path)?;
    super::verify_pack::parse_index_object_hashes(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// List all object hashes in storage. Loose objects are always enumerated; when
/// `full` is set, packed objects from every `objects/pack/*.idx` are merged in
/// (deduplicated against loose objects that may also be packed).
fn list_all_objects_in_storage(storage: &ClientStorage, full: bool) -> io::Result<Vec<ObjectHash>> {
    let objects_dir = storage.base_path();
    if !objects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut hashes: HashSet<ObjectHash> = HashSet::new();
    for entry in fs::read_dir(objects_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // The `pack/` directory has a 4-char name, so it is skipped here and
        // handled separately below.
        if dir_name.len() != 2 {
            continue;
        }

        for sub_entry in fs::read_dir(&path)? {
            let sub_entry = sub_entry?;
            let sub_path = sub_entry.path();
            if sub_path.is_file()
                && let Some(hash) = try_parse_loose_object(dir_name, &sub_path)
            {
                hashes.insert(hash);
            }
        }
    }

    if full {
        let pack_dir = objects_dir.join("pack");
        if pack_dir.exists() {
            for entry in fs::read_dir(&pack_dir)? {
                let path = entry?.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "idx") {
                    for hash in list_objects_in_pack_idx(&path)? {
                        hashes.insert(hash);
                    }
                }
            }
        }
    }

    Ok(hashes.into_iter().collect())
}

async fn check_single_object(
    object_id: &str,
    storage: &ClientStorage,
    strict: bool,
) -> CliResult<FsckResult> {
    let hash = parse_object_hash(object_id)
        .ok_or_else(|| CliError::command_usage(format!("invalid object ID: {}", object_id)))?;

    let (check_result, has_errors) = verify_object(&hash, storage, false, true, strict).await?;

    let overall_status = match check_result.status {
        CheckStatus::Ok => {
            println!("Object {} is valid", object_id);
            CheckStatus::Ok
        }
        CheckStatus::Missing => {
            report(FsckMsgId::Missing, &check_result.object_type, object_id);
            check_result.status
        }
        CheckStatus::HashMismatch => check_result.status,
        CheckStatus::InvalidFormat => {
            // Error already reported by verify_object, no need to report again
            check_result.status
        }
    };

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
        has_errors,
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
        has_errors: false,
    };

    // Get all object hashes (loose + packed unless `--no-full`)
    let all_hashes = list_all_objects_in_storage(storage, args.full_enabled())
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?;

    // Stage 1: Check all 256 object directories
    check_directories(storage, &all_hashes, args.verbose)?;

    // Sort hashes lexicographically for stage 2
    let mut sorted_hashes: Vec<String> = all_hashes.iter().map(|h| h.to_string()).collect();
    sorted_hashes.sort();

    // Stage 2: Check each object (sorted by hash)
    check_objects(
        &sorted_hashes,
        storage,
        &mut result,
        args.verbose,
        args.connectivity_only,
        args.strict,
    )
    .await?;

    // Stage 3: Check HEAD link
    let head_is_unborn = check_head().await;

    // Stage 4: Check reflog entries
    if !args.no_reflogs {
        check_reflogs(storage, &mut result, args.verbose).await?;
    }

    // Stage 5: Check refs point to valid objects
    check_and_fix_refs(args, storage, &mut result, args.connectivity_only).await?;

    // Stage 6: Check index
    check_index(storage, &mut result, args.verbose)?;

    // Stage 7: Check connectivity (re-verify all objects in storage order)
    check_connectivity(
        &all_hashes,
        storage,
        &mut result,
        args.verbose,
        args.name_objects,
        args.connectivity_only,
    )
    .await?;

    // Stage 8: Find dangling and unreachable objects
    find_dangling_unreachable(
        storage,
        &mut result,
        args.unreachable,
        args.no_reflogs,
        args.dangling_enabled(),
        args.lost_found,
        args.full_enabled(),
    )
    .await?;

    // Stage 9: Report root commits
    if args.root {
        find_and_report_roots(storage, args.full_enabled()).await?;
    }

    // Stage 10: Report tagged commits
    if args.tags {
        find_and_report_tags().await?;
    }

    // Print notices
    print_notices(head_is_unborn, &result);

    Ok(result)
}

/// Check all 256 object directories and print progress
fn check_directories(
    storage: &ClientStorage,
    all_hashes: &[ObjectHash],
    verbose: bool,
) -> CliResult<()> {
    // Count objects per prefix directory
    let mut prefix_counts = vec![0usize; 256];
    for hash in all_hashes {
        let hash_str = hash.to_string();
        if hash_str.len() >= 2
            && let Ok(prefix) = u8::from_str_radix(&hash_str[0..2], 16)
        {
            prefix_counts[prefix as usize] += 1;
        }
    }

    // Count pack objects
    let mut pack_count = 0;
    let pack_dir = storage.base_path().join("pack");
    if pack_dir.exists()
        && let Ok(entries) = fs::read_dir(&pack_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "idx")
                && let Ok(count) = count_pack(&path)
            {
                pack_count += count;
            }
        }
    }

    // Print directory progress
    if verbose {
        // --verbose: match git fsck output
        println!("Checking object directory");
    } else {
        // default: show progress
        println!("Checking object directories: 100% (256/256), done.");
    }

    // Print pack objects if any
    if pack_count > 0 {
        println!(
            "Checking objects: 100% ({}/{}), done.",
            pack_count, pack_count
        );
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
    connectivity_only: bool,
    strict: bool,
) -> CliResult<()> {
    for hash_str in sorted_hashes {
        let hash = match parse_object_hash(hash_str) {
            Some(h) => h,
            None => continue,
        };

        if verbose {
            // Get object type for verbose output only
            if let Ok(obj_type) = storage.get_object_type(&hash) {
                let type_name = match obj_type {
                    ObjectType::Blob => "blob",
                    ObjectType::Tree => "tree",
                    ObjectType::Commit => "commit",
                    ObjectType::Tag => "tag",
                    _ => "unknown",
                };
                println!("Checking {} {}", type_name, hash);
            } else {
                println!("Checking {}", hash);
            }
        }

        let (check_result, reported_errors) =
            verify_object(&hash, storage, connectivity_only, true, strict).await?;
        result.objects_checked += 1;
        result.has_errors |= reported_errors;

        match check_result.status {
            CheckStatus::Ok => result.objects_ok += 1,
            CheckStatus::Missing => {
                result.objects_corrupted += 1;
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = check_result.status.clone();
                }
            }
            CheckStatus::HashMismatch => {
                result.objects_corrupted += 1;
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = check_result.status.clone();
                }
            }
            CheckStatus::InvalidFormat => {
                result.objects_corrupted += 1;
                if result.overall_status == CheckStatus::Ok {
                    result.overall_status = CheckStatus::InvalidFormat;
                }
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
                Ok(Some(_)) => false, // Branch exists, not unborn
                Ok(None) => true,     // Branch doesn't exist, unborn
                Err(_) => true,       // Error, treat as unborn
            }
        }
        Ok(Head::Detached(_)) => false, // Detached HEAD, not unborn
        Err(_) => true,                 // Error, treat as unborn
    }
}

/// Build a map from object hash to human-readable names
/// Returns a map like: "abc123..." -> "refs/heads/master", "HEAD@{1234567890}~2^1:src/"
async fn build_object_name_map() -> std::collections::HashMap<String, String> {
    let mut name_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let db_conn = db::get_db_conn_instance().await;

    // Collect names from refs (e.g., "refs/heads/master")
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .unwrap_or_default();

    for ref_entry in refs {
        if let Some(commit_hash) = &ref_entry.commit {
            let ref_name = ref_entry.name.clone().unwrap_or_default();
            // Store ref name (may have multiple names per object)
            name_map
                .entry(commit_hash.clone())
                .and_modify(|e| *e = format!("{}, {}", e, ref_name))
                .or_insert(ref_name);
        }
    }

    // Collect names from reflogs (e.g., "HEAD@{1234567890}")
    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .unwrap_or_default();

    // Group reflog entries by hash and ref_name, sorted by timestamp desc
    use std::collections::BTreeMap;
    let mut reflog_by_hash: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    for entry in reflogs {
        let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');
        if !is_null_oid(&entry.new_oid) {
            reflog_by_hash
                .entry(entry.new_oid.clone())
                .or_default()
                .push((entry.timestamp, entry.ref_name.clone()));
        }
    }

    // For each hash, format the reflog names with timestamps and positions
    for (hash, mut entries) in reflog_by_hash {
        // Sort by timestamp descending (most recent first)
        entries.sort_by_key(|b| std::cmp::Reverse(b.0));

        let names: Vec<String> = entries
            .iter()
            .enumerate()
            .map(|(i, (_, ref_name))| {
                if i == 0 {
                    format!("{}@{{{}}}", ref_name, entries[0].0)
                } else {
                    format!("{}@{{{}}}~{}", ref_name, entries[0].0, i)
                }
            })
            .collect();

        let combined = names.join(", ");
        name_map
            .entry(hash)
            .and_modify(|e| *e = format!("{}, {}", e, combined))
            .or_insert(combined);
    }

    // Collect names from index (e.g., ":src/main.rs" or "src/main.rs")
    let index_path = path::index();
    if index_path.exists()
        && let Ok(index) = Index::load(&index_path)
    {
        for entry in index.tracked_entries(0) {
            let hash_str = entry.hash.to_string();
            let path_name = format!(":{}", entry.name);
            name_map
                .entry(hash_str)
                .and_modify(|e| *e = format!("{}, {}", e, path_name))
                .or_insert(path_name);
        }
    }

    name_map
}

/// Print notices (unborn branch, missing refs, etc.)
fn print_notices(head_is_unborn: bool, _result: &FsckResult) {
    if head_is_unborn {
        eprintln!("notice: HEAD points to an unborn branch (main)");
        eprintln!("notice: No default references");
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

        if !is_null_oid(&entry.old_oid)
            && let Some(_hash) = parse_object_hash(&entry.old_oid)
            && !storage.exist(&_hash)
        {
            result.reflog_issues += 1;
            result.has_errors |= report(FsckMsgId::Missing, "unknown", &entry.old_oid);
        }

        if !is_null_oid(&entry.new_oid)
            && let Some(_hash) = parse_object_hash(&entry.new_oid)
            && !storage.exist(&_hash)
        {
            result.reflog_issues += 1;
            result.has_errors |= report(FsckMsgId::Missing, "unknown", &entry.new_oid);
        }
    }
    Ok(())
}

/// Check index
fn check_index(storage: &ClientStorage, result: &mut FsckResult, verbose: bool) -> CliResult<()> {
    if verbose {
        println!("Checking cache tree of .libra/index");
    }

    let index_result = check_index_file(storage)?;
    result.index_valid = index_result.valid;

    if !index_result.valid {
        // Index corruption is a real integrity error: surface it as a non-zero
        // exit (aligning with `git fsck`), not just a status downgrade.
        result.has_errors = true;
        if result.overall_status == CheckStatus::Ok {
            result.overall_status = CheckStatus::InvalidFormat;
        }
    }
    Ok(())
}

/// Check connectivity (re-verify all objects)
async fn check_connectivity(
    all_hashes: &[ObjectHash],
    storage: &ClientStorage,
    result: &mut FsckResult,
    verbose: bool,
    name_objects: bool,
    connectivity_only: bool,
) -> CliResult<()> {
    let count = all_hashes.len();
    if verbose {
        println!("Checking connectivity ({} objects)", count);
    }

    // Build object name map if --name-objects is used
    let object_names = if name_objects && verbose {
        build_object_name_map().await
    } else {
        std::collections::HashMap::new()
    };

    for hash in all_hashes {
        if verbose {
            let hash_str = hash.to_string();
            if name_objects {
                let name = object_names
                    .get(hash_str.as_str())
                    .map(|s| s.as_str())
                    .unwrap_or(":");
                println!("Checking {} ({})", hash, name);
            } else {
                println!("Checking {}", hash);
            }
        }
        let (check_result, reported_errors) =
            verify_object(hash, storage, connectivity_only, false, false).await?;
        result.has_errors |= reported_errors;
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
    full: bool,
) -> CliResult<ReachabilityContext> {
    let mut ctx = ReachabilityContext::new();

    // Collect all objects in storage
    ctx.all_objects = list_all_objects_in_storage(storage, full)
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
        if let Some(commit_hash_str) = &ref_entry.commit
            && let Some(hash) = parse_object_hash(commit_hash_str)
        {
            ctx.refs_reachable.insert(hash);
        }
    }

    // Collect objects from reflogs
    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load reflogs: {}", e)))?;

    for entry in reflogs {
        let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');
        if !is_null_oid(&entry.old_oid)
            && let Some(hash) = parse_object_hash(&entry.old_oid)
        {
            ctx.reflog_objects.insert(hash);
        }
        if !is_null_oid(&entry.new_oid)
            && let Some(hash) = parse_object_hash(&entry.new_oid)
        {
            ctx.reflog_objects.insert(hash);
        }
    }

    // Collect objects from index
    let index_path = path::index();
    if index_path.exists()
        && let Ok(index) = Index::load(&index_path)
    {
        for entry in index.tracked_entries(0) {
            ctx.index_objects.insert(entry.hash);
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
    let mut queue: std::collections::VecDeque<ObjectHash> =
        starting_points.iter().copied().collect();

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
/// Note: Objects in reflog are NOT reported as dangling - reflog is a valid reference.
/// Only objects that are completely unreachable (not in refs, reflog, or index) are reported.
///
/// With --unreachable flag: prints all unreachable objects.
/// Default (dangling): only prints dangling commits (matching git fsck behavior).
/// With --no-dangling: skips dangling object reporting entirely.
/// With --lost-found: writes dangling/unreachable objects to .libra/lost-found/ (implies --no-reflogs for dangling detection)
async fn find_dangling_unreachable(
    storage: &ClientStorage,
    _result: &mut FsckResult,
    unreachable: bool,
    no_reflogs: bool,
    dangling: bool,
    lost_found: bool,
    full: bool,
) -> CliResult<()> {
    let ctx = collect_reachability_context(storage, full).await?;

    // --lost-found implies --no-reflogs for dangling detection (matching git fsck behavior)
    let effective_no_reflogs = no_reflogs || lost_found;

    // Build the set of starting points: refs + reflog entries
    // This matches git fsck behavior: objects reachable from reflog entries are not dangling
    let mut starting_points = ctx.refs_reachable.clone();

    // Only include reflog objects if --no-reflogs is not specified
    if !effective_no_reflogs {
        starting_points.extend(ctx.reflog_objects.iter().copied());
    }

    starting_points.extend(ctx.index_objects.iter().copied());

    // Mark all objects reachable from refs + reflog + index
    let all_reachable = bfs_mark_reachable(&starting_points, storage);

    // Collect dangling/unreachable objects for lost-found
    let mut lost_found_objects: Vec<(ObjectHash, String)> = Vec::new(); // (hash, obj_type)

    // Find objects not reachable from any starting point
    for hash in &ctx.all_objects {
        if all_reachable.contains(hash) {
            continue; // Reachable from refs, reflog, or index
        }

        let obj_type = match storage.get_object_type(hash) {
            Ok(t) => t.to_string(),
            Err(_) => "unknown".to_string(),
        };

        // Collect objects for lost-found
        if lost_found {
            lost_found_objects.push((*hash, obj_type.clone()));
        }

        if unreachable {
            // --unreachable: report all unreachable objects
            report(FsckMsgId::Unreachable, &obj_type, &hash.to_string());
        } else if dangling {
            // --dangling (default): only report dangling commits (matching git fsck)
            if obj_type == "commit" {
                report(FsckMsgId::Dangling, &obj_type, &hash.to_string());
            }
        }
        // --no-dangling: skip dangling reporting entirely
    }

    // Write lost-found objects if --lost-found is specified
    if lost_found && !lost_found_objects.is_empty() {
        write_lost_found_objects(storage, &lost_found_objects).await?;
    }

    Ok(())
}

/// Find and report root commits (commits with no parents)
async fn find_and_report_roots(storage: &ClientStorage, full: bool) -> CliResult<()> {
    use git_internal::internal::object::commit::Commit;

    let all_hashes = list_all_objects_in_storage(storage, full)
        .map_err(|e| CliError::fatal(format!("failed to list objects: {}", e)))?;

    for hash in all_hashes {
        // Only check commit objects
        let Ok(obj_type) = storage.get_object_type(&hash) else {
            continue;
        };

        if obj_type != ObjectType::Commit {
            continue;
        }

        // Load the commit and check if it has no parents
        let Ok(commit) = load_object::<Commit>(&hash) else {
            continue;
        };

        if commit.parent_commit_ids.is_empty() {
            // This is a root commit
            println!("root {}", hash);
        }
    }

    Ok(())
}

/// Find and report tagged commits
/// Output format matches git fsck --tags:
/// - For annotated tags: "tagged commit <commit-hash> (<tag-name>) in <tag-object-hash>"
async fn find_and_report_tags() -> CliResult<()> {
    use sea_orm::EntityTrait;

    use crate::internal::model::reference;

    let db_conn = db::get_db_conn_instance().await;

    // Load all refs that are tags (refs/tags/*)
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load refs: {}", e)))?;

    for ref_entry in refs {
        let ref_name = match &ref_entry.name {
            Some(name) => name,
            None => continue,
        };

        // Only process tag refs (refs/tags/*)
        if !ref_name.starts_with("refs/tags/") {
            continue;
        }

        let tag_name = ref_name
            .strip_prefix("refs/tags/")
            .expect("INVARIANT: ref_name was guarded by starts_with(\"refs/tags/\") above");
        let commit_hash = match &ref_entry.commit {
            Some(hash) => hash,
            None => continue,
        };

        // Check if this is an annotated tag (tag object exists)
        // For now, just report the tagged commit
        println!("tagged commit {} ({})", commit_hash, tag_name);
    }

    Ok(())
}

/// Write dangling/unreachable objects to .libra/lost-found/
/// - commit/tree objects: written to lost-found/commit/<hash> with hash as content
/// - blob objects: written to lost-found/other/<hash> with blob content
async fn write_lost_found_objects(
    storage: &ClientStorage,
    objects: &[(ObjectHash, String)], // (hash, object_type)
) -> CliResult<()> {
    use std::{fs::OpenOptions, io::Write};

    let lost_found_dir = storage
        .base_path()
        .parent()
        .expect("storage should have parent")
        .join("lost-found");

    // Create lost-found directory structure
    let commit_dir = lost_found_dir.join("commit");
    let other_dir = lost_found_dir.join("other");
    fs::create_dir_all(&commit_dir)
        .map_err(|e| CliError::fatal(format!("failed to create lost-found/commit: {}", e)))?;
    fs::create_dir_all(&other_dir)
        .map_err(|e| CliError::fatal(format!("failed to create lost-found/other: {}", e)))?;

    for (hash, obj_type) in objects {
        let hash_str = hash.to_string();

        match obj_type.as_str() {
            "commit" => {
                // Write commit hash to lost-found/commit/<hash>
                let file_path = commit_dir.join(&hash_str);
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                    .map_err(|e| {
                        CliError::fatal(format!("failed to create {}: {}", file_path.display(), e))
                    })?;
                writeln!(file, "{}", hash_str).map_err(|e| {
                    CliError::fatal(format!("failed to write {}: {}", file_path.display(), e))
                })?;
            }
            "tree" => {
                // Write tree hash to lost-found/other/<hash>
                let file_path = other_dir.join(&hash_str);
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                    .map_err(|e| {
                        CliError::fatal(format!("failed to create {}: {}", file_path.display(), e))
                    })?;
                writeln!(file, "{}", hash_str).map_err(|e| {
                    CliError::fatal(format!("failed to write {}: {}", file_path.display(), e))
                })?;
            }
            "blob" => {
                // Write blob content to lost-found/other/<hash>
                let file_path = other_dir.join(&hash_str);
                let data = storage.get(hash).map_err(|e| {
                    CliError::fatal(format!("failed to read blob {}: {}", hash_str, e))
                })?;
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                    .map_err(|e| {
                        CliError::fatal(format!("failed to create {}: {}", file_path.display(), e))
                    })?;
                file.write_all(&data).map_err(|e| {
                    CliError::fatal(format!("failed to write {}: {}", file_path.display(), e))
                })?;
            }
            _ => {
                // Unknown type: write hash to other
                let file_path = other_dir.join(&hash_str);
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                    .map_err(|e| {
                        CliError::fatal(format!("failed to create {}: {}", file_path.display(), e))
                    })?;
                writeln!(file, "{}", hash_str).map_err(|e| {
                    CliError::fatal(format!("failed to write {}: {}", file_path.display(), e))
                })?;
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
    connectivity_only: bool,
) -> CliResult<()> {
    let ref_result = check_refs(storage, connectivity_only).await?;
    result.refs_checked = ref_result.checked;
    result.refs_ok = ref_result.ok;
    result.refs_broken = ref_result.broken;

    if ref_result.broken > 0 {
        if result.overall_status == CheckStatus::Ok {
            result.overall_status = CheckStatus::Missing;
        }
        result.has_errors = true; // Broken refs (missing objects) should cause failure
    }
    Ok(())
}

/// Whether a signature timezone is a well-formed `±HHMM` offset within ±1400
/// (the widest real-world UTC offset). Used by `--strict`.
fn is_valid_timezone(tz: &str) -> bool {
    let bytes = tz.as_bytes();
    if bytes.len() != 5 || (bytes[0] != b'+' && bytes[0] != b'-') {
        return false;
    }
    let digits = &tz[1..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let hours: i32 = digits[0..2].parse().unwrap_or(99);
    let minutes: i32 = digits[2..4].parse().unwrap_or(99);
    minutes < 60 && hours * 100 + minutes <= 1400
}

/// The object type a tree entry of the given mode must resolve to (used by
/// `--strict` to flag mode/type mismatches).
fn expected_type_for_mode(mode: TreeItemMode) -> ObjectType {
    match mode {
        TreeItemMode::Tree => ObjectType::Tree,
        TreeItemMode::Commit => ObjectType::Commit, // gitlink / submodule
        TreeItemMode::Blob | TreeItemMode::BlobExecutable | TreeItemMode::Link => ObjectType::Blob,
    }
}

/// Whether tree entries are in Git's canonical sort order: by name, treating a
/// tree entry's name as if it had a trailing `/`.
fn tree_entries_sorted(items: &[git_internal::internal::object::tree::TreeItem]) -> bool {
    fn sort_key(item: &git_internal::internal::object::tree::TreeItem) -> Vec<u8> {
        let mut key = item.name.as_bytes().to_vec();
        if item.mode == TreeItemMode::Tree {
            key.push(b'/');
        }
        key
    }
    items
        .windows(2)
        .all(|pair| sort_key(&pair[0]) <= sort_key(&pair[1]))
}

/// Verify a single object's integrity
/// If connectivity_only is true, only checks that objects exist (not their content)
/// If report_errors is true, reports errors immediately; otherwise just returns status
/// If strict is true, applies the additional `--strict` format/graph checks
/// Returns (ObjectCheckResult, has_error)
async fn verify_object(
    hash: &ObjectHash,
    storage: &ClientStorage,
    connectivity_only: bool,
    report_errors: bool,
    strict: bool,
) -> CliResult<(ObjectCheckResult, bool)> {
    let mut has_error = false;

    // Check if object exists
    if !storage.exist(hash) {
        if report_errors {
            has_error |= report(FsckMsgId::Missing, "unknown", &hash.to_string());
        }
        return Ok((
            ObjectCheckResult {
                object_id: hash.to_string(),
                object_type: "unknown".to_string(),
                status: CheckStatus::Missing,
                error_message: Some("Object not found in storage".to_string()),
                size: 0,
            },
            has_error,
        ));
    }

    // Get object type
    let obj_type = match storage.get_object_type(hash) {
        Ok(t) => t,
        Err(_) => {
            // Cannot determine object type - object data is corrupted
            if report_errors {
                has_error |= report(FsckMsgId::UnknownType, "unknown", &hash.to_string());
            }
            return Ok((
                ObjectCheckResult {
                    object_id: hash.to_string(),
                    object_type: "unknown".to_string(),
                    status: CheckStatus::InvalidFormat,
                    error_message: Some(format!("Object {} has unknown type", hash)),
                    size: 0,
                },
                has_error,
            ));
        }
    };

    // --connectivity-only: only check that objects exist, skip content validation
    if connectivity_only {
        return Ok((
            ObjectCheckResult {
                object_id: hash.to_string(),
                object_type: obj_type.to_string(),
                status: CheckStatus::Ok,
                error_message: None,
                size: 0,
            },
            false,
        ));
    }

    // Get raw data for full validation
    let data = match storage.get(hash) {
        Ok(d) => d,
        Err(e) => {
            if report_errors {
                has_error |= report(
                    FsckMsgId::HashMismatch,
                    &obj_type.to_string(),
                    &hash.to_string(),
                );
            }
            return Ok((
                ObjectCheckResult {
                    object_id: hash.to_string(),
                    object_type: obj_type.to_string(),
                    status: CheckStatus::HashMismatch,
                    error_message: Some(format!("Failed to read object: {}", e)),
                    size: 0,
                },
                has_error,
            ));
        }
    };

    let size = data.len();

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
        if report_errors {
            has_error |= report(
                FsckMsgId::HashMismatch,
                &obj_type.to_string(),
                &hash.to_string(),
            );
        }
        return Ok((
            ObjectCheckResult {
                object_id: hash.to_string(),
                object_type: obj_type.to_string(),
                status: CheckStatus::HashMismatch,
                error_message: Some(format!(
                    "Hash mismatch: expected {}, computed {}",
                    hash,
                    hex::encode(computed_bytes)
                )),
                size,
            },
            has_error,
        ));
    }

    // Verify object format and run type-specific checks
    match obj_type {
        ObjectType::Blob => {
            if Blob::from_bytes(&data, *hash).is_err() {
                return Ok((
                    ObjectCheckResult {
                        object_id: hash.to_string(),
                        object_type: obj_type.to_string(),
                        status: CheckStatus::InvalidFormat,
                        error_message: Some(format!("Object {} has invalid blob format", hash)),
                        size,
                    },
                    false,
                ));
            }
        }
        ObjectType::Tree => {
            match Tree::from_bytes(&data, *hash) {
                Ok(tree) => {
                    // Check tree entries
                    for item in &tree.tree_items {
                        // Check for problematic pathnames
                        if item.name == "." && report_errors {
                            has_error |= report(FsckMsgId::HasDot, "tree", &hash.to_string());
                        } else if item.name == ".." && report_errors {
                            has_error |= report(FsckMsgId::HasDotdot, "tree", &hash.to_string());
                        } else if item.name == ".libra" && report_errors {
                            has_error |= report(FsckMsgId::HasDotlibra, "tree", &hash.to_string());
                        }
                        // Check for empty name component
                        if item.name.is_empty() && report_errors {
                            has_error |= report(FsckMsgId::EmptyName, "tree", &hash.to_string());
                        }
                        // Check for full pathname
                        if item.name.starts_with('/') && report_errors {
                            has_error |= report(FsckMsgId::FullPathname, "tree", &hash.to_string());
                        }
                        // Check for null sha1
                        if item.id.as_ref().iter().all(|&b| b == 0) && report_errors {
                            has_error |= report(FsckMsgId::NullSha1, "tree", &hash.to_string());
                        }
                    }

                    if strict && report_errors {
                        for item in &tree.tree_items {
                            // Each entry's target must exist with a matching type.
                            if !storage.exist(&item.id) {
                                has_error |=
                                    report(FsckMsgId::Missing, "tree", &item.id.to_string());
                            } else if let Ok(actual) = storage.get_object_type(&item.id)
                                && actual != expected_type_for_mode(item.mode)
                            {
                                has_error |=
                                    report(FsckMsgId::BadObjectSha1, "tree", &hash.to_string());
                            }
                        }
                        // Entries must be in Git's canonical sort order.
                        if !tree_entries_sorted(&tree.tree_items) {
                            has_error |=
                                report(FsckMsgId::TreeNotSorted, "tree", &hash.to_string());
                        }
                    }
                }
                Err(_) => {
                    if report_errors {
                        has_error |=
                            report(FsckMsgId::BadTree, &obj_type.to_string(), &hash.to_string());
                    }
                    return Ok((
                        ObjectCheckResult {
                            object_id: hash.to_string(),
                            object_type: obj_type.to_string(),
                            status: CheckStatus::InvalidFormat,
                            error_message: Some(format!("Object {} has invalid tree format", hash)),
                            size,
                        },
                        has_error,
                    ));
                }
            }
        }
        ObjectType::Commit => {
            match Commit::from_bytes(&data, *hash) {
                Ok(commit) => {
                    // Check required fields
                    if commit.author.name.is_empty() && report_errors {
                        has_error |= report(FsckMsgId::MissingAuthor, "commit", &hash.to_string());
                    }
                    if commit.author.email.is_empty() && report_errors {
                        has_error |= report(FsckMsgId::MissingEmail, "commit", &hash.to_string());
                    }
                    if commit.committer.name.is_empty() && report_errors {
                        has_error |=
                            report(FsckMsgId::MissingCommitter, "commit", &hash.to_string());
                    }
                    if commit.committer.email.is_empty() && report_errors {
                        has_error |= report(FsckMsgId::MissingEmail, "commit", &hash.to_string());
                    }

                    if strict && report_errors {
                        // Emails must contain '@'.
                        if !commit.author.email.is_empty() && !commit.author.email.contains('@') {
                            has_error |= report(FsckMsgId::BadEmail, "commit", &hash.to_string());
                        }
                        if !commit.committer.email.is_empty()
                            && !commit.committer.email.contains('@')
                        {
                            has_error |= report(FsckMsgId::BadEmail, "commit", &hash.to_string());
                        }
                        // Timezones must be well-formed and within range.
                        if !is_valid_timezone(&commit.author.timezone)
                            || !is_valid_timezone(&commit.committer.timezone)
                        {
                            has_error |=
                                report(FsckMsgId::BadTimezone, "commit", &hash.to_string());
                        }
                        // The tree must exist and be a tree.
                        if !storage.exist(&commit.tree_id) {
                            has_error |=
                                report(FsckMsgId::MissingTree, "commit", &hash.to_string());
                        } else if let Ok(tree_type) = storage.get_object_type(&commit.tree_id)
                            && tree_type != ObjectType::Tree
                        {
                            has_error |=
                                report(FsckMsgId::BadObjectSha1, "commit", &hash.to_string());
                        }
                        // Parents must exist and be commits.
                        for parent in &commit.parent_commit_ids {
                            if !storage.exist(parent) {
                                has_error |=
                                    report(FsckMsgId::Missing, "commit", &parent.to_string());
                            } else if let Ok(parent_type) = storage.get_object_type(parent)
                                && parent_type != ObjectType::Commit
                            {
                                has_error |=
                                    report(FsckMsgId::BadObjectSha1, "commit", &hash.to_string());
                            }
                        }
                    }
                }
                Err(_) => {
                    // Commit object exists but cannot be parsed - data corruption
                    if report_errors {
                        has_error |= report(FsckMsgId::BadObjectSha1, "commit", &hash.to_string());
                    }
                    return Ok((
                        ObjectCheckResult {
                            object_id: hash.to_string(),
                            object_type: obj_type.to_string(),
                            status: CheckStatus::InvalidFormat,
                            error_message: Some(format!(
                                "Object {} has invalid commit format",
                                hash
                            )),
                            size,
                        },
                        has_error,
                    ));
                }
            }
        }
        ObjectType::Tag => {
            let tag = match GitTag::from_bytes(&data, *hash) {
                Ok(tag) => tag,
                Err(error) => {
                    let msg_id = tag_parse_error_msg_id(&error);
                    if report_errors {
                        has_error |= report(msg_id, "tag", &hash.to_string());
                    }
                    return Ok((
                        ObjectCheckResult {
                            object_id: hash.to_string(),
                            object_type: obj_type.to_string(),
                            status: CheckStatus::InvalidFormat,
                            error_message: Some(format!(
                                "Object {} has invalid tag format: {}",
                                hash, error
                            )),
                            size,
                        },
                        has_error,
                    ));
                }
            };

            if tag.tag_name.trim().is_empty() {
                if report_errors {
                    has_error |= report(FsckMsgId::BadTagName, "tag", &hash.to_string());
                }
                return Ok((
                    ObjectCheckResult {
                        object_id: hash.to_string(),
                        object_type: obj_type.to_string(),
                        status: CheckStatus::InvalidFormat,
                        error_message: Some(format!(
                            "Object {} has invalid tag format: empty tag name",
                            hash
                        )),
                        size,
                    },
                    has_error,
                ));
            }

            if !storage.exist(&tag.object_hash) {
                if report_errors {
                    has_error |= report(
                        FsckMsgId::Missing,
                        &tag.object_type.to_string(),
                        &tag.object_hash.to_string(),
                    );
                }
                return Ok((
                    ObjectCheckResult {
                        object_id: hash.to_string(),
                        object_type: obj_type.to_string(),
                        status: CheckStatus::Missing,
                        error_message: Some(format!(
                            "Tag {} points to missing {} {}",
                            hash, tag.object_type, tag.object_hash
                        )),
                        size,
                    },
                    has_error,
                ));
            }

            if let Ok(actual_type) = storage.get_object_type(&tag.object_hash)
                && actual_type != tag.object_type
            {
                if report_errors {
                    has_error |= report(FsckMsgId::BadObjectSha1, "tag", &hash.to_string());
                }
                return Ok((
                    ObjectCheckResult {
                        object_id: hash.to_string(),
                        object_type: obj_type.to_string(),
                        status: CheckStatus::InvalidFormat,
                        error_message: Some(format!(
                            "Tag {} declares target type {} but target {} is {}",
                            hash, tag.object_type, tag.object_hash, actual_type
                        )),
                        size,
                    },
                    has_error,
                ));
            }
        }
        _ => {
            if report_errors {
                has_error |= report(
                    FsckMsgId::UnknownType,
                    &obj_type.to_string(),
                    &hash.to_string(),
                );
            }
        }
    }

    Ok((
        ObjectCheckResult {
            object_id: hash.to_string(),
            object_type: obj_type.to_string(),
            status: CheckStatus::Ok,
            error_message: None,
            size,
        },
        has_error,
    ))
}

/// Result of checking refs
#[derive(Clone)]
struct RefCheckResult {
    checked: usize,
    ok: usize,
    broken: usize,
    broken_ref_names: Vec<String>,
}

/// Check all refs point to valid objects
async fn check_refs(storage: &ClientStorage, connectivity_only: bool) -> CliResult<RefCheckResult> {
    let mut result = RefCheckResult {
        checked: 0,
        ok: 0,
        broken: 0,
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
                    match verify_object(&hash, storage, connectivity_only, false, false).await {
                        Ok((check, _reported)) if check.status == CheckStatus::Ok => {
                            result.ok += 1;
                        }
                        Ok((_check, _reported)) => {
                            // Object exists but is corrupted - already reported in check_objects
                            result.broken += 1;
                            let ref_name = ref_entry.name.clone().unwrap_or_default();
                            result.broken_ref_names.push(ref_name.clone());
                        }
                        Err(_e) => {
                            result.broken += 1;
                            let ref_name = ref_entry.name.clone().unwrap_or_default();
                            result.broken_ref_names.push(ref_name.clone());
                        }
                    }
                } else {
                    result.broken += 1;
                    let ref_name = ref_entry.name.clone().unwrap_or_default();
                    result.broken_ref_names.push(ref_name.clone());
                    report(FsckMsgId::Missing, "commit", commit_hash_str);
                }
            } else {
                result.broken += 1;
                let ref_name = ref_entry.name.clone().unwrap_or_default();
                result.broken_ref_names.push(ref_name.clone());
                // Invalid hash format - report as bad ref content
                eprintln!("bad ref content: {}: invalid hash format", ref_name);
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
            eprintln!("index corruption: {}", e);
            return Ok(result);
        }
    };

    // Step 2: Validate each index entry.
    let entries = index.tracked_entries(0);

    for entry in entries {
        result.entries_checked += 1;

        if let Some(msg_id) = validate_index_entry(entry, storage) {
            result.entries_corrupted += 1;
            result.valid = false;
            // Report and track error
            let _ = report(msg_id, "blob", &entry.hash.to_string());
            continue;
        }

        result.entries_ok += 1;
    }

    // Step 3: Check for entries in non-zero stages (merge conflict markers)
    for stage in [1, 2, 3] {
        let conflict_entries = index.tracked_entries(stage);
        if !conflict_entries.is_empty() {
            for entry in conflict_entries {
                eprintln!("index conflict marker: {} (stage {})", entry.name, stage);
                result.entries_checked += 1;
            }
        }
    }

    Ok(result)
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

/// Validate a single index entry against storage. Returns Some(FsckMsgId) on failure.
fn validate_index_entry(
    entry: &git_internal::internal::index::IndexEntry,
    storage: &ClientStorage,
) -> Option<FsckMsgId> {
    if !is_valid_index_mode(entry.mode) {
        eprintln!("invalid index mode: {}", entry.name);
        return Some(FsckMsgId::InvalidIndexMode);
    }

    if entry.flags.stage > 3 {
        eprintln!("invalid index stage: {}", entry.name);
        return Some(FsckMsgId::InvalidIndexStage);
    }

    if !storage.exist(&entry.hash) {
        return Some(FsckMsgId::Missing);
    }

    if let Ok(obj_type) = storage.get_object_type(&entry.hash)
        && obj_type != ObjectType::Blob
    {
        return Some(FsckMsgId::IndexEntryWrongType);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{FsckMsgId, tag_parse_error_msg_id};

    #[test]
    fn is_valid_timezone_accepts_in_range_and_rejects_invalid() {
        assert!(super::is_valid_timezone("+0000"));
        assert!(super::is_valid_timezone("-0800"));
        assert!(super::is_valid_timezone("+1400"));
        assert!(!super::is_valid_timezone("+9900"), "hours out of range");
        assert!(!super::is_valid_timezone("+0060"), "minutes must be < 60");
        assert!(!super::is_valid_timezone("0000"), "missing sign");
        assert!(!super::is_valid_timezone("+00:0"), "non-digit");
        assert!(!super::is_valid_timezone("+000"), "wrong length");
    }

    #[test]
    fn list_objects_in_pack_idx_rejects_malformed_index() {
        let dir = tempfile::tempdir().expect("temp dir");
        let idx = dir.path().join("bad.idx");
        std::fs::write(&idx, b"not a valid pack index").expect("write garbage idx");

        let error = super::list_objects_in_pack_idx(&idx).expect_err("malformed idx must error");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidData,
            "a malformed pack index should surface as InvalidData"
        );
    }

    #[test]
    fn tag_parse_error_msg_id_keeps_object_type_errors_specific() {
        assert_eq!(
            tag_parse_error_msg_id(&"Missing object type"),
            FsckMsgId::MissingType
        );
        assert_eq!(
            tag_parse_error_msg_id(&"Invalid object type"),
            FsckMsgId::MissingType
        );
        assert_eq!(
            tag_parse_error_msg_id(&"Missing object hash"),
            FsckMsgId::MissingObject
        );
    }
}
