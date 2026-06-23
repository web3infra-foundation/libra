# `libra log`

Show commit history.

**Aliases:** `hist`, `history`

## Synopsis

```
libra log [OPTIONS] [-- PATHS...]
```

## Description

`libra log` displays the commit history starting from the current HEAD. It supports multiple
output formats including oneline, custom pretty-print, graph visualization, and structured
JSON. Commits can be filtered by author, date range, and file paths. Diff output (`--patch`,
`--stat`, `--shortstat`, `--name-only`, `--name-status`) can be limited to specific paths.

Human mode preserves the current `--oneline`, `--graph`, `--pretty`, `--stat`, `--patch`, and
related output styles. `--quiet` suppresses human output but still validates the requested
history range.

## Options

### `-n, --number <N>`

Limit the number of commits shown.

```bash
libra log -n 5
libra log --number 10
```

### `--oneline`

Shorthand for `--pretty=oneline --abbrev-commit`. Shows each commit on a single line with
an abbreviated hash and subject.

```bash
libra log --oneline
```

### `--abbrev-commit`

Show abbreviated commit hashes instead of full 40-character hashes.

```bash
libra log --abbrev-commit
```

### `--abbrev <LENGTH>`

Set the length of abbreviated commit hashes.

```bash
libra log --abbrev 8
```

### `--no-abbrev-commit`

Show full commit hashes. Overrides `--abbrev-commit`.

```bash
libra log --no-abbrev-commit
```

### `-p, --patch`

Show the diff (patch) for each commit. Can be combined with path arguments to limit
the diff to specific files.

```bash
libra log -p
libra log -p -- src/main.rs
```

### `--name-only`

Show only the names of changed files for each commit.

```bash
libra log --name-only
```

### `--name-status`

Show names and status (added/modified/deleted) of changed files for each commit.

```bash
libra log --name-status
libra log --name-status -- src/
```

### `--stat`

Show diffstat (file change statistics) for each commit, showing insertions and deletions
per file.

### `--shortstat`

Show only the last line of the `--stat` output: ` N files changed, M insertions(+), K
deletions(-)` for each commit (the insertion/deletion clauses are omitted when zero),
without the per-file breakdown.

```bash
libra log --stat
libra log --shortstat
libra log --range main..feature
libra log --all --oneline
libra log --reverse --oneline
libra log --follow src/main.rs
```

### `--author <PATTERN>`

Filter commits to only those whose author name or email matches the given pattern.

```bash
libra log --author alice
libra log --author "alice@example.com"
```

### `--grep <PATTERN>` / `-i` / `--invert-grep`

Filter commits by message. `--grep` keeps commits whose message contains the
(case-sensitive) substring. `-i` / `--regexp-ignore-case` makes the match
case-insensitive (author/committer matching is already case-insensitive in
Libra). `--invert-grep` keeps commits whose message does *not* match.

```bash
libra log --grep "fix(" -n 20
libra log --grep fix -i              # case-insensitive
libra log --grep WIP --invert-grep   # hide WIP commits
```

### `--since <DATE>`

Show commits more recent than the specified date.

```bash
libra log --since 2026-01-01
libra log --since "2 weeks ago"
```

### `--until <DATE>`

Show commits older than the specified date.

```bash
libra log --until 2026-03-01
```

### `--pretty <FORMAT>`

Custom pretty-print format string. Supports placeholders like `%h` (short hash), `%s`
(subject), `%an` (author name), `%ae` (author email), `%ad` (author date), etc.

```bash
libra log --pretty="%h - %s (%an)"
libra log --pretty="format:%H %s"
```

### `--format <FORMAT>`

Alias for `--pretty=<FORMAT>` (Git's `--format`). Accepts the same preset names and
`%`-placeholder templates as `--pretty`. Mutually exclusive with `--pretty`.

```bash
libra log --format="%h %s"
libra log --format=oneline
```

### `--decorate[=<style>]`

Print ref names (branches, tags) next to commits. Styles: `short` (default), `full`, `no`.

```bash
libra log --decorate
libra log --decorate=full
```

### `--no-decorate`

Do not print ref names. Overrides `--decorate`.

```bash
libra log --no-decorate
```

### `--graph`

Draw a text-based graphical representation of the commit history, showing branching and
merging visually.

```bash
libra log --graph
libra log --oneline --graph
```

### `--range <SPEC>`

Limit history to a revision range. Supported forms:
- `A..B` — commits reachable from `B` but not `A`.
- `A...B` — symmetric difference (commits in `A` or `B` but not their merge base).
- Single ref, e.g. `main` or `HEAD~3`.

```bash
libra log --range main..feature
libra log --range HEAD~3..HEAD
```

### `--all`

Show commits reachable from all local branches and tags instead of only HEAD.

```bash
libra log --all
libra log --all --oneline
```

### `--reverse`

Print commits in reverse chronological order (oldest first).

```bash
libra log --reverse
libra log --reverse --oneline
```

### `--author-date-order`

Order commits by author date instead of committer date (newest first). Libra
sorts purely by timestamp and does not add Git's extra topological ("no parent
before its children") constraint. Relative to Libra's own committer-date default
the order changes only when author and committer dates differ; relative to Git it
can additionally differ wherever the topological constraint would reorder
commits.

```bash
libra log --author-date-order
libra log --author-date-order --oneline
```

### `--follow <FILE>`

Best-effort continuation of a file's history across renames. The file is resolved
relative to the current directory.

```bash
libra log --follow src/main.rs
```

### `--parents` / `--children`

Append commit ids after each commit hash. `--parents` shows each commit's parent
ids; `--children` shows, for each commit, the ids of the *other commits in this
log's output* that have it as a parent (the child map is built over the rendered
commit set, so children outside the shown range are not listed). The ids use the
same abbreviation as the commit hash and appear in the full and oneline formats.
The two flags are mutually exclusive.

