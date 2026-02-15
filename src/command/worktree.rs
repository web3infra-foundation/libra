use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{
    command::restore::{self, RestoreArgs},
    internal::head::Head,
    utils::util,
};

/// CLI arguments for the `worktree` subcommand.
///
/// This type is wired into the top-level CLI and dispatches to individual
/// worktree subcommands such as `add`, `list`, `move`, etc.

#[derive(Parser, Debug)]
pub struct WorktreeArgs {
    #[clap(subcommand)]
    pub command: WorktreeSubcommand,
}
/// All supported `worktree` subcommands.
///
/// These roughly mirror `git worktree` operations while keeping Libra-specific
/// semantics (for example, `remove` does not delete directories on disk).
#[derive(Subcommand, Debug)]
pub enum WorktreeSubcommand {
    Add {
        path: String,
    },
    List,
    Lock {
        path: String,
        #[clap(long)]
        reason: Option<String>,
    },
    Unlock {
        path: String,
    },
    Move {
        src: String,
        dest: String,
    },
    Prune,
    Remove {
        path: String,
    },
    Repair,
}

/// A single worktree entry persisted in `worktrees.json`.
///
/// `path` is always stored as a canonical absolute path.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct WorktreeEntry {
    path: String,
    is_main: bool,
    locked: bool,
    lock_reason: Option<String>,
}

/// Top-level state persisted in `worktrees.json`.
///
/// The state contains the main worktree and any number of linked worktrees.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
struct WorktreeState {
    worktrees: Vec<WorktreeEntry>,
}

/// RAII guard that temporarily changes the process current directory.
///
/// When created with `change_to`, it switches the current directory to the
/// provided path and remembers the previous one. When dropped, it restores
/// the original directory, even if the inner operation panics or early-returns.
struct DirGuard {
    old_dir: PathBuf,
}

impl DirGuard {
    fn change_to(new_dir: &Path) -> io::Result<Self> {
        let old_dir = env::current_dir()?;
        env::set_current_dir(new_dir)?;
        Ok(Self { old_dir })
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.old_dir);
    }
}

/// Entry point for the `worktree` subcommand.
///
/// This function verifies that a Libra repository exists and then dispatches
/// to the concrete handler for the requested worktree operation. Any `io::Error`
/// returned from handlers is formatted as a `fatal:` message on stderr.
pub async fn execute(args: WorktreeArgs) {
    if !util::check_repo_exist() {
        return;
    }

    let result = match args.command {
        WorktreeSubcommand::Add { path } => add_worktree(path).await,
        WorktreeSubcommand::List => list_worktrees(),
        WorktreeSubcommand::Lock { path, reason } => lock_worktree(path, reason),
        WorktreeSubcommand::Unlock { path } => unlock_worktree(path),
        WorktreeSubcommand::Move { src, dest } => move_worktree(src, dest),
        WorktreeSubcommand::Prune => prune_worktrees(),
        WorktreeSubcommand::Remove { path } => remove_worktree(path),
        WorktreeSubcommand::Repair => repair_worktrees(),
    };

    if let Err(e) = result {
        eprintln!("fatal: {}", e);
    }
}

/// Returns the path to the on-disk worktree state file.
fn state_path() -> PathBuf {
    util::storage_path().join("worktrees.json")
}

/// Loads the current `WorktreeState` from disk, ensuring a main worktree entry.
///
/// If the state file does not exist or is empty, this function initializes a
/// fresh state with a single main worktree derived from the storage path, then
/// persists it before returning.
fn load_state() -> io::Result<WorktreeState> {
    let path = state_path();
    if !path.exists() {
        let mut state = WorktreeState::default();
        ensure_main_entry(&mut state)?;
        save_state(&state)?;
        return Ok(state);
    }
    let data = fs::read(&path)?;
    if data.is_empty() {
        let mut state = WorktreeState::default();
        ensure_main_entry(&mut state)?;
        save_state(&state)?;
        return Ok(state);
    }
    let mut state: WorktreeState =
        serde_json::from_slice(&data).map_err(|e| io::Error::other(e.to_string()))?;
    ensure_main_entry(&mut state)?;
    Ok(state)
}

