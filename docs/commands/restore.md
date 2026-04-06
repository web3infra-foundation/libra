# libra restore

Restore working tree files.

## Synopsis

```
libra restore [--source <tree-ish>] [--staged] [--worktree] <pathspec>...
```

## Description

`libra restore` restores files in the working tree or index from a given source. By default, it restores files in the working tree from the index. With `--staged`, it restores the index from HEAD (or the specified `--source`).

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `--source <tree-ish>` | `-s` | Restore from the specified commit instead of the index |
| `--staged` | `-S` | Restore the index (unstage). Defaults source to HEAD |
| `--worktree` | `-W` | Restore the working tree (default when `--staged` is not given) |

## Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Emit structured JSON output |
| `--quiet` | Suppress human-readable output |

## JSON Output

```json
{
  "command": "restore",
  "data": {
    "source": "HEAD",
    "worktree": true,
    "staged": false,
    "restored_files": ["src/main.rs"],
    "deleted_files": []
  }
}
```

## Error Codes

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-CLI-003` | Failed to resolve source reference |
| `LBR-CLI-002` | Invalid path encoding |
| `LBR-IO-001` | Failed to read index or object |
| `LBR-IO-002` | Failed to write worktree file |
| `LBR-NET-001` | LFS download failed |

## Examples

```bash
# Restore a file from the index (discard unstaged changes)
libra restore file.txt

# Unstage a file (restore index from HEAD)
libra restore --staged file.txt

# Restore from a specific commit
libra restore --source HEAD~1 src/main.rs

# Restore both working tree and index
libra restore -S -W file.txt

# JSON output
libra restore --json --source HEAD .
```
