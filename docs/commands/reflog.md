# `libra reflog`

Manage the log of reference changes (HEAD, branches).

## Synopsis

```
libra reflog show [<ref_name>] [--pretty <format>] [--since <date>] [--until <date>] [--grep <pattern>] [--author <pattern>] [-n <N>] [-p/--patch] [--stat]
libra reflog delete <selector>...
libra reflog exists <ref_name>
```

## Description

`libra reflog` records and displays the history of reference changes in the repository. Every time HEAD or a branch tip moves (commit, merge, rebase, reset, checkout, etc.), a reflog entry is created with the old and new object IDs, a timestamp, the committer identity, and a description of the action.

Reflog entries are stored in the SQLite `reflog` table, providing transactional safety and queryable history. This contrasts with Git's flat-file approach where each ref has a separate reflog file under `.git/logs/`.

The `show` subcommand is the primary interface for inspecting reflog history, with filtering by time range, message content, and author. The `delete` subcommand removes specific entries, and `exists` is a plumbing command for scripts to check whether a reference has any reflog entries.

## Options

### Subcommand: `show`

Display reflog entries for a reference.

| Option / Argument | Short | Long | Description |
|-------------------|-------|------|-------------|
| `<ref_name>` | | | Reference to show. Defaults to `HEAD`. Bare branch names are expanded to `refs/heads/<name>`; names containing `/` are checked against configured remotes and expanded to `refs/remotes/<name>` if a matching remote exists. |
| Pretty format | | `--pretty` | Output format. One of: `oneline` (default), `short`, `medium`, `full`. |
| Since | | `--since` | Show entries newer than the given date. Accepts human-readable date strings (e.g. `2024-01-01`, `yesterday`). |
| Until | | `--until` | Show entries older than the given date. Same date format as `--since`. |
| Grep | | `--grep` | Filter entries whose `action: message` text contains the given pattern (case-insensitive). |
| Author | | `--author` | Filter entries whose committer name or email contains the given pattern (case-insensitive). |
| Limit | `-n` | `--number` | Maximum number of entries to display. |
| Patch | `-p` | `--patch` | Show the diff introduced by the commit referenced in each reflog entry. |
| Stat | | `--stat` | Show diffstat (files changed, insertions, deletions) for each reflog entry. |

```bash
# Show HEAD reflog (default)
libra reflog show

# Show reflog for a specific branch
libra reflog show feature-branch

# Show with medium format (includes date)
libra reflog show --pretty medium

# Filter by date range
libra reflog show --since 2024-01-01 --until 2024-06-30

# Filter by action/message content
libra reflog show --grep "commit"

# Filter by author
libra reflog show --author "alice"

# Limit output and show diffs
libra reflog show -n 5 -p

# Show stat summaries
libra reflog show --stat
```

### Subcommand: `delete`

Delete specific reflog entries by selector.

| Argument | Description |
|----------|-------------|
| `<selector>...` | One or more reflog selectors in `ref@{N}` format (e.g., `HEAD@{3}`, `main@{0}`). Multiple selectors can target different refs; entries within the same ref are deleted in reverse index order to preserve indices. |

```bash
# Delete a single reflog entry
libra reflog delete HEAD@{3}

# Delete multiple entries
libra reflog delete HEAD@{1} HEAD@{3} main@{0}
```

### Subcommand: `exists`

Check whether a reference has any reflog entries. Exits with success (0) if at least one entry exists, or failure if no entries are found. Primarily intended for use in scripts and automation.

| Argument | Description |
|----------|-------------|
| `<ref_name>` | Reference name to check (required). |

```bash
# Check if HEAD has reflog entries
libra reflog exists HEAD

# Check a branch
libra reflog exists main
```

## Common Commands

```bash
# View recent HEAD reflog entries
libra reflog show

# View reflog for a branch with dates
libra reflog show main --pretty medium

# Find commits by a specific author in the reflog
libra reflog show --author "alice" -n 10

# Find merge-related reflog entries
libra reflog show --grep "merge"

# Show recent entries with diffs
libra reflog show -n 3 -p

# Delete a stale reflog entry
libra reflog delete HEAD@{5}

# Check if a branch has reflog (scripting)
libra reflog exists feature-branch
```

## Human Output

**`reflog show`** (oneline format, default):

```text
abc1234 HEAD@{0}: commit: add new feature
def5678 HEAD@{1}: checkout: moving from main to feature-branch
ghi9012 HEAD@{2}: commit: initial commit
```

**`reflog show --pretty short`**:

```text
commit abc1234
Reflog: HEAD@{0} (Alice <alice@example.com>)
Reflog message: commit: add new feature
Author: Alice <alice@example.com>

  add new feature
```

**`reflog show --pretty medium`** (includes date):