/// Atomically writes the given `WorktreeState` to disk.
///
/// The state is first written to a temporary file and then moved into place.
/// On Windows, the existing file is removed before `rename` to avoid platform
/// specific failures when the destination already exists.
fn save_state(state: &WorktreeState) -> io::Result<()> {
    let path = state_path();
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(state).map_err(|e| io::Error::other(e.to_string()))?;
    if let Some(parent) = tmp.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&tmp, data)?;
    #[cfg(windows)]
    {
        if path.exists() {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
        fs::rename(&tmp, &path)?;
    }

    #[cfg(not(windows))]
    {
        fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// Normalizes the given path into an absolute, canonical path where possible.
///
/// For existing paths this is a thin wrapper around `fs::canonicalize`. For
/// non-existing paths we join with the current directory first, and when the
/// parent directory already exists we canonicalize the parent and reattach
/// the final component. This keeps persisted worktree paths stable while
/// avoiding directory creation side effects.
fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let p = path.as_ref();
    if p.exists() {
        fs::canonicalize(p)
    } else {
        let base = util::cur_dir();
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        };
        if let Some(parent) = joined.parent().filter(|p| p.exists()) {
            let canonical_parent = fs::canonicalize(parent)?;
            if let Some(name) = joined.file_name() {
                return Ok(canonical_parent.join(name));
            }
            return Ok(canonical_parent);
        }
        Ok(joined)
    }
}

/// Ensures that the main worktree entry exists and is unique.
///
/// The main worktree is always considered to be the parent directory of the
/// `.libra` storage directory. If an entry with that path exists, it becomes
/// the sole `is_main = true` entry. Otherwise a new main entry is appended and
/// all existing entries are marked as non-main.
fn ensure_main_entry(state: &mut WorktreeState) -> io::Result<()> {
    let storage = util::storage_path();
    let repo_root = storage
        .parent()
        .ok_or_else(|| io::Error::other("invalid storage path"))?;
    let main_path = canonicalize(repo_root)?;
    if let Some(idx) = state
        .worktrees
        .iter()
        .position(|w| Path::new(&w.path) == main_path)
    {
        for (i, w) in state.worktrees.iter_mut().enumerate() {
            w.is_main = i == idx;
        }
    } else {
        for w in &mut *state.worktrees {
            w.is_main = false;
        }
        state.worktrees.push(WorktreeEntry {
            path: main_path.to_string_lossy().to_string(),
            is_main: true,
            locked: false,
            lock_reason: None,
        });
    }
    Ok(())
}

/// Finds a mutable worktree entry by canonical path.
fn find_entry_mut<'a>(state: &'a mut WorktreeState, path: &Path) -> Option<&'a mut WorktreeEntry> {
    state
        .worktrees
        .iter_mut()
        .find(|w| Path::new(&w.path) == path)
}

/// Finds an immutable worktree entry by canonical path.
fn find_entry<'a>(state: &'a WorktreeState, path: &Path) -> Option<&'a WorktreeEntry> {
    state.worktrees.iter().find(|w| Path::new(&w.path) == path)
}

