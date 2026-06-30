//! `libra rerere` — REuse REcorded REsolution. Records how a merge conflict was
//! resolved and replays that resolution when the identical conflict reappears.
//!
//! Storage lives under `.libra/rerere/`:
//! - `<id>/preimage`  — the conflicted file content (with markers) as first seen
//! - `<id>/postimage` — the resolved content once the user fixes it
//! - `MERGE_RR`       — `id<TAB>path` lines for conflicts currently being tracked
//!
//! `<id>` is the SHA-256 of the conflicted file's bytes. This version matches a
//! conflict only when the whole conflicted file is byte-identical to a recorded
//! preimage (Git's per-hunk normalisation / ours-theirs-swap independence and
//! the automatic merge/rebase/cherry-pick integration are a documented Phase B
//! follow-up).

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use git_internal::internal::index::Index;
use sha2::{Digest, Sha256};

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
    path, util,
};

const CONFLICT_START: &str = "<<<<<<<";
const CONFLICT_SEP: &str = "=======";
const CONFLICT_END: &str = ">>>>>>>";

pub const RERERE_EXAMPLES: &str = "\
EXAMPLES:
    libra rerere                  Record preimages / replay resolutions for current conflicts
    libra rerere status           List the conflicts being tracked
    libra rerere diff             Show what changed since each preimage was recorded
    libra rerere forget <path>    Drop the recorded resolution for a path
    libra rerere clear            Stop tracking the current conflicts
    libra rerere gc               Prune old recorded resolutions";

/// Reuse recorded conflict resolutions.
#[derive(Parser, Debug)]
#[command(after_help = RERERE_EXAMPLES)]
pub struct RerereArgs {
    #[command(subcommand)]
    pub command: Option<RerereSubcommand>,
}

#[derive(Subcommand, Debug)]
pub enum RerereSubcommand {
    /// List the paths whose conflicts are currently being tracked.
    Status,
    /// Show the diff between each recorded preimage and the current file.
    Diff,
    /// Drop the recorded resolution(s) for the given paths.
    Forget {
        #[clap(value_name = "PATHSPEC", required = true)]
        paths: Vec<String>,
    },
    /// Stop tracking the current conflicts (keeps recorded resolutions).
    Clear,
    /// Prune recorded resolutions older than the configured thresholds.
    Gc,
}

