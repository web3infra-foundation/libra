# `libra restore`

Restore working tree files or index entries from a source.

**Alias:** `unstage`

## Synopsis

```
libra restore [--source <tree-ish>] [--staged] [--worktree] [--overlay|--no-overlay] <pathspec>...
libra restore [--source <tree-ish>] [--staged] [--worktree] [--overlay|--no-overlay] --pathspec-from-file=<file> [--pathspec-file-nul]
libra restore (--ours | --theirs) <pathspec>...
libra restore (--merge | --conflict=<merge|diff3>) <pathspec>...
libra restore --ignore-unmerged [--source <tree-ish>] <pathspec>...
```

## Description

`libra restore` restores files in the working tree or index from a given source. By default (when neither `--staged` nor `--worktree` is specified), it restores files in the working tree from the index -- effectively discarding unstaged changes. With `--staged`, it restores the index from HEAD (or the specified `--source`), which unstages files. With both `-S` and `-W`, it restores both the index and working tree simultaneously.

For new workflows, use `libra restore` directly. `libra checkout -- <path>` and `libra checkout <tree-ish> -- <path>` are accepted only as Git-compatible aliases for this path-restore behavior.

Either positional `<pathspec>` arguments or `--pathspec-from-file=<file>` are required. Pathspecs accept one or more file paths or directory paths. The special path `.` restores all files.

When a source commit contains files that do not exist in the current worktree, those files are created. In the default `--no-overlay` mode, tracked files that exist in the target being restored but not in the source are removed from that target. `--overlay` keeps those tracked files instead. The output reports both `restored_files` and `deleted_files` separately.

LFS-managed files are automatically downloaded from the LFS server when restoring from a commit that references LFS pointers.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Pathspec | | positional | One or more files or directories to restore. Use `.` for all files. Mutually exclusive with `--pathspec-from-file`. |
| Source | `-s` | `--source <tree-ish>` | Restore from the specified commit or tree-ish instead of the default source. When omitted, the default source depends on the mode: index for worktree restore, HEAD for staged restore. |
| Staged | `-S` | `--staged` | Restore the index (unstage files). Defaults the source to HEAD if `--source` is not given. |
| Worktree | `-W` | `--worktree` | Restore the working tree. This is the default when `--staged` is not given. |
| Ours | `-2` | `--ours` | For an unmerged path, write conflict stage 2 (our side) to the working tree. Mutually exclusive with `--theirs`, `--source`, `--staged`, and `--ignore-unmerged`. |
| Theirs | `-3` | `--theirs` | For an unmerged path, write conflict stage 3 (their side) to the working tree. Same exclusions as `--ours`. |
| Merge | | `--merge` | Re-create conflict markers from unmerged index stages in the working tree. The index is left unmerged. |
| Conflict style | | `--conflict <merge\|diff3>` | Re-create conflict markers with the selected style. Passing this flag implies `--merge`. |
| Ignore unmerged | | `--ignore-unmerged` | Skip unmerged paths instead of erroring; the remaining paths still restore. |
| Overlay | | `--overlay` | Keep tracked paths that are missing from the source. |
| No overlay | | `--no-overlay` | Remove tracked paths missing from the source. This is the default. |
| Pathspec file | | `--pathspec-from-file <file>` | Read pathspecs from a file, or from stdin when `<file>` is `-`. |
| NUL pathspec file | | `--pathspec-file-nul` | Treat `--pathspec-from-file` input as NUL-separated instead of newline-separated. |
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

**Conflict-stage restore: `--ours` / `-2`, `--theirs` / `-3`, `--merge`, `--conflict`, `--ignore-unmerged`**

When a merge leaves a path unmerged, the index holds up to three conflict stages: stage 1 (the merge base), stage 2 ("ours" — the current branch), and stage 3 ("theirs" — the branch being merged). After editing a conflict-marked file you can take one whole side back:

```bash
# Take our side of the conflict
libra restore --ours file.txt

# Take their side of the conflict
libra restore --theirs file.txt

# Re-create merge conflict markers after editing the conflicted file
libra restore --merge file.txt

# Re-create conflict markers with the merge base included
libra restore --conflict=diff3 file.txt
```

These flags read the conflict stages and rewrite **only the working tree** — the index is intentionally left unmerged, so `libra status` still reports the conflict until you stage a resolution with `libra add`. They are worktree-only by design and therefore reject `--source` and `--staged` at the CLI layer (`LBR-CLI-002`, exit code 129). `--ours`, `--theirs`, and `--merge` are mutually exclusive; `--conflict=<style>` selects the marker style for the merge path. If the requested stage is absent (for example a modify/delete conflict has no "their" version), the command fails with `LBR-CONFLICT-001` and exit 128.

