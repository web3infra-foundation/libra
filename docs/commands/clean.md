# `libra clean`

Remove untracked files (and optionally directories) from the working tree.

## Synopsis

```
libra clean -n [-d] [-x | -X] [-e <pattern>]... [--json] [--quiet]
libra clean -f[f] [-d] [-x | -X] [-e <pattern>]... [--json] [--quiet]
libra clean -i [-d] [-x | -X] [-e <pattern>]... [--quiet]
```

## Description

`libra clean` removes untracked files from the working tree. Like Git,
Libra requires an explicit mode flag: `-n` for a dry-run preview, `-f`
for actual deletion, or `-i` to choose items interactively. Running
`libra clean` without any of these is an error *unless*
`clean.requireForce` is set to `false`. This prevents accidental data
loss by forcing the user to state intent explicitly.

By default, only files are removed and `.libraignore` rules are honored
(ignored files are skipped). The `-d` flag opts into removing untracked
directories as well; `-x` opts into removing files the ignore rules would
otherwise protect; `-X` flips the rules so that *only* ignored files are
removed. Every candidate path is canonicalized and verified to reside
inside the worktree root before deletion, preventing symlink-escape
attacks.

## Options

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Dry run | `-n` | `--dry-run` | Show what would be removed without deleting anything. |
| Force | `-f` | `--force` | Actually remove untracked files. Repeat (`-ff`) to also remove nested repositories (see below). |
| Interactive | `-i` | `--interactive` | Choose which untracked items to remove via a menu. Mutually exclusive with `--json` and `-n`. |
| Directories | `-d` | `--dir` | Also remove untracked directories (otherwise only files). |
| Include ignored | `-x` | | Remove untracked files **including** those matched by `.libraignore`. |
| Only ignored | `-X` | | Remove **only** untracked files that are matched by `.libraignore`. |
| Exclude | `-e` | `--exclude <pattern>` | Add an extra exclusion pattern; may be repeated. |
| JSON | | `--json` | Emit structured JSON output (see below). |
| Quiet | `-q` | `--quiet` | Suppress human-readable stdout **and** the `Skipping repository` / `warning: failed to remove` stderr warnings. This is the global `-q`/`--quiet` flag (`OutputConfig.quiet`), not a clean-specific field. |

`-x` and `-X` are mutually exclusive — `-x` *includes* ignored files in
addition to normally-untracked ones, `-X` restricts the operation to
ignored files only.

### Option Details

**`-n` / `--dry-run`**

Preview mode. Lists every untracked path that *would* be deleted without
touching the filesystem:

```bash
$ libra clean -n
Would remove build/output.log
Would remove notes.txt
```

**`-f` / `--force`**

Deletion mode. Removes every untracked path and reports each removal:

```bash
$ libra clean -f
Removing build/output.log
Removing notes.txt
```

**`-d` / `--dir`**

Opt-in for untracked directories. Without `-d`, untracked directories
are left in place (their contents are still considered if the directory
itself is tracked). With `-d`, the directory tree is walked and the
empty directory is removed after its files are.

**`-x`**

Override `.libraignore`. Without this flag, ignored files (build
artifacts, caches, etc.) are skipped. With `-x`, they are treated like
any other untracked file and removed.

**`-X`**

Inverse of `-x`. Removes only the files that `.libraignore` would
normally protect. Useful for "clean my build artifacts but leave
hand-edited files alone."

**`--exclude <pattern>`**

Add an additional exclusion pattern (in `.libraignore` syntax) for this
invocation only. Can be passed multiple times to layer patterns:

```bash
libra clean -f --exclude '*.log' --exclude 'tmp/**'
```

**`-i` / `--interactive`**

Presents a menu of untracked candidates and lets you refine the
selection before anything is deleted. Every candidate starts selected;
the menu offers six subcommands (matching `git clean -i`):

