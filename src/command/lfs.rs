//! LFS subcommands for authentication, batch negotiation, lock management, and integrating media storage with standard workflows.
//!
//! LFS 子命令，用于身份验证、批量协商、锁管理和与标准工作流集成媒体存储。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{File, OpenOptions},
    io,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
    str::FromStr,
};

use clap::Subcommand;
use git_internal::{
    hash::ObjectHash,
    internal::{
        index::Index,
        object::{blob::Blob, commit::Commit, tree::Tree},
        pack::entry::Entry,
    },
};
use reqwest::StatusCode;
use sea_orm::EntityTrait;
use walkdir::WalkDir;

use crate::{
    command::{
        lfs_schema::{LfsFileOutput, LfsOutput},
        load_object, status,
    },
    internal::{
        branch::Branch,
        db::get_db_conn_instance,
        head::Head,
        model::reflog,
        protocol::lfs_client::{LFSClient, LockListError},
        tag::{self, TagObject},
    },
    lfs_structs::LockListQuery,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        lfs,
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        path,
        path_ext::PathExt,
        util,
    },
};

/// `--help` examples shown in `libra lfs --help` output (attached in
/// `src/cli.rs` via `after_help` on the `Lfs` subcommand).
///
/// `lfs` exposes the attribute flows (`track`/`untrack`/`ls-files`), the three
/// lock-server flows (`locks`/`lock`/`unlock`), the `install`/`uninstall`
/// compatibility no-ops, and the object-sync flows (`push`/`fetch`/`prune`/
/// `checkout`). The banner pins the canonical invocation per sub-command plus a
/// JSON variant so users can map intent to invocation without reading the design
/// doc. Cross-cutting `--help` EXAMPLES rollout per `docs/improvement/README.md`
/// item B.
pub const LFS_EXAMPLES: &str = "\
EXAMPLES:
    libra lfs track                       List currently tracked LFS attribute patterns
    libra lfs track '*.bin' '*.psd'       Add LFS patterns to .libra_attributes
    libra lfs untrack '*.bin'             Remove an LFS pattern
    libra lfs ls-files                    List LFS-tracked files in the working tree
    libra lfs ls-files --long --size      Show full OIDs and sizes
    libra lfs locks                       List remote locks for the current branch
    libra lfs lock build/output.bin       Acquire a remote lock on a file
    libra lfs unlock build/output.bin     Release a lock you own
    libra lfs unlock --force --id <id>    Force-release a lock owned by someone else
    libra lfs install                     No-op compatibility shim (built-in LFS, no filters)
    libra lfs push origin main            Push LFS objects on local refs to a remote
    libra lfs fetch origin main           Fetch LFS objects for remote-tracking refs
    libra lfs prune --dry-run             Preview unreferenced local LFS objects to delete
    libra lfs checkout                    Restore pointer files to full LFS content
    libra lfs --json ls-files             Structured JSON output for agents";

/// [Docs](https://github.com/git-lfs/git-lfs/tree/main/docs/man)
#[derive(Subcommand, Debug)]
pub enum LfsCmds {
    /// View or add LFS paths to Libra Attributes (root)
    Track {
        /// One or more glob patterns to mark as LFS-tracked (e.g. `*.bin`). Omit to list current patterns
        pattern: Option<Vec<String>>,
    },
    /// Remove LFS paths from Libra Attributes
    Untrack {
        /// One or more glob patterns to remove from `.libra_attributes`
        path: Vec<String>,
    },
    /// Lists currently locked files from the Libra LFS server. (Current Branch)
    Locks {
        /// Filter to a single lock id
        #[clap(long, short, value_name = "ID")]
        id: Option<String>,
        /// Filter locks to a specific repository-relative path
        #[clap(long, short, value_name = "PATH")]
        path: Option<String>,
        /// Maximum number of locks to return
        #[clap(long, short, value_name = "N")]
        limit: Option<u64>,
    },
    /// Set a file as "locked" on the Libra LFS server
    Lock {
        /// String path name of the locked file. This should be relative to the root of the repository working directory
        path: String,
    },
    /// Remove "locked" setting for a file on the Libra LFS server
    Unlock {
        /// Repository-relative path of the file to unlock
        path: String,
        /// Force-release a lock you do not own (requires server-side permission)
        #[clap(long, short)]
        force: bool,
        /// Unlock by lock id instead of by path
        #[clap(long, short, value_name = "ID")]
        id: Option<String>,
    },
    /// Show information about Libra LFS files in the index and working tree (current branch)
    LsFiles {
        /// Show the entire 64 character OID, instead of just first 10.
        #[clap(long, short)]
        long: bool,
        /// Show the size of the LFS object between parenthesis at the end of a line.
        #[clap(long, short)]
        size: bool,
        /// Show only the lfs tracked file names.
        #[clap(long, short)]
        name_only: bool,
    },
    /// No-op compatibility shim: Libra has built-in LFS, so no global filter is installed
    Install,
    /// No-op compatibility shim: built-in LFS has no global filter to remove
    Uninstall,
    /// Push LFS objects referenced by local refs to the remote
    Push {
        /// Remote name (first positional). Defaults to the current branch upstream when omitted
        remote: Option<String>,
        /// Local refs to scan (remaining positionals). Defaults to the current HEAD branch.
        /// To push by ref the remote must be given first: `libra lfs push <remote> <ref>...`
        #[clap(value_name = "REF")]
        refs: Vec<String>,
    },
    /// Fetch LFS objects referenced by remote-tracking refs from the remote
    Fetch {
        /// Remote name (first positional). Defaults to the current branch upstream when omitted
        remote: Option<String>,
        /// Remote-tracking refs to scan (remaining positionals). Defaults to the current branch's
        /// upstream ref. To fetch by ref the remote must be given first: `libra lfs fetch <remote> <ref>...`
        #[clap(value_name = "REF")]
        refs: Vec<String>,
    },
    /// Delete local LFS objects not referenced by any reachable ref, the index, or worktree pointers
    Prune {
        /// Show what would be deleted without removing anything
        #[clap(long, short = 'n')]
        dry_run: bool,
    },
    /// Restore working-tree pointer files to full LFS object content from the local cache
    Checkout {
        /// Optional paths to restore (defaults to all LFS-tracked pointer files)
        #[clap(value_name = "PATH")]
        path: Vec<String>,
    },
}

