# `libra shortlog`

Summarize reachable commits by author.

**Alias:** `slog`

## Synopsis

```
libra shortlog [<revision>|<A>..<B>] [-n] [-s] [-e] [-c] [--no-merges] [-w[=<spec>]] [--format <format>] [--since <date>] [--until <date>]
```

## Description

`libra shortlog` summarizes reachable commits grouped by author, primarily used for release announcements and contributor overviews. It walks the commit graph from the specified revision or double-dot range (defaulting to HEAD) and aggregates commits per author, displaying each author's commit count and optionally their commit subjects.

By default, authors are sorted alphabetically by name. With `-n`, they are sorted by commit count (descending). The `-s` flag produces a summary with only counts, suppressing individual commit subjects. The `-e` flag includes the email address in the output. `-c` switches grouping to committer identity, and `--no-merges` excludes commits with more than one parent.

Date filtering via `--since` and `--until` restricts which commits are included based on their committer timestamp, supporting formats like `YYYY-MM-DD`, `"N days ago"`, and Unix timestamps.

If a root `.mailmap` file exists, Libra applies it before grouping. Malformed or overlong mailmap lines are skipped with warnings; symlinked `.mailmap` files are ignored.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| Numbered | `-n` | `--numbered` | Sort output by number of commits per author (descending) instead of alphabetically. |
| Summary | `-s` | `--summary` | Suppress commit descriptions; show only per-author commit counts. |
| Email | `-e` | `--email` | Show the email address of each author alongside their name. When enabled, authors are grouped by `name <email>` pair. |
| Committer | `-c` | `--committer` | Group commits by committer identity instead of author. |
| No merges | | `--no-merges` | Exclude merge commits (those with more than one parent) from the summary and totals. |
| Width | `-w` | | Wrap human subject lines. Bare `-w` uses `76,6,9`; custom values use `-w=<width>[,<indent1>[,<indent2>]]`. |
| Format | | `--format <format>` | Render each per-commit subject with a limited template: `%s`, `%h`, `%H`, `%an`, `%ae`, `%cn`, `%ce`, `%%`. |
| Since | | `--since <date>` | Only include commits more recent than the specified date. |
| Until | | `--until <date>` | Only include commits older than the specified date. |
| Revision | | positional (optional) | The revision or `A..B` range to summarize. Defaults to `HEAD`. |
| JSON | | `--json` | Emit structured JSON output. |
| Quiet | | `--quiet` | Suppress human-readable output. |

### Option Details

**`-n` / `--numbered`**

Sorts authors by descending commit count. When two authors have the same count, they are sorted alphabetically:

```bash
$ libra shortlog -n
   5  Alice
   3  Bob
   1  Charlie
```

**`-s` / `--summary`**

Produces compact output with only counts, omitting individual commit subjects:

```bash
$ libra shortlog -s
   2  Test User
```

Without `-s`, commit subjects are listed under each author:

```bash
$ libra shortlog
   2  Test User
      initial
      follow-up
```

**`-e` / `--email`**

Appends the email address to each author. When enabled, authors with the same name but different emails are listed separately:

```bash
$ libra shortlog -e
   2  Test User <test@example.com>
      initial
      follow-up
```

**`--since` / `--until`**

Filter commits by committer timestamp. Supported date formats include:

- `YYYY-MM-DD` (e.g., `2026-01-01`)
- Relative dates (e.g., `"7 days ago"`, `"2 weeks ago"`)
- Unix timestamps

```bash
# Commits in the last month
libra shortlog --since "30 days ago"

# Commits in a date range
libra shortlog --since 2026-01-01 --until 2026-03-31
```

**Revision argument**

Specify a starting point other than HEAD, or a double-dot range. `A..B` means commits reachable from `B` that are not reachable from `A`.

```bash
# Summarize the last 5 commits
libra shortlog HEAD~5

# Summarize commits after v1.0 up to HEAD
libra shortlog v1.0..HEAD

# Summarize from a tag
libra shortlog v1.0
```

Three-dot ranges, `^ref` exclusions, multiple separate revisions, pathspecs, `--all`, `--branches`, and `--tags` are not supported by `shortlog`.

**`-c` / `--committer` and `--no-merges`**

`-c` groups by committer name/email instead of author name/email. `--no-merges` removes commits with more than one parent before aggregation, so totals and per-author counts exclude merge commits.

**`-w`**

Wraps human subject lines only. Bare `-w` uses Git's default `76,6,9`. Custom values require an equals sign, for example `-w=72` or `-w=72,6,9`; this differs from Git's attached `-w72` spelling. JSON and `--machine` subjects are never wrapped.

**`--format`**

Changes each commit description before aggregation output. Supported placeholders are `%s`, `%h`, `%H`, `%an`, `%ae`, `%cn`, `%ce`, and `%%`. Unknown placeholders return `LBR-CLI-002`.

```bash
libra shortlog --format "%h %an %s"
```

**`.mailmap`**

When `.mailmap` exists at the repository root, author or committer identities are resolved before grouping. Libra supports the common forms:

```text
Proper Name <proper@example.com> Commit Name <commit@example.com>
Proper Name <proper@example.com> <commit@example.com>
<proper@example.com> <commit@example.com>
Proper Name <commit@example.com>
```

## Common Commands