```text
Would remove the following items:
  *   1: build/output.log
  *   2: notes.txt
*** Commands ***
    1: clean                2: filter by pattern    3: select by numbers
    4: ask each             5: quit                 6: help
What now>
```

- **clean** — delete the currently-selected items and exit.
- **filter by pattern** — enter `.libraignore`-style globs to deselect
  matching items; a pattern that matches a directory also deselects
  everything beneath it (ancestor inheritance). A blank line returns.
- **select by numbers** — replace the selection by index: single numbers,
  comma/space lists, closed ranges (`2-5`), open ranges (`7-`), `*` for
  all, and a `-` prefix to deselect (`-3`). Out-of-range tokens are ignored.
- **ask each** — confirm each selected item with `Remove <path>? [y/N]`.
- **quit** — exit without deleting anything.
- **help** — print the per-subcommand help screen.

Commands accept the leading number, the full word, or a case-insensitive
first letter. The interactive loop never touches the filesystem itself:
it returns the chosen paths to the same tolerant deletion path used by
`-f`. EOF (a closed/piped stdin with nothing left to read) is treated as
`quit`, so a non-interactive invocation can never hang. `-i` cannot be
combined with `--json` (machine output) or `-n` (dry-run) — both are
rejected at preflight with `LBR-CLI-002`.

**Nested repositories (`-ff`)**

A directory whose immediate children include `.git` or `.libra` is an
independent repository. A single `-f` **skips** such directories (and
every file under them) and prints `Skipping repository <path>` to stderr,
so a stray `clean` can never wipe out an unrelated checkout. Pass a second
`-f` (`-ff`, count-based — `-f -f` works too) to opt into removing nested
repositories, mirroring `git clean -ffd`. Preview first with
`libra clean -n -ffd`.

**Tolerant removal**

Deletion no longer aborts on the first failure. If an individual path
cannot be removed (for example, a read-only file), Libra prints
`warning: failed to remove <path>: <detail>` to stderr, records the path
in the JSON `failed` array, and continues with the remaining candidates.
After emitting the (partial) success list, the command exits `128`
(`LBR-IO-002`) so callers still observe the failure.

**Combining `-n` and `-f`**: When both flags are passed, the dry-run
takes precedence and no files are deleted.

## Common Commands

```bash
# Preview what would be removed
libra clean -n

# Remove all untracked files (files only)
libra clean -f

# Also remove untracked directories
libra clean -fd

# Remove untracked files including ignored ones (build artifacts, caches)
libra clean -fx

# Remove only ignored files (keep hand-edited files intact)
libra clean -fX

# Choose interactively which untracked items to remove
libra clean -i

# Preview removing nested repositories too, before committing to -ff
libra clean -n -ffd

# Force-remove untracked files AND any nested .git/.libra repositories
libra clean -ffd

# Layer an additional exclusion pattern on top of .libraignore
libra clean -f --exclude '*.log'

# Preview in JSON format (useful for scripting)
libra clean -n --json
```

## Human Output

Dry-run:

```text
Would remove build/output.log
Would remove notes.txt
```

Forced removal:

```text
Removing build/output.log
Removing notes.txt
```

`--quiet` suppresses stdout.

## Structured Output (JSON)

```json
{
  "ok": true,
  "command": "clean",
  "data": {
    "dry_run": true,
    "removed": ["build/output.log", "notes.txt"]
  }
}
```

`removed` is empty when there is nothing to clean. When tolerant removal
hits a path it cannot delete, the envelope also carries a `failed` array
of the paths that survived, and the process exits `128`:

```json
{
  "ok": true,
  "command": "clean",
  "data": {
    "dry_run": false,
    "removed": ["notes.txt"],
    "failed": ["locked/output.bin"]
  }
}
```

`failed` is omitted entirely when every removal succeeded (it is a
`#[serde(default, skip_serializing_if = "Vec::is_empty")]` field, so
older parsers stay forward-compatible). Interactive mode (`-i`) never
emits JSON.

