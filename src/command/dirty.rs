//! `libra dirty` — advisory dirty-set marks (lore.md §1.1, a Libra
//! extension; Git has no equivalent). Marks paths in the `working_dirty`
//! cache without reading file contents or touching the index: over-reporting
//! is the safe direction, so manual marks never invalidate the cache's scan
//! snapshot. Consumed by `status --cached` / `--check-dirty`; the cache is
//! rebuilt authoritatively by `status --scan`.

use clap::Parser;
use serde::Serialize;

use crate::{
    internal::dirty::{DirtyCache, native_path_to_stored},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        util,
    },
};

pub const DIRTY_EXAMPLES: &str = "\
EXAMPLES:
    libra dirty src/main.rs                Mark a path dirty in the cache (no file reads)
    libra dirty a.txt b.txt                Mark several paths
    libra dirty --list                     Show the cached dirty set and its freshness
    libra status --scan                    Rebuild the cache authoritatively
    libra status --cached                  Consume the cache instead of walking the tree
    libra --json dirty --list              Structured output for agents

NOTES:
    Marks are advisory: they can only make the cached view over-report (safe),
    never hide a change. Nonexistent paths are legal — a deletion IS dirty.
    Default `libra status` never reads or writes the cache.";

/// Mark paths dirty in the dirty-set cache, or list it (Libra extension).
#[derive(Parser, Debug)]
#[command(after_help = DIRTY_EXAMPLES)]
pub struct DirtyArgs {
    /// Paths to mark dirty (repo-relative or cwd-relative; must stay inside
    /// the repository). May not exist — a deletion is dirty too.
    #[clap(required_unless_present = "list")]
    pub paths: Vec<String>,

    /// List the cached dirty set instead of marking.
    #[clap(long, conflicts_with = "paths")]
    pub list: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
enum DirtyOutput {
    Mark {
        marked: Vec<String>,
        total_cached: usize,
        cache_state: String,
    },
    List {
        entries: Vec<DirtyListEntry>,
        cache_state: String,
        scanned_at: Option<String>,
    },
}

#[derive(Debug, Serialize)]
struct DirtyListEntry {
    path: String,
    kind: String,
    source: String,
    marked_at: String,
    verified_at: Option<String>,
}

async fn cache_state_label() -> String {
    use crate::internal::dirty::{DirtyCache, current_index_fingerprint};
    let Ok(index_path) = crate::utils::path::try_index() else {
        return "missing".to_string();
    };
    let Ok(fingerprint) = current_index_fingerprint(&index_path) else {
        return "missing".to_string();
    };
    let head = crate::internal::head::Head::current_commit()
        .await
        .map(|oid| oid.to_string());
    match DirtyCache::meta().await {
        Ok(meta) => DirtyCache::classify(meta.as_ref(), &fingerprint, head.as_deref())
            .as_str()
            .to_string(),
        Err(_) => "missing".to_string(),
    }
}

pub async fn execute(args: DirtyArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

pub async fn execute_safe(args: DirtyArgs, output: &OutputConfig) -> CliResult<()> {
    if util::require_repo().is_err() {
        return Err(CliError::repo_not_found());
    }
    if args.list {
        let entries: Vec<DirtyListEntry> = DirtyCache::list()
            .await
            .map_err(|e| {
                CliError::fatal(format!("failed to read the dirty cache: {e}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?
            .into_iter()
            .map(|entry| DirtyListEntry {
                path: entry.path,
                kind: entry.kind,
                source: entry.source,
                marked_at: entry.marked_at,
                verified_at: entry.verified_at,
            })
            .collect();
        let meta = DirtyCache::meta().await.ok().flatten();
        let report = DirtyOutput::List {
            entries,
            cache_state: cache_state_label().await,
            scanned_at: meta.and_then(|meta| meta.scanned_at),
        };
        if output.is_json() {
            return emit_json_data("dirty", &report, output);
        }
        if let DirtyOutput::List {
            entries,
            cache_state,
            ..
        } = &report
            && !output.quiet
        {
            for entry in entries {
                println!("{}\t{}\t{}", entry.kind, entry.source, entry.path);
            }
            eprintln!("cache: {cache_state}");
        }
        return Ok(());
    }

    // Validate ALL paths first — refuse atomically if any escapes the repo
    // root. Nonexistent paths are legal (a deletion IS dirty); no file
    // contents are read and the index is never touched.
    let mut stored: Vec<String> = Vec::with_capacity(args.paths.len());
    let mut offenders: Vec<String> = Vec::new();
    for raw in &args.paths {
        let workdir_relative = util::to_workdir_path(raw);
        // Must be a purely relative path inside the worktree: reject absolute
        // results (input outside the repo root) and ANY parent/root/prefix
        // component, not just a leading `..`.
        let escapes = workdir_relative.is_absolute()
            || workdir_relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            });
        if escapes {
            offenders.push(raw.clone());
        } else {
            // CLI args are Strings, so this is infallible in practice; treat a
            // failure as an escaping/invalid path rather than mangle it.
            match native_path_to_stored(&workdir_relative) {
                Ok(text) => stored.push(text),
                Err(_) => offenders.push(raw.clone()),
            }
        }
    }
    if !offenders.is_empty() {
        return Err(CliError::fatal(format!(
            "paths escape the repository root: {}",
            offenders.join(", ")
        ))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint("dirty marks are repo-relative; pass paths inside the working tree"));
    }
    DirtyCache::mark_paths(&stored).await.map_err(|e| {
        CliError::fatal(format!("failed to write the dirty cache: {e}"))
            .with_stable_code(StableErrorCode::IoWriteFailed)
    })?;
    let total = DirtyCache::list().await.map(|rows| rows.len()).unwrap_or(0);
    let report = DirtyOutput::Mark {
        marked: stored,
        total_cached: total,
        cache_state: cache_state_label().await,
    };
    if output.is_json() {
        return emit_json_data("dirty", &report, output);
    }
    if let DirtyOutput::Mark { marked, .. } = &report
        && !output.quiet
    {
        println!("marked {} path(s) dirty", marked.len());
    }
    Ok(())
}