pub async fn execute(cmd: LfsCmds) -> CliResult<()> {
    execute_safe(cmd, &OutputConfig::default()).await
}

pub async fn execute_safe(cmd: LfsCmds, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_lfs(cmd, output).await?;
    render_lfs_output(&result, output)
}

async fn run_lfs(cmd: LfsCmds, output: &OutputConfig) -> CliResult<LfsOutput> {
    // TODO: attributes file should be created in current dir, NOT root dir
    let attr_path = path::attributes().to_string_or_panic();
    match cmd {
        LfsCmds::Track { pattern } => {
            match pattern {
                Some(pattern) => {
                    let pattern = convert_patterns_to_workdir(pattern); //
                    let patterns = add_lfs_patterns(&attr_path, pattern).map_err(|e| {
                        CliError::io(format!("failed to update '{attr_path}': {e}"))
                    })?;
                    Ok(LfsOutput {
                        action: "track".to_string(),
                        patterns,
                        ..LfsOutput::default()
                    })
                }
                None => {
                    let lfs_patterns = lfs::extract_lfs_patterns(&attr_path)
                        .map_err(|e| CliError::io(format!("failed to read '{attr_path}': {e}")))?;
                    Ok(LfsOutput {
                        action: "track-list".to_string(),
                        patterns: lfs_patterns,
                        ..LfsOutput::default()
                    })
                }
            }
        }
        LfsCmds::Untrack { path } => {
            // only remove totally same pattern with path ?
            let path = convert_patterns_to_workdir(path); //
            let patterns = untrack_lfs_patterns(&attr_path, path)
                .map_err(|e| CliError::io(format!("failed to update '{attr_path}': {e}")))?;
            Ok(LfsOutput {
                action: "untrack".to_string(),
                patterns,
                ..LfsOutput::default()
            })
        }
        LfsCmds::Locks { id, path, limit } => {
            let refspec = current_refspec_or_err().await?;
            tracing::debug!("refspec: {}", refspec);
            let query = LockListQuery {
                id: id.unwrap_or_default(),
                path: path.unwrap_or_default(),
                limit: limit.map(|l| l.to_string()).unwrap_or_default(),
                cursor: "".to_string(),
                refspec: refspec.clone(),
            };
            let locks = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .get_locks(query)
                .await
                .map_err(map_lock_list_error)?
                .locks;
            Ok(LfsOutput {
                action: "locks".to_string(),
                locks,
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::Lock { path } => {
            // Only check existence
            if !Path::new(&path).exists() {
                return Err(
                    CliError::fatal(format!("pathspec '{path}' did not match any files"))
                        .with_stable_code(StableErrorCode::CliInvalidTarget),
                );
            }

            let refspec = current_refspec_or_err().await?;
            let code = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .lock(path.clone(), refspec.clone())
                .await
                .map_err(|e| {
                    CliError::network(format!("LFS lock request failed: {e}"))
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?;
            if code == StatusCode::FORBIDDEN {
                return Err(
                    CliError::fatal("You must have push access to create a lock")
                        .with_stable_code(StableErrorCode::AuthPermissionDenied),
                );
            } else if code == StatusCode::CONFLICT {
                return Err(CliError::conflict("lock already exists")
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked));
            } else if !code.is_success() {
                return Err(CliError::network(format!(
                    "LFS lock failed with status {}",
                    code.as_u16()
                ))
                .with_detail("status", code.as_u16()));
            }
            Ok(LfsOutput {
                action: "lock".to_string(),
                path: Some(path),
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::Unlock { path, force, id } => {
            // When `--id` is provided the lock is looked up by id on the
            // server (see the `Some(id) => id` branch below); `path` is
            // only kept as a label for the audit output. Skipping the
            // path-existence and clean-tree checks in that case avoids
            // friction when unlocking a file that has been deleted
            // locally but still holds a server-side lock.
            if !force && id.is_none() {
                if !Path::new(&path).exists() {
                    return Err(CliError::fatal(format!(
                        "pathspec '{path}' did not match any files"
                    ))
                    .with_stable_code(StableErrorCode::CliInvalidTarget));
                }
                if !status::is_clean().await {
                    return Err(CliError::conflict("working tree not clean")
                        .with_stable_code(StableErrorCode::ConflictOperationBlocked));
                }
            }
            let refspec = current_refspec_or_err().await?;
            let id = match id {
                None => {
                    // get id by path
                    let locks = LFSClient::get()
                        .await
                        .map_err(|e| {
                            CliError::fatal(e.to_string())
                                .with_stable_code(StableErrorCode::NetworkUnavailable)
                        })?
                        .get_locks(LockListQuery {
                            refspec: refspec.clone(),
                            path: path.clone(),
                            id: "".to_string(),
                            cursor: "".to_string(),
                            limit: "".to_string(),
                        })
                        .await
                        .map_err(map_lock_list_error)?
                        .locks;
                    if locks.is_empty() {
                        return Err(CliError::fatal(format!("no lock found for path '{path}'"))
                            .with_stable_code(StableErrorCode::RepoStateInvalid));
                    }
                    locks[0].id.clone()
                }
                Some(id) => id,
            };
            let code = LFSClient::get()
                .await
                .map_err(|e| {
                    CliError::fatal(e.to_string())
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?
                .unlock(id.clone(), refspec.clone(), force)
                .await
                .map_err(|e| {
                    CliError::network(format!("LFS unlock request failed: {e}"))
                        .with_stable_code(StableErrorCode::NetworkUnavailable)
                })?;
            if code == StatusCode::FORBIDDEN {
                return Err(CliError::fatal("You must have push access to unlock")
                    .with_stable_code(StableErrorCode::AuthPermissionDenied));
            } else if !code.is_success() {
                return Err(CliError::network(format!(
                    "LFS unlock failed with status {}",
                    code.as_u16()
                ))
                .with_detail("status", code.as_u16()));
            }
            Ok(LfsOutput {
                action: "unlock".to_string(),
                path: Some(path),
                id: Some(id),
                refspec: Some(refspec),
                ..LfsOutput::default()
            })
        }
        LfsCmds::LsFiles {
            long,
            size,
            name_only,
        } => {
            let idx_file = path::index();
            let index = Index::load(&idx_file)
                .map_err(|e| CliError::io(format!("failed to load index: {e}")))?;
            let entries = index.tracked_entries(0);
            let storage = util::objects_storage();
            let mut files = Vec::new();
            for entry in entries {
                let path_abs = util::workdir_to_absolute(&entry.name);
                if lfs::is_lfs_tracked(&path_abs) {
                    let data = storage.get(&entry.hash).map_err(|e| {
                        CliError::io(format!("failed to read blob {}: {e}", entry.hash))
                    })?;
                    if let Some((oid, lfs_size)) = lfs::parse_pointer_data(&data) {
                        let is_pointer = lfs::parse_pointer_file(&path_abs).is_ok();
                        // An asterisk (*) after the OID indicates a full object, a minus (-) indicates an LFS pointer.
                        // or not exists (-)
                        let _type = if is_pointer || !path_abs.exists() {
                            "-"
                        } else {
                            "*"
                        };
                        let full_oid = oid.clone();
                        let oid = if long { oid } else { oid[..10].to_owned() };
                        let (size_value, display_size) = if size {
                            let display = util::auto_unit_bytes(lfs_size);
                            (Some(lfs_size), Some(format!(" ({display:.2})")))
                        } else {
                            (None, None)
                        };
                        files.push(LfsFileOutput {
                            path: entry.name.clone(),
                            oid,
                            full_oid,
                            marker: _type.to_string(),
                            size: size_value,
                            display_size,
                        });
                    }
                }
            }
            Ok(LfsOutput {
                action: "ls-files".to_string(),
                files,
                name_only,
                show_size: size,
                ..LfsOutput::default()
            })
        }
        // `git lfs install`/`uninstall` configure global smudge/clean filters and
        // pre-push hooks. Libra has built-in LFS (it matches `.libra_attributes`
        // directly and needs no filters), so these are intentional no-ops that
        // exit 0 — preventing automation that calls `git lfs install` from
        // breaking. See decision D5 in `docs/improvement/compatibility/declined.md`.
        LfsCmds::Install => Ok(LfsOutput {
            action: "install".to_string(),
            ..LfsOutput::default()
        }),
        LfsCmds::Uninstall => Ok(LfsOutput {
            action: "uninstall".to_string(),
            ..LfsOutput::default()
        }),
        // Implemented in later batches of `.omo/plans/lfs-improvement-plan.md`:
        // the CLI surface parses now so automation can target it, but the
        // behaviour is not yet available.
        LfsCmds::Fetch { remote, refs } => run_lfs_fetch(remote, refs, output).await,
        LfsCmds::Push { remote, refs } => run_lfs_push(remote, refs, output).await,
        LfsCmds::Prune { dry_run } => run_lfs_prune(dry_run).await,
        LfsCmds::Checkout { path } => run_lfs_checkout(path).await,
    }
}

/// Collects every LFS OID that must be kept locally: those referenced by any
/// reachable commit (branch tips, tags, HEAD, and reflog OIDs — the BFS handles
/// ancestry) plus those staged in the current index. Staging coverage prevents
/// `prune` from deleting an object the user has `add`ed but not yet committed.
async fn collect_reachable_lfs_oids() -> CliResult<HashSet<String>> {
    let mut roots: Vec<ObjectHash> = Vec::new();

    if let Some(head) = Head::current_commit().await {
        roots.push(head);
    }
    for branch in Branch::list_branches_best_effort(None).await {
        roots.push(branch.commit);
    }
    if let Ok(tags) = tag::list().await {
        for tag in tags {
            if let TagObject::Commit(commit) = &tag.object {
                roots.push(commit.id);
            }
        }
    }
    // Reflog OIDs keep objects reachable only through the reflog (e.g. after a
    // `reset`). Unparseable/zero OIDs are skipped here and unreadable commits are
    // tolerated by the scanner.
    let db = get_db_conn_instance().await;
    if let Ok(entries) = reflog::Entity::find().all(&db).await {
        for entry in entries {
            for raw in [entry.old_oid, entry.new_oid] {
                if let Ok(hash) = ObjectHash::from_str(&raw) {
                    roots.push(hash);
                }
            }
        }
    }

    let mut oids: HashSet<String> = scan_lfs_pointers(&roots)
        .await?
        .into_iter()
        .map(|pointer| pointer.oid)
        .collect();
    oids.extend(index_lfs_oids()?);
    oids.extend(worktree_lfs_oids());
    Ok(oids)
}

/// Returns the OIDs of LFS pointer blobs currently staged in the index.
fn index_lfs_oids() -> CliResult<HashSet<String>> {
    let index = Index::load(path::index())
        .map_err(|e| CliError::io(format!("failed to load index: {e}")))?;
    let storage = util::objects_storage();
    let mut oids = HashSet::new();
    for entry in index.tracked_entries(0) {
        let path_abs = util::workdir_to_absolute(&entry.name);
        if !lfs::is_lfs_tracked(&path_abs) {
            continue;
        }
        if let Ok(data) = storage.get(&entry.hash)
            && let Some((oid, _)) = lfs::parse_pointer_data(&data)
        {
            oids.insert(oid);
        }
    }
    Ok(oids)
}

fn worktree_lfs_oids() -> HashSet<String> {
    let root = util::working_dir();
    let mut oids = HashSet::new();
    let entries = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.depth() == 1
                && entry.file_type().is_dir()
                && entry.file_name().to_str() == Some(".libra"))
        });

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "skipping unreadable path while scanning worktree LFS pointers"
                );
                continue;
            }
        };
        if !entry.file_type().is_file() || !lfs::is_lfs_tracked(entry.path()) {
            continue;
        }
        if let Ok((oid, _)) = lfs::parse_pointer_file(entry.path()) {
            oids.insert(oid);
        }
    }
    oids
}

/// `libra lfs prune [-n/--dry-run]` — delete local LFS objects not referenced by
/// any reachable ref or the index. Malformed cache entries are skipped (never
/// deleted, never panicked on), and individual removal failures degrade to a
/// warning so one stuck file does not abort the sweep.
async fn run_lfs_prune(dry_run: bool) -> CliResult<LfsOutput> {
    let reachable = collect_reachable_lfs_oids().await?;
    let objects_dir = util::storage_path().join("lfs/objects");

    let mut pruned_files = Vec::new();
    let mut size_freed: u64 = 0;
    let mut stack = vec![objects_dir];
    while let Some(dir) = stack.pop() {
        let read_dir = match std::fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
                continue;
            }
            let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Only well-formed 64-char hex OIDs are candidates: skipping malformed
            // names avoids `lfs_object_path`'s slice panic and never deletes a
            // non-object file.
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                tracing::warn!(file = %entry_path.display(), "skipping malformed LFS cache entry");
                continue;
            }
            if reachable.contains(name) {
                continue;
            }
            let entry_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if dry_run {
                pruned_files.push(name.to_string());
                size_freed += entry_size;
            } else {
                match std::fs::remove_file(&entry_path) {
                    Ok(()) => {
                        pruned_files.push(name.to_string());
                        size_freed += entry_size;
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: failed to remove LFS object {}: {e}",
                            entry_path.display()
                        );
                    }
                }
            }
        }
    }
    pruned_files.sort();

    // Tidy up sharding directories (`objects/<a>/<b>/`) left empty by deletions.
    if !dry_run && !pruned_files.is_empty() {
        remove_empty_subdirs(&util::storage_path().join("lfs/objects"));
    }

    Ok(LfsOutput {
        action: "prune".to_string(),
        pruned_files,
        size_freed,
        dry_run,
        ..LfsOutput::default()
    })
}

