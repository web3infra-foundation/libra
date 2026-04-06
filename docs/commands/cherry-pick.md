# libra cherry-pick

Apply the changes introduced by some existing commits.

## Synopsis

```
libra cherry-pick [-n] <commit>...
```

## Description

`libra cherry-pick` applies the changes introduced by the specified commits onto the current branch. This is useful for selectively applying commits from one branch to another without merging.

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `--no-commit` | `-n` | Stage changes without creating a commit (single commit only) |

## Arguments

| Argument | Description |
|----------|-------------|
| `<commit>...` | One or more commits to cherry-pick (hashes or refs) |

## Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## JSON Output

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234...",
        "short_source": "abc1234",
        "new_commit": "def5678...",
        "short_new": "def5678"
      }
    ],
    "no_commit": false
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
| `LBR-CLI-002` | Multiple commits with --no-commit / merge commit |
| `LBR-CONFLICT-001` | Conflict during cherry-pick |

## Examples

```bash
# Cherry-pick a single commit
libra cherry-pick abc1234

# Cherry-pick multiple commits
libra cherry-pick abc1234 def5678

# Cherry-pick without auto-committing
libra cherry-pick -n abc1234

# JSON output for agents
libra cherry-pick --json abc1234
```
