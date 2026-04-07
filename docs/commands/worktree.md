# `libra worktree`

Manage multiple working trees attached to this repository.

**Alias:** `libra wt`

## Synopsis

```
libra worktree add <path>
libra worktree list
libra worktree lock <path> [--reason <text>]
libra worktree unlock <path>
libra worktree move <src> <dest>
libra worktree prune
libra worktree remove <path>
libra worktree repair
```

## Description

`libra worktree` manages multiple working trees that share a single repository database and object store. This allows you to have several checkouts of the same repository simultaneously, which is useful for working on multiple branches at once, running builds while editing code, or testing changes in isolation.

Each linked worktree is a directory containing a `.libra` symlink pointing back to the shared storage directory. The main worktree is the original repository directory. All worktrees share the same SQLite database, object store, and configuration.

Worktree metadata is persisted in a `worktrees.json` file inside the `.libra` storage directory. Each entry tracks the filesystem path, whether it is the main worktree, its lock status, and an optional lock reason. The state file is written atomically via a temporary file rename to prevent corruption.

When a new worktree is added and HEAD points to a commit, the worktree is automatically populated with the committed content from HEAD (not staged index changes).

## Options

### Subcommand: `add`

Create a new linked worktree at the given filesystem path.

| Argument | Description |
|----------|-------------|
| `<path>` | Filesystem path for the new worktree. Can be relative or absolute. The directory is created if it does not exist. Must not be inside `.libra` storage, must not already be registered, and must be empty if it exists. |

```bash
# Create a new worktree for a feature branch
libra worktree add ../my-feature

# Create using absolute path
libra worktree add /tmp/libra-test
```

### Subcommand: `list`

List all registered worktrees and their state.

```bash
libra worktree list
```

### Subcommand: `lock`

Mark a worktree as locked to prevent it from being pruned or removed.

| Argument / Flag | Description |
|-----------------|-------------|
| `<path>` | Filesystem path of the worktree to lock. |
| `--reason` | Optional human-readable explanation for why the worktree is locked. |

```bash
# Lock a worktree
libra worktree lock ../my-feature

# Lock with a reason
libra worktree lock ../my-feature --reason "long-running experiment"
```

### Subcommand: `unlock`

Remove the lock from a previously locked worktree. Idempotent: unlocking an already-unlocked worktree is a no-op.

| Argument | Description |
|----------|-------------|
| `<path>` | Filesystem path of the worktree to unlock. |

```bash
libra worktree unlock ../my-feature
```

### Subcommand: `move`

Move or rename an existing linked worktree. The directory is renamed on disk and the registry is updated. Cannot move the main worktree or a locked worktree.

| Argument | Description |
|----------|-------------|
| `<src>` | Current filesystem path of the worktree. |
| `<dest>` | New filesystem path. Must not already exist on disk or in the registry. Cannot be inside `.libra` storage. |

```bash
libra worktree move ../my-feature ../my-feature-v2
```

### Subcommand: `prune`

Remove worktrees from the registry whose directories no longer exist on disk. The main worktree and locked worktrees are never pruned.

```bash
libra worktree prune
```

### Subcommand: `remove`

Unregister a worktree from the state file. The directory on disk is intentionally left untouched to avoid destructive behavior. Cannot remove the main worktree or a locked worktree.

| Argument | Description |
|----------|-------------|
| `<path>` | Filesystem path of the worktree to unregister. |

```bash
libra worktree remove ../my-feature
```

### Subcommand: `repair`

Repair worktree metadata by removing duplicate entries (same canonical path) and ensuring exactly one main worktree entry exists. Only writes the state file if changes are actually made.

```bash
libra worktree repair
```

## Common Commands

```bash
# Create a new worktree
libra worktree add ../experiment

# List all worktrees
libra wt list

# Lock a worktree to protect it
libra wt lock ../experiment --reason "production hotfix in progress"

# Unlock when done
libra wt unlock ../experiment

# Move a worktree to a new location
libra wt move ../experiment ../experiment-v2

# Clean up worktrees whose directories were deleted
libra wt prune

# Unregister a worktree (keeps files on disk)
libra wt remove ../experiment-v2

# Fix inconsistent worktree metadata
libra wt repair
```

## Human Output

**`worktree add`**:

```text
/Users/alice/projects/my-feature
```

**`worktree list`**:

```text
main /Users/alice/projects/my-repo
worktree /Users/alice/projects/my-feature
worktree /Users/alice/projects/hotfix [locked: production hotfix in progress]
```

**`worktree prune`** (with stale entries):

```text
Will prune 2 worktrees:
  /Users/alice/projects/old-experiment
  /Users/alice/projects/deleted-branch
Pruned 2 worktrees
```

**`worktree prune`** (nothing to prune):