/// Recursively removes empty subdirectories of `dir` (bottom-up), leaving `dir`
/// itself in place. Used to clean up empty LFS sharding directories after prune.
fn remove_empty_subdirs(dir: &Path) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_subdirs(&path);
            // Succeeds only when the directory is now empty.
            let _ = std::fs::remove_dir(&path);
        }
    }
}

/// `libra lfs checkout [<path>...]` — restore working-tree pointer files to their
/// full LFS object content from the local cache. Files already materialized (not
/// pointers) are skipped; a missing cache object leaves the pointer untouched
/// with a notice. The cache object's hash is verified before it overwrites the
/// working-tree file.
async fn run_lfs_checkout(paths: Vec<String>) -> CliResult<LfsOutput> {
    let filter: HashSet<String> = paths.into_iter().collect();
    let index = Index::load(path::index())
        .map_err(|e| CliError::io(format!("failed to load index: {e}")))?;

    let mut restored_paths = Vec::new();
    for entry in index.tracked_entries(0) {
        if !filter.is_empty() && !filter.contains(&entry.name) {
            continue;
        }
        let path_abs = util::workdir_to_absolute(&entry.name);
        if !lfs::is_lfs_tracked(&path_abs) {
            continue;
        }
        // Only restore files that are currently pointer files in the worktree.
        let Ok((oid, _size)) = lfs::parse_pointer_file(&path_abs) else {
            continue;
        };
        let cache_path = lfs::lfs_object_path(&oid);
        if !cache_path.exists() {
            eprintln!(
                "warning: LFS object {oid} for '{}' is not in the local cache; \
                 run `libra lfs fetch` first",
                entry.name
            );
            continue;
        }
        // Verify the cache entity matches its OID before overwriting the pointer.
        match lfs::calc_lfs_file_hash(&cache_path) {
            Ok(hash) if hash == oid => {
                std::fs::copy(&cache_path, &path_abs).map_err(|e| {
                    CliError::io(format!("failed to restore '{}': {e}", entry.name))
                })?;
                restored_paths.push(entry.name.clone());
            }
            _ => {
                return Err(CliError::io(format!(
                    "cached LFS object {oid} failed hash verification; refusing to restore '{}'",
                    entry.name
                ))
                .with_stable_code(StableErrorCode::RepoCorrupt));
            }
        }
    }
    restored_paths.sort();

    Ok(LfsOutput {
        action: "checkout".to_string(),
        restored_paths,
        ..LfsOutput::default()
    })
}

