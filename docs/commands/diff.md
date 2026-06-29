# `libra diff`

Compare differences between HEAD, the index, the working tree, or two revisions.

## Synopsis

```
libra diff [<pathspec>...]
libra diff --staged [<pathspec>...]
libra diff --old <commit> --new <commit> [<pathspec>...]
libra diff [--name-only | --name-status | --numstat | --stat | --shortstat | --summary]
           [-s | --no-patch] [--exit-code] [--check] [-R] [-z]
libra diff [--algorithm <name>] [--output <file>]
```

## Description

`libra diff` shows changes between different states of the repository. By default it compares the index against the working tree (unstaged changes). With `--staged`, it compares HEAD against the index (staged changes). With `--old` and `--new`, it compares two arbitrary commits.

The diff engine supports multiple algorithms (histogram by default, with myers and myersMinimal as alternatives). Output can be directed to a file with `--output`, and several summary formats are available (`--name-only`, `--name-status`, `--numstat`, `--stat`, `--shortstat`, `--summary`). A status-only check is possible with `-s`/`--no-patch` and `--exit-code`, and `-z`/`--null` makes the name/numstat outputs NUL-terminated for safe scripting. `--word-diff[=<mode>]` re-renders the patch at word granularity (matching Git's structure; like all Libra diffs, the exact word grouping can differ from Git on ambiguous changes, and hunk headers keep Libra's unified-diff format).

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
| Word diff | | `--word-diff[=<mode>]` | Re-render the patch at word granularity. MODE is `plain` (default; removed words `[-…-]`, added `{+…+}`), `color` (highlight in a terminal, no brackets), `porcelain` (one token per line, `-`/`+`/` ` prefixes, `~` for newlines), or `none` (regular patch). Words are whitespace-delimited. Must be written as `--word-diff` or `--word-diff=<mode>`. |
| Numstat | | `--numstat` | Show insertion/deletion counts in a machine-friendly tab-separated format. |
| Stat | | `--stat` | Show a diffstat summary with +/- bar graph. |
| Context lines | `-U<n>` | `--unified=<n>` | Number of context lines around each change in the patch (default 3). Changes only the surrounding context, not the `+`/`-` lines, so `--stat`/`--name-only`/`--numstat` counts are unaffected; the `--json` hunk ranges and line arrays follow `<n>`. |
| Ignore whitespace | `-w` | `--ignore-all-space` | Ignore all whitespace when comparing lines. A change that is only whitespace is not reported (the file drops out if that is its only change); context lines are shown from the new side. Affected files are re-diffed, so `--stat`/`--name-only`/`--numstat`/JSON all reflect the whitespace-ignored result. Honors `-U<n>`. |
| Ignore whitespace amount | `-b` | `--ignore-space-change` | Ignore changes in the *amount* of whitespace: runs of whitespace are treated as a single space and trailing whitespace is ignored, but the presence of whitespace still matters (`a  b` matches `a b`; `a b` still differs from `ab`). Same re-diff/drop behavior as `-w`. `-w` wins if both are given. |
| Ignore EOL whitespace | | `--ignore-space-at-eol` | Ignore whitespace changes at end of line only; leading and internal whitespace compare exactly. Same re-diff/drop behavior as `-w`. `-w`/`-b` win if combined. |
| Ignore blank lines | | `--ignore-blank-lines` | Ignore changes whose lines are all blank (truly empty): a change consisting only of added/removed empty lines is not reported (an added/deleted file whose only content is blank lines is still listed with zero counts), while a blank line near a real edit is shown in full. Re-diffs affected files (so `--stat`/`--name-only`/`--numstat`/JSON reflect the result); honors `-U<n>`. Composes with a whitespace flag (`-w`/`-b`/`--ignore-space-at-eol`): a line that is blank after whitespace-normalization then counts as blank. |
| Shortstat | | `--shortstat` | Show only the trailing summary line of `--stat` (files changed / insertions / deletions), omitting a clause when its count is zero. |
| Summary | | `--summary` | Show a condensed summary of created files, deleted files, and (with `-M`) renames (no line for plain content edits); mode-only changes are not surfaced. |
| No patch | `-s` | `--no-patch` | Suppress the patch (diff body). Combine with `--exit-code` for a status-only check. |
| Exit code | | `--exit-code` | Still print the diff, but exit with code 1 when there are differences (0 otherwise). Unlike `--quiet`, the diff is not suppressed. |
| NUL output | `-z` | `--null` | NUL-terminate `--name-only`/`--name-status`/`--numstat` records (and split the `--name-status` status and path into separate NUL fields); other modes are unaffected. |
| Whitespace check | | `--check` | Instead of the diff, warn about whitespace errors on added lines (trailing whitespace and space-before-tab in the indent), printing `<path>:<line>: <message>` and exiting 2 when any are found. Git's blank-at-eof check is not performed; takes precedence over other output modes. |
| Reverse | `-R` | `--reverse` | Swap the two sides so additions become deletions and vice-versa (the patch that would undo the change). |
| Text | `-a` | `--text` | Treat all files as text: diff the content even of files detected as binary (a NUL byte in either side, or non-UTF-8 content), suppressing the "Binary files … differ" line. Libra's diff is text-based, so a non-UTF-8 change that is identical after lossy-UTF-8 conversion still shows "Binary files … differ". |
| Binary patch | | `--binary` | Emit a `GIT binary patch` (base85 `literal` chunks for both directions) for binary files instead of "Binary files … differ". The patch is valid and appliable, but its compressed bytes are not byte-identical to Git's (Libra deflates with a different zlib and always emits `literal`, not Git's smaller-of-literal/delta). |
| No external diff | | `--no-ext-diff` | Disable the external diff driver for this run, forcing the built-in engine. |
| External diff | | `--ext-diff` | Allow the configured external diff driver (`diff.external`) to generate each file's patch (it is used by default when configured; this flag is the explicit opposite of `--no-ext-diff`). |
| Color moved lines | | `--color-moved[=<mode>]` | In colored output, color lines that were deleted in one place and added in another with a distinct color (removed → bold magenta, added → bold cyan). Bare `--color-moved` and the block modes (`default`/`zebra`/`blocks`/`dimmed-zebra`) are accepted but approximated by `plain` — every moved line is colored; Libra does not implement Git's conservative moved-block significance/zebra striping. `--color-moved=no` / `--no-color-moved` turns it off. Only affects colored output (a terminal or `--color=always`). |
| No moved-line color | | `--no-color-moved` | Do not color moved lines differently (the default; countermands an earlier `--color-moved`). |
| Find renames | `-M[<n>]` | `--find-renames[=<n>]` | Detect renames: a deleted + added pair whose content is similar enough is reported as one rename (`similarity index N%` / `rename from`/`rename to`, and `R<score>` / `old => new` in the name-status/numstat/summary surfaces). Bare `-M` uses a 50% threshold; `-M<n>` / `-M<n>%` / `--find-renames=<n>` set it (a bare integer is read like Git as `0.<digits>`, so `-M5` is 50% and `-M100%` is exact-only). The similarity index matches Git for typical content (a different chunk hash means contrived hash-collision inputs can differ); when several files are renamed at once the chosen old/new pairings can differ from Git's. Off by default (Libra does not auto-enable via `diff.renames`); a pathspec cannot directly follow a bare `-M`/`--find-renames` — place it before the flag or after `--`. |
| No renames | | `--no-renames` | Turn off rename detection (the default; countermands an earlier `-M`/`--find-renames`). |
| Relative | | `--relative[=<path>]` | Restrict the diff to a directory and show paths relative to it: with a value, `<path>` is resolved from the current directory; bare `--relative` uses the current directory. Files outside the directory are excluded and the prefix is stripped from displayed paths (also in `--stat` and JSON). With an external `diff.external` driver, the file-set restriction still applies but the prefix is NOT stripped from the driver's verbatim output. |
| No relative | | `--no-relative` | Show full repo-root-relative paths. This is Libra's default; accepted for Git parity and takes precedence over `--relative`. |
| No indent heuristic | | `--no-indent-heuristic` | Disable the indent heuristic for hunk boundaries. Accepted no-op: Libra's diff does not apply Git's indent heuristic. (Git's `--indent-heuristic` is not supported.) |
| Textconv | | `--textconv` | Run textconv filters to make content human-diffable: a file whose `diff=<driver>` attribute (in the root `.libra_attributes`) names a driver with a configured `diff.<driver>.textconv` command has each side converted by that command before diffing. On by default for `diff` (like Git); this flag is the explicit opposite of `--no-textconv`. The resulting patch is for reading, not applying. A failing textconv command is a fatal error; textconv is not applied under `--check` or when `diff.external` is active; only the root `.libra_attributes` is consulted. |
| No textconv | | `--no-textconv` | Diff the raw content, skipping any textconv filter (countermands an earlier `--textconv`). |
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

# Patch with a different amount of context (0, or more than the default 3)
libra diff -U0
libra diff --unified=5 src/main.rs

# Ignore whitespace-only changes (re-indentation shows nothing)
libra diff -w

# Ignore only changes in the amount of whitespace (a  b == a b)
libra diff -b

# Ignore changes that are only blank lines
libra diff --ignore-blank-lines

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
- `--shortstat` (just the trailing summary line of `--stat`, with zero-count clauses omitted)
- `--summary` (condensed create/delete/rename summary; renames appear with `-M`, mode-only changes are not surfaced)
- `-s` / `--no-patch` suppresses the patch body (for status-only checks)
- `--exit-code` still prints the diff but exits `1` when there are differences
- `-z` / `--null` NUL-terminates `--name-only`/`--name-status`/`--numstat` records (status and path become separate NUL fields under `--name-status`)
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

The `status` field is one of: `added`, `deleted`, `modified`, or `renamed`. A
`renamed` entry (only produced under `-M`/`--find-renames`) additionally carries
`rename_from` (the original path; `path` holds the new name) and `similarity`
(the similarity index as a whole percent), e.g.:

```json
{
  "path": "src/new.txt",
  "status": "renamed",
  "rename_from": "src/old.txt",
  "similarity": 90,
  "insertions": 1,
  "deletions": 1,
  "hunks": [ /* ... */ ]
}
```

A binary file (unless `--text`) carries `binary` as a `[old_size, new_size]` byte-count pair, its `insertions`/`deletions` are `0`, and `hunks` is empty.

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

### `--word-diff` and `--color-words`

`--word-diff[=<mode>]` is supported (see the option above): it re-renders the patch at word granularity in `plain` (default), `color`, `porcelain`, or `none` mode, with whitespace-delimited words. The shorthand `--color-words` and a custom `--word-diff-regex` are not yet implemented.

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
| Short stat | `--shortstat` | `--shortstat` | N/A |
| Summary | `--summary` | `--summary` | `--summary` |
| Suppress patch | `-s` / `--no-patch` | `-s` / `--no-patch` | N/A |
| Exit code | `--exit-code` | `--exit-code` | N/A |
| NUL-terminated output | `-z` / `--null` | `-z` | N/A |
| Whitespace check | `--check` (trailing-ws / space-before-tab) | `--check` | N/A |
| Reverse diff | `-R` / `--reverse` | `-R` | N/A |
| Treat as text | `-a` / `--text` (force content diff of binary files) | `-a` / `--text` | N/A |
| Word diff | `--word-diff[=<mode>]` (no `--color-words`/`--word-diff-regex`) | `--word-diff` / `--color-words` | N/A |
| Binary diff (binary patch) | `--binary` (valid/appliable; compressed bytes differ from Git's) | `--binary` | N/A |
| Context lines | `-U<n>` / `--unified=<n>` (default 3) | `-U<n>` / `--unified=<n>` | `--context <n>` |
| Ignore whitespace | `-w` / `--ignore-all-space` | `-w` / `--ignore-all-space` | N/A |
| Ignore whitespace amount | `-b` / `--ignore-space-change` | `-b` / `--ignore-space-change` | N/A |
| Ignore EOL whitespace | `--ignore-space-at-eol` | `--ignore-space-at-eol` | N/A |
| Ignore blank lines | `--ignore-blank-lines` | `--ignore-blank-lines` | N/A |
| Color | Auto (terminal detection) | `--color` / `--no-color` | `--color` / `--no-color` |
| Disallow external diff | `--no-ext-diff` (disables the configured `diff.external` driver, forcing the built-in engine) | `--no-ext-diff` | N/A |
| External diff tool | `diff.external` + `--ext-diff` / `--no-ext-diff` | `diff.external` + `--ext-diff` / `--no-ext-diff` (GIT_EXTERNAL_DIFF protocol; patch output only) | `--tool <name>` |
| Quiet (exit code only) | `--quiet` | `--quiet` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Rename detection | `-M[<n>]` / `--find-renames[=<n>]` (similarity matches Git for typical content; opt-in, not auto-enabled via `diff.renames`) | `-M` / `--find-renames` | Automatic |
| Moved-line color | `--color-moved[=<mode>]` / `--no-color-moved` (`plain` semantics; block modes approximated) | `--color-moved[=<mode>]` | N/A |
| Textconv | `--textconv` / `--no-textconv` (on by default; `.libra_attributes` `diff=<driver>` + `diff.<driver>.textconv`) | `--textconv` / `--no-textconv` | N/A |
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