```text
No worktrees to prune
```

## Design Rationale

### Why JSON-file persistence instead of filesystem links like Git?

Git tracks worktrees through a combination of filesystem structure: the main `.git/worktrees/` directory contains per-worktree directories with `gitdir`, `HEAD`, and `commondir` files, and each linked worktree has a `.git` file (not directory) pointing back. This approach is tightly coupled to Git's file-based architecture and requires careful cross-referencing between multiple locations.

Libra uses a single `worktrees.json` file in the shared storage directory. This provides several advantages: all worktree metadata is in one queryable location, state is written atomically (via temp-file rename), and the format is trivially inspectable by both humans and AI agents. The symlink from each linked worktree's `.libra` back to the shared storage is simpler than Git's bidirectional pointer system. The trade-off is that the JSON file is a single point of truth that must be kept consistent, which is why `repair` exists.

### Why `--reason` on lock?

Git's `git worktree lock` also supports `--reason`, and Libra preserves this. Lock reasons are valuable in team environments and when AI agents manage worktrees: they provide context about why a worktree should not be pruned or removed. Without a reason, a locked worktree is opaque, and another user (or agent) cannot determine whether the lock is still relevant. The reason is displayed in `list` output, making lock status self-documenting.

### Why does `remove` not delete directories on disk?

Deleting files is a destructive operation that cannot be undone. Libra's `remove` only unregisters the worktree from the JSON state file, leaving the directory intact. This is a deliberate safety choice: the user can inspect and manually delete the directory when they are confident it is no longer needed. This also prevents accidental data loss if a worktree contains uncommitted work. Git's `git worktree remove` does delete the directory by default, which has been a source of lost work.

### Why does `move` reject locked worktrees?

A locked worktree signals that it should not be modified. Moving it would change its filesystem path, which could break references to that path in other tools, scripts, or agent configurations. The user must explicitly unlock the worktree before moving it, ensuring the action is intentional.

### Why does `add` populate from HEAD instead of the index?

When creating a linked worktree, Libra restores content from the HEAD commit rather than the current index state. This ensures the new worktree reflects the last committed state, not any staged-but-uncommitted changes that exist only in the original worktree's context. This matches user expectations: a new worktree starts from a known good state.

## Parameter Comparison: Libra vs Git vs jj

| Operation | Libra | Git | jj |
|-----------|-------|-----|----|
| Create worktree | `worktree add <path>` | `worktree add <path> [<branch>]` | `workspace add <path>` |
| Create on branch | Not supported | `worktree add <path> <branch>` | `workspace add <path>` (then `jj edit`) |
| Create detached | Not supported | `worktree add --detach <path> <commit>` | N/A |
| List worktrees | `worktree list` | `worktree list [--porcelain]` | `workspace list` |
| Lock | `worktree lock <path> [--reason]` | `worktree lock [--reason] <worktree>` | N/A |
| Unlock | `worktree unlock <path>` | `worktree unlock <worktree>` | N/A |
| Move | `worktree move <src> <dest>` | `worktree move <worktree> <new-path>` | N/A |
| Prune | `worktree prune` | `worktree prune [--dry-run]` | N/A (automatic) |
| Remove | `worktree remove <path>` (registry only) | `worktree remove [--force] <worktree>` (deletes dir) | `workspace forget <name>` |
| Repair | `worktree repair` | `worktree repair [<path>...]` | N/A |
| Alias | `wt` | N/A | N/A |
| Branch per worktree | Not supported | Automatic (new branch or existing) | Automatic (new working copy commit) |
| Storage | JSON file (`worktrees.json`) | Filesystem structure (`.git/worktrees/`) | Operation log |
| Worktree link | Symlink to shared `.libra` | `.git` file pointing to `gitdir` | Symlink to shared `.jj` |

Note: jj uses the term "workspace" instead of "worktree". Each workspace automatically gets its own working copy commit, and workspaces are tracked in the operation log. jj workspaces are simpler than Git worktrees because jj's change-based model does not require separate branch management per workspace.

## Error Handling

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-IO-001` | Worktree path cannot be inside `.libra` storage |
| `LBR-IO-001` | Target exists and is not a directory |
| `LBR-IO-001` | Target directory exists and is not empty |
| `LBR-IO-001` | Target already contains a `.libra` entry |
| `LBR-CLI-003` | No such worktree (for lock, unlock, move, remove) |
| `LBR-CLI-003` | Cannot move or remove main worktree |
| `LBR-CLI-003` | Cannot move or remove locked worktree |
| `LBR-IO-001` | Destination already exists (for move) |
| `LBR-IO-001` | Destination already registered as worktree (for move) |
| `LBR-IO-002` | Failed to write worktrees.json |
| `LBR-IO-001` | Failed to populate worktree from HEAD |
