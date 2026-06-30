//! `libra replace` — substitute one object for another whenever an object is
//! read, a focused subset of `git replace`.
//!
//! A replacement is stored as a loose ref under `.libra/refs/replace/<oid>`
//! whose content is the replacement oid (Git's `refs/replace/` namespace). The
//! peel happens in [`crate::command::load_object`] via [`resolve`], so every
//! reader that goes through `load_object` (`log`, `show`, `rev-parse` peeling,
//! …) transparently sees the replacement — not just one call site.
//!
//! Integrating these loose refs into the SQLite reference table (so `show-ref` /
//! `for-each-ref` list them) and `--graft` / `--edit` / `--convert-graft-file`
//! are documented follow-ups.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    str::FromStr,
    sync::OnceLock,
};

use clap::Parser;
use git_internal::hash::ObjectHash;

use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
    util,
};

const REPLACE_REF_DIR: &str = "refs/replace";
/// Bound on how many `refs/replace` hops are followed, so a cycle or a long
/// chain can never spin forever inside the hot object-load path.
const MAX_REPLACE_DEPTH: usize = 8;

/// Process-global cache of `oid -> replacement oid`, loaded once on first use.
static REPLACE_MAP: OnceLock<HashMap<ObjectHash, ObjectHash>> = OnceLock::new();

tokio::task_local! {
    /// When set on the current task, [`resolve`] is a no-op. The `replace`
    /// command names objects by their *literal* oid, so it suppresses the peel
    /// while resolving its own arguments (otherwise creating a replacement would
    /// change how its own arguments resolve — e.g. `HEAD~1` after `HEAD` was
    /// already replaced). Task-local (not process-global) so a concurrent task
    /// on the multi-thread runtime is never affected.
    static SUPPRESS_PEEL: bool;
}

fn peel_suppressed() -> bool {
    SUPPRESS_PEEL
        .try_with(|suppressed| *suppressed)
        .unwrap_or(false)
}

pub const REPLACE_EXAMPLES: &str = "\
EXAMPLES:
    libra replace <object> <replacement>   Replace one object with another on read
    libra replace -f <object> <repl>       Replace even across object types / overwrite
    libra replace -d <object>...           Delete replacement(s)
    libra replace -l [<pattern>]           List replaced object ids (the default)";

/// Create, list, or delete object replacements (`refs/replace/*`).
#[derive(Parser, Debug)]
#[command(after_help = REPLACE_EXAMPLES)]
pub struct ReplaceArgs {
    /// Overwrite an existing replacement and allow a type mismatch.
    #[clap(short = 'f', long)]
    pub force: bool,

    /// Delete the replacement for each given object.
    #[clap(short = 'd', long)]
    pub delete: bool,

    /// List replaced object ids (optionally filtered by a substring).
    #[clap(short = 'l', long)]
    pub list: bool,

    /// Objects / replacement (see EXAMPLES).
    #[clap(value_name = "ARG")]
    pub args: Vec<String>,
}

// ----------------------------------------------------------------------------
// peel hook — called by `command::load_object`
// ----------------------------------------------------------------------------

/// Resolve an object id through `refs/replace`, following a chain (cycle-bounded).
/// When no replacements exist this is a cheap no-op, so it is safe on the hot
/// object-load path.
pub fn resolve(hash: ObjectHash) -> ObjectHash {
    if peel_suppressed() {
        return hash;
    }
    let map = REPLACE_MAP.get_or_init(load_replace_map);
    if map.is_empty() {
        return hash;
    }
    // Follow the chain, detecting cycles with a visited set and capping the
    // length, so a self-loop or `A->B->A` cycle deterministically stops at the
    // last unique hash rather than depending on depth parity.
    let mut current = hash;
    let mut visited = HashSet::new();
    visited.insert(current);
    for _ in 0..MAX_REPLACE_DEPTH {
        match map.get(&current) {
            Some(&next) if visited.insert(next) => current = next,
            _ => break,
        }
    }
    current
}

/// Scan `.libra/refs/replace/` into a map. Best-effort: a malformed entry is
/// skipped rather than breaking every object read (robustness over strictness).
fn load_replace_map() -> HashMap<ObjectHash, ObjectHash> {
    let mut map = HashMap::new();
    let Some(dir) = replace_dir() else {
        return map;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return map; // absent directory ⇒ no replacements
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(src) = ObjectHash::from_str(name) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(dst) = ObjectHash::from_str(content.trim()) else {
            continue;
        };
        map.insert(src, dst);
    }
    map
}

