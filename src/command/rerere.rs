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
//! preimage (Git's per-hunk normalisation / ours-theirs-swap independence remain
//! a documented follow-up).
//!
//! When `rerere.enabled` is set, [`auto_update`] is invoked automatically by the
//! merge / rebase / cherry-pick sequencers (at both conflict and resolution
//! time) so preimages are recorded, known resolutions replayed, and postimages
//! recorded without a manual `libra rerere`. With `rerere.enabled` unset
//! (the default) those hooks are complete no-ops.

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use git_internal::internal::index::Index;
use sha2::{Digest, Sha256};

use crate::{
    internal::config::ConfigKv,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        path, util,
    },
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
        // The bare `libra rerere` never auto-stages replayed resolutions — that
        // is what `rerere.autoUpdate` / `--rerere-autoupdate` control, and they
        // only apply to the automatic merge/rebase/cherry-pick integration.
        None => apply(&rr_dir, false).await,
        Some(RerereSubcommand::Status) => status(&rr_dir),
        Some(RerereSubcommand::Diff) => diff(&rr_dir),
        Some(RerereSubcommand::Forget { paths }) => forget(&rr_dir, &paths),
        Some(RerereSubcommand::Clear) => clear(&rr_dir),
        Some(RerereSubcommand::Gc) => gc(&rr_dir),
    }
}

/// Whether reuse-recorded-resolution is turned on for this repository
/// (`rerere.enabled`, default off). The automatic merge/rebase/cherry-pick
/// integration is a no-op unless this returns `true`, so leaving the config
/// unset keeps those commands' behaviour byte-for-byte unchanged.
pub(crate) async fn is_enabled() -> bool {
    matches!(
        ConfigKv::get("rerere.enabled")
            .await
            .ok()
            .flatten()
            .map(|entry| entry.value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("true" | "1" | "yes" | "on")
    )
}

/// Whether replayed resolutions should also be staged (`rerere.autoUpdate`,
/// default off). The per-command `--rerere-autoupdate` flag ORs with this.
async fn autoupdate_configured() -> bool {
    matches!(
        ConfigKv::get("rerere.autoUpdate")
            .await
            .ok()
            .flatten()
            .map(|entry| entry.value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("true" | "1" | "yes" | "on")
    )
}

/// Automatic hook for the merge/rebase/cherry-pick sequencers.
///
/// A no-op unless `rerere.enabled` is set. When enabled it runs the same
/// record/replay pass as `libra rerere`, so calling it at both the moment a
/// conflict is written and the moment it is resolved (or `--continue`d)
/// records preimages, replays known resolutions, and records postimages —
/// whichever applies to the current working-tree state. `auto_update` (the
/// command's `--rerere-autoupdate` flag) ORed with `rerere.autoUpdate` decides
/// whether a replayed file is also staged. Errors are surfaced to the caller,
/// which should treat them as non-fatal to the underlying operation.
pub(crate) async fn auto_update(auto_update: bool) -> CliResult<()> {
    if !is_enabled().await {
        return Ok(());
    }
    let rr_dir = rerere_dir()?;
    let stage_replayed = auto_update || autoupdate_configured().await;
    apply(&rr_dir, stage_replayed).await
}

/// The default action: for every tracked file that currently contains conflict
/// markers, record its preimage (or replay a known resolution); for every
/// tracked conflict that has since been resolved, record its postimage. When
/// `stage_replayed` is set, a file resolved by replay is also staged.
async fn apply(rr_dir: &Path, stage_replayed: bool) -> CliResult<()> {
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

    // 2. Visit each tracked file that currently has conflict markers. A conflict
    // lives at index stages 1-3 (there is no stage-0 entry for it), so gather
    // distinct paths across every stage — iterating only stage 0 (as
    // `tracked_files()` does) would miss exactly the conflicted files that the
    // merge/rebase/cherry-pick sequencers leave behind.
    let mut seen_paths = std::collections::HashSet::new();
    let mut candidates: Vec<String> = Vec::new();
    for stage in 0..=3 {
        for entry in index.tracked_entries(stage) {
            if seen_paths.insert(entry.name.clone()) {
                candidates.push(entry.name.clone());
            }
        }
    }
    for path in &candidates {
        let path = path.as_str();
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
            if stage_replayed {
                stage_path(path).await?;
            }
        } else {
            write_entry(rr_dir, &id, "preimage", &content)?;
            if !merge_rr.iter().any(|(p, _)| p == path) {
                merge_rr.push((path.to_string(), id));
            }
            println!("Recorded preimage for '{path}'.");
        }
    }

    // Only persist MERGE_RR when there is something to track, or a file already
    // exists that may need updating/clearing. This keeps an ordinary commit in a
    // `rerere.enabled` repo — where `auto_update` runs after every commit — from
    // creating a spurious empty MERGE_RR, so it stays a true no-op.
    if !merge_rr.is_empty() || rr_dir.join("MERGE_RR").exists() {
        write_merge_rr(rr_dir, &merge_rr)?;
    }
    Ok(())
}

/// Stage a single resolved path (used when `--rerere-autoupdate` /
/// `rerere.autoUpdate` is in effect): stage the resolved content at stage 0 via
/// the normal `add` path, then drop any leftover conflict stages 1-3 so the
/// index reports the path fully resolved (`ls-files -u` empty). `add` alone
/// writes stage 0 but does not clear the unmerged stages a sequencer left.
async fn stage_path(path: &str) -> CliResult<()> {
    let args = crate::command::add::AddArgs {
        pathspec: vec![path.to_string()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
        pathspec_from_file: None,
        pathspec_file_nul: false,
        chmod: None,
        renormalize: false,
        ignore_missing: false,
    };
    crate::command::add::run_add(&args).await?;

    let index_path = path::index();
    let mut index = Index::load(&index_path).map_err(|error| {
        CliError::fatal(format!("failed to load index: {error}"))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::RepoStateInvalid)
    })?;
    // Remove ALL three conflict stages (do not short-circuit), noting whether
    // any were present so we only rewrite the index when something changed.
    let mut cleared = false;
    for stage in [1u8, 2, 3] {
        if index.remove(path, stage).is_some() {
            cleared = true;
        }
    }
    if cleared {
        index.save(&index_path).map_err(|error| {
            CliError::fatal(format!("failed to save index: {error}"))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::RepoStateInvalid)
        })?;
    }
    Ok(())
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