/// `libra lfs push [<remote>] [<ref>...]` — upload the LFS objects referenced by
/// the current branch to the remote.
///
/// Scope (lfs-improvement-plan Batch 1, option b): push always operates on the
/// **current HEAD branch**, because `LFSClient::push_objects` verifies locks
/// using the current refspec and index. Explicit refs that are not the current
/// branch are rejected rather than uploaded with a mismatched lock-verify
/// context.
async fn run_lfs_push(
    remote: Option<String>,
    refs: Vec<String>,
    output: &OutputConfig,
) -> CliResult<LfsOutput> {
    let current_branch = match Head::current().await {
        Head::Branch(name) => name,
        Head::Detached(_) => {
            return Err(CliError::fatal(
                "libra lfs push requires a branch checkout (HEAD is detached)",
            )
            .with_stable_code(StableErrorCode::RepoStateInvalid));
        }
    };

    // Only the current branch is supported (see scope note above).
    for name in &refs {
        let bare = name.strip_prefix("refs/heads/").unwrap_or(name);
        if bare != current_branch {
            return Err(CliError::fatal(format!(
                "libra lfs push currently supports only the current branch ('{current_branch}'); \
                 '{name}' is not the current branch"
            ))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
            .with_hint(
                "switch to the branch you want to push, then run `libra lfs push <remote>`",
            ));
        }
    }

    // Unborn branch (no commits): nothing to push, no network.
    let Some(head_commit) = Head::current_commit().await else {
        return Ok(LfsOutput {
            action: "push".to_string(),
            ..LfsOutput::default()
        });
    };

    let pointers = scan_lfs_pointers(&[head_commit]).await?;
    if pointers.is_empty() {
        return Ok(LfsOutput {
            action: "push".to_string(),
            ..LfsOutput::default()
        });
    }

    // Every referenced object must be present locally; load its pointer blob as
    // an `Entry` for `push_objects` (which re-parses, re-checks, and verifies
    // locks). A missing local object is a hard error — never a silent skip.
    let mut entries = Vec::new();
    for pointer in &pointers {
        if !lfs::lfs_object_path(&pointer.oid).exists() {
            return Err(CliError::fatal(format!(
                "LFS object {} is missing from the local cache; cannot push",
                pointer.oid
            ))
            .with_stable_code(StableErrorCode::RepoStateInvalid)
            .with_hint("run `libra lfs fetch` or restore the object before pushing."));
        }
        let blob = load_object::<Blob>(&pointer.blob_hash)
            .map_err(|e| CliError::io(format!("failed to load LFS pointer blob: {e}")))?;
        entries.push(Entry::from(blob));
    }

    let quiet = output.is_json();
    let client = lfs_client_for_remote(remote.as_deref()).await?;
    client.push_objects(&entries, quiet).await.map_err(|e| {
        CliError::network(e.to_string()).with_stable_code(StableErrorCode::NetworkUnavailable)
    })?;

    let pushed_oids = pointers.iter().map(|p| p.oid.clone()).collect();
    Ok(LfsOutput {
        action: "push".to_string(),
        pushed_oids,
        ..LfsOutput::default()
    })
}

