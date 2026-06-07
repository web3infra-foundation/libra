# `libra diff`

Compare differences between HEAD, the index, the working tree, or two revisions.

## Synopsis

```
libra diff [<pathspec>...]
libra diff --staged [<pathspec>...]
libra diff --old <commit> --new <commit> [<pathspec>...]
libra diff [--name-only | --name-status | --numstat | --stat]
libra diff [--algorithm <name>] [--output <file>]
```

## Description

`libra diff` shows changes between different states of the repository. By default it compares the index against the working tree (unstaged changes). With `--staged`, it compares HEAD against the index (staged changes). With `--old` and `--new`, it compares two arbitrary commits.

The diff engine currently accepts `--algorithm=histogram` only. `myers` and `myersMinimal` are parsed for compatibility but fail closed with `LBR-CLI-002` until alternate backends are implemented. Output can be directed to a file with `--output`, and several summary formats are available (`--name-only`, `--name-status`, `--numstat`, `--stat`, `--raw`).

Pathspec arguments filter the diff to only show changes in matching files or directories.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Old commit | | `--old <COMMIT>` | Specifies the "old" side of the comparison. Defaults to HEAD when `--staged`, or the index otherwise. |
| New commit | | `--new <COMMIT>` | Specifies the "new" side. Requires `--old`. Conflicts with `--staged`. |
| Staged | | `--staged` | Compare HEAD against the index (staged changes). Conflicts with `--new`. |
| Pathspec | | positional | One or more files or directories to restrict the diff. |
| Algorithm | | `--algorithm <name>` | Diff algorithm label: `histogram` (default and only supported value). `myers` and `myersMinimal` fail closed with `LBR-CLI-002`. |
| Output file | | `--output <FILENAME>` | Write human-readable output to a file instead of stdout. Ignored in `--json` mode. |
| Name only | | `--name-only` | Show only the names of changed files. |
| Name status | | `--name-status` | Show changed file names with a status letter (`A`/`D`/`M`/`T`; renames/copies use `R<score>`/`C<score>` plus old and new paths). |
| Numstat | | `--numstat` | Show insertion/deletion counts in a machine-friendly tab-separated format. |
| Stat | | `--stat` | Show a diffstat summary with +/- bar graph. |
| Raw | | `--raw` | Emit Git's raw format (`:<old-mode> <new-mode> <old-sha> <new-sha> <status>\t<path>`; renames/copies emit `R<score>`/`C<score>` and both paths). |
| JSON | | `--json` | Emit structured JSON output. |
| Quiet | | `--quiet` | Suppress stdout; exit code 1 if differences exist, 0 otherwise. When combined with `--output`, the file is still written. |
| Exit code | | `--exit-code` | Exit 1 if there are differences (still printing the diff), 0 otherwise. |
| Ignore space change | `-b` | `--ignore-space-change` | Ignore trailing whitespace and treat runs of whitespace as a single space when comparing. |
| Ignore all space | `-w` | `--ignore-all-space` | Ignore all whitespace when comparing lines. |
| Ignore blank lines | | `--ignore-blank-lines` | Ignore changes whose lines are all blank. |
| Unified context | `-U` | `--unified <N>` | Show `<N>` lines of context. Priority: `-U` > `diff.context` > 3. |
| Find renames | `-M` | `--find-renames[=<n>]` | Detect renames at an optional similarity threshold (`-M80`, `-M80%`; default 50%). |
| Find copies | `-C` | `--find-copies[=<n>]` | Detect basic copies (source = a modified/deleted file). |
| No renames | | `--no-renames` | Disable rename/copy detection even if enabled by config. |
| Relative | | `--relative[=<path>]` | Restrict the diff to a subdirectory and show paths relative to it. |
| Word diff | | `--word-diff[=<mode>]` | Word-level diff. `plain` (default) uses `[-del-]`/`{+add+}`; `color` uses ANSI. |
| Word diff regex | | `--word-diff-regex <re>` | Word boundary regex (max 4096 bytes; also `diff.wordRegex`). |
| Function context | `-W` | `--function-context` | Expand each hunk's context to the surrounding function boundaries. |

### Configuration