## Design Rationale

### Why require an explicit mode flag?

Git's `clean` without `-f` (and without `clean.requireForce = false`)
prints an error asking for `-f`. Libra mirrors that guardrail: by default
(`clean.requireForce = true`) you must pass one of `-n`, `-f`, or `-i`,
which eliminates an entire class of "I accidentally ran clean" incidents.
Unlike earlier versions of Libra, the guardrail is now configurable —
setting `clean.requireForce = false` (local config wins over global)
allows a bare `libra clean` to delete, matching Git's configuration
contract for scripted environments that opt out deliberately.

### Why interactive mode (`-i`)?

Earlier versions of Libra declined `git clean -i` on the grounds that
AI-agent and scripting workflows cannot drive an interactive prompt.
That reasoning held for the *automation* path but left a real parity gap
for humans triaging a messy working tree. `-i` is now implemented as a
pure, fully unit-tested state machine over generic `BufRead`/`Write`, so
it is exercised in tests without a TTY and never blocks an agent: the
loop is reachable only when `-i` is passed explicitly, is mutually
exclusive with `--json` (the machine-readable path) and `-n` (preview),
and treats EOF as `quit`. Agents keep using the `-n --json` → `-f`
two-step; humans get the menu.

### Why ship `-d` / `-x` / `-X` after originally declining them?

The original `clean` design intentionally declined the directory and
ignore-override flags out of safety concerns (`docs/improvement/clean.md`
listed them as non-goals). Subsequent user feedback showed that build
workflows in agent-driven environments routinely need to clear ignored
artifacts, and the missing flags forced users to fall back on raw `rm
-rf` which is strictly less safe than `clean` (no symlink-escape
verification, no dry-run preview). The flags were added with the same
worktree-confinement and symlink checks as the base mode, preserving the
safety guarantees while restoring parity with `git clean`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Dry run | `-n` / `--dry-run` | `-n` / `--dry-run` | N/A (no clean command) |
| Force delete | `-f` / `--force` | `-f` / `--force` | N/A |
| Remove directories | `-d` / `--dir` | `-d` | N/A |
| Ignore override (all) | `-x` | `-x` | N/A |
| Ignore override (only ignored) | `-X` | `-X` | N/A |
| Exclude pattern | `-e` / `--exclude <pattern>` (repeatable) | `-e <pattern>` (repeatable) | N/A |
| Interactive mode | `-i` / `--interactive` | `-i` | N/A |
| Quiet mode | global `-q` / `--quiet` (per-command `--quiet` not exposed; uses `OutputConfig.quiet`) | `-q` / `--quiet` | N/A |
| Nested-repo double force | `-ff` (count-based) | `-ff` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Pathspec filter | Not supported | `<pathspec>...` | N/A |
| Require force config | `clean.requireForce` (default true) | `clean.requireForce` (default true) | N/A |

Note: jj does not have a `clean` command because its working-copy model
tracks all files automatically and untracked files are not a concept in
the jj data model.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Missing `-f` / `-n` / `-i` (and `clean.requireForce` not `false`) | `LBR-CLI-002` | 129 |
| `-i` combined with `--json` | `LBR-CLI-002` | 129 |
| `-i` combined with `-n` (dry-run) | `LBR-CLI-002` | 129 |
| `-x` combined with `-X` | `LBR-CLI-002` | 129 |
| Corrupted index or untracked scan failure | `LBR-IO-001` | 128 |
| Path resolves outside the worktree | `LBR-CONFLICT-002` | 128 |
| File deletion failed (tolerant — partial list still emitted) | `LBR-IO-002` | 128 |

Exit codes are the default *coarse* mapping; set `LIBRA_FINE_EXIT_CODES=1`
for the fine-grained codes (`LBR-CLI-002` → 2, etc.). `clap` parse errors
(an unknown flag) exit `2` directly without a `CleanError`.
