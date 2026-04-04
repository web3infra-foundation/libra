# libra stash

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

`libra stash` saves your local modifications to a new stash entry and reverts the working directory to match HEAD. The modifications can be restored later with `libra stash pop` or `libra stash apply`.

## Subcommands

### push

Save your local modifications to a new stash and clean the working directory.

| Option | Description |
|--------|-------------|
| `-m <message>` | Optional message for the stash entry |

### pop

Apply the top stash entry and remove it from the stash list. Equivalent to `apply` followed by `drop`.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}` |

### list

List all stash entries.

### apply

Apply a stash entry without removing it from the stash list.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}` |

### drop

Remove a single stash entry from the stash list.

| Argument | Description |
|----------|-------------|
| `<stash>` | Stash reference, e.g. `stash@{1}`. Defaults to `stash@{0}` |

## Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## JSON Output

When `--json` is passed, all subcommands produce a JSON envelope:

```json
{
  "command": "stash",
  "data": { "action": "push", "message": "WIP on main: abc1234 ...", "stash_id": "..." }
}
```

The `data.action` field is one of: `push`, `pop`, `apply`, `drop`, `list`.

### list JSON schema

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

## Error Codes

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-REPO-003` | No local changes to save / no initial commit |
| `LBR-CLI-002` | Invalid stash reference syntax |
| `LBR-CLI-003` | Stash does not exist |
| `LBR-CONFLICT-001` | Merge conflict during stash apply |

## Examples

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