/// Implements `worktree add <path>`.
///
/// This command:
/// - validates the requested path is outside `.libra` storage,
/// - creates the target directory if it does not exist,
/// - rejects paths that canonicalize inside `.libra` (with cleanup),
/// - ensures the worktree is not already registered,
/// - writes a `.libra` link file pointing at the shared storage, and
/// - when `HEAD` exists, populates the new worktree from the index without
///   touching the shared index itself.
async fn add_worktree(path: String) -> io::Result<()> {
    let storage = util::storage_path();
    let target = canonicalize(&path)?;

    if util::is_sub_path(&target, &storage) {
        return Err(io::Error::other(
            "worktree path cannot be inside .libra storage",
        ));
    }

    if target.exists() && !target.is_dir() {
        return Err(io::Error::other("target exists and is not a directory"));
    }

    if !target.exists() {
        fs::create_dir_all(&target)?;
    }

    let canonical_target = canonicalize(&target)?;
    if util::is_sub_path(&canonical_target, &storage) {
        fs::remove_dir_all(&target)?;
        return Err(io::Error::other(
            "worktree path cannot be inside .libra storage",
        ));
    }

    let mut state = load_state()?;
    if state
        .worktrees
        .iter()
        .any(|w| Path::new(&w.path) == canonical_target)
    {
        println!("worktree already exists at {}", canonical_target.display());
        return Ok(());
    }

    let link_path = target.join(util::ROOT_DIR);
    if link_path.exists() {
        return Err(io::Error::other("target already contains a .libra entry"));
    }

    let storage_str = storage.to_string_lossy().to_string();
    let content = format!("gitdir: {}\n", storage_str);
    fs::write(&link_path, content)?;

    if Head::current_commit().await.is_some() {
        let _guard = DirGuard::change_to(&target)?;
        // With staged: false and source: None, restore populates the new worktree from the index
        // without modifying the shared index itself.
        restore::execute(RestoreArgs {
            pathspec: vec![util::working_dir_string()],
            source: None,
            worktree: true,
            staged: false,
        })
        .await;
    }

    state.worktrees.push(WorktreeEntry {
        path: canonical_target.to_string_lossy().to_string(),
        is_main: false,
        locked: false,
        lock_reason: None,
    });
    save_state(&state)?;

    println!("{}", canonical_target.display());

    Ok(())
}

/// Implements `worktree list`.
///
/// Each registered worktree is printed on its own line as either
/// `main <path>` or `worktree <path>`, with optional `[locked: <reason>]`
/// suffix when the entry is locked.
fn list_worktrees() -> io::Result<()> {
    let state = load_state()?;
    for w in state.worktrees {
        let mut line = String::new();
        if w.is_main {
            line.push_str("main ");
        } else {
            line.push_str("worktree ");
        }
        line.push_str(&w.path);
        if w.locked {
            line.push_str(" [locked");
            if let Some(reason) = w.lock_reason.as_ref()
                && !reason.is_empty()
            {
                line.push_str(": ");
                line.push_str(reason);
            }
            line.push(']');
        }
        println!("{}", line);
    }
    Ok(())
}

/// Implements `worktree lock <path> [--reason <msg>]`.
///
/// Marks the specified worktree entry as locked and persists an optional
/// human-readable reason. Locking is a state-only operation and does not
/// alter directories on disk.
fn lock_worktree(path: String, reason: Option<String>) -> io::Result<()> {
    let mut state = load_state()?;
    let target = canonicalize(path)?;
    let entry = match find_entry_mut(&mut state, &target) {
        Some(e) => e,
        None => return Err(io::Error::other("no such worktree")),
    };
    if entry.locked {
        return Ok(());
    }
    entry.locked = true;
    entry.lock_reason = reason;
    save_state(&state)?;
    Ok(())
}

/// Implements `worktree unlock <path>`.
///
/// Clears the lock flag and reason for the specified worktree entry if it is
/// currently locked. Unlocking is idempotent and leaves the filesystem untouched.
fn unlock_worktree(path: String) -> io::Result<()> {
    let mut state = load_state()?;
    let target = canonicalize(path)?;
    let entry = match find_entry_mut(&mut state, &target) {
        Some(e) => e,
        None => return Err(io::Error::other("no such worktree")),
    };
    if !entry.locked {
        return Ok(());
    }
    entry.locked = false;
    entry.lock_reason = None;
    save_state(&state)?;
    Ok(())
}