fn replace_dir() -> Option<PathBuf> {
    util::try_get_storage_path(None)
        .ok()
        .map(|root| root.join(REPLACE_REF_DIR))
}

// ----------------------------------------------------------------------------
// CLI
// ----------------------------------------------------------------------------

pub async fn execute(args: ReplaceArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

pub async fn execute_safe(args: ReplaceArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    // The `replace` command names objects by their literal oid, so the peel must
    // not rewrite its own argument resolution. Scope the suppression to this
    // task so concurrent tasks on the runtime are unaffected.
    SUPPRESS_PEEL.scope(true, run(args)).await
}

async fn run(args: ReplaceArgs) -> CliResult<()> {
    if args.delete {
        if args.args.is_empty() {
            return Err(usage("`replace -d` needs at least one object"));
        }
        return delete(&args.args).await;
    }
    if args.list || args.args.len() <= 1 {
        return list(args.args.first().map(String::as_str));
    }
    if args.args.len() == 2 {
        return create(&args.args[0], &args.args[1], args.force).await;
    }
    Err(usage(
        "too many arguments: use `replace <object> <replacement>`, `-d <object>...`, or `-l`",
    ))
}

async fn create(object: &str, replacement: &str, force: bool) -> CliResult<()> {
    let obj = resolve_any(object).await?;
    let repl = resolve_any(replacement).await?;
    if obj == repl {
        return Err(fatal(format!("cannot replace object {obj} with itself")));
    }

    // Git refuses a cross-type replacement unless forced.
    let storage = util::objects_storage();
    let obj_type = storage
        .get_object_type(&obj)
        .map_err(|error| fatal(format!("cannot read object {obj}: {error}")))?;
    let repl_type = storage
        .get_object_type(&repl)
        .map_err(|error| fatal(format!("cannot read object {repl}: {error}")))?;
    if obj_type != repl_type && !force {
        return Err(fatal(format!(
            "object {obj} is a {obj_type} but {repl} is a {repl_type}; pass -f to force"
        )));
    }

    let dir = replace_dir().ok_or_else(CliError::repo_not_found)?;
    let path = dir.join(obj.to_string());
    if path.exists() && !force {
        return Err(fatal(format!(
            "replacement for {obj} already exists; pass -f to overwrite"
        )));
    }
    fs::create_dir_all(&dir).map_err(write_err)?;
    fs::write(&path, format!("{repl}\n")).map_err(write_err)?;
    Ok(())
}

async fn delete(objects: &[String]) -> CliResult<()> {
    let dir = replace_dir().ok_or_else(CliError::repo_not_found)?;
    for spec in objects {
        let obj = resolve_any(spec).await?;
        let path = dir.join(obj.to_string());
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(fatal(format!("no replacement for {obj}")));
            }
            Err(error) => return Err(write_err(error)),
        }
    }
    Ok(())
}

fn list(pattern: Option<&str>) -> CliResult<()> {
    let Some(dir) = replace_dir() else {
        return Ok(());
    };
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(read_err(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_err)?;
        if let Some(name) = entry.file_name().to_str()
            && ObjectHash::from_str(name).is_ok()
            && pattern.is_none_or(|p| name.contains(p))
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    for name in names {
        println!("{name}");
    }
    Ok(())
}

/// Resolve an argument to an object id: a full object-hash string of any type
/// that exists, otherwise a commit-ish / ref via `get_commit_base`.
async fn resolve_any(spec: &str) -> CliResult<ObjectHash> {
    if let Ok(hash) = ObjectHash::from_str(spec)
        && util::objects_storage().get(&hash).is_ok()
    {
        return Ok(hash);
    }
    util::get_commit_base(spec)
        .await
        .map_err(|error| fatal(format!("not a valid object '{spec}': {error}")))
}

fn usage(message: &str) -> CliError {
    CliError::command_usage(message.to_string())
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::CliInvalidArguments)
}

fn fatal(message: String) -> CliError {
    CliError::fatal(message)
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::CliInvalidTarget)
}

fn read_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("failed to read refs/replace: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoReadFailed)
}

fn write_err(error: std::io::Error) -> CliError {
    CliError::fatal(format!("failed to write refs/replace: {error}"))
        .with_exit_code(128)
        .with_stable_code(StableErrorCode::IoWriteFailed)
}
