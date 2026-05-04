# `libra stash`

Stash the changes in a dirty working directory away.

## Synopsis

```
libra stash push [-m <message>]
libra stash pop [<stash>]
libra stash list
libra stash apply [<stash>]
libra stash drop [<stash>]
libra stash show [<stash>] [--name-only | --name-status]
libra stash branch <branch> [<stash>]
libra stash clear [--force]
```

## Description

`libra stash` saves your local modifications to a new stash entry and reverts the working directory to match HEAD. The modifications can be restored later with `libra stash pop` or `libra stash apply`. If `stash push` is run on a clean working tree, it exits successfully as a no-op and reports that there are no local changes to save.

Stash entries are stored as specially-structured commit objects under `.libra/refs/stash`, with a flat-file list tracking the stash stack. Each stash captures both the index state and worktree state at the time of creation.

## Options

### Subcommands

#### `push`

Save your local modifications to a new stash and clean the working directory.

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Message | `-m` | `--message` | Optional descriptive message for the stash entry. If omitted, a default "WIP on `<branch>`: `<short-hash>` ..." message is generated. |

```bash
# Save with default message
libra stash push

# Save with a descriptive message
libra stash push -m "work in progress on feature X"
```

#### `pop`

Apply the top stash entry and remove it from the stash list. Equivalent to `apply` followed by `drop`.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}` (the most recent stash). |

```bash
# Pop the latest stash
libra stash pop

# Pop a specific stash
libra stash pop stash@{2}
```

#### `list`

List all stash entries with their index, message, and stash ID.

```bash
libra stash list
```

#### `apply`

Apply a stash entry without removing it from the stash list. Useful when you want to apply the same stash to multiple branches.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}`. |

```bash
libra stash apply
libra stash apply stash@{1}
```

#### `drop`

Remove a single stash entry from the stash list without applying it.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}`. |

```bash
libra stash drop
libra stash drop stash@{1}
```

#### `show`

Show the file-level changes recorded in a stash entry.

| Argument / Flag | Description |
|-----------------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}`. |
| `--name-only` | Show only the changed file names, one per line. |
| `--name-status` | Show file names prefixed with the status code (`A` / `M` / `D`). |

`--name-only` and `--name-status` are mutually exclusive in human render mode; the JSON envelope always carries the full `files` list with status, regardless of which hint is set.

```bash
# File-level summary of stash@{0}
libra stash show

# Inspect a specific stash entry
libra stash show stash@{1}

# File names only
libra stash show --name-only
```

#### `branch`

Create a new branch from a stash entry, apply the stash on it, then drop the entry. Useful when a stash applies cleanly only on a branch that no longer exists, or when you want to resume the stashed work as a normal branch.

| Argument | Description |
|----------|-------------|
| `<branch>` | Name of the new branch to create. Required. |
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}`. |

```bash
# Branch off the latest stash and drop it
libra stash branch hotfix

# Branch off a specific stash
libra stash branch hotfix stash@{2}
```

#### `clear`

Remove every stash entry. Outside `--json` / `--machine` mode, `--force` is required to prevent accidental data loss.

| Flag | Description |
|------|-------------|
| `--force` | Skip the confirmation requirement. Mandatory in human mode; bypassed automatically in JSON / machine mode. |

```bash
# Human mode (refuses without --force)
libra stash clear --force

# JSON mode (--force not required)
libra stash clear --json
```

### Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## Common Commands

```bash
# Save current changes
libra stash push

# Save with a message
libra stash push -m "work in progress on feature X"

# List stashes
libra stash list

# Apply and remove the latest stash
libra stash pop

# Apply without removing
libra stash apply

# Drop a specific stash
libra stash drop stash@{1}

# JSON output for scripting
libra stash list --json
```

## Human Output

**`stash push`** (with changes):

```text
Saved working directory and index state WIP on main: abc1234 ...
```

**`stash push`** (clean working tree):

```text
No local changes to save
```

**`stash list`**:

```text
stash@{0}: WIP on main: abc1234 initial commit
stash@{1}: On main: work in progress on feature X
```

**`stash pop` / `stash apply`**:

```text
On branch main
Changes restored from stash@{0}
```

**`stash drop`**:

```text
Dropped stash@{0} (abc1234...)
```

## Structured Output (JSON)

When `--json` is passed, all subcommands produce a JSON envelope:

```json
{
  "command": "stash",
  "data": { "action": "push", "message": "WIP on main: abc1234 ...", "stash_id": "..." }
}
```

On a clean working tree, `stash push --json` returns:

```json
{
  "command": "stash",
  "data": { "action": "noop", "message": "No local changes to save" }
}
```

The `data.action` field is one of: `noop`, `push`, `pop`, `apply`, `drop`, `list`, `show`, `branch`, `clear`.

### `list` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "list",
    "entries": [
      { "index": 0, "message": "WIP on main: ...", "stash_id": "abc1234..." }
    ]
  }
}
```