```bash
libra log --oneline --parents
libra log --children
```

### `-L <RANGE:FILE>`

Accept Git-style line-range syntax. Full blame-level precision is not yet
implemented; the flag is parsed and applied as a path filter.

```bash
libra log -L1,10:src/main.rs
```

### `[PATHS...]`

Limit diff output to the specified paths. Used with `-p`, `--name-only`, `--name-status`,
`--stat`, or `--shortstat`.

```bash
libra log -- src/
libra log -p -- src/main.rs tests/
```

## Common Commands

```bash
libra log
libra log -n 5
libra log --oneline --graph
libra log --author alice --since 2026-01-01
libra log --name-status src/
libra --json log -n 1
```

## Human Output

Default human mode shows commits in a detailed multi-line format:

```text
commit abc1234def5678901234567890abcdef12345678 (HEAD -> main, origin/main)
Author: Test User <test@example.com>
Date:   Sat Mar 30 10:00:00 2026 +0800

    Add new feature
```

Oneline format:

```text
abc1234 (HEAD -> main) Add new feature
def5678 Fix bug in parser
```

Graph format:

```text
* abc1234 (HEAD -> main) Add new feature
* def5678 Fix bug in parser
|\ 
| * 1234567 Feature branch commit
|/
* 7890abc Initial commit
```

`--quiet` suppresses all human output.

## Structured Output