/// Implements `worktree move <src> <dest>`.
///
/// This command:
/// - resolves both source and destination paths,
/// - rejects moves of the main or a locked worktree,
/// - ensures the destination does not already exist on disk or in the registry,
/// - updates the registry to point at the new path and saves it, and then
/// - renames the directory on disk, attempting to roll back registry changes
///   if the rename fails.
fn move_worktree(src: String, dest: String) -> io::Result<()> {
    let mut state = load_state()?;
    let src_path = canonicalize(&src)?;
    let dest_path = canonicalize(&dest)?;

    if find_entry(&state, &dest_path).is_some() {
        return Err(io::Error::other(
            "destination already registered as worktree",
        ));
    }

    let index = state
        .worktrees
        .iter()
        .position(|w| Path::new(&w.path) == src_path)
        .ok_or_else(|| io::Error::other("no such worktree"))?;

    if state.worktrees[index].is_main {
        return Err(io::Error::other("cannot move main worktree"));
    }
    if state.worktrees[index].locked {
        return Err(io::Error::other("cannot move locked worktree"));
    }

    if dest_path.exists() {
        return Err(io::Error::other("destination already exists"));
    }

    let old_path = state.worktrees[index].path.clone();
    state.worktrees[index].path = dest_path.to_string_lossy().to_string();
    if let Err(e) = save_state(&state) {
        state.worktrees[index].path = old_path;
        return Err(e);
    }

    if let Err(e) = fs::rename(&src_path, &dest_path) {
        state.worktrees[index].path = old_path;
        save_state(&state)?;
        return Err(e);
    }

    Ok(())
}

/// Implements `worktree prune`.
///
/// Any non-main worktree whose directory no longer exists on disk is removed
/// from the registry. Before mutating state, the function prints the set of
/// paths that will be pruned so the user can see what is being cleaned up.
fn prune_worktrees() -> io::Result<()> {
    let mut state = load_state()?;
    let to_prune: Vec<_> = state
        .worktrees
        .iter()
        .filter(|w| {
            let path = Path::new(&w.path);
            !path.exists() && !w.is_main
        })
        .map(|w| w.path.clone())
        .collect();

    if to_prune.is_empty() {
        println!("No worktrees to prune");
        return Ok(());
    }

    println!("Will prune {} worktrees:", to_prune.len());
    for path in &to_prune {
        println!("  {}", path);
    }

    state.worktrees.retain(|w| {
        let path = Path::new(&w.path);
        path.exists() || w.is_main
    });
    save_state(&state)?;

    println!("Pruned {} worktrees", to_prune.len());
    Ok(())
}

/// Implements `worktree remove <path>`.
///
/// The specified worktree is removed from the registry, provided it is neither
/// the main worktree nor locked. The directory on disk is intentionally left
/// untouched to avoid destructive behavior.
fn remove_worktree(path: String) -> io::Result<()> {
    let mut state = load_state()?;
    let target = canonicalize(path)?;

    let index = state
        .worktrees
        .iter()
        .position(|w| Path::new(&w.path) == target)
        .ok_or_else(|| io::Error::other("no such worktree"))?;

    let entry = &state.worktrees[index];
    if entry.is_main {
        return Err(io::Error::other("cannot remove main worktree"));
    }
    if entry.locked {
        return Err(io::Error::other("cannot remove locked worktree"));
    }

    state.worktrees.remove(index);
    save_state(&state)?;

    Ok(())
}

/// Implements `worktree repair`.
///
/// This command removes duplicate worktree entries that point to the same
/// canonical path and re-applies the invariant that there is exactly one
/// main worktree entry. The repaired state is only written back if changes
/// were actually made.
fn repair_worktrees() -> io::Result<()> {
    let mut state = load_state()?;
    let mut changed = false;

    let mut seen = Vec::<PathBuf>::new();
    state.worktrees.retain(|w| {
        let p = Path::new(&w.path);
        if seen.iter().any(|s| s == p) {
            changed = true;
            false
        } else {
            seen.push(p.to_path_buf());
            true
        }
    });

    ensure_main_entry(&mut state)?;

    if changed {
        save_state(&state)?;
    }

    Ok(())
}