pub async fn execute(args: RerereArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

pub async fn execute_safe(args: RerereArgs, _output: &OutputConfig) -> CliResult<()> {
    let rr_dir = rerere_dir()?;
    match args.command {
        None => update(&rr_dir),
        Some(RerereSubcommand::Status) => status(&rr_dir),
        Some(RerereSubcommand::Diff) => diff(&rr_dir),
        Some(RerereSubcommand::Forget { paths }) => forget(&rr_dir, &paths),
        Some(RerereSubcommand::Clear) => clear(&rr_dir),
        Some(RerereSubcommand::Gc) => gc(&rr_dir),
    }
}

/// The default action: for every tracked file that currently contains conflict
/// markers, record its preimage (or replay a known resolution); for every
/// tracked conflict that has since been resolved, record its postimage.
fn update(rr_dir: &Path) -> CliResult<()> {
    let workdir = util::working_dir();
    let index = load_index()?;
    let mut merge_rr = read_merge_rr(rr_dir)?;

    // 1. Record postimages for previously-tracked conflicts that are now resolved.
    let mut resolved_paths = Vec::new();
    for (path, id) in &merge_rr {
        let content = read_or_empty(&workdir.join(path))?;
        // An empty read means the file is gone or genuinely empty; either way it
        // is no longer a conflict, but we only record a non-empty resolution.
        if !content.is_empty() && !is_conflicted(&content) {
            write_entry(rr_dir, id, "postimage", &content)?;
            println!("Recorded resolution for '{path}'.");
            resolved_paths.push(path.clone());
        }
    }
    merge_rr.retain(|(path, _)| !resolved_paths.contains(path));

    // 2. Visit each tracked file that currently has conflict markers.
    for tracked in index.tracked_files() {
        let Some(path) = tracked.to_str() else {
            continue;
        };
        let absolute = workdir.join(path);
        let Ok(content) = fs::read(&absolute) else {
            continue;
        };
        if !is_conflicted(&content) {
            continue;
        }
        let id = conflict_id(&content);
        let postimage = entry_path(rr_dir, &id, "postimage");
        // Replay only when BOTH the recorded preimage and postimage exist — a
        // defensive guard so a stray postimage can never overwrite a file.
        if postimage.exists() && entry_path(rr_dir, &id, "preimage").exists() {
            let resolution = fs::read(&postimage).map_err(read_err)?;
            fs::write(&absolute, &resolution).map_err(write_err)?;
            println!("Resolved '{path}' using a previously recorded resolution.");
        } else {
            write_entry(rr_dir, &id, "preimage", &content)?;
            if !merge_rr.iter().any(|(p, _)| p == path) {
                merge_rr.push((path.to_string(), id));
            }
            println!("Recorded preimage for '{path}'.");
        }
    }

    write_merge_rr(rr_dir, &merge_rr)
}

fn status(rr_dir: &Path) -> CliResult<()> {
    for (path, _) in read_merge_rr(rr_dir)? {
        println!("{path}");
    }
    Ok(())
}

fn diff(rr_dir: &Path) -> CliResult<()> {
    let workdir = util::working_dir();
    for (path, id) in read_merge_rr(rr_dir)? {
        let Ok(preimage) = fs::read_to_string(entry_path(rr_dir, &id, "preimage")) else {
            continue;
        };
        let current_bytes = read_or_empty(&workdir.join(&path))?;
        let current = String::from_utf8_lossy(&current_bytes);
        let patch = diffy::create_patch(&preimage, &current);
        println!("* {path}");
        print!("{patch}");
    }
    Ok(())
}

fn forget(rr_dir: &Path, paths: &[String]) -> CliResult<()> {
    let mut removed = false;
    let mut kept = Vec::new();
    for (path, id) in read_merge_rr(rr_dir)? {
        if paths.iter().any(|p| p == &path) {
            remove_dir_all_ok(&rr_dir.join(&id))?;
            removed = true;
        } else {
            kept.push((path, id));
        }
    }
    write_merge_rr(rr_dir, &kept)?;
    if !removed {
        return Err(CliError::command_usage(format!(
            "no recorded resolution for: {}",
            paths.join(", ")
        ))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::CliInvalidTarget));
    }
    Ok(())
}

fn clear(rr_dir: &Path) -> CliResult<()> {
    let merge_rr = rr_dir.join("MERGE_RR");
    if merge_rr.exists() {
        fs::remove_file(&merge_rr).map_err(write_err)?;
    }
    Ok(())
}

/// Prune cache entries: a resolved entry (has a postimage) is kept for
/// `gc.rerereResolved` days, an unresolved one (preimage only) for
/// `gc.rerereUnresolved` days. Defaults: 60 / 15 days. Time is taken from the
/// preimage file's modification time.
fn gc(rr_dir: &Path) -> CliResult<()> {
    const RESOLVED_TTL_SECS: u64 = 60 * 24 * 60 * 60;
    const UNRESOLVED_TTL_SECS: u64 = 15 * 24 * 60 * 60;

    let now = std::time::SystemTime::now();
    let entries = match fs::read_dir(rr_dir) {
        Ok(entries) => entries,
        // No cache directory yet → nothing to prune.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(read_err(error)),
    };
    for entry in entries {
        let dir = entry.map_err(read_err)?.path();
        if !dir.is_dir() {
            continue;
        }
        let resolved = dir.join("postimage").exists();
        let ttl = if resolved {
            RESOLVED_TTL_SECS
        } else {
            UNRESOLVED_TTL_SECS
        };
        // Age the entry from the relevant file's mtime; a missing file just
        // skips it, but an unexpected stat error surfaces.
        let reference = if resolved {
            dir.join("postimage")
        } else {
            dir.join("preimage")
        };
        let mtime = match reference.metadata().and_then(|m| m.modified()) {
            Ok(mtime) => mtime,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(read_err(error)),
        };
        // A future mtime (clock skew) counts as age 0 — i.e. fresh, not pruned.
        let age = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
        if age > ttl {
            remove_dir_all_ok(&dir)?;
        }
    }
    Ok(())
}