`--json` / `--machine` returns a filtered, structured commit list:

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [
      {
        "hash": "abc123...",
        "short_hash": "abc1234",
        "author_name": "Test User",
        "author_email": "test@example.com",
        "author_date": "2026-03-30T10:00:00+08:00",
        "committer_name": "Test User",
        "committer_email": "test@example.com",
        "committer_date": "2026-03-30T10:00:00+08:00",
        "subject": "base",
        "body": "",
        "parents": [],
        "refs": ["HEAD -> main"],
        "files": [
          { "path": "tracked.txt", "status": "added" }
        ]
      }
    ],
    "total": 1
  }
}
```

### Schema Notes

- `-n` also applies in JSON mode
- `total` reflects the filtered commit count only when `-n` is not supplied; with `-n`, it is always `null`
- `--graph`, `--pretty`, and `--oneline` do not change the JSON schema
- `--decorate` only affects human rendering; JSON always returns a `refs` array, and auxiliary ref metadata is collected best-effort
- `files` is always a structured change summary and never includes patch text

## Design Rationale

### `--range` instead of positional revision syntax

Git accepts `git log A..B` where the revision expression is a positional argument.
Libra exposes the same semantics via `--range A..B` to keep positional arguments
reserved for pathspecs and to avoid ambiguity with the existing `-- -N` test
pattern.

### `--all` implementation

`--all` enumerates local branches and lightweight tags from the SQLite
`reference` table, collects their tip commits, and walks the union of those
histories.

### `--reverse`

`--reverse` collects the filtered commits and prints them oldest-first. It
applies after all other filters, so `-n` still limits the result set.

### `--author-date-order`

`--author-date-order` sorts the result set by author timestamp (newest first)
instead of the default committer timestamp. The sort is purely by timestamp —
Libra does not impose Git's topological constraint — so it diverges from the
default only when a commit's author and committer dates differ (e.g. after a
rebase or cherry-pick). `--reverse` still flips the final ordering.

### `--follow`

`--follow` performs best-effort rename detection by walking history and matching
removed/added blob hashes. It does not handle complex directory renames or
content-similar renames.

### `-L`

`-L` is parsed and accepted; full blame-level line attribution is not yet
implemented. The flag acts as a path filter in the current release.

### `--graph` with text rendering

Libra implements `--graph` as a text-based ASCII/Unicode graph renderer, similar to Git's
built-in graph output. Unlike GUI tools (GitKraken, SourceTree) or Git's `--format` with
external graph renderers, Libra's graph is rendered inline in the terminal. This keeps the
CLI self-contained and ensures consistent output across platforms. The graph renderer handles
branching, merging, and octopus merges, drawing connecting lines between parent and child
commits.

### JSON always returns `refs` array regardless of `--decorate`

In human output, `--decorate` controls whether ref names (branch, tag) are shown next to
commit hashes. In JSON mode, the `refs` array is always populated regardless of the
`--decorate` flag. This design choice reflects the principle that JSON output should be
maximally informative for programmatic consumers. An AI agent or CI tool parsing JSON output
should not need to remember to pass `--decorate` to get ref information. The `--decorate`
flag only affects the human rendering layer.

## Parameter Comparison: Libra vs Git vs jj

| Parameter / Flag | Git | jj | Libra |
|---|---|---|---|
| Show log | `git log` | `jj log` | `libra log` |
| Limit count | `git log -n <N>` | `jj log -n <N>` | `libra log -n <N>` |
| Oneline format | `git log --oneline` | Default format is oneline | `libra log --oneline` |
| Abbreviated hash | `git log --abbrev-commit` | Default | `libra log --abbrev-commit` |
| Abbrev length | `git log --abbrev=<N>` | N/A | `libra log --abbrev <N>` |
| Full hash | `git log --no-abbrev-commit` | `jj log --no-short-hash` | `libra log --no-abbrev-commit` |
| Show patch | `git log -p` | `jj diff -r <rev>` (separate cmd) | `libra log -p` / `--patch` |
| Name only | `git log --name-only` | N/A | `libra log --name-only` |
| Name and status | `git log --name-status` | N/A | `libra log --name-status` |
| Diffstat | `git log --stat` | `jj diff --stat -r <rev>` | `libra log --stat` |
| Short diffstat | `git log --shortstat` | N/A | `libra log --shortstat` |
| Filter by author | `git log --author=<pat>` | `jj log --author <pat>` (revset) | `libra log --author <pat>` |
| Since date | `git log --since=<date>` | Revset expression | `libra log --since <date>` |
| Until date | `git log --until=<date>` | Revset expression | `libra log --until <date>` |
| Custom format | `git log --pretty=<fmt>` / `--format=<fmt>` | `jj log -T <template>` | `libra log --pretty <fmt>` / `--format <fmt>` |
| Decorate refs | `git log --decorate` | Always shown | `libra log --decorate` |
| No decorate | `git log --no-decorate` | N/A | `libra log --no-decorate` |
| Graph view | `git log --graph` | `jj log` (default has graph) | `libra log --graph` |
| All refs | `git log --all` | `jj log -r 'all()'` | `libra log --all` |
| Branches only | `git log --branches` | `jj log -r 'branches()'` | N/A |
| Remotes only | `git log --remotes` | `jj log -r 'remote_branches()'` | N/A |
| Revision range | `git log A..B` | `jj log -r 'A..B'` | `libra log --range A..B` |
| Grep message | `git log --grep=<pat>` | Revset `description()` | `libra log --grep <pat>` |
| Case-insensitive grep | `git log -i --grep=<pat>` | N/A | `libra log -i --grep <pat>` |
| Invert grep | `git log --invert-grep --grep=<pat>` | N/A | `libra log --invert-grep --grep <pat>` |
| Path filter | `git log -- <paths>` | N/A (use revset) | `libra log -- <paths>` |
| Reverse order | `git log --reverse` | `jj log --reversed` | `libra log --reverse` |
| Author-date order | `git log --author-date-order` | N/A | `libra log --author-date-order` (timestamp-only) |
| Follow renames | `git log --follow <file>` | N/A | `libra log --follow <file>` |
| Structured JSON output | N/A | N/A | `--json` / `--machine` |
| Error hints | Minimal | Minimal | Every error type has an actionable hint |

## Error Handling

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Outside a repository | `LBR-REPO-001` | 128 | -- |
| Empty branch or empty HEAD | `LBR-REPO-003` | 128 | "create a commit first before running 'libra log'" |
| Invalid date argument | `LBR-CLI-002` | 129 | -- |
| Invalid `--decorate` option | `LBR-CLI-002` | 129 | -- |
| Invalid object name | `LBR-CLI-003` | 129 | "check the revision name and try again" |
| Corrupted commit/tree/blob | `LBR-REPO-002` | 128 | -- |
| Failed to read historical objects | `LBR-REPO-002` | 128 | -- |

## Compatibility Notes

- `--branches` and `--remotes` are not yet implemented
- `--all` traverses local branches and lightweight tags; remote tracking refs and
  stashes are not included
- Revision range syntax is available via `--range A..B` and `--range A...B`;
  positional `git log A..B` syntax is not supported
- `--follow` uses best-effort rename detection and may miss complex renames
- `-L` is accepted but does not yet provide blame-level line precision
- `--reverse` is supported
- `--author-date-order` is supported (timestamp-only; no topological constraint)
- jj's log uses a template language (`-T`) for formatting; Libra uses Git-compatible `--pretty` format strings
- In JSON mode, `files` contains structured change summaries; patch text is never included in JSON output
