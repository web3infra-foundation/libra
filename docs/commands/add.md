# `libra add`

Stage file contents for the next commit.

## Synopsis

```
libra add [OPTIONS] [PATHSPEC...]
libra add -A
libra add -u [PATHSPEC...]
libra add --refresh [PATHSPEC...]
```

## Description

`libra add` stages file changes from the working tree into the index, preparing them
for the next `libra commit`. It supports pathspecs, glob patterns, `--dry-run` preview,
and `--refresh` to re-stat already tracked entries without staging new content.

The command resolves pathspecs relative to the current working directory, validates them
against the repository root, and respects `.libraignore` rules. Files tracked by LFS are
automatically staged as pointer files. The `-A` flag stages all changes (adds, modifies,
removes) across the entire working tree, while `-u` updates only tracked files without
adding new ones.

## Options

### `[PATHSPEC...]`

One or more files or directories to stage. Paths are resolved relative to the current
directory. Required unless `-A`, `-u`, or `--refresh` is specified.

```bash
libra add file.txt
libra add src/ tests/
libra add .
```

### `-A, --all`

Update the index to match the entire working tree. Stages new files, modifications, and
deletions. When no pathspec is given, all files in the working tree are updated. Mutually
exclusive with `-u` and `--refresh`.

```bash
libra add -A
```

### `-u, --update`

Update the index only where it already has entries matching the pathspec. Stages
modifications and deletions of tracked files but does not add new (untracked) files.
Mutually exclusive with `-A` and `--refresh`.

```bash
libra add -u
libra add -u src/
```

### `--refresh`

Refresh index entries for all files currently in the index. Updates only metadata
(timestamps, file size) of existing index entries to match the working tree, without
adding new files or removing entries. Mutually exclusive with `-A` and `-u`.

```bash
libra add --refresh
```

### `-f, --force`

Allow adding files that are otherwise ignored by `.libraignore`.

```bash
libra add -f ignored_file.log
```

### `-n, --dry-run`

Preview what would be staged without actually modifying the index. Output shows which
files would be added, modified, or removed.

```bash
libra add -n file.txt
libra add --dry-run .
```

### `-v, --verbose`

Produce more detailed output, showing per-file actions during staging.

```bash
libra add -v src/
```

### `--ignore-errors`

Continue staging remaining files when individual paths fail. Failed paths are reported
in the output but do not cause the command to exit with an error. The default can be set
via the `add.ignoreErrors` config key (a local/global boolean); an explicit
`--ignore-errors` / `--no-ignore-errors` on the command line overrides the config.

```bash
libra add --ignore-errors src/
```

### `--chmod=(+|-)x`

Record the executable bit in the **index** for the matching entries: `+x` stores mode
`100755`, `-x` stores `100644`. Only the index is changed — the working-tree file's
filesystem permissions are never touched. The candidate set includes already-tracked
entries that match the pathspec even if their content is unchanged. Under
`core.fileMode = false` the mode is still recorded, with a warning.

### `--renormalize`

Re-stage already-tracked files matching the pathspec, implying `-u` (tracked files only;
untracked files are never added, and tracked files deleted from the working tree are
staged as deletions). **Intentionally different from Git:** libra has no clean/CRLF
filter, so this force-rewrites the tracked entries' blobs rather than normalizing line
endings.

### `--pathspec-from-file <file>` / `--pathspec-file-nul`

