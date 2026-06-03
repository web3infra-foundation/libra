# `libra branch`

Create, delete, rename, inspect, or list branches.

**Alias:** `br`

## Synopsis

```
libra branch [<new_branch>] [<commit_hash>]
libra branch -l [-r | -a] [--contains <commit>] [--no-contains <commit>]
libra branch -l [--merged [<commit>] | --no-merged [<commit>]] [--points-at <object>] [--ignore-case]
libra branch -d <name>
libra branch -D <name>
libra branch -m [<old>] <new>
libra branch -u <upstream>
libra branch --unset-upstream [<branch>]
libra branch --show-current
```

## Description

`libra branch` manages local and remote-tracking branch references stored in the SQLite database. Without arguments it lists local branches, highlighting the current branch with an asterisk. When given a positional `<new_branch>` argument it creates a new branch pointing at HEAD (or at `<commit_hash>` when provided).

Deletion comes in two flavours: `-d` performs a safe delete that checks whether the branch has been fully merged into the current branch before removing it, while `-D` force-deletes regardless of merge status. Both refuse to delete the branch you are currently on.

The `--contains` and `--no-contains` filters (aliased as `--with` and `--without`) let you narrow the branch list to those whose history does or does not include a particular commit, defaulting to HEAD when the commit argument is omitted.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<new_branch>` | positional | Create a new branch pointing at HEAD or `<commit_hash>` |
| | `<commit_hash>` | positional (requires `new_branch`) | Base commit for the new branch |
| `-l` | `--list` | | List branches (default when no action is specified) |
| `-D` | `--delete-force` | `<name>` | Force-delete a branch, even if not fully merged |
| `-d` | `--delete` | `<name>` | Safe-delete a branch (must be fully merged) |
| `-u` | `--set-upstream-to` | `<upstream>` | Set upstream tracking for the current branch |
| | `--unset-upstream` | `[branch]` | Remove upstream tracking (the current branch when no name is given) |
| | `--show-current` | | Print the current branch name or detached HEAD state |
| `-m` | `--move` | `<old> <new>` or `<new>` | Rename a branch; with one argument renames the current branch |
| `-r` | `--remotes` | | Show remote-tracking branches only |
| `-a` | `--all` | | Show local and remote-tracking branches |
| | `--contains` | `[commit]` (default HEAD) | Only list branches containing the commit. Alias: `--with` |
| | `--no-contains` | `[commit]` (default HEAD) | Only list branches not containing the commit. Alias: `--without` |
| | `--merged` | `[commit]` (default HEAD) | Only list branches whose tip is merged into the commit. Mutually exclusive with `--no-merged` |
| | `--no-merged` | `[commit]` (default HEAD) | Only list branches whose tip is not merged into the commit |
| | `--points-at` | `<object>` | Only list branches whose tip points exactly at the object |
| | `--ignore-case` | | Sort branch names case-insensitively in list output |

### Flag examples

```bash
# Create a branch from HEAD
libra branch feature-x

# Create a branch from another branch or commit
libra branch feature-x main
libra branch hotfix abc1234

# List local branches
libra branch -l

# List all branches (local + remote)
libra branch -l -a

# List branches containing the latest release tag
libra branch --contains v2.0

# List branches that do NOT contain HEAD
libra branch --no-contains

# List branches already merged into HEAD (safe to delete)
libra branch --merged

# List branches not yet merged into main
libra branch --no-merged main

# List branches whose tip is exactly at HEAD
libra branch --points-at HEAD

# List branches sorted case-insensitively
libra branch --ignore-case

# Safe-delete a merged branch
libra branch -d topic

# Force-delete regardless of merge status
libra branch -D experiment

# Rename current branch
libra branch -m new-name

# Rename any branch
libra branch -m old-name new-name

# Set upstream tracking
libra branch -u origin/main

# Remove upstream tracking (current branch)
libra branch --unset-upstream

# Remove upstream tracking for a named branch
libra branch --unset-upstream feature

# Show current branch name
libra branch --show-current

# JSON output for agents
libra branch --json --show-current
```

## Common Commands

```bash
libra branch feature-x                  # Create a branch from HEAD
libra branch feature-x main             # Create a branch from another branch
libra branch -d topic                   # Delete a fully merged branch
libra branch -D topic                   # Force-delete a branch
libra branch --merged                   # List branches merged into HEAD
libra branch --no-merged main           # List branches not yet merged into main
libra branch --points-at HEAD           # List branches whose tip is at HEAD
libra branch --unset-upstream           # Remove tracking for the current branch
libra branch --set-upstream-to origin/main
                                        # Set upstream for the current branch
libra branch --json --show-current      # Structured JSON output for agents
```

## Human Output

- List: prints the branch list with `*` marking the current branch. Branches with upstream tracking show a `[<remote>/<branch>]` suffix (e.g. `* main [origin/main]`).
- Safe delete: `Deleted branch feature (was abc123...)`
- Rename: `Renamed branch 'old' to 'new'`
- `--set-upstream-to`: `Branch 'main' set up to track remote branch 'origin/main'`
- `--unset-upstream`: `Removed upstream tracking for branch 'main'`, or `Branch 'main' has no upstream tracking to remove` when none was configured
- `--show-current`: prints the current branch name, or `HEAD detached at <hash>` when detached

## Structured Output (JSON examples)

`--json` / `--machine` uses `action` to distinguish operations:

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "create",
    "name": "feature",
    "commit": "abc123..."
  }
}
```

