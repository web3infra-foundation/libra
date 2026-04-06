# `libra restore`

Restore working tree files or index entries from a source.

**Alias:** `unstage`

## Synopsis

```
libra restore [--source <tree-ish>] [--staged] [--worktree] <pathspec>...
```

## Description

`libra restore` restores files in the working tree or index from a given source. By default (when neither `--staged` nor `--worktree` is specified), it restores files in the working tree from the index -- effectively discarding unstaged changes. With `--staged`, it restores the index from HEAD (or the specified `--source`), which unstages files. With both `-S` and `-W`, it restores both the index and working tree simultaneously.

The `<pathspec>` argument is required and accepts one or more file paths or directory paths. The special path `.` restores all files.

When a source commit contains files that do not exist in the current worktree, those files are created. When the current worktree contains files that do not exist in the source, those files are deleted. The output reports both `restored_files` and `deleted_files` separately.

LFS-managed files are automatically downloaded from the LFS server when restoring from a commit that references LFS pointers.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Pathspec | | positional (required) | One or more files or directories to restore. Use `.` for all files. |
| Source | `-s` | `--source <tree-ish>` | Restore from the specified commit or tree-ish instead of the default source. When omitted, the default source depends on the mode: index for worktree restore, HEAD for staged restore. |
| Staged | `-S` | `--staged` | Restore the index (unstage files). Defaults the source to HEAD if `--source` is not given. |
| Worktree | `-W` | `--worktree` | Restore the working tree. This is the default when `--staged` is not given. |
| JSON | | `--json` | Emit structured JSON output. |
| Quiet | | `--quiet` | Suppress human-readable output. |

### Option Details

**`--source` / `-s`**

Specify a commit, tag, or any tree-ish as the restore source:

```bash
# Restore from the previous commit
libra restore --source HEAD~1 src/main.rs

# Restore from a specific commit hash
libra restore -s abc1234 lib/
```

**`--staged` / `-S`**

Restores the index from HEAD (or `--source`), effectively unstaging files:

```bash
# Unstage a file
libra restore --staged file.txt

# Unstage all files
libra restore --staged .
```

**`--worktree` / `-W`**

Explicitly targets the working tree. This is the default when `--staged` is not specified, so it is only needed when combining with `--staged`:

```bash
# Restore both index and working tree
libra restore -S -W file.txt
```

## Common Commands

```bash
# Discard unstaged changes to a file (restore from index)
libra restore file.txt

# Unstage a file (restore index from HEAD)
libra restore --staged file.txt

# Restore from a specific commit
libra restore --source HEAD~1 src/main.rs

# Restore both working tree and index
libra restore -S -W file.txt

# Restore everything from HEAD
libra restore --source HEAD .

# JSON output for scripting
libra restore --json --source HEAD .
```

## Human Output

```text
Restored src/main.rs
Restored lib/utils.rs
Deleted old_file.txt
```

`--quiet` suppresses all output.

## Structured Output (JSON)

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

When restoring from the index (no `--source` specified for worktree restore), the `source` field is `null`.

## Design Rationale

### Why separate from checkout?

Git's `checkout` command serves two very different purposes: switching branches and restoring files. This overloading is widely recognized as one of Git's worst UX decisions. Git itself addressed this by introducing `git restore` (for files) and `git switch` (for branches) in Git 2.23. Libra follows this split from the start, making `restore` exclusively about file content and never about branch operations.

### Why explicit `--worktree` / `--staged` flags?

Git's `restore` defaults to worktree-only restoration and requires `--staged` to target the index. Libra follows the same convention but makes the flags orthogonal and composable:

- No flag: worktree only (from index).
- `--staged`: index only (from HEAD).
- `--staged --worktree`: both targets.

This explicit model eliminates the confusion in Git's `checkout` where `git checkout -- file` restores the worktree and `git checkout HEAD -- file` restores both worktree and index, a distinction that many users never internalize.

### Why is `--source` auto-set to HEAD for `--staged`?

When unstaging files, the natural source is HEAD (the last commit). Requiring `--source HEAD` every time would be tedious and error-prone. Libra auto-defaults to HEAD when `--staged` is used without `--source`, matching Git's behavior and user expectations.

### Why require pathspec?

Unlike `git restore` which can operate on the entire worktree with `--worktree`, Libra requires at least one pathspec argument. This prevents accidental restoration of the entire working tree. Use `.` as a pathspec when you intentionally want to restore everything.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Pathspec | `<pathspec>...` (required) | `<pathspec>...` (optional) | `jj restore <paths>...` |
| Source commit | `-s` / `--source <tree-ish>` | `-s` / `--source <tree>` | `--from <revision>` |
| Target worktree | `-W` / `--worktree` | `-W` / `--worktree` (default) | Default behavior |
| Target index/staging | `-S` / `--staged` | `-S` / `--staged` | N/A (no staging area) |
| Both targets | `-S -W` | `-S -W` | N/A |
| Overlay mode | Not supported | `--overlay` / `--no-overlay` | N/A |
| Conflict resolution | Not supported | `--ours` / `--theirs` / `--merge` | `--restore-descendants` |
| Patch mode | Not supported | `-p` / `--patch` | N/A |
| Progress | Not supported | `--progress` / `--no-progress` | N/A |
| Target revision | Not supported | N/A | `--to <revision>` |
| Restore changes into | Not supported | N/A | `--changes-in <revision>` |
| JSON output | `--json` | Not supported | N/A |
| Quiet mode | `--quiet` | Not supported | N/A |

Note: jj's `restore` operates on revisions rather than a staging area, restoring the content of one revision into another. It does not distinguish between staged and unstaged changes.

## Error Handling

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-CLI-003` | Failed to resolve source reference |
| `LBR-CLI-002` | Invalid path encoding |
| `LBR-IO-001` | Failed to read index or object |
| `LBR-IO-002` | Failed to write worktree file |
| `LBR-NET-001` | LFS download failed |