/// A deduplicated LFS pointer discovered by the commit-graph scanner. `oid` and
/// `size` drive fetch; `blob_hash` (the pointer blob's object id) lets push load
/// the pointer `Entry` to hand to `LFSClient::push_objects`.
#[derive(Debug, Clone)]
struct ScannedPointer {
    oid: String,
    size: u64,
    blob_hash: ObjectHash,
}

/// Scans the commit graph reachable from `start_commits` (BFS) and returns the
/// deduplicated LFS pointers referenced by any reachable commit.
///
/// Each commit is judged with its **own** root `.libra_attributes` (parsed from
/// the blob in that commit's tree) rather than the working tree's cached
/// patterns, so historical LFS rules are honoured. Subtree objects are loaded to
/// enumerate paths, but only blobs on LFS-matched paths are read to parse their
/// pointer data — non-matching blob content is never loaded.
async fn scan_lfs_pointers(start_commits: &[ObjectHash]) -> CliResult<Vec<ScannedPointer>> {
    const ATTRIBUTES_NAME: &str = ".libra_attributes";

    let mut visited: HashSet<ObjectHash> = HashSet::new();
    let mut queue: VecDeque<ObjectHash> = start_commits.iter().copied().collect();
    let mut found: HashMap<String, ScannedPointer> = HashMap::new();

    while let Some(commit_id) = queue.pop_front() {
        if !visited.insert(commit_id) {
            continue;
        }
        // Tolerate unreadable roots (e.g. zero/old reflog OIDs passed by prune):
        // skip rather than abort the whole scan. A commit that loads is expected
        // to have a readable tree, so that stays strict below.
        let Ok(commit) = load_object::<Commit>(&commit_id) else {
            continue;
        };
        for parent in &commit.parent_commit_ids {
            if !visited.contains(parent) {
                queue.push_back(*parent);
            }
        }

        let tree = load_object::<Tree>(&commit.tree_id)
            .map_err(|e| CliError::io(format!("failed to load tree {}: {e}", commit.tree_id)))?;
        let items = tree.get_plain_items();

        // Read this commit's own root `.libra_attributes`, if present.
        let Some(attr_hash) = items
            .iter()
            .find_map(|(path, hash)| (path.as_os_str() == ATTRIBUTES_NAME).then_some(*hash))
        else {
            continue;
        };
        let attr_blob = load_object::<Blob>(&attr_hash)
            .map_err(|e| CliError::io(format!("failed to load {ATTRIBUTES_NAME}: {e}")))?;
        let patterns = lfs::parse_lfs_patterns_from_bytes(&attr_blob.data);
        if patterns.is_empty() {
            continue;
        }

        for (path, blob_hash) in &items {
            if path.as_os_str() == ATTRIBUTES_NAME {
                continue;
            }
            // Match historical paths structurally against this commit's patterns.
            let abs = util::working_dir().join(path);
            if !lfs::path_matches_lfs_patterns(&abs, &patterns) {
                continue;
            }
            let blob = load_object::<Blob>(blob_hash)
                .map_err(|e| CliError::io(format!("failed to load blob {blob_hash}: {e}")))?;
            if let Some((oid, size)) = lfs::parse_pointer_data(&blob.data) {
                found.entry(oid.clone()).or_insert(ScannedPointer {
                    oid,
                    size,
                    blob_hash: *blob_hash,
                });
            }
        }
    }

    let mut pointers: Vec<ScannedPointer> = found.into_values().collect();
    pointers.sort_by(|a, b| a.oid.cmp(&b.oid));
    Ok(pointers)
}

/// Builds an LFS client for an explicit remote, or the current branch upstream
/// when `remote` is `None`. Client construction reads config only (no network);
/// failures (e.g. unknown remote) map to a fatal network-class error.
async fn lfs_client_for_remote(remote: Option<&str>) -> CliResult<LFSClient> {
    let client = match remote {
        Some(remote) => LFSClient::new_from_remote(remote).await,
        None => LFSClient::new().await,
    };
    client.map_err(|e| {
        CliError::fatal(e.to_string()).with_stable_code(StableErrorCode::NetworkUnavailable)
    })
}

/// Resolves the start commits for a `fetch` from the requested refs.
///
/// With no refs, defaults to the current branch tip. For each given ref the
/// remote-tracking ref (`<remote>/<ref>`) is preferred, falling back to a local
/// ref of the same name (with a stderr note).
async fn resolve_fetch_commits(
    remote: Option<&str>,
    refs: &[String],
) -> CliResult<Vec<ObjectHash>> {
    if refs.is_empty() {
        return Ok(Head::current_commit().await.into_iter().collect());
    }

    let remote = remote.unwrap_or("origin");
    let mut commits = Vec::new();
    for name in refs {
        let tracking = format!("{remote}/{name}");
        match util::get_commit_base_typed(&tracking).await {
            Ok(hash) => commits.push(hash),
            Err(_) => match util::get_commit_base_typed(name).await {
                Ok(hash) => {
                    eprintln!(
                        "warning: no remote-tracking ref '{tracking}', scanning local '{name}'"
                    );
                    commits.push(hash);
                }
                Err(e) => {
                    return Err(
                        CliError::fatal(format!("could not resolve ref '{name}': {e}"))
                            .with_stable_code(StableErrorCode::CliInvalidTarget),
                    );
                }
            },
        }
    }
    Ok(commits)
}

