//! LFS subcommands for authentication, batch negotiation, lock management, and integrating media storage with standard workflows.

use std::{
    fs::{File, OpenOptions},
    io,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
};

use clap::Subcommand;
use git_internal::internal::index::Index;
use reqwest::StatusCode;

use crate::{
    command::{
        lfs_schema::{LfsFileOutput, LfsOutput},
        status,
    },
    internal::{
        head::Head,
        protocol::lfs_client::{LFSClient, LockListError},
    },
    lfs_structs::LockListQuery,
    utils::{
        error::{CliError, CliResult, StableErrorCode, emit_legacy_stderr},
        lfs,
        output::{OutputConfig, emit_json_data},
        path,
        path_ext::PathExt,
        util,
    },
};

/// `--help` examples shown in `libra lfs --help` output (attached in
/// `src/cli.rs` via `after_help` on the `Lfs` subcommand).
///
/// `lfs` exposes six sub-commands: `track` (read/add attributes patterns),
/// `untrack`, `ls-files`, and the three lock-server flows (`locks`,
/// `lock`, `unlock`). The banner pins the canonical invocation per
/// sub-command plus a JSON variant so users can map intent to invocation
/// without reading the design doc. Cross-cutting `--help` EXAMPLES
/// rollout per `docs/improvement/README.md` item B.
pub const LFS_EXAMPLES: &str = "\
EXAMPLES:
    libra lfs track                       List currently tracked LFS attribute patterns
    libra lfs track '*.bin' '*.psd'       Add LFS patterns to .libraattributes
    libra lfs untrack '*.bin'             Remove an LFS pattern
    libra lfs ls-files                    List LFS-tracked files in the working tree
    libra lfs ls-files --long --size      Show full OIDs and sizes
    libra lfs locks                       List remote locks for the current branch
    libra lfs lock build/output.bin       Acquire a remote lock on a file
    libra lfs unlock build/output.bin     Release a lock you own
    libra lfs unlock --force --id <id>    Force-release a lock owned by someone else
    libra lfs --json ls-files             Structured JSON output for agents";

/// [Docs](https://github.com/git-lfs/git-lfs/tree/main/docs/man)
#[derive(Subcommand, Debug)]
pub enum LfsCmds {
    /// View or add LFS paths to Libra Attributes (root)
    Track { pattern: Option<Vec<String>> },
    /// Remove LFS paths from Libra Attributes
    Untrack { path: Vec<String> },
    /// Lists currently locked files from the Libra LFS server. (Current Branch)
    Locks {
        #[clap(long, short)]
        id: Option<String>,
        #[clap(long, short)]
        path: Option<String>,
        #[clap(long, short)]
        limit: Option<u64>,
    },
    /// Set a file as "locked" on the Libra LFS server
    Lock {
        /// String path name of the locked file. This should be relative to the root of the repository working directory
        path: String,
    },
    /// Remove "locked" setting for a file on the Libra LFS server
    Unlock {
        path: String,
        #[clap(long, short)]
        force: bool,
        #[clap(long, short)]
        id: Option<String>,
    },
    /// Show information about Git LFS files in the index and working tree (current branch)
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
}

pub async fn execute(cmd: LfsCmds) -> CliResult<()> {
    execute_safe(cmd, &OutputConfig::default()).await
}

pub async fn execute_safe(cmd: LfsCmds, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    let result = run_lfs(cmd).await?;
    render_lfs_output(&result, output)
}

async fn run_lfs(cmd: LfsCmds) -> CliResult<LfsOutput> {
    // TODO: attributes file should be created in current dir, NOT root dir
    let attr_path = path::attributes().to_string_or_panic();
    match cmd {
        LfsCmds::Track { pattern } => {
            // TODO: deduplicate
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
                .await;
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
            if !force {
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
                .await;
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
    }
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
            for pattern in &result.patterns {
                println!("Tracking \"{pattern}\"");
            }
        }
        "track-list" if !result.patterns.is_empty() => {
            println!("Listing tracked patterns");
            for pattern in &result.patterns {
                println!("    {} ({})", pattern, util::ATTRIBUTES);
            }
        }
        "untrack" => {
            for pattern in &result.patterns {
                println!("Untracking \"{pattern}\"");
            }
        }
        "locks" => {
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
            for file in &result.files {
                let tail = file.display_size.as_deref().unwrap_or("");
                if result.name_only {
                    println!("{}{}", file.path, tail);
                } else {
                    println!("{} {} {}{}", file.oid, file.marker, file.path, tail);
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
        Head::Detached(_) => {
            emit_legacy_stderr("fatal: HEAD is detached");
            None
        }
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
    let mut added = Vec::new();
    for pattern in patterns {
        if lfs_patterns.contains(&pattern) {
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
        // delete the specified lfs patterns
        for pattern in &patterns {
            let pattern = pattern.replace(" ", r"\ ");
            if line.trim_start().starts_with(&pattern) && line.contains("filter=lfs") {
                matched_pattern = Some(pattern);
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
}