| Key | Type | Default | Effect |
|-----|------|---------|--------|
| `diff.context` | integer | `3` | Default unified context when `-U` is not given (a non-numeric value is a usage error). |
| `diff.renames` | `false`/`true`/`copy`/`copies` | off (Libra default) | Enable rename (`true`) or rename+copy (`copy`/`copies`) detection without a flag. |
| `diff.renameLimit` | integer | `1000` | Skip inexact rename/copy detection when `deleted × added` exceeds this (warns). |
| `diff.wordRegex` | regex | built-in | Word boundary for `--word-diff`. |
| `diff.noPrefix` | bool | `false` | Omit the `a/`/`b/` path prefixes in unified output. |

### Option Details

**`--old` / `--new`**

Compare two specific commits. `--new` requires `--old` to also be specified:

```bash
# Compare two commits
libra diff --old HEAD~3 --new HEAD

# Compare a tag to HEAD
libra diff --old v1.0 --new HEAD
```

**`--staged`**

Show what has been staged for the next commit:

```bash
libra diff --staged
libra diff --staged src/
```

**`--algorithm`**

Select the diff algorithm label. Only `histogram` is currently implemented:

```bash
libra diff --algorithm histogram
# myers and myersMinimal currently fail closed with LBR-CLI-002
```

**`--output`**

Write diff output to a file. Useful for saving diffs for review:

```bash
libra diff --output changes.patch
libra diff --staged --output staged.diff
```

**Summary formats:**

```bash
# Just file names
libra diff --name-only

# File names with status letters
libra diff --name-status
# Output: M	src/main.rs
#         A	src/new_file.rs

# Machine-friendly counts
libra diff --numstat
# Output: 5	2	src/main.rs

# Visual bar graph
libra diff --stat
# Output:  src/main.rs | 7 +++++--
```

## Common Commands

```bash
# Show unstaged changes
libra diff

# Show staged changes
libra diff --staged

# Compare two commits
libra diff --old HEAD~1 --new HEAD

# Show diff stats for a subdirectory
libra diff --stat src/

# Save diff to a file
libra diff --output my.patch

# Ignore whitespace; widen context to 5 lines
libra diff -w -U5

# Exit 1 if there are differences (for scripts/CI)
libra diff --exit-code

# Detect renames and copies
libra diff -M -C

# Restrict to a subdirectory, paths relative to it
libra diff --relative=src

# Word-level diff and raw machine format
libra diff --word-diff=plain
libra diff --raw

# Expand hunks to whole functions
libra diff -W

# JSON output for agents
libra --json diff --staged
```

## Human Output

Supported output modes:

- Default unified diff (with ANSI color when terminal is detected)
- `--name-only`
- `--name-status`
- `--numstat`
- `--stat`
- `--raw`
- `--quiet` suppresses stdout and uses exit `1` to signal that differences exist

`--output <file>` writes human-readable output to a file. In `--quiet` mode the file is still written, but differences still return exit `1`. In `--json` mode this flag is ignored and output always goes to stdout.

Output is automatically paged when connected to a terminal.