### `pop` / `apply` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "pop",
    "index": 0,
    "stash_id": "abc1234...",
    "branch": "main"
  }
}
```

### `drop` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "drop",
    "index": 0,
    "stash_id": "abc1234..."
  }
}
```

### `show` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "show",
    "stash": "stash@{0}",
    "stash_id": "abc1234...",
    "files": [
      { "path": "src/foo.rs", "status": "M" }
    ],
    "files_changed": {
      "total": 1,
      "added": 0,
      "modified": 1,
      "deleted": 0
    }
  }
}
```

The structured envelope always emits the full `files` list. The `--name-only` / `--name-status` flags only affect human render output.

### `branch` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "branch",
    "branch": "hotfix",
    "stash": "stash@{0}",
    "stash_id": "abc1234...",
    "applied": true,
    "dropped": true
  }
}
```

### `clear` JSON schema

```json
{
  "command": "stash",
  "data": {
    "action": "clear",
    "cleared_count": 3
  }
}
```

## Design Rationale

### Why no `--keep-index`?

Git's `stash push --keep-index` stashes changes but leaves the index (staged files) intact. This is primarily used for testing staged changes before committing. In practice, this flag creates confusing state because the worktree is reset but the index is not, leading to a mismatch that surprises users. Libra's approach is simpler: stash captures everything, and `libra restore --staged` provides a cleaner way to manipulate the index independently.

### Why no `--include-untracked` / `--all`?

Git's `--include-untracked` (`-u`) and `--all` (`-a`) stash untracked and ignored files respectively. These flags are rarely needed and add significant complexity to the stash storage format (requiring additional tree objects). In Libra, untracked files are not part of the version-controlled state and should be managed through other means (e.g., `libra clean` for removal, or simply leaving them in place).

### Why a curated subcommand model?

Git's stash has grown organically and supports `git stash` as a shorthand for `git stash push`, plus `git stash save` (deprecated) and the plumbing pair `git stash create` / `git stash store`. Libra exposes the eight subcommands users actually reach for in practice: `push`, `pop`, `list`, `apply`, `drop`, `show`, `branch`, and `clear`. The plumbing pair (`create` / `store`) and the `save` shorthand are deferred — see [`docs/improvement/compatibility/declined.md`](../improvement/compatibility/declined.md) sections D8 and D9. This keeps the surface aligned with stock Git for everyday workflows while leaving rarely-used plumbing out of the maintained surface.

### Why `stash@{N}` syntax instead of plain indices?

Libra preserves Git's `stash@{N}` reference syntax for familiarity. Users migrating from Git can use the same muscle memory. The parser also accepts bare integers in some contexts, but the canonical form remains `stash@{N}`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Push (save changes) | `stash push` | `stash push` / `stash save` (deprecated) | N/A (no stash; use `jj new` to shelve) |
| Message | `-m <message>` | `-m <message>` | N/A |
| Keep index | Not supported | `--keep-index` / `--no-keep-index` | N/A |
| Include untracked | Not supported | `-u` / `--include-untracked` | N/A |
| Include all (ignored too) | Not supported | `-a` / `--all` | N/A |
| Pathspec (partial stash) | Not supported | `-- <pathspec>...` | N/A |
| Pop | `stash pop [ref]` | `stash pop [--index] [<stash>]` | N/A |
| Apply | `stash apply [ref]` | `stash apply [--index] [<stash>]` | N/A |
| Drop | `stash drop [ref]` | `stash drop [<stash>]` | N/A |
| List | `stash list` | `stash list [<log-options>]` | N/A |
| Show file-level summary | `stash show [<stash>] [--name-only \| --name-status]` | `stash show [-p] [<stash>]` | N/A |
| Create branch from stash | `stash branch <branch> [<stash>]` | `stash branch <branch> [<stash>]` | N/A |
| Clear all stashes | `stash clear [--force]` | `stash clear` | N/A |
| Plumbing create/store | Not supported (deferred — see compatibility/declined.md D8/D9) | `stash create` / `stash store` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Quiet mode | `--quiet` | `-q` / `--quiet` | N/A |

Note: jj does not have a stash command. Its change-based model allows creating anonymous changes with `jj new` that serve a similar purpose to stashing.

## Error Handling

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-REPO-003` | No initial commit |
| `LBR-CLI-002` | Invalid stash reference syntax |
| `LBR-CLI-003` | Stash does not exist |
| `LBR-CONFLICT-001` | Merge conflict during stash apply |