/// `libra lfs fetch [<remote>] [<ref>...]` — download LFS objects referenced by
/// the requested (remote-tracking) refs that are missing from the local cache.
async fn run_lfs_fetch(
    remote: Option<String>,
    refs: Vec<String>,
    output: &OutputConfig,
) -> CliResult<LfsOutput> {
    let start_commits = resolve_fetch_commits(remote.as_deref(), &refs).await?;
    let pointers = scan_lfs_pointers(&start_commits).await?;

    // Only objects missing from the local cache need a download. If none are
    // missing, fetch is a pure no-op — no remote contact required.
    let missing: Vec<&ScannedPointer> = pointers
        .iter()
        .filter(|p| !lfs::lfs_object_path(&p.oid).exists())
        .collect();
    if missing.is_empty() {
        return Ok(LfsOutput {
            action: "fetch".to_string(),
            ..LfsOutput::default()
        });
    }

    // Suppress download progress on stdout when emitting structured output so
    // the JSON/machine envelope is the only thing on stdout.
    let quiet = output.is_json();
    let client = lfs_client_for_remote(remote.as_deref()).await?;

    let mut fetched_oids = Vec::new();
    for pointer in missing {
        let final_path = lfs::lfs_object_path(&pointer.oid);
        // `download_object` writes the file but does NOT create the two-level
        // sharding parent (`objects/<a>/<b>/`); create it first.
        let Some(parent) = final_path.parent() else {
            return Err(CliError::io(format!(
                "invalid LFS object path for oid {}",
                pointer.oid
            )));
        };
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CliError::io(format!("failed to create LFS object directory: {e}")))?;
        let tmp_path = parent.join(format!("{}.tmp", pointer.oid));

        // A transport/checksum failure must never leave a partial `.tmp` behind
        // or corrupt the OID path: clean up and surface a network error.
        if let Err(e) = client
            .download_object(&pointer.oid, pointer.size, &tmp_path, None, quiet)
            .await
        {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(CliError::network(format!(
                "failed to download LFS object {}: {e}",
                pointer.oid
            ))
            .with_stable_code(StableErrorCode::NetworkUnavailable));
        }

        // `download_object` returns Ok even when the remote 404s (it writes a
        // pointer to the target), so verify the entity hash independently before
        // promoting `.tmp` to the final OID path.
        match lfs::calc_lfs_file_hash(&tmp_path) {
            Ok(hash) if hash == pointer.oid => {
                tokio::fs::rename(&tmp_path, &final_path)
                    .await
                    .map_err(|e| {
                        CliError::io(format!("failed to store LFS object {}: {e}", pointer.oid))
                    })?;
                fetched_oids.push(pointer.oid.clone());
            }
            _ => {
                // Not a real entity (e.g. remote 404 wrote a pointer): drop the
                // temp file and leave the object reported as still missing.
                let _ = tokio::fs::remove_file(&tmp_path).await;
                if !quiet {
                    eprintln!(
                        "warning: LFS object {} unavailable on the remote; skipped",
                        pointer.oid
                    );
                }
            }
        }
    }

    Ok(LfsOutput {
        action: "fetch".to_string(),
        fetched_oids,
        ..LfsOutput::default()
    })
}