```text
commit abc1234
Reflog: HEAD@{0} (Alice <alice@example.com>)
Reflog message: commit: add new feature
Author: Alice <alice@example.com>
Date:   Mon Jan 15 10:30:00 2024 -0800

  add new feature
```

**`reflog show --pretty full`** (includes committer):

```text
commit abc1234
Reflog: HEAD@{0} (Alice <alice@example.com>)
Reflog message: commit: add new feature
Author: Alice <alice@example.com>
Commit: Alice <alice@example.com>

  add new feature
```

**`reflog exists`** (ref found):

No output, exit code 0.

**`reflog exists`** (ref not found):

```text
fatal: reflog entry for 'nonexistent' not found
```

## Design Rationale

### Why subcommand-based instead of Git's implicit `show`?

Git treats `git reflog` as a shorthand for `git reflog show`, and its subcommands (`expire`, `delete`, `exists`) are somewhat hidden. Libra makes all operations explicit subcommands: `show`, `delete`, and `exists`. This eliminates ambiguity for both human users and AI agents, making the command surface fully discoverable through `--help`. It also aligns with Libra's general principle that every operation should be a named subcommand rather than an implicit default.

### Why `--grep` and `--author` filtering?

Git's `git reflog` supports filtering through `git log` options because it shares the same underlying machinery. However, the connection is not obvious to users. Libra provides `--grep` and `--author` as first-class options on `reflog show`, making it immediately clear that reflog entries can be searched by content and committer. Both filters are case-insensitive for convenience. The `--grep` filter matches against the combined `action: message` string (e.g., `commit: add new feature`), so users can filter by action type or message content.

### Why `FormatterKind` instead of Git's `--format`?

Git's `--format` accepts arbitrary format strings with `%H`, `%s`, etc. placeholders. This is powerful but complex and rarely used for reflogs. Libra provides four named formats (`oneline`, `short`, `medium`, `full`) via `--pretty` that cover the common use cases. This is simpler to implement, easier to document, and sufficient for reflog inspection. The `oneline` default is compact for scanning; `medium` adds dates for forensics; `full` adds the committer for audit trails.

### Why `--patch` and `--stat` on reflog?

These options are borrowed from `libra log` and allow users to see what actually changed at each reflog entry without having to separately run `libra show` or `libra diff` for each commit. This is particularly useful when investigating a regression: the reflog shows when HEAD moved, and `--patch`/`--stat` shows what changed at each step.

### Why SQLite instead of flat files?

Git stores reflogs as append-only text files under `.git/logs/`. This is simple but has no transactional guarantees and requires parsing to query. Libra stores reflog entries in the SQLite `reflog` table, which provides ACID transactions, structured queries, and the ability to delete individual entries without rewriting the entire file. The trade-off is that reflogs are not human-readable on disk, but the `reflog show` command provides all necessary inspection capabilities.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Show reflog | `reflog show [ref]` | `reflog [show] [ref]` (implicit) | `op log` (operation log) |
| Default ref | `HEAD` | `HEAD` | N/A (shows all operations) |
| Format | `--pretty oneline\|short\|medium\|full` | `--format <string>` / `--oneline` | Built-in format |
| Date filter (since) | `--since <date>` | `--since <date>` (via log options) | N/A |
| Date filter (until) | `--until <date>` | `--until <date>` (via log options) | N/A |
| Message filter | `--grep <pattern>` | `--grep <pattern>` (via log options) | N/A |
| Author filter | `--author <pattern>` | N/A (not directly on reflog) | N/A |
| Limit entries | `-n <N>` | `-n <N>` (via log options) | `-n <N>` |
| Show patch | `-p` / `--patch` | `-p` (via log options) | `--patch` on `op show` |
| Show stat | `--stat` | `--stat` (via log options) | `--stat` on `op show` |
| Delete entries | `reflog delete <selector>...` | `reflog delete <ref@{N}>` | N/A (operation log is append-only) |
| Check existence | `reflog exists <ref>` | `reflog exists <ref>` | N/A |
| Expire old entries | Not supported | `reflog expire` | N/A (GC handles cleanup) |
| Storage | SQLite table | Flat files (`.git/logs/`) | Operation log (custom format) |

Note: jj does not have a reflog. Instead, it maintains an operation log (`jj op log`) that records every repository mutation. This provides similar forensic capabilities but at the operation level rather than the reference level.

## Error Handling

| Code | Condition |
|------|-----------|
| `LBR-REPO-001` | Not a libra repository |
| `LBR-CLI-002` | Invalid `--since` or `--until` date format |
| `LBR-CLI-002` | Invalid reflog selector format (must be `ref@{N}`) |
| `LBR-CLI-003` | Reflog entry not found (for `exists` or `delete`) |
| `LBR-IO-001` | Failed to read reflog entries from database |
| `LBR-IO-002` | Failed to delete reflog entries from database |