A plain `libra restore` over an unmerged path refuses to act and reports `path '<file>' is unmerged` (`LBR-CONFLICT-001`, exit 128) so a conflict is never silently overwritten or skipped. Pass `--ignore-unmerged` to skip the unmerged paths and restore the rest:

```bash
# Restore everything from HEAD, skipping still-conflicted paths
libra restore --ignore-unmerged --source HEAD .
```

`--merge` and `--conflict` reject binary or very large conflicted files before writing conflict markers; use `--ours` or `--theirs` for those cases.

**Overlay and pathspec files**

`restore` defaults to no-overlay mode: tracked files missing from the source are removed from the restored target. Use `--overlay` to keep them:

```bash
# Keep tracked files that are absent from HEAD~1
libra restore --source HEAD~1 --overlay .
```

Pathspec files are useful for large scripted restores:

```bash
# Newline-separated pathspecs
libra restore --pathspec-from-file=paths.txt

# NUL-separated pathspecs from stdin
printf 'src/app.rs\0README.md\0' | libra restore --pathspec-from-file=- --pathspec-file-nul
```

In NUL mode, pathspec bytes are preserved literally except for the separator. In newline mode, Libra trims CRLF and surrounding whitespace and does not implement Git's C-quoting / `core.quotePath` decoding.

> **Not yet supported:** `--conflict=zdiff3`, `-p` / `--patch`, Git C-quoting for non-NUL `--pathspec-from-file`, and restore-time `core.autocrlf` renormalization are deferred. See [COMPATIBILITY.md](../../COMPATIBILITY.md).

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

# Take our / their side of a merge conflict
libra restore --ours file.txt
libra restore --theirs file.txt

# Restore from HEAD, skipping still-conflicted paths
libra restore --ignore-unmerged --source HEAD .

# Re-create conflict markers
libra restore --merge file.txt
libra restore --conflict=diff3 file.txt

# Keep tracked files missing from the source
libra restore --source HEAD~1 --overlay .

# Read pathspecs from stdin
printf 'src/app.rs\nREADME.md\n' | libra restore --pathspec-from-file=-

# JSON output for scripting
libra restore --json --source HEAD .
```

## Human Output

```text
Updated 3 path(s) from HEAD
```

The confirmation reports a count over the union of files restored *and*
deleted (i.e. when a tracked file is removed in the source it gets
deleted from the worktree/index). When `--source` is omitted, the
source label is `HEAD` for `--staged` restores and `the index` for
worktree-only restores:

```text
Updated 1 path(s) from the index
```

`--quiet` suppresses all output. If neither a restored nor a deleted
path matched, no confirmation is emitted (so a no-op restore is
silent).

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

Git's `checkout` command serves two very different purposes: switching branches and restoring files. This overloading is widely recognized as one of Git's worst UX decisions. Git itself addressed this by introducing `git restore` (for files) and `git switch` (for branches) in Git 2.23. Libra follows this split from the start, making `restore` the preferred command for file content and never for branch operations. `checkout -- <path>` remains available only as a compatibility alias for users bringing Git muscle memory.

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
| Overlay mode | `--overlay` / `--no-overlay` (default) | `--overlay` / `--no-overlay` | N/A |
| Conflict resolution | `--ours` / `-2`, `--theirs` / `-3`, `--merge`, `--conflict=merge\|diff3` (worktree-only); `zdiff3` deferred | `--ours` / `--theirs` / `--merge` | `--restore-descendants` |
| Skip unmerged | `--ignore-unmerged` | `--ignore-unmerged` | N/A |
| Pathspec from file | `--pathspec-from-file` / `--pathspec-file-nul`; non-NUL C-quoting deferred | `--pathspec-from-file` / `--pathspec-file-nul` | N/A |
| Patch mode | Not supported (deferred) | `-p` / `--patch` | N/A |
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
| `LBR-CONFLICT-001` | Path is unmerged and no conflict-resolution flag was given; `--ours`/`--theirs` requested a missing conflict stage; or `--merge`/`--conflict` rejected a binary or oversized conflicted file (exit 128) |

> Mutually exclusive flags (`--ours`/`--theirs`/`--merge`/`--conflict`/`--source`/`--staged`/`--ignore-unmerged`, or `--overlay` with `--no-overlay`) are rejected as `LBR-CLI-002` with exit code 129.
