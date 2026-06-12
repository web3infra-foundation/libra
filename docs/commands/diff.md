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

The diff engine supports multiple algorithms (histogram by default, with myers and myersMinimal as alternatives). Output can be directed to a file with `--output`, and several summary formats are available (`--name-only`, `--name-status`, `--numstat`, `--stat`).

Pathspec arguments filter the diff to only show changes in matching files or directories.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Old commit | | `--old <COMMIT>` | Specifies the "old" side of the comparison. Defaults to HEAD when `--staged`, or the index otherwise. |
| New commit | | `--new <COMMIT>` | Specifies the "new" side. Requires `--old`. Conflicts with `--staged`. |
| Staged | | `--staged` | Compare HEAD against the index (staged changes). Conflicts with `--new`. |
| Pathspec | | positional | One or more files or directories to restrict the diff. |
| Algorithm | | `--algorithm <name>` | Diff algorithm: `histogram` (default), `myers`, or `myersMinimal`. |
| Output file | | `--output <FILENAME>` | Write human-readable output to a file instead of stdout. Ignored in `--json` mode. |
| Name only | | `--name-only` | Show only the names of changed files. |
| Name status | | `--name-status` | Show changed file names with a status letter (A/D/M). |
| Numstat | | `--numstat` | Show insertion/deletion counts in a machine-friendly tab-separated format. |
| Stat | | `--stat` | Show a diffstat summary with +/- bar graph. |
| JSON | | `--json` | Emit structured JSON output. |
| Quiet | | `--quiet` | Suppress stdout; exit code 1 if differences exist, 0 otherwise. When combined with `--output`, the file is still written. |

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

Select the diff algorithm. Histogram (the default) generally produces more readable diffs for code:

```bash
libra diff --algorithm myers
libra diff --algorithm myersMinimal
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

The `status` field is one of: `added`, `deleted`, `modified`.

The `old_ref` and `new_ref` fields indicate what was compared (e.g., `"index"`, `"working tree"`, `"HEAD"`, or a commit reference).

## Design Rationale

### Why `--old` / `--new` instead of positional commit arguments?

Git uses positional arguments for commit comparison (`git diff HEAD~1 HEAD`), but this creates ambiguity with pathspec arguments. Is `git diff main src/` comparing the `main` branch to `src/`, or showing changes in `src/` since `main`? Git resolves this with the `--` separator, but the ambiguity remains a source of confusion.

Libra uses explicit named flags (`--old`, `--new`) to eliminate all ambiguity. Any positional arguments are always pathspecs. This is particularly valuable for AI agents that construct commands programmatically -- there is exactly one way to express each intent.

### Why histogram as the default algorithm?

Git defaults to the Myers algorithm for historical reasons. The histogram algorithm (introduced in Git 2.0 as an option) generally produces more readable diffs for source code because it is better at identifying moved blocks and avoids pathological cases with repeated lines. Libra defaults to histogram for better out-of-the-box quality. Myers and myersMinimal remain available for compatibility and edge cases.

### Why no `--cached` alias?

Git supports both `--staged` and `--cached` as synonyms. This duplication serves no purpose and makes documentation harder to search. Libra standardizes on `--staged` as the single canonical name, matching the terminology used in `libra status` and `libra restore --staged`.

### Why `--new` requires `--old`?

Allowing `--new` without `--old` would create an ambiguous comparison (new compared to what?). Requiring `--old` when `--new` is specified makes the comparison explicit and predictable. For the common case of comparing against HEAD, use `--staged` instead.

### Why no `--word-diff` or `--color-words`?

These Git options provide alternative diff presentations that are useful for prose but rarely needed for code. Libra focuses on the unified diff format that is universally understood by tools and AI agents. Word-level diffing can be added as a future enhancement if demand warrants it.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Unstaged changes | `diff` (default) | `diff` (default) | `jj diff` (shows all uncommitted) |
| Staged changes | `--staged` | `--staged` / `--cached` | N/A (no staging area) |
| Two commits | `--old <A> --new <B>` | `<A> <B>` or `<A>..<B>` | `--from <A> --to <B>` |
| Pathspec filter | `<pathspec>...` | `-- <pathspec>...` | `<paths>...` |
| Algorithm | `--algorithm` (histogram/myers/myersMinimal) | `--diff-algorithm` (patience/histogram/myers/minimal) | N/A (uses internal algorithm) |
| Output to file | `--output <file>` | `--output <file>` | N/A (use shell redirect) |
| Name only | `--name-only` | `--name-only` | `--name-only` |
| Name with status | `--name-status` | `--name-status` | N/A |
| Numeric stats | `--numstat` | `--numstat` | `--stat` (combined) |
| Stat summary | `--stat` | `--stat` | `--stat` |
| Summary | Not supported | `--summary` | `--summary` |
| Word diff | Not supported | `--word-diff` / `--color-words` | N/A |
| Binary diff | Not supported | `--binary` | N/A |
| Context lines | Not supported | `-U<n>` / `--unified=<n>` | `--context <n>` |
| Ignore whitespace | Not supported | `-w` / `--ignore-all-space` | N/A |
| Color | Auto (terminal detection) | `--color` / `--no-color` | `--color` / `--no-color` |
| External diff tool | Not supported | `--ext-diff` / `--no-ext-diff` | `--tool <name>` |
| Quiet (exit code only) | `--quiet` | `--quiet` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Rename detection | Not supported | `-M` / `--find-renames` | Automatic |
| Copy detection | Not supported | `-C` / `--find-copies` | N/A |
| Three-dot diff | Not supported | `<A>...<B>` (merge base) | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Outside a repository | `LBR-REPO-001` | 128 |
| Invalid revision | `LBR-CLI-003` | 129 |
| Failed to read the index or object store | `LBR-REPO-002` | 128 |
| Failed to read a file | `LBR-IO-001` | 128 |
| Failed to write the output file | `LBR-IO-002` | 128 |