Read pathspecs from `<file>` (or `-` for stdin) instead of the command line — mutually
exclusive with positional pathspecs. Items are newline-separated unless
`--pathspec-file-nul` is given (then NUL-separated; Git's `add` has no `-z` short option).
The input is capped at 128 MiB. **Partial:** quoted/escaped pathspecs are not unescaped —
bytes are taken verbatim.

### `--ignore-missing`

Only valid together with `--dry-run`. Paths that do not exist in the working tree are
skipped with a warning instead of erroring; paths ignored by `.libraignore` are reported
as ignored. **Intentionally different from Git:** the "would this be ignored even if
missing" check is simplified to "skip missing paths with a warning".

### `--sparse` (declined) · `-N`, `--intent-to-add` (deferred)

`--sparse` is declined — libra does not support sparse checkout. `-N` / `--intent-to-add`
is deferred: the on-disk index format has no intent-to-add bit (see the design notes).
Both return a usage error (exit 129) inside a repository.

## Common Commands

```bash
libra add file.txt
libra add src/
libra add .
libra add -n file.txt
libra add --refresh
libra add --ignore-errors src/
libra add --chmod=+x build.sh              # record the executable bit in the index (not the worktree)
libra add --renormalize .                  # re-stage tracked files (force-rewrite their blobs)
libra add --pathspec-from-file paths.txt   # stage paths read from a file ('-' for stdin)
libra add --pathspec-from-file=- --pathspec-file-nul   # NUL-separated paths from stdin
libra add --dry-run --ignore-missing a b   # preview; skip paths missing from the working tree
```

## Human Output

Default human mode writes the staging summary to `stdout`.

Single file:

```text
add 'src/main.rs' (new file)
```

Multiple files:

```text
add 'src/main.rs' (new file)
add 'src/lib.rs' (modified)
add 'old.txt' (deleted)
```

Dry-run:

```text
add 'src/main.rs' (new file)
add 'src/lib.rs' (modified)
(dry run, no files were staged)
```

Ignored files produce a warning on `stderr`:

```text
warning: all specified paths are ignored by .libraignore
Hint: use '-f' to force staging of ignored files
```

`--quiet` suppresses all `stdout` output but preserves `stderr` warnings.

## Structured Output

`libra add` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": ["src/lib.rs"],
    "removed": ["old.txt"],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": false
  }
}
```

Dry-run:

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["src/main.rs"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [],
    "dry_run": true
  }
}
```

Partial failure with `--ignore-errors`:

```json
{
  "ok": true,
  "command": "add",
  "data": {
    "added": ["good.txt"],
    "modified": [],
    "removed": [],
    "refreshed": [],
    "ignored": [],
    "failed": [
      {"path": "bad.bin", "message": "file too large"}
    ],
    "dry_run": false
  }
}
```

### Schema Notes

- `added` / `modified` / `removed` correspond to new, changed, and deleted files staged
- `refreshed` is populated only when `--refresh` is used
- `ignored` lists paths skipped by `.libraignore`
- `failed` lists paths that failed to stage, each with `path` and `message`
- `dry_run` is `true` when `-n` / `--dry-run` is passed; no files are actually staged

## Design Rationale

### No `--intent-to-add` / `-N`

Git's `--intent-to-add` (`-N`) records an empty blob for untracked files so that they
appear in `git diff` output without actually staging their content. This is a workflow
convenience for reviewing new files before staging them. Libra omits this flag because
`libra status` already shows untracked files clearly, and `libra diff` is designed to
work with the full working tree state. The two-step "intent then stage" workflow adds
cognitive overhead without meaningfully improving the review experience. Users who want
to review new files before committing can use `libra add --dry-run` followed by
`libra diff --staged` after staging.

### No `--patch` / `-p` interactive staging

Git's `--patch` mode provides an interactive hunk-by-hunk staging interface within the
terminal. Libra deliberately omits interactive staging from the CLI `add` command because
the `libra code` TUI provides a richer, visual staging experience with full file and hunk
selection. Interactive terminal prompts are also incompatible with AI agent workflows
(MCP/stdio mode), which are a primary design target for Libra. Keeping `libra add`
non-interactive ensures it works identically in human, scripted, and agent contexts.

### `--refresh` as explicit flag

In Git, `git add --refresh` silently updates stat information for tracked files. Libra
surfaces this as a first-class mode that is mutually exclusive with `-A` and `-u` (enforced
by clap argument groups). This makes the intent explicit: `--refresh` never stages new
content, only updates metadata. The mutual exclusivity prevents confusing combinations like
`-A --refresh` where the user's intent would be ambiguous.

### `.libraignore` instead of `.gitignore`

Libra uses `.libraignore` files for its ignore policy rather than `.gitignore`. This avoids
conflicts when a Libra repository coexists with or is converted from a Git repository, and
makes it clear which VCS owns the ignore rules. The ignore file format is compatible with
Git's pattern syntax (globs, negation with `!`, directory-only patterns with trailing `/`).
`libra init` creates a root `.libraignore` in non-bare repositories, and Git imports or
non-bare clones copy existing `.gitignore` files to matching `.libraignore` files.