```bash
# Default shortlog from HEAD
libra shortlog

# Summary with counts only, sorted by count
libra shortlog -n -s

# Include email addresses
libra shortlog -e

# Last 5 commits summary
libra shortlog HEAD~5

# Commits in a release range
libra shortlog v1.0..HEAD -n -s

# Wrap human subject lines
libra shortlog -w=72

# Custom per-commit description
libra shortlog --format "%h %s"

# Commits in a date range
libra shortlog --since 2026-01-01 --until 2026-03-31

# JSON output for scripting
libra shortlog --json
```

## Human Output

Default (alphabetical, with subjects):

```text
   2  Test User
      initial
      follow-up
```

Summary mode (`-s`) suppresses subjects. `-e` appends `<email>`.

Subject extraction skips embedded signature headers and uses the first meaningful commit message line.

The count column is right-aligned with consistent width based on the maximum count across all authors.

`-w` wraps human subject lines only. A single subject longer than 64 KiB is truncated for human output with a warning; JSON output keeps the full subject.

## Structured Output (JSON)

```json
{
  "ok": true,
  "command": "shortlog",
  "data": {
    "revision": "HEAD",
    "numbered": false,
    "summary": false,
    "email": false,
    "total_authors": 1,
    "total_commits": 2,
    "authors": [
      {
        "name": "Test User",
        "email": null,
        "count": 2,
        "subjects": ["initial", "follow-up"]
      }
    ]
  }
}
```

In summary mode, `subjects` is an empty array. When `-e` is enabled, the `email` field contains the resolved email string; otherwise it is `null`.

The `total_authors` and `total_commits` fields provide aggregate counts for quick consumption by scripts and agents.

`--format` changes `subjects[]` to the rendered template result. `-w` never changes JSON or `--machine` subjects.

## Design Rationale

### Why no `--group`?

Git's `shortlog --group=trailer:<key>` and `--group=author`/`--group=committer` allow grouping by different commit metadata fields or trailer values. This is a niche feature used primarily for analyzing co-authored commits or commits attributed via `Signed-off-by` trailers. Libra omits `--group` to keep the command focused on its primary use case: summarizing contributions by author. The overwhelmingly common usage of shortlog is author-based grouping, and supporting arbitrary grouping would require a generic aggregation framework that adds complexity without proportional value.

### Why positional revision instead of piped input?

Git's `shortlog` can operate in two modes: reading from `git log` output piped via stdin, or directly traversing commit history. The piped mode (`git log | git shortlog`) is a Unix-philosophy composability feature, but it requires parsing serialized commit data, which is fragile and format-dependent.

Libra takes the revision as a positional argument and always reads directly from the commit graph. This is simpler, faster (no serialization/deserialization), and works naturally with the `--json` output mode. For filtering beyond what `--since`/`--until` provide, use `libra log --json` with external tooling.

### Why `--since`/`--until` instead of full log options?

Git's `shortlog` inherits the full set of `git log` options when used directly (not piped). Libra supports the common release-summary subset: revision or `A..B`, date filtering, committer grouping, no-merge filtering, mailmap, wrapping, and a small `--format` template. Broader log filters such as `--author`, `--grep`, pathspecs, and multi-ref traversal remain out of scope for `shortlog`.

### Why committer timestamp for filtering?

The `--since`/`--until` filters use the committer timestamp (not the author timestamp), matching Git's behavior. The committer timestamp reflects when a commit was actually applied to the current branch (e.g., after rebase), which is more relevant for release-period summaries than the original authoring date.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Numbered sort | `-n` / `--numbered` | `-n` / `--numbered` | N/A (no shortlog command) |
| Summary only | `-s` / `--summary` | `-s` / `--summary` | N/A |
| Show email | `-e` / `--email` | `-e` / `--email` | N/A |
| Since date | `--since <date>` | `--since <date>` / `--after <date>` | N/A |
| Until date | `--until <date>` | `--until <date>` / `--before <date>` | N/A |
| Revision | `<revision>` or `A..B` (positional) | `<revision range>...` | N/A |
| Group by | Not supported | `--group=author\|committer\|trailer:<key>` | N/A |
| Format | `--format <format>` subset | `--format=<format>` | N/A |
| Committer grouping | `-c` / `--committer` | `--committer` (deprecated, use `--group=committer`) | N/A |
| Piped input | Not supported | Reads from stdin when piped | N/A |
| No merges | `--no-merges` | `--no-merges` | N/A |
| Author filter | Not supported | `--author=<pattern>` | N/A |
| Grep filter | Not supported | `--grep=<pattern>` | N/A |
| Width limit | `-w` / `-w=<spec>` | `-w[<width>[,<indent1>[,<indent2>]]]` | N/A |
| Mailmap | root `.mailmap` | `.mailmap`, `mailmap.file`, `mailmap.blob` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Quiet mode | `--quiet` | Not supported | N/A |

Note: jj does not have a shortlog command. Similar information can be obtained by filtering `jj log` output, but there is no built-in author aggregation.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid `--since` / `--until` date | `LBR-CLI-002` | 129 |
| Invalid `-w` spec or unsupported `--format` placeholder | `LBR-CLI-002` | 129 |
| Invalid revision | `LBR-CLI-003` | 129 |
| Unsupported range syntax (`A...B`, `^ref`) | `LBR-CLI-003` | 129 |
| HEAD has no commit | `LBR-REPO-003` | 128 |
| Failed to read refs or commit graph | `LBR-IO-001` / `LBR-REPO-002` | 128 |
| Mailmap warning with `--exit-code-on-warning` | `LBR-WARN-001` | 9 |