List action:

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "list",
    "branches": [
      { "name": "main", "current": true, "commit": "abc1234...", "tracking": { "remote": "origin", "merge": "main" } },
      { "name": "feature", "current": false, "commit": "def5678..." }
    ]
  }
}
```

The optional `tracking` object appears only for branches with upstream
tracking configured; untracked entries omit the key entirely (backward
compatible).

Unset-upstream action (`had_upstream` is `false` for an idempotent no-op):

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "unset-upstream",
    "branch": "main",
    "had_upstream": true
  }
}
```

Show-current action:

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "show-current",
    "name": "main",
    "detached": false,
    "commit": "abc1234..."
  }
}
```

Supported actions:

- `list`: `branches`
- `create`: `name`, `commit`
- `delete`: `name`, `commit`, `force`
- `rename`: `old_name`, `new_name`
- `set-upstream`: `branch`, `upstream`
- `unset-upstream`: `branch`, `had_upstream`
- `show-current`: `name`, `detached`, `commit`
- `list` entries optionally include `tracking`: `{ remote, merge }`

## Design Rationale

### Why no --track/--no-track?

Git's `--track` and `--no-track` flags control whether a new branch automatically sets up an upstream relationship. Libra omits these from `branch` because tracking configuration is handled explicitly through `--set-upstream-to` or at switch time via `libra switch --track`. This separation keeps `branch` focused on ref creation and avoids the confusing implicit behavior where `git branch feature origin/feature` silently configures tracking. When an agent creates a branch, it should know whether tracking was configured -- explicit is better than implicit.

### Why --contains/--no-contains with aliases --with/--without?

The `--contains` and `--no-contains` flags mirror Git for compatibility, but Libra adds shorter `--with` and `--without` aliases. These read more naturally in scripts (`libra branch --with v2.0`) and reduce typing. The flags accept an optional commit argument that defaults to HEAD, which covers the most common case of "which branches include my current work?"

### Why SQLite-backed refs?

Git stores branch references as individual files under `.git/refs/heads/`. This causes problems at scale: monorepos with thousands of branches suffer from filesystem overhead, packed-refs contention, and race conditions during concurrent updates. Libra stores all references in a SQLite database (`libra.db`), which provides:

- **Atomic transactions**: branch create/delete/rename are single-transaction operations with no risk of partial writes or corrupted ref files.
- **Efficient queries**: listing branches, filtering with `--contains`, and upstream lookups are SQL queries rather than directory scans.
- **Concurrency safety**: SQLite's WAL mode handles concurrent reads and serialized writes without external locking.
- **Consistent snapshots**: operations that read multiple refs (like `--contains` filtering) see a consistent view of the ref store.

The trade-off is that refs are not directly inspectable as plain files. Libra compensates with structured JSON output for tooling integration.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Create branch | `git branch <name>` | `libra branch <name>` | `jj branch create <name>` |
| Create from commit | `git branch <name> <commit>` | `libra branch <name> <commit>` | `jj branch create <name> -r <rev>` |
| List branches | `git branch [-l]` | `libra branch [-l]` | `jj branch list` |
| Delete (safe) | `git branch -d <name>` | `libra branch -d <name>` | `jj branch delete <name>` |
| Delete (force) | `git branch -D <name>` | `libra branch -D <name>` | `jj branch delete <name>` (always force) |
| Rename | `git branch -m <old> <new>` | `libra branch -m <old> <new>` | Not supported |
| Set upstream | `git branch -u <upstream>` | `libra branch -u <upstream>` | N/A (no upstream concept) |
| Show current | `git branch --show-current` | `libra branch --show-current` | `jj log -r @` |
| Remote branches | `git branch -r` | `libra branch -r` | `jj branch list --all` |
| All branches | `git branch -a` | `libra branch -a` | `jj branch list --all` |
| Contains filter | `git branch --contains <commit>` | `libra branch --contains <commit>` | `jj log -r 'branches() & ancestors(<rev>)'` |
| Auto-track | `git branch --track` | N/A (use `switch --track`) | N/A |
| Structured output | No | `--json` / `--machine` | `--template` |
| Fuzzy suggestions | No | Levenshtein-based "did you mean" | No |

## Error Handling

| Scenario | Error Code | Hint |
|----------|-----------|------|
| Invalid start point or missing branch | `LBR-CLI-003` | "use 'libra branch -l' to list branches" + fuzzy suggestions |
| Invalid branch name | `LBR-CLI-002` | "branch names cannot contain spaces, '..', '@{', or control characters." |
| Branch already exists | `LBR-CONFLICT-002` | "delete it first or choose a different name." |
| Current branch cannot be deleted | `LBR-REPO-003` | "switch to a different branch first." |
| Branch not fully merged (safe delete) | `LBR-REPO-003` | "use '-D' to force-delete." |
| Locked/internal branch | `LBR-CLI-003` | -- |
| HEAD is detached (rename/upstream) | `LBR-REPO-003` | -- |
| Failed to write refs | `LBR-IO-002` | -- |
| Storage query failed | `LBR-IO-001` | -- |
| Stored reference corrupt | `LBR-REPO-002` | -- |