## Structured Output (JSON)

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "index",
    "new_ref": "working tree",
    "files": [
      {
        "path": "tracked.txt",
        "status": "modified",
        "insertions": 1,
        "deletions": 0,
        "hunks": [
          {
            "old_start": 1,
            "old_lines": 1,
            "new_start": 1,
            "new_lines": 2,
            "lines": [" tracked", "+updated"]
          }
        ]
      }
    ],
    "total_insertions": 1,
    "total_deletions": 0,
    "files_changed": 1
  }
}
```

The `status` field is one of: `added`, `deleted`, `modified`, `typechange`, `renamed`, or `copied`. Rename/copy and mode-change entries may also include `old_path`, `similarity`, `old_mode`, `new_mode`, `old_sha`, and `new_sha`.

The `old_ref` and `new_ref` fields indicate what was compared (e.g., `"index"`, `"working tree"`, `"HEAD"`, or a commit reference).

## Design Rationale

### Why `--old` / `--new` instead of positional commit arguments?

Git uses positional arguments for commit comparison (`git diff HEAD~1 HEAD`), but this creates ambiguity with pathspec arguments. Is `git diff main src/` comparing the `main` branch to `src/`, or showing changes in `src/` since `main`? Git resolves this with the `--` separator, but the ambiguity remains a source of confusion.

Libra uses explicit named flags (`--old`, `--new`) to eliminate all ambiguity. Any positional arguments are always pathspecs. This is particularly valuable for AI agents that construct commands programmatically -- there is exactly one way to express each intent.

### Why only histogram as the accepted algorithm?

Libra keeps the `--algorithm` surface narrow until alternate backends are wired through the diff engine. `histogram` is the accepted compatibility label today; `myers` and `myersMinimal` are rejected with `LBR-CLI-002` rather than silently producing a diff with a different backend than requested.

### Why no `--cached` alias?

Git supports both `--staged` and `--cached` as synonyms. This duplication serves no purpose and makes documentation harder to search. Libra standardizes on `--staged` as the single canonical name, matching the terminology used in `libra status` and `libra restore --staged`.

### Why `--new` requires `--old`?

Allowing `--new` without `--old` would create an ambiguous comparison (new compared to what?). Requiring `--old` when `--new` is specified makes the comparison explicit and predictable. For the common case of comparing against HEAD, use `--staged` instead.

### `--word-diff` color is intentionally different

`--word-diff=plain` matches Git's `[-del-]`/`{+add+}` markers exactly. `--word-diff=color` reuses Libra's own colour stack, so the ANSI escape sequences are not guaranteed to be byte-identical to Git's — scripts should depend on `--word-diff=plain` (or `--raw`/`--numstat`), not on the colour encoding.

### Deferred and fail-closed surface

- `--cc` / `--combined` (multi-parent merge diffs) are **deferred**: Libra has no multi-parent diff engine yet, so the flag is not exposed rather than producing wrong output.
- `.gitattributes` diff drivers and `diff.mnemonicprefix` are **deferred**.
- `--algorithm=myers` / `myersMinimal` are **unsupported** and fail closed with `LBR-CLI-002`; only `histogram` validates (backed by git-internal's Myers implementation — the label does not switch backends).

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Unstaged changes | `diff` (default) | `diff` (default) | `jj diff` (shows all uncommitted) |
| Staged changes | `--staged` | `--staged` / `--cached` | N/A (no staging area) |
| Two commits | `--old <A> --new <B>` | `<A> <B>` or `<A>..<B>` | `--from <A> --to <B>` |
| Pathspec filter | `<pathspec>...` | `-- <pathspec>...` | `<paths>...` |
| Algorithm | `--algorithm=histogram` only; `myers`/`myersMinimal` fail closed | `--diff-algorithm` (patience/histogram/myers/minimal) | N/A (uses internal algorithm) |
| Output to file | `--output <file>` | `--output <file>` | N/A (use shell redirect) |
| Name only | `--name-only` | `--name-only` | `--name-only` |
| Name with status | `--name-status` | `--name-status` | N/A |
| Numeric stats | `--numstat` | `--numstat` | `--stat` (combined) |
| Stat summary | `--stat` | `--stat` | `--stat` |
| Summary | Not supported | `--summary` | `--summary` |
| Raw format | `--raw` | `--raw` | N/A |
| Word diff | `--word-diff[=plain\|color]` (color intentionally-different) | `--word-diff` / `--color-words` | N/A |
| Binary diff | `Binary files differ` marker | `--binary` | N/A |
| Context lines | `-U<n>` / `--unified` / `diff.context` | `-U<n>` / `--unified=<n>` | `--context <n>` |
| Function context | `-W` / `--function-context` | `-W` / `--function-context` | N/A |
| Ignore whitespace | `-w` / `-b` / `--ignore-blank-lines` | `-w` / `--ignore-all-space` | N/A |
| Relative subtree | `--relative[=<path>]` | `--relative` | N/A |
| Color | Auto (terminal detection) | `--color` / `--no-color` | `--color` / `--no-color` |
| No prefix | `diff.noPrefix` config | `--no-prefix` / `diff.noPrefix` | N/A |
| External diff tool | Not supported | `--ext-diff` / `--no-ext-diff` | `--tool <name>` |
| Quiet (exit code only) | `--quiet` | `--quiet` | N/A |
| Exit code | `--exit-code` | `--exit-code` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Rename detection | `-M` / `--find-renames` / `diff.renames` | `-M` / `--find-renames` | Automatic |
| Copy detection | `-C` / `--find-copies` (basic) | `-C` / `--find-copies` | N/A |
| Combined (`--cc`) | Deferred | `--cc` / `--combined` | N/A |
| Three-dot diff | Not supported | `<A>...<B>` (merge base) | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Outside a repository | `LBR-REPO-001` | 128 |
| Invalid revision | `LBR-CLI-003` | 129 |
| Failed to read the index or object store | `LBR-REPO-002` | 128 |
| Failed to read a file | `LBR-IO-001` | 128 |
| Failed to write the output file | `LBR-IO-002` | 128 |
