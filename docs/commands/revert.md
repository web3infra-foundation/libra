# libra revert

Revert some existing commits.

## Synopsis

```
libra revert [-n] <commit>
```

## Description

`libra revert` creates a new commit that undoes the changes introduced by the specified commit. This is useful for safely undoing changes while preserving history.

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `--no-commit` | `-n` | Stage the revert changes without creating a commit |

## Arguments

| Argument | Description |
|----------|-------------|
| `<commit>` | Commit to revert (hash, branch name, or HEAD) |

## Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## JSON Output

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234...",
    "short_reverted": "abc1234",
    "new_commit": "def5678...",
    "short_new": "def5678",
    "no_commit": false,
    "files_changed": 3
  }
}
```

When `--no-commit` is used, `new_commit` and `short_new` are `null`.

## Error Codes

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-REPO-003` | Detached HEAD state |
| `LBR-CLI-003` | Invalid commit reference |
| `LBR-CLI-002` | Merge commit revert not supported |
| `LBR-CONFLICT-001` | Conflict during revert |
| `LBR-IO-001` | Failed to load object |
| `LBR-IO-002` | Failed to save object or update HEAD |

## Examples

```bash
# Revert the most recent commit
libra revert HEAD

# Revert a specific commit
libra revert abc1234

# Revert without auto-committing
libra revert -n HEAD

# JSON output for agents
libra revert --json HEAD
```
