# `libra clean`

Remove untracked files from the working tree.

## Synopsis

```
libra clean -n
libra clean -f
libra clean [--json] [--quiet]
```

## Description

`libra clean` removes untracked files from the working tree. Unlike Git, Libra requires an explicit mode flag: `-n` for a dry-run preview or `-f` for actual deletion. Running `libra clean` without either flag is an error. This prevents accidental data loss by forcing the user to state intent explicitly.

Only files are removed; untracked directories are left in place. The command scans the index to determine which working-tree files are untracked and applies the `.libraignore` policy before building the removal list. Every candidate path is canonicalized and verified to reside inside the worktree root before deletion, preventing symlink-escape attacks.

## Options

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Dry run | `-n` | `--dry-run` | Show what would be removed without deleting anything. Prints each candidate path prefixed with "Would remove". |
| Force | `-f` | `--force` | Actually remove untracked files. Prints each removed path prefixed with "Removing". |
| JSON | | `--json` | Emit structured JSON output (see below). |
| Quiet | | `--quiet` | Suppress all human-readable stdout. |

### Option Details

**`-n` / `--dry-run`**

Preview mode. Lists every untracked file that *would* be deleted without touching the filesystem:

```bash
$ libra clean -n
Would remove build/output.log
Would remove notes.txt
```

**`-f` / `--force`**

Deletion mode. Removes every untracked file and reports each removal:

```bash
$ libra clean -f
Removing build/output.log
Removing notes.txt
```

**Combining `-n` and `-f`**: When both flags are passed, the dry-run takes precedence and no files are deleted (the implementation checks `dry_run` first).

## Common Commands

```bash
# Preview what would be removed
libra clean -n

# Remove all untracked files
libra clean -f

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

`--quiet` suppresses `stdout`.

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

`removed` is empty when there is nothing to clean.

## Design Rationale

### Why only `-n` and `-f` (no `-d`, `-x`, `-X`)?

Git's `clean` command has accumulated a large surface area: `-d` removes directories, `-x` ignores `.gitignore`, `-X` removes *only* ignored files, and `-e` adds extra exclusion patterns. Each flag interacts with the others in subtle ways that are a frequent source of data loss.

Libra deliberately omits these flags:

- **No `-d`**: Removing empty directories is a cosmetic concern that rarely matters for build workflows. Tools like `make clean` or `cargo clean` handle build-artifact directories more precisely. Adding `-d` would require recursive directory traversal with its own safety checks.
- **No `-x` / `-X`**: These flags override ignore rules, which is the single most dangerous operation in `git clean`. Libra's philosophy is that ignored files should stay ignored. If you need to remove ignored build artifacts, use the build system's own clean target.
- **No `-e` (exclude pattern)**: Without `-x`/`-X`, additional exclusion patterns add no value since ignore rules are already respected unconditionally.

### Why require an explicit mode flag?

Git's `clean` without `-f` (and without `clean.requireForce = false`) prints an error asking for `-f`. This is a config-dependent guardrail. Libra makes the guardrail unconditional: you must always pass `-n` or `-f`. There is no configuration to weaken this requirement. This eliminates an entire class of "I accidentally ran clean" incidents.

### Why no interactive mode (`-i`)?

Git's interactive clean mode presents a menu for selecting files. Libra targets AI-agent and scripting workflows where interactive prompts are unusable. The dry-run/force two-step workflow achieves the same safety with full automation support: run `-n --json` to inspect, then `-f` to execute.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Dry run | `-n` / `--dry-run` | `-n` / `--dry-run` | N/A (no clean command) |
| Force delete | `-f` / `--force` | `-f` / `--force` | N/A |
| Remove directories | Not supported | `-d` | N/A |
| Ignore override (all) | Not supported | `-x` | N/A |
| Ignore override (only ignored) | Not supported | `-X` | N/A |
| Exclude pattern | Not supported | `-e <pattern>` | N/A |
| Interactive mode | Not supported | `-i` | N/A |
| Quiet mode | `--quiet` | `-q` / `--quiet` | N/A |
| JSON output | `--json` | Not supported | N/A |
| Pathspec filter | Not supported | `<pathspec>...` | N/A |
| Require force config | Always required | `clean.requireForce` (default true) | N/A |

Note: jj does not have a `clean` command because its working-copy model tracks all files automatically and untracked files are not a concept in the jj data model.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Missing `-f` / `-n` | `LBR-CLI-002` | 129 |
| Corrupted index or untracked scan failure | `LBR-IO-001` | 128 |
| Path resolves outside the worktree | `LBR-CONFLICT-002` | 128 |
| File deletion failed | `LBR-IO-002` | 128 |