fn render_lfs_output(result: &LfsOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("lfs", result, output);
    }
    if output.quiet {
        return Ok(());
    }

    match result.action.as_str() {
        "track" => {
            // Same silent-empty UX class as the `track-list` and `locks`
            // fixes (v0.17.1065 / v0.17.1066): if every requested pattern
            // was already tracked, `add_lfs_patterns` returns an empty
            // `added` Vec and the user previously saw zero output. Emit
            // a confirmed-already-tracked notice so the command never
            // looks like a hang.
            if result.patterns.is_empty() {
                println!("No new patterns added (already tracked)");
            } else {
                for pattern in &result.patterns {
                    println!("Tracking \"{pattern}\"");
                }
            }
        }
        "track-list" => {
            // Always print the header so `libra lfs track` (list mode) is
            // never silent — pre-v0.17.1065 an empty pattern list rendered
            // nothing at all and the user couldn't tell whether the command
            // ran or hung. Matches `git lfs track`'s behavior on an empty
            // repo (header + no rows).
            println!("Listing tracked patterns");
            for pattern in &result.patterns {
                println!("    {} ({})", pattern, util::ATTRIBUTES);
            }
        }
        "untrack" => {
            // Same silent-empty fix: if the file had no matching LFS
            // patterns for the user-supplied args, we previously
            // printed nothing. Emit a confirmed-no-op notice.
            if result.patterns.is_empty() {
                println!("No matching LFS patterns to untrack");
            } else {
                for pattern in &result.patterns {
                    println!("Untracking \"{pattern}\"");
                }
            }
        }
        "locks" => {
            // Same UX class as the `track-list` empty fix in v0.17.1065:
            // an empty lock list previously printed nothing, leaving the
            // user unable to distinguish "no locks held" from "command
            // hung" or "wrong subcommand". Emit a confirmed-empty notice
            // so the success signal is always visible.
            if result.locks.is_empty() {
                println!("No locks on the current branch");
            } else {
                let max_path_len = result
                    .locks
                    .iter()
                    .map(|lock| lock.path.len())
                    .max()
                    .unwrap_or(0);
                for lock in &result.locks {
                    println!(
                        "{:<path_width$}\tID:{}",
                        lock.path,
                        lock.id,
                        path_width = max_path_len
                    );
                }
            }
        }
        "install" => {
            println!(
                "Libra uses native built-in LFS support; no global filter installation is needed."
            );
        }
        "uninstall" => {
            println!("Libra LFS uses built-in support; nothing to uninstall (no-op).");
        }
        "fetch" => {
            if result.fetched_oids.is_empty() {
                println!("No missing LFS objects to fetch");
            } else {
                println!("Fetched {} LFS object(s)", result.fetched_oids.len());
            }
        }
        "push" => {
            if result.pushed_oids.is_empty() {
                println!("No LFS objects to push");
            } else {
                println!("Pushed {} LFS object(s)", result.pushed_oids.len());
            }
        }
        "prune" => {
            let verb = if result.dry_run {
                "Would prune"
            } else {
                "Pruned"
            };
            let size = util::auto_unit_bytes(result.size_freed);
            println!("{verb} {} files ({size:.2})", result.pruned_files.len());
        }
        "checkout" => {
            if result.restored_paths.is_empty() {
                println!("No LFS pointer files to restore");
            } else {
                println!("Restored {} LFS file(s)", result.restored_paths.len());
            }
        }
        "lock" => {
            if let Some(path) = &result.path {
                println!("Locked {path}");
            }
        }
        "unlock" => {
            if let Some(path) = &result.path {
                println!("Unlocked {path}");
            }
        }
        "ls-files" => {
            // Same silent-empty fix: a repo with no LFS-tracked files
            // previously rendered zero stdout. `--name-only` consumers
            // (e.g. shell pipelines) intentionally expect bare output,
            // so the notice is gated on the not-name-only path.
            if result.files.is_empty() {
                if !result.name_only {
                    println!("No LFS files in the working tree");
                }
            } else {
                for file in &result.files {
                    let tail = file.display_size.as_deref().unwrap_or("");
                    if result.name_only {
                        println!("{}{}", file.path, tail);
                    } else {
                        println!("{} {} {}{}", file.oid, file.marker, file.path, tail);
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

pub(crate) async fn current_refspec() -> Option<String> {
    match Head::current().await {
        Head::Branch(name) => Some(format!("refs/heads/{name}")),
        // Return None silently — every caller wraps the None branch in
        // a typed error (`current_refspec_or_err` → CliError, the
        // `lfs_client.rs` `push_objects` site → LfsPushError). Pre-fix
        // we also `emit_legacy_stderr("fatal: HEAD is detached")` here,
        // which doubled the error envelope on stderr (legacy line +
        // typed-error envelope from the caller), confusing `--json` /
        // `--machine` consumers.
        Head::Detached(_) => None,
    }
}

async fn current_refspec_or_err() -> CliResult<String> {
    current_refspec().await.ok_or_else(|| {
        CliError::fatal("HEAD is detached").with_stable_code(StableErrorCode::RepoStateInvalid)
    })
}

fn map_lock_list_error(error: LockListError) -> CliError {
    match error {
        LockListError::Request(detail) => {
            CliError::network(format!("failed to query LFS locks: {detail}"))
        }
        LockListError::Http { status, message } => {
            if status == StatusCode::FORBIDDEN {
                CliError::fatal("You must have push access to list locks")
                    .with_stable_code(StableErrorCode::AuthPermissionDenied)
            } else {
                CliError::network(format!(
                    "LFS get locks failed with status {}",
                    status.as_u16()
                ))
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_detail("status", status.as_u16())
                .with_detail("body", message)
            }
        }
        LockListError::Decode(detail) => {
            CliError::network(format!("failed to decode LFS locks response: {detail}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
        }
    }
}

/// temp
fn convert_patterns_to_workdir(patterns: Vec<String>) -> Vec<String> {
    patterns
        .into_iter()
        .map(|p| util::to_workdir_path(&p).to_string_or_panic())
        .collect()
}

fn add_lfs_patterns(file_path: &str, patterns: Vec<String>) -> io::Result<Vec<String>> {
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(file_path)?;

    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;

        let mut last_byte = [0; 1];
        file.read_exact(&mut last_byte)?;

        // ensure the last byte is '\n'
        if last_byte[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }

    let lfs_patterns = lfs::extract_lfs_patterns(file_path)?;
    let mut added: Vec<String> = Vec::new();
    for pattern in patterns {
        if lfs_patterns.contains(&pattern) || added.contains(&pattern) {
            continue;
        }
        added.push(pattern.clone());
        let pattern = format!(
            "{} filter=lfs diff=lfs merge=lfs -text\n",
            pattern.replace(" ", r"\ ")
        );
        file.write_all(pattern.as_bytes())?;
    }

    Ok(added)
}

fn untrack_lfs_patterns(file_path: &str, patterns: Vec<String>) -> io::Result<Vec<String>> {
    if !Path::new(file_path).exists() {
        return Ok(Vec::new());
    }
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);

    let mut lines: Vec<String> = Vec::new();
    let mut removed = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let mut matched_pattern = None;
        // delete the specified lfs patterns. We compare against the
        // on-disk (escaped-space) form, but record the *original* input
        // pattern in `removed` so the return value is symmetric with
        // `add_lfs_patterns` (both surface the un-escaped user-facing
        // form).
        for pattern in &patterns {
            let escaped = pattern.replace(" ", r"\ ");
            if line.trim_start().starts_with(&escaped) && line.contains("filter=lfs") {
                matched_pattern = Some(pattern.clone());
                break;
            }
        }
        match matched_pattern {
            Some(pattern) => removed.push(pattern),
            None => lines.push(line),
        }
    }

    // clear the file
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(file_path)?;

    for line in lines {
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_lock_list_error_forbidden_maps_to_auth_permission_denied() {
        let err = map_lock_list_error(LockListError::Http {
            status: StatusCode::FORBIDDEN,
            message: "forbidden".to_string(),
        });
        assert_eq!(err.stable_code(), StableErrorCode::AuthPermissionDenied);
        assert!(err.message().contains("push access"));
    }

    #[test]
    fn map_lock_list_error_decode_maps_to_network_protocol() {
        let err = map_lock_list_error(LockListError::Decode("invalid json".to_string()));
        assert_eq!(err.stable_code(), StableErrorCode::NetworkProtocol);
        assert!(err.message().contains("decode"));
    }

    #[test]
    fn map_lock_list_error_http_maps_status_and_body_detail() {
        let err = map_lock_list_error(LockListError::Http {
            status: StatusCode::BAD_GATEWAY,
            message: "upstream unavailable".to_string(),
        });
        assert_eq!(err.stable_code(), StableErrorCode::NetworkProtocol);
        assert_eq!(err.details().get("status"), Some(&serde_json::json!(502)));
        assert_eq!(
            err.details().get("body"),
            Some(&serde_json::json!("upstream unavailable"))
        );
    }

    #[test]
    fn add_lfs_patterns_deduplicates_within_a_single_call() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_string_lossy().into_owned();
        let added = add_lfs_patterns(
            &path,
            vec![
                "*.png".to_string(),
                "*.png".to_string(),
                "*.jpg".to_string(),
            ],
        )
        .expect("add_lfs_patterns");
        assert_eq!(added, vec!["*.png".to_string(), "*.jpg".to_string()]);

        let on_disk = lfs::extract_lfs_patterns(&path).expect("extract");
        assert_eq!(on_disk, vec!["*.png".to_string(), "*.jpg".to_string()]);
    }

    #[test]
    fn untrack_lfs_patterns_returns_unescaped_form_symmetric_with_add() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_string_lossy().into_owned();

        // Track a pattern with an internal space; on-disk form is escaped.
        let added =
            add_lfs_patterns(&path, vec!["my dir/*.png".to_string()]).expect("add_lfs_patterns");
        assert_eq!(added, vec!["my dir/*.png".to_string()]);

        // Untrack with the un-escaped user-facing form. The return value
        // must match the input, not the on-disk escaped form, so that
        // `LfsOutput.patterns` from track and untrack is symmetric.
        let removed = untrack_lfs_patterns(&path, vec!["my dir/*.png".to_string()])
            .expect("untrack_lfs_patterns");
        assert_eq!(removed, vec!["my dir/*.png".to_string()]);

        let on_disk = lfs::extract_lfs_patterns(&path).expect("extract");
        assert!(on_disk.is_empty(), "expected empty, got {on_disk:?}");
    }

    /// Thin clap wrapper so the `LfsCmds` subcommand can be parsed in isolation
    /// to verify positional-argument bindings (remote / refs / path / flags).
    #[derive(clap::Parser, Debug)]
    struct LfsCmdWrapper {
        #[command(subcommand)]
        cmd: LfsCmds,
    }

    fn parse_lfs(args: &[&str]) -> LfsCmds {
        use clap::Parser;
        let mut full = vec!["lfs"];
        full.extend_from_slice(args);
        LfsCmdWrapper::try_parse_from(full)
            .expect("LfsCmds should parse")
            .cmd
    }

    #[test]
    fn test_lfs_push_parses_no_args() {
        match parse_lfs(&["push"]) {
            LfsCmds::Push { remote, refs } => {
                assert_eq!(remote, None);
                assert!(refs.is_empty());
            }
            other => panic!("expected Push, got {other:?}"),
        }
    }

    #[test]
    fn test_lfs_push_parses_remote_and_refs() {
        // First positional binds to `remote`; the rest are `refs`.
        match parse_lfs(&["push", "origin", "main"]) {
            LfsCmds::Push { remote, refs } => {
                assert_eq!(remote.as_deref(), Some("origin"));
                assert_eq!(refs, vec!["main".to_string()]);
            }
            other => panic!("expected Push, got {other:?}"),
        }
        match parse_lfs(&["push", "origin", "main", "feature"]) {
            LfsCmds::Push { remote, refs } => {
                assert_eq!(remote.as_deref(), Some("origin"));
                assert_eq!(refs, vec!["main".to_string(), "feature".to_string()]);
            }
            other => panic!("expected Push, got {other:?}"),
        }
        // A lone positional is the remote, NOT a ref.
        match parse_lfs(&["push", "main"]) {
            LfsCmds::Push { remote, refs } => {
                assert_eq!(remote.as_deref(), Some("main"));
                assert!(refs.is_empty());
            }
            other => panic!("expected Push, got {other:?}"),
        }
    }

    #[test]
    fn test_lfs_fetch_parses_remote_and_refs() {
        match parse_lfs(&["fetch"]) {
            LfsCmds::Fetch { remote, refs } => {
                assert_eq!(remote, None);
                assert!(refs.is_empty());
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
        match parse_lfs(&["fetch", "origin", "main"]) {
            LfsCmds::Fetch { remote, refs } => {
                assert_eq!(remote.as_deref(), Some("origin"));
                assert_eq!(refs, vec!["main".to_string()]);
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    #[test]
    fn test_lfs_prune_parses() {
        match parse_lfs(&["prune"]) {
            LfsCmds::Prune { dry_run } => assert!(!dry_run),
            other => panic!("expected Prune, got {other:?}"),
        }
        match parse_lfs(&["prune", "--dry-run"]) {
            LfsCmds::Prune { dry_run } => assert!(dry_run),
            other => panic!("expected Prune, got {other:?}"),
        }
        match parse_lfs(&["prune", "-n"]) {
            LfsCmds::Prune { dry_run } => assert!(dry_run),
            other => panic!("expected Prune, got {other:?}"),
        }
    }

    #[test]
    fn test_lfs_checkout_parses() {
        match parse_lfs(&["checkout"]) {
            LfsCmds::Checkout { path } => assert!(path.is_empty()),
            other => panic!("expected Checkout, got {other:?}"),
        }
        match parse_lfs(&["checkout", "a/b.bin", "c.psd"]) {
            LfsCmds::Checkout { path } => {
                assert_eq!(path, vec!["a/b.bin".to_string(), "c.psd".to_string()]);
            }
            other => panic!("expected Checkout, got {other:?}"),
        }
    }

    #[test]
    fn test_lfs_install_uninstall_parse() {
        assert!(matches!(parse_lfs(&["install"]), LfsCmds::Install));
        assert!(matches!(parse_lfs(&["uninstall"]), LfsCmds::Uninstall));
    }
}
