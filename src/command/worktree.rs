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

#[derive(Parser, Debug)]
pub struct WorktreeArgs {
    #[clap(subcommand)]
    pub command: WorktreeSubcommand,
}

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

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WorktreeEntry {
    path: String,
    is_main: bool,
    locked: bool,
    lock_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
struct WorktreeState {
    worktrees: Vec<WorktreeEntry>,
}

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

fn state_path() -> PathBuf {
    util::storage_path().join("worktrees.json")
}

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
        Ok(joined)
    }
}

fn ensure_main_entry(state: &mut WorktreeState) -> io::Result<()> {
    let main_path = canonicalize(util::working_dir())?;
    let main_str = main_path.to_string_lossy().to_string();
    let existing_main_index = state.worktrees.iter().position(|w| w.is_main);
    let current_index = state
        .worktrees
        .iter()
        .position(|w| Path::new(&w.path) == main_path);

    match (existing_main_index, current_index) {
        (Some(_), Some(cur_idx)) => {
            state.worktrees[cur_idx].is_main = false;
        }
        (Some(_), None) => {
            state.worktrees.push(WorktreeEntry {
                path: main_str,
                is_main: false,
                locked: false,
                lock_reason: None,
            });
        }
        (None, Some(cur_idx)) => {
            state.worktrees[cur_idx].is_main = true;
        }
        (None, None) => {
            state.worktrees.push(WorktreeEntry {
                path: main_str,
                is_main: true,
                locked: false,
                lock_reason: None,
            });
        }
    }
    Ok(())
}

fn find_entry_mut<'a>(state: &'a mut WorktreeState, path: &Path) -> Option<&'a mut WorktreeEntry> {
    state
        .worktrees
        .iter_mut()
        .find(|w| Path::new(&w.path) == path)
}

fn find_entry<'a>(state: &'a WorktreeState, path: &Path) -> Option<&'a WorktreeEntry> {
    state.worktrees.iter().find(|w| Path::new(&w.path) == path)
}

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

    let link_path = target.join(util::ROOT_DIR);
    if link_path.exists() {
        return Err(io::Error::other("target already contains a .libra entry"));
    }

    let storage_str = storage.to_string_lossy().to_string();
    let content = format!("gitdir: {}\n", storage_str);
    fs::write(&link_path, content)?;

    if Head::current_commit().await.is_some() {
        let _guard = DirGuard::change_to(&target)?;
        restore::execute(RestoreArgs {
            pathspec: vec![util::working_dir_string()],
            source: None,
            worktree: true,
            staged: false,
        })
        .await;
    }

    let mut state = load_state()?;
    let canonical_target = canonicalize(&target)?;
    let target_str = canonical_target.to_string_lossy().to_string();
    if state
        .worktrees
        .iter()
        .any(|w| Path::new(&w.path) == canonical_target)
    {
        return Ok(());
    }
    state.worktrees.push(WorktreeEntry {
        path: target_str.clone(),
        is_main: false,
        locked: false,
        lock_reason: None,
    });
    save_state(&state)?;

    println!("{}", target_str);

    Ok(())
}

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

fn move_worktree(src: String, dest: String) -> io::Result<()> {
    let mut state = load_state()?;
    let src_path = canonicalize(&src)?;
    let dest_path = canonicalize(&dest)?;

    if find_entry(&state, &dest_path).is_some() {
        return Err(io::Error::other(
            "destination already registered as worktree",
        ));
    }

    let entry = match find_entry_mut(&mut state, &src_path) {
        Some(e) => e,
        None => return Err(io::Error::other("no such worktree")),
    };

    if entry.is_main {
        return Err(io::Error::other("cannot move main worktree"));
    }
    if entry.locked {
        return Err(io::Error::other("cannot move locked worktree"));
    }

    if dest_path.exists() {
        return Err(io::Error::other("destination already exists"));
    }

    fs::rename(&src_path, &dest_path)?;

    entry.path = dest_path.to_string_lossy().to_string();
    save_state(&state)?;

    Ok(())
}

fn prune_worktrees() -> io::Result<()> {
    let mut state = load_state()?;
    let before = state.worktrees.len();
    state.worktrees.retain(|w| {
        let path = Path::new(&w.path);
        path.exists() || w.is_main
    });
    let removed = before.saturating_sub(state.worktrees.len());
    if removed > 0 {
        save_state(&state)?;
    }
    println!("Pruned {} worktrees", removed);
    Ok(())
}

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
