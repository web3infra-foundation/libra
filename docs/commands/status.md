# `libra status`

Show the working tree status.

**Alias:** `st`

## Synopsis

```
libra status [OPTIONS]
```

## Description

`libra status` shows the state of the working tree and staging area: which files are staged
for the next commit, which have modifications not yet staged, and which are untracked. It also
reports the current branch, detached HEAD state, upstream tracking information, and active
merge/rebase/bisect state when one is in progress.

The command computes the diff between HEAD, the index, and the working tree to classify files
into staged, unstaged, and untracked categories. It supports multiple output formats: a
human-readable long format (default), a short format (`--short`), a machine-readable porcelain
format, and structured JSON for agent consumption.

## Options

### `-s, --short`

Give the output in the short format. Each file is shown on a single line with a two-character
status code (e.g., `M ` for staged modified, ` M` for unstaged modified, `??` for untracked).
Conflicts with `--porcelain`.

```bash
libra status -s
libra status --short
```

### `--porcelain [VERSION]`

Output in a machine-readable format. Accepts an optional version argument: `v1` (default) or
`v2` for extended format. Conflicts with `--short`.

```bash
libra status --porcelain
libra status --porcelain v1
libra status --porcelain v2
```

### `--branch`

Include branch information in short or porcelain output. Shows the current branch and its
tracking relationship on the first line.

```bash
libra status --short --branch
libra status --porcelain --branch
```

### `--show-stash`

Show the number of stash entries. Only effective in standard (long) output mode.

```bash
libra status --show-stash
```

### `--ignored`

Include ignored files in the output.

```bash
libra status --ignored
```

### `--untracked-files <MODE>`

Control how untracked files are displayed. Accepted values: `normal` (default, shows untracked
directories but not their contents), `all` (recursively lists files within untracked directories),
`no` (hides untracked files entirely).

```bash
libra status --untracked-files=no
libra status --untracked-files=all
```

### `--exit-code`

Exit with code 1 if the working tree has changes, exit 0 if clean. Useful for scripting
and CI pipelines to detect dirty state without parsing output.

```bash
libra status --exit-code
libra status --quiet --exit-code   # silent dirty check
```

### `-z`

Terminate porcelain entries with a NUL byte (`\0`) instead of a newline, for safe
machine parsing of paths. Implies `--porcelain=v1` when no other format is given;
combine with `--short` or `--porcelain=v2` for NUL-terminated short/v2 records. In
NUL short output, staged renames are emitted as `R  <new>\0<orig>\0`; in v2 output,
rename rows use `<path>\0<orig_path>` instead of the non-`-z` TAB separator.

```bash
libra status -z                 # NUL-terminated porcelain v1
libra status --porcelain=v2 -z  # NUL-terminated porcelain v2
```

## Configuration

`libra status` reads these Git-compatible config keys when the matching CLI flag is not explicit:

| Key | Values | Effect |
|---|---|---|
| `status.showUntrackedFiles` | `no`, `normal`, `all` | Default for `--untracked-files` |
| `status.branch` | boolean | Default for `--branch` in short/porcelain output |
| `status.short` | boolean | Default to short output when no porcelain/`-z` format is requested |

Invalid `status.*` values are non-fatal: Libra emits a warning, falls back to the built-in default,
and honors global `--exit-code-on-warning` by exiting 9 after successful command execution.

## Common Commands

```bash
libra status
libra status --short
libra status --json
libra status --exit-code
```

## Human Output

Default human mode writes the status summary to `stdout`.

Clean working tree:

```text
On branch main
nothing to commit, working tree clean
```

With changes:

```text
On branch main
Your branch is ahead of 'origin/main' by 2 commits.
  (use "libra push" to publish your local commits)

Changes to be committed:
        new file:   src/feature.rs
        modified:   src/lib.rs

Changes not staged for commit:
        modified:   README.md

Untracked files:
        notes.txt
```

Detached HEAD:

```text
HEAD detached at abc1234
nothing to commit, working tree clean
```

Short format (`--short`):

```text
A  src/feature.rs
M  src/lib.rs
 M README.md
?? notes.txt
```

`--quiet` suppresses all `stdout` output. Combined with `--exit-code`, it acts as a
silent dirty check (exit 1 if dirty, exit 0 if clean).

## Structured Output