// ── helpers ──

/// Whether `content` contains a conflict marker.
fn is_conflicted(content: &[u8]) -> bool {
    content
        .split(|&b| b == b'\n')
        .any(|line| starts_with(line, CONFLICT_START))
        && content
            .split(|&b| b == b'\n')
            .any(|line| starts_with(line, CONFLICT_SEP) || starts_with(line, CONFLICT_END))
}

fn starts_with(line: &[u8], prefix: &str) -> bool {
    line.starts_with(prefix.as_bytes())
}

/// The cache id for a conflicted file: the SHA-256 of its bytes.
fn conflict_id(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

fn entry_path(rr_dir: &Path, id: &str, name: &str) -> PathBuf {
    rr_dir.join(id).join(name)
}

fn write_entry(rr_dir: &Path, id: &str, name: &str, content: &[u8]) -> CliResult<()> {
    let dir = rr_dir.join(id);
    fs::create_dir_all(&dir).map_err(write_err)?;
    fs::write(dir.join(name), content).map_err(write_err)
}

fn read_merge_rr(rr_dir: &Path) -> CliResult<Vec<(String, String)>> {
    let merge_rr = rr_dir.join("MERGE_RR");
    let text = match fs::read_to_string(&merge_rr) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_err(error)),
    };
    let mut entries = Vec::new();
    for line in text.lines() {
        if let Some((id, path)) = line.split_once('\t') {
            // Only trust a well-formed SHA-256 hex id — a corrupted or injected
            // id (e.g. `../..`) must never reach a filesystem path join.
            if is_valid_id(id) {
                entries.push((path.to_string(), id.to_string()));
            }
        }
    }
    Ok(entries)
}

/// A cache id is exactly a 64-character lowercase SHA-256 hex string (the form
/// `hex::encode` produces); anything else is rejected so a corrupted or injected
/// id can never reach a filesystem path join.
fn is_valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Remove a cache directory, treating "already gone" as success and surfacing
/// any other I/O error.
fn remove_dir_all_ok(dir: &Path) -> CliResult<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(write_err(error)),
    }
}

/// Read a possibly-absent file: missing → empty, other error → fatal.
fn read_or_empty(path: &Path) -> CliResult<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(read_err(error)),
    }
}

fn write_merge_rr(rr_dir: &Path, entries: &[(String, String)]) -> CliResult<()> {
    fs::create_dir_all(rr_dir).map_err(write_err)?;
    let body: String = entries
        .iter()
        .map(|(path, id)| format!("{id}\t{path}\n"))
        .collect();
    fs::write(rr_dir.join("MERGE_RR"), body).map_err(write_err)
}

fn rerere_dir() -> CliResult<PathBuf> {
    let storage = util::try_get_storage_path(None).map_err(|_| CliError::repo_not_found())?;
    Ok(storage.join("rerere"))
}

fn load_index() -> CliResult<Index> {
    Index::load(path::index()).map_err(|error| {
        CliError::fatal(format!("failed to load index: {error}"))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    })
}

fn read_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("rerere: read error: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoReadFailed)
}

fn write_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("rerere: write error: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoWriteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_conflict_markers() {
        let conflicted = b"a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> other\nd\n";
        assert!(is_conflicted(conflicted));
        assert!(!is_conflicted(b"a\nb\nc\n"));
        // A lone marker without a separator is not a conflict.
        assert!(!is_conflicted(b"<<<<<<< only\n"));
    }

    #[test]
    fn conflict_id_is_stable_and_content_addressed() {
        let a = conflict_id(b"<<<<<<<\nx\n=======\ny\n>>>>>>>\n");
        let b = conflict_id(b"<<<<<<<\nx\n=======\ny\n>>>>>>>\n");
        let c = conflict_id(b"<<<<<<<\nx\n=======\nz\n>>>>>>>\n");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }
}
