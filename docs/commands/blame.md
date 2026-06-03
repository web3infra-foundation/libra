# `libra blame`

Trace each line of a file to the commit that last introduced it.

## Synopsis

```
libra blame [OPTIONS] <file> [<commit>]
libra --json blame [OPTIONS] <file> [<commit>]
libra --quiet blame [OPTIONS] <file> [<commit>]
```

## Description

`libra blame` annotates each line of a file with the commit hash, author name, date, and line number of the commit that last modified that line. It walks the commit history from the specified revision (defaulting to HEAD) backward through parent commits, using diff operations to attribute lines to the earliest commit that introduced them.

The output format matches Git's blame format for familiarity: a short hash, author name (truncated to 15 characters), date, line number, and line content on each line.

For large files, the `-L` option restricts output to a specific line range, reducing both computation time and output volume.

## Options

| Option | Short | Long | Description |
|--------|-------|------|-------------|
| File | | positional (required) | The file to blame. Must exist in the specified revision. |
| Commit | | positional (optional) | The revision to start blame from. Defaults to `HEAD`. |
| Line range | `-L` | `-L <RANGE>` | Restrict blame to a line range. See formats below. |
| JSON | | global `--json` | Emit structured JSON output. Pass before the subcommand: `libra --json blame ...`. |
| Quiet | | global `--quiet` | Validate inputs but suppress all blame output. Pass before the subcommand: `libra --quiet blame ...`. |
| Long hash | `-l` | | Show the full commit hash instead of the abbreviated form. |
| Raw timestamp | `-t` | | Show the raw commit timestamp (epoch seconds) instead of a formatted date. |
| File name | `-f` | | Show the file name for each blamed line. |
| Original line number | `-n` | | Show the original (pre-image) line number. |
| Suppress metadata | `-s` | | Suppress the author name and date columns. |
| Show email | `-e` | | Show the author email in angle brackets instead of the name. |
| Ignore whitespace | `-w` | | Ignore whitespace-only changes when assigning blame. |
| Porcelain | `-p` | `--porcelain` | Machine-readable porcelain output (one record per line). |
| Detect moved | `-M` | | Parsed only; cross-file move detection is not implemented (blame still walks this file). Optional threshold `-M=<num>`. |
| Detect copied | `-C` | | Parsed only; cross-file copy detection is not implemented. Optional threshold `-C=<num>`. |

### Line Range Formats (`-L`)

The `-L` flag supports three formats:

| Format | Meaning | Example |
|--------|---------|---------|
| `N` | Single line N | `-L 10` |
| `N,M` | Lines N through M (inclusive) | `-L 10,20` |
| `N,+C` | C lines starting at line N | `-L 10,+5` (lines 10-14) |

Line numbers are 1-based. A `start` past the end of the file is an error; an
`end` (or `+count`) past the end of the file is clamped to the last line.

```bash
# Blame a single line
libra blame -L 42 src/main.rs

# Blame a range
libra blame -L 10,20 src/main.rs

# Blame 5 lines starting at line 100
libra blame -L 100,+5 src/main.rs
```

## Common Commands

```bash
# Blame a file at HEAD
libra blame src/main.rs

# Blame at a specific commit
libra blame src/main.rs abc1234

# Blame lines 10-20
libra blame -L 10,20 src/main.rs

# Blame 5 lines from line 10
libra blame -L 10,+5 src/main.rs

# JSON output for agents
libra --json blame src/main.rs

# Suppress author/date columns (hash + line + content only)
libra blame -s src/main.rs

# Full hash with original line numbers and author email
libra blame -l -n -e src/main.rs

# Ignore whitespace-only changes when assigning blame
libra blame -w src/main.rs

# Machine-readable porcelain output for IDEs/scripts
libra blame --porcelain src/main.rs
```

## Human Output

```text
abc12345 (Author Name     2026-03-30 10:00:00 +0800 1) line content
def67890 (Other Author    2026-03-28 14:30:00 +0800 2) another line
abc12345 (Author Name     2026-03-30 10:00:00 +0800 3) third line
```

Each line shows:
- **Short hash** (8 characters): the commit that last changed this line.
- **Author name** (padded to 15 characters, truncated with `...` if longer).
- **Date**: formatted in the local timezone as `YYYY-MM-DD HH:MM:SS +ZZZZ`.
- **Line number**: 1-based line number in the file.
- **Content**: the actual line content.

With `-e`, the author column shows `<email>` instead of the author name. With
`-n`, the line number is the original pre-image line number when attribution
was inherited from an older commit. With `-f`, the file name is prefixed before
the hash.

