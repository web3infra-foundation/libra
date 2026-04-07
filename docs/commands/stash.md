# `libra stash`

Stash the changes in a dirty working directory away.

## Synopsis

```
libra stash push [-m <message>]
libra stash pop [<stash>]
libra stash list
libra stash apply [<stash>]
libra stash drop [<stash>]
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

The `data.action` field is one of: `noop`, `push`, `pop`, `apply`, `drop`, `list`.

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

## Design Rationale

### Why no `--keep-index`?

Git's `stash push --keep-index` stashes changes but leaves the index (staged files) intact. This is primarily used for testing staged changes before committing. In practice, this flag creates confusing state because the worktree is reset but the index is not, leading to a mismatch that surprises users. Libra's approach is simpler: stash captures everything, and `libra restore --staged` provides a cleaner way to manipulate the index independently.

### Why no `--include-untracked` / `--all`?

Git's `--include-untracked` (`-u`) and `--all` (`-a`) stash untracked and ignored files respectively. These flags are rarely needed and add significant complexity to the stash storage format (requiring additional tree objects). In Libra, untracked files are not part of the version-controlled state and should be managed through other means (e.g., `libra clean` for removal, or simply leaving them in place).

### Why a simplified subcommand model?

Git's stash has grown organically and supports `git stash` as a shorthand for `git stash push`, plus `git stash save` (deprecated), `git stash branch`, `git stash show`, and `git stash create`/`git stash store` (plumbing). Libra keeps only the five essential operations: `push`, `pop`, `list`, `apply`, `drop`. This covers the core workflow (save, restore, inspect, clean up) without the maintenance burden of rarely-used variants.

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
| Show diff | Not supported | `stash show [-p] [<stash>]` | N/A |
| Create branch from stash | Not supported | `stash branch <branch> [<stash>]` | N/A |
| Clear all stashes | Not supported | `stash clear` | N/A |
| Plumbing create/store | Not supported | `stash create` / `stash store` | N/A |
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