`libra status` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "branch",
      "name": "main"
    },
    "has_commits": true,
    "upstream": {
      "remote_ref": "origin/main",
      "ahead": 2,
      "behind": 0,
      "gone": false
    },
    "staged": {
      "new": ["src/feature.rs"],
      "modified": ["src/lib.rs"],
      "deleted": []
    },
    "unstaged": {
      "modified": ["README.md"],
      "deleted": []
    },
    "untracked": ["notes.txt"],
    "ignored": [],
    "renames": [],
    "repo_state": null,
    "is_clean": false
  }
}
```

Clean working tree:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "branch",
      "name": "main"
    },
    "has_commits": true,
    "upstream": null,
    "staged": {
      "new": [],
      "modified": [],
      "deleted": []
    },
    "unstaged": {
      "modified": [],
      "deleted": []
    },
    "untracked": [],
    "ignored": [],
    "renames": [],
    "repo_state": null,
    "is_clean": true
  }
}
```

Detached HEAD:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "detached",
      "oid": "abc1234def5678..."
    },
    "has_commits": true,
    "upstream": null,
    "staged": { "new": [], "modified": [], "deleted": [] },
    "unstaged": { "modified": [], "deleted": [] },
    "untracked": [],
    "ignored": [],
    "renames": [],
    "repo_state": null,
    "is_clean": true
  }
}
```

### Schema Notes

- `head.type` is `"branch"` or `"detached"`
- When on a branch, `head.name` is the branch name; when detached, `head.oid` is the commit hash
- `upstream` is `null` when no tracking branch is configured or HEAD is detached
- `upstream.gone` is `true` when the remote tracking branch no longer exists
- `upstream.ahead` / `upstream.behind` are `null` when `gone` is `true`
- `repo_state` is `null`, `"merge"`, `"rebase"`, or `"bisect"`. Cherry-pick has no
  persistent sequencer state in Libra today, so status does not report it.
- `is_clean` is `true` when all staged, unstaged, and untracked lists are empty
- `has_commits` is `false` in a freshly initialized repository with no commits
- `stash_entries` (optional, integer): present only when `--show-stash` is
  passed. Counts the entries on the stash stack (matching `libra stash list`)
  and may be `0`. Omitted entirely without `--show-stash` so JSON consumers
  can distinguish "stash subsystem not queried" from "stash subsystem
  queried, returned zero" — i.e. the field's *presence* signals an
  explicit opt-in, not the existence of stashed work.

## Design Rationale

### Porcelain v1 and v2

`libra status --porcelain` (no version) emits Git's classic v1 short-format
layout (`XY <path>` per file). `libra status --porcelain v2` emits the
extended v2 line layout — for each tracked file:

```text
1 XY <sub> <mode_HEAD> <mode_index> <mode_worktree> <hash_HEAD> <hash_index> <path>
2 XY <sub> <mode_HEAD> <mode_index> <mode_worktree> <hash_HEAD> <hash_index> R<score> <path>\t<orig_path>
u XY <sub> <mode_stage1> <mode_stage2> <mode_stage3> <mode_worktree> <hash_stage1> <hash_stage2> <hash_stage3> <path>
```

Untracked entries collapse to `? <path>` and ignored entries to `! <path>`,
matching Git's own v2 encoding. Staged renames are detected automatically with
a fixed 50% similarity threshold and render as `2` rows with `R<score>`;
unmerged index stages render as `u` rows. Type changes render with `T` in the
short and porcelain v2 status columns.

Most consumers should still prefer `--json` (or `--machine` for compact
single-line JSON): the JSON envelope carries the same staged/unstaged/
untracked partitioning plus upstream tracking, `renames`, `repo_state`, and `stash_entries`, and
is far easier to parse than v2's positional text columns. Use
`--porcelain v2` only when you specifically need Git-compatible output
for tooling that already speaks the v2 grammar.

### Repository State Hints

Long human output reports in-progress merge, rebase, and bisect sessions before the file list.
Rebase hints include `libra rebase --continue` and `libra rebase --abort`; bisect output only
states that bisect is in progress because bisect does not have `--continue`/`--abort` flags.
Short and porcelain modes keep stdout machine-readable and omit these human hints; use JSON
`repo_state` when scripts need the same state.

### Explicit `--exit-code` instead of implicit behavior

Git's `git status` always exits 0 regardless of repository state, and checking for dirty state
requires `git diff --exit-code` or parsing `git status --porcelain` output. Libra adds an
explicit `--exit-code` flag that returns exit 1 when the working tree is dirty. This is
intentionally opt-in (rather than default) to avoid breaking scripts that check `$?` after
`libra status`. Combined with `--quiet`, it provides a zero-output, exit-code-only dirty check
that is cleaner than parsing text output.

### `--show-stash` in standard mode only

The `--show-stash` flag only affects the long (standard) human-readable output, not short or
porcelain formats. This matches Git's behavior where `--show-stash` appends a stash summary
line to the long format. In JSON output, stash information could be added to the envelope in a
future iteration without needing a separate flag, since JSON consumers can simply ignore fields
they do not need.

### Enhanced upstream tracking info in JSON

Git's porcelain v1 does not include upstream tracking information; porcelain v2 adds a header
line with ahead/behind counts. Libra's JSON output always includes a full `upstream` object
with `remote_ref`, `ahead`, `behind`, and `gone` fields when a tracking branch is configured.
This rich upstream data is critical for AI agents and CI tools that need to determine whether
a branch needs to be pushed or pulled, without having to run separate `libra log` or
`libra branch -vv` commands.

## Parameter Comparison: Libra vs Git vs jj

| Parameter / Flag | Git | jj | Libra |
|---|---|---|---|
| Show status | `git status` | `jj status` / `jj st` | `libra status` |
| Short format | `git status -s` / `--short` | N/A (always short) | `libra status -s` / `--short` |
| Porcelain v1 | `git status --porcelain` | N/A | `libra status --porcelain` |
| Porcelain v2 | `git status --porcelain=v2` | N/A | `libra status --porcelain v2` |
| Branch info in short | `git status -sb` | Always shown | `libra status --short --branch` |
| Show stash count | `git status --show-stash` | N/A | `libra status --show-stash` (standard mode) |
| Show ignored files | `git status --ignored` | N/A | `libra status --ignored` |
| Untracked files control | `git status -u<mode>` | N/A (always shows) | `libra status --untracked-files=<mode>` |
| `status.*` config | `status.showUntrackedFiles`, `status.branch`, `status.short` | N/A | supported |
| Exit code for dirty | `git diff --exit-code` | N/A | `libra status --exit-code` |
| Quiet mode | `git status -q` | N/A | `libra status --quiet` (global flag) |
| Column display | `git status --column` | N/A | N/A |
| Ahead/behind display | `git status -sb` (text only) | N/A | Human + structured `upstream` object in JSON |
| Find renames | `git status -M` | Automatic | automatic staged rename detection (fixed 50% threshold) |
| Ignore submodules | `git status --ignore-submodules` | N/A | N/A (no submodules) |
| Structured JSON output | N/A | N/A | `--json` / `--machine` |
| Error hints | Minimal | Minimal | Every error type has an actionable hint |

## Exit Code Behavior

| Flag | Clean | Dirty |
|------|-------|-------|
| (default) | exit 0 | exit 0 |
| `--exit-code` | exit 0 | exit 1 |

`--exit-code` enables a silent dirty check useful for scripting. When combined with
`--quiet`, no output is produced -- only the exit code signals the repository state.

## Error Handling

Every `StatusError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Index file corrupted | `LBR-REPO-002` | 128 | "the index file may be corrupted" |
| Invalid path encoding | `LBR-CLI-003` | 129 | "path contains invalid characters" |
| Failed to hash a file | `LBR-IO-001` | 128 | -- |
| Cannot list working directory | `LBR-IO-001` | 128 | -- |
| Working directory not found | `LBR-REPO-001` | 128 | -- |
| Bare repository | `LBR-REPO-003` | 128 | "this operation must be run in a work tree" |

## Compatibility Notes

- `--porcelain v2` emits ordinary `1`, rename `2`, and unmerged `u` rows with mode/hash metadata
- `status.showUntrackedFiles`, `status.branch`, and `status.short` config defaults are honored; CLI flags override them
- JSON includes `repo_state` for merge/rebase/bisect, and human long output prints rebase/bisect recovery hints
- jj's `jj status` always uses a short format and does not distinguish staged from unstaged changes (jj has no staging area)
- Git's rename-control flags and config (`--find-renames` / `-M`, `--renames`, `--no-renames`, `status.renames`, `status.renameLimit`) are deferred; Libra status currently applies automatic staged rename detection at a fixed 50% threshold
- Copy detection (`C` rows), pathspec filtering, and submodule status details are deferred
- `--column` display is not supported