`--quiet` validates the revision, file, and line range but suppresses all output. This is useful for scripted checks ("does this file exist at this revision?").

Output is automatically paged when connected to a terminal.

## Structured Output (JSON)

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "tracked.txt",
    "revision": "abc123...",
    "lines": [
      {
        "line_number": 1,
        "short_hash": "abc12345",
        "hash": "abc123...",
        "author": "Test User",
        "date": "2026-03-30T10:00:00+00:00",
        "content": "tracked",
        "email": "test@example.com",
        "timestamp": 1774864800,
        "timezone": "+0000",
        "summary": "update tracked",
        "original_line_number": 1
      }
    ]
  }
}
```

The `revision` field contains the full commit hash that was used as the blame starting point. Each line entry includes both the `short_hash` (8 characters) and full `hash` for programmatic use. The appended `email`, `timestamp`, `timezone`, `summary`, and `original_line_number` fields support `-e`, `-t`, `-n`, and porcelain rendering while preserving the existing fields.

When the file is empty, the `lines` array is empty and human output shows "File is empty".

## Design Rationale

### Why no `--reverse`?

Git's `blame --reverse` shows the last revision in which a line existed, walking forward in history instead of backward. This is useful for finding when a line was *removed*, but it requires forward-history traversal which is computationally expensive and architecturally different from normal blame. Libra omits this to keep the blame implementation simple and fast. To find when a line was removed, use `libra log -p -- <file>` and search for the deletion.

### Why a simplified line range format?

Git's `-L` supports complex formats including regex-based function matching (`-L :<funcname>`) and `/regex/` line selection. These are powerful but depend on language-specific configuration (`.gitattributes` `diff` driver) and are rarely used correctly. Libra supports only numeric ranges (`N`, `N,M`, `N,+C`), which are unambiguous and sufficient for the vast majority of blame operations. AI agents can easily determine line numbers from file content without needing regex-based function matching.

### Why default to HEAD instead of working tree?

Git's blame defaults to HEAD and requires `git blame --contents <file>` to blame the working-tree version. Libra follows the same convention: blame always operates on committed content. This ensures reproducible results -- the same command with the same commit always produces the same output, regardless of working-tree state.

### Why positional commit argument instead of a flag?

The commit argument is positional (second argument after the file path) rather than a flag like `--commit` or `--rev`. This matches Git's syntax for familiarity and keeps the common case (`libra blame file.rs`) concise. Since the file path is always the first positional argument, there is no ambiguity.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| File | `<file>` (positional, required) | `<file>` (positional, required) | N/A (jj has no blame; use `jj annotate`) |
| Revision | `<commit>` (positional, default HEAD) | `<rev>` (positional, default HEAD) | `-r <revision>` (in `jj annotate`) |
| Line range (numeric) | `-L N,M` / `-L N,+C` / `-L N` | `-L <start>,<end>` | N/A |
| Line range (regex) | Not supported | `-L :<funcname>` / `-L /regex/` | N/A |
| Reverse blame | Not supported | `--reverse` | N/A |
| Show email | `-e` | `-e` / `--show-email` | N/A |
| Show timestamp | Default; `-t` for raw epoch | `-t` (raw timestamp) | N/A |
| Display flags | `-l` / `-f` / `-n` / `-s` | `-l` / `-f` / `-n` / `-s` | N/A |
| Ignore whitespace | `-w` | `-w` | N/A |
| Porcelain format | `--porcelain` / `-p` | `--porcelain` / `--line-porcelain` | N/A |
| Incremental output | Not supported | `--incremental` | N/A |
| Move/copy detection | Partial (`-M` / `-C` parsed only, no cross-file detection) | `-M` / `-C` | N/A |
| Ignore revisions | Not supported | `--ignore-rev` / `--ignore-revs-file` | N/A |
| Working tree contents | Not supported | `--contents <file>` | N/A |
| Date format | Not supported (fixed) | `--date <format>` | N/A |
| Encoding | Not supported | `--encoding <encoding>` | N/A |
| JSON output | `--json` | Not supported | Not supported |
| Quiet mode | `--quiet` | Not supported | N/A |
| Pager | Automatic | Configurable | Configurable |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Outside a repository | `LBR-REPO-001` | 128 |
| Invalid revision or missing file | `LBR-CLI-003` | 129 |
| Invalid `-L` range | `LBR-CLI-002` | 129 |
| Failed to read the commit or object | `LBR-REPO-002` | 128 |
