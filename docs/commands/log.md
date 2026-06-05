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
`--stat`, `--name-only`, `--name-status`) can be limited to specific paths.

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

```bash
libra log --stat
```

### `--author <PATTERN>`

Filter commits to only those whose author name or email matches the given pattern.

```bash
libra log --author alice
libra log --author "alice@example.com"
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

### `--committer <PATTERN>`

Filter commits to those whose committer name or email matches the given pattern
(case-insensitive substring), symmetric with `--author`.

```bash
libra log --committer alice
```

### `--grep <PATTERN>` / `-i`, `--regexp-ignore-case`

Filter commits whose message matches the given **regular expression** (regex crate
syntax, case-sensitive by default). `^`/`$` match at line boundaries within the
commit message (so `^fix` matches the subject and `^Signed-off-by` matches a footer).
Add `-i` / `--regexp-ignore-case` for case-insensitive matching. Patterns are capped
at 4 KiB; an invalid or oversized pattern fails fast with `LBR-CLI-002` (exit 129).

> This is an intentional difference from older Libra behavior: `--grep` was previously
> a plain substring match. A plain pattern (e.g. `fix`) still behaves as a literal.

```bash
libra log --grep '^fix'
libra log --grep 'CVE-\d{4}' -i
```

### `--merges` / `--no-merges` / `--min-parents <N>` / `--max-parents <N>`

Filter by parent count. `--merges` shows only merge commits (≥ 2 parents, an alias for
`--min-parents=2`); `--no-merges` shows only non-merge commits (an alias for
`--max-parents=1`). `--min-parents`/`--max-parents` set explicit bounds.

```bash
libra log --merges
libra log --no-merges
libra log --max-parents=1
```

### `--first-parent`

When walking history, follow only the first parent of each merge commit, so the output
stays on the mainline and never descends into merged side branches.

```bash
libra log --first-parent
libra log --merges --first-parent
```

### `-S <STRING>` / `-G <REGEX>` (pickaxe)

Search history by content change. These two have **different semantics**, matching `git log`:

- **`-S <string>`** shows commits where the **number of occurrences** of the literal
  `string` changed between a file's parent-side and child-side *full content*. A change
  that leaves the count the same (e.g. editing the same line elsewhere) does **not** match.
- **`-G <regex>`** shows commits where any **added or removed diff line** matches the regex
  (occurrence counts are irrelevant). `-G` patterns share the regex `--grep` rules (4 KiB
  cap; an invalid pattern fails with `LBR-CLI-002`, exit 129).

`-S` and `-G` are mutually exclusive, combine with pathspec (AND), and a load/diff failure
on a corrupt object surfaces as `LBR-REPO-002` (exit 128) rather than skipping the commit.

```bash
libra log -S secret_key            # commits that add/remove an occurrence of secret_key
libra log -G 'TODO\(.*\)'          # commits that touch a line matching the regex
libra log -S api_token -- src/     # scope the search to a path
```

### `--pretty <FORMAT>`

Custom pretty-print format string. A bare template, `format:<template>`, and
`tformat:<template>` are all accepted; `tformat:` appends a trailing newline to each
commit's output. Unknown `%`-escapes are preserved literally.

Supported placeholders:

| Placeholder | Expands to |
|-------------|------------|
| `%H` / `%h` | Full / abbreviated commit hash |
| `%T` / `%t` | Full / abbreviated tree hash |
| `%P` / `%p` | Full / abbreviated parent hashes (space-separated; empty for a root commit) |
| `%s` / `%f` | Subject line / sanitized subject (spaces → `-`) |
| `%b` / `%B` | Message body (subject stripped) / raw body |
| `%an` / `%ae` / `%ad` | Author name / email / date |
| `%cn` / `%ce` / `%cd` | Committer name / email / date |
| `%d` | Ref decorations |
| `%n` | A newline |
| `%x<HH>` | The byte for the two hex digits (e.g. `%x09` → tab, `%x20` → space) |

```bash
libra log --pretty="%h - %s (%an)"
libra log --pretty="format:%H %s"
libra log --pretty="tformat:%h%x09%s"   # hash <tab> subject, one per line
```

Abbreviated hashes (`%h`/`%t`/`%p`) follow `--abbrev-commit` / `--no-abbrev-commit`.
The abbreviation length never slices a multi-byte boundary, and every placeholder degrades
safely (root commits, empty bodies, and malformed `%x` escapes never error). Under
`--json` / `--machine`, `--pretty` is a no-op — the structured schema is unchanged.
Note: Git's `--log-size` is not implemented; passing it is rejected by the argument parser.

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

### `[PATHS...]`

Limit diff output to the specified paths. Used with `-p`, `--name-only`, `--name-status`,
or `--stat`.

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

### No `--all` / `--branches` / `--remotes` flags yet

Git's `--all` shows commits reachable from all refs (branches, tags, stashes), while
`--branches` and `--remotes` filter to local or remote branches respectively. Libra
currently walks the commit graph from HEAD only. Implementing `--all` requires a ref
enumeration pass over the SQLite `reference` table to collect all branch tips and tag
targets, then merging multiple commit walks into a single timeline. This is planned but
not yet implemented. The current single-HEAD walk covers the most common use case
(inspecting the current branch history) and avoids the complexity of multi-root graph
merging.

### No revision range (`A..B`) syntax yet

Git's revision range syntax (`main..feature`, `main...feature`, `HEAD~3..HEAD`) is a
powerful but complex feature that requires a full revision parser supporting symbolic refs,
ancestry operators (`~`, `^`), and set operations (difference, symmetric difference). Libra
does not yet implement a revision parser. The `-n` flag and `--since`/`--until` date filters
provide basic history scoping. A full revision range parser is on the roadmap and will
support both Git-compatible syntax and additional Libra-specific extensions.

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
| Filter by author | `git log --author=<pat>` | `jj log --author <pat>` (revset) | `libra log --author <pat>` |
| Since date | `git log --since=<date>` | Revset expression | `libra log --since <date>` |
| Until date | `git log --until=<date>` | Revset expression | `libra log --until <date>` |
| Custom format | `git log --pretty=<fmt>` | `jj log -T <template>` | `libra log --pretty <fmt>` |
| Decorate refs | `git log --decorate` | Always shown | `libra log --decorate` |
| No decorate | `git log --no-decorate` | N/A | `libra log --no-decorate` |
| Graph view | `git log --graph` | `jj log` (default has graph) | `libra log --graph` |
| All refs | `git log --all` | `jj log -r 'all()'` | N/A (not yet implemented) |
| Branches only | `git log --branches` | `jj log -r 'branches()'` | N/A |
| Remotes only | `git log --remotes` | `jj log -r 'remote_branches()'` | N/A |
| Revision range | `git log A..B` | `jj log -r 'A..B'` | N/A (not yet implemented) |
| Grep message | `git log --grep=<pat>` | Revset `description()` | `libra log --grep <regex>` (regex; `-i` for case-insensitive) |
| Committer filter | `git log --committer=<pat>` | N/A | `libra log --committer <pat>` |
| Pickaxe (string count) | `git log -S<string>` | N/A | `libra log -S <string>` |
| Pickaxe (diff-line regex) | `git log -G<regex>` | N/A | `libra log -G <regex>` |
| Path filter | `git log -- <paths>` | N/A (use revset) | `libra log -- <paths>` |
| Reverse order | `git log --reverse` | `jj log --reversed` | N/A |
| Merge commits only | `git log --merges` | N/A | `libra log --merges` (also `--no-merges`/`--min-parents`/`--max-parents`) |
| First parent only | `git log --first-parent` | N/A | `libra log --first-parent` |
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

- `--all`, `--branches`, and `--remotes` are not yet implemented; log walks from HEAD only
- Revision range syntax (`A..B`, `A...B`) is not yet supported; use `-n` and `--since`/`--until` for scoping
- jj's log uses a template language (`-T`) for formatting; Libra uses Git-compatible `--pretty` format strings
- `--grep` uses regular-expression matching (a semantic change from the earlier substring match); `^`/`$` are line anchors and `-i` enables case-insensitive matching
- `--reverse` for chronological order is not yet implemented
- In JSON mode, `files` contains structured change summaries; patch text is never included in JSON output