## Parameter Comparison: Libra vs Git vs jj

| Parameter / Flag | Git | jj | Libra |
|---|---|---|---|
| Stage a file | `git add file.txt` | N/A (jj auto-tracks) | `libra add file.txt` |
| Stage everything | `git add .` or `git add -A` | N/A (automatic) | `libra add .` or `libra add -A` |
| Update tracked only | `git add -u` | N/A | `libra add -u` |
| Dry-run preview | `git add -n` / `--dry-run` | N/A | `libra add -n` / `--dry-run` |
| Force add ignored | `git add -f` | N/A | `libra add -f` |
| Refresh stat info | `git add --refresh` | N/A | `libra add --refresh` |
| Verbose output | `git add -v` | N/A | `libra add -v` |
| Ignore errors | `git add --ignore-errors` | N/A | `libra add --ignore-errors` |
| Intent to add | `git add -N` / `--intent-to-add` | N/A | N/A (not implemented) |
| Interactive patch | `git add -p` / `--patch` | N/A | N/A (use `libra code` TUI) |
| Interactive select | `git add -i` / `--interactive` | N/A | N/A (use `libra code` TUI) |
| Edit diff before staging | `git add -e` / `--edit` | N/A | N/A |
| Chmod only | `git add --chmod=+x` | N/A | N/A |
| Sparse checkout paths | `git add --sparse` | N/A | N/A |
| Ignore file | `.gitignore` | N/A (jj uses `.gitignore`) | `.libraignore` |
| Structured JSON output | N/A | N/A | `--json` / `--machine` |
| Error hints | Minimal | N/A | Every error type has an actionable hint |

## Error Handling

Every `AddError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Not inside a repository | `LBR-REPO-001` | 128 | "run 'libra init' to create a repository" |
| Pathspec matched nothing | `LBR-CLI-003` | 129 | "check the spelling and use 'libra status' to see what changed" |
| Path outside repository root | `LBR-CLI-003` | 129 | "only files within the repository root can be staged" |
| Invalid path encoding | `LBR-CLI-003` | 129 | "path contains invalid UTF-8 characters" |
| Index file corrupted | `LBR-REPO-002` | 128 | "the index file may be corrupted; try 'libra status' to verify" |
| Failed to save index | `LBR-IO-002` | 128 | "check disk space and file permissions" |
| Refresh failed | `LBR-IO-001` | 128 | -- |
| Entry creation failed | `LBR-IO-002` | 128 | -- |
| Working directory error | `LBR-REPO-001` | 128 | "cannot determine the working tree" |
| Status computation failed | `LBR-REPO-002` | 128 | -- |
| All paths ignored (nothing staged) | `LBR-ADD-001` | 128 | "use -f if you really want to add them" |
| No pathspec and no mode flag | `LBR-CLI-002` | 129 | "maybe you wanted to say 'libra add .'?" |
| Invalid `--chmod` value (not `+x`/`-x`) | `LBR-CLI-003` | 129 | "only '+x' and '-x' are accepted" |
| `--sparse` (declined) | `LBR-CLI-003` | 129 | "libra does not support sparse checkout; remove --sparse" |
| `-N` / `--intent-to-add` (declined) | `LBR-CLI-003` | 129 | "intent-to-add needs extended index capabilities, currently unsupported" |
| `--pathspec-from-file` read failure / over 128 MiB | `LBR-IO-001` | 128 | -- |
| `--dry-run --ignore-missing` skipped a missing path (with `--exit-code-on-warning`) | `LBR-WARN-001` | 9 | -- |

## Compatibility Notes

- jj does not have an `add` command; it automatically tracks all working tree changes
- Libra's `add` is required before `commit`, matching Git's explicit staging model
- `.libraignore` uses the same pattern syntax as `.gitignore` but is a separate file; imports
  and non-bare clones copy `.gitignore` rules instead of deleting or renaming the originals
- LFS-tracked files are automatically converted to pointer files during staging
