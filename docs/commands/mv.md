# `libra mv`

Move or rename files and directories.

## Synopsis

```
libra mv [<options>] <source>... <destination>
```

## Description

`libra mv` moves or renames files and directories in the working tree and updates the index accordingly. The last argument is always the destination; all preceding arguments are sources. When there are multiple sources, the destination must be an existing directory.

The command validates that all source paths exist, are tracked in the index, are not in a conflicted state, and reside within the repository working directory. Directory moves are performed as a single filesystem rename, with individual index entries updated for each tracked file within the directory. A directory source must contain at least one tracked entry; untracked files inside an otherwise tracked directory are carried along by the filesystem rename but are not added to the index.

After all filesystem moves succeed, the index is updated atomically: old entries are removed and new entries (with recalculated blob hashes) are inserted. The index is saved only after all operations complete successfully.

## Options

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Verbose | `-v` | `--verbose` | Print each rename operation as it happens. |
| Dry run | `-n` | `--dry-run` | Show what would be moved without actually performing any moves. |
| Force | `-f` | `--force` | Overwrite an existing destination file instead of reporting an error. Only works for regular files and symlinks; directories cannot be overwritten. |
| Skip errors | `-k` | `--skip-errors` | Skip individual source moves that would fail validation and continue with valid sources. |
| Sparse | | `--sparse` | Accepted as a **no-op** for `git mv --sparse` script compatibility. Libra has no sparse-checkout cone, so every path is always considered present and the flag changes nothing. |

### Option Details

**`-v` / `--verbose`**

Prints each rename operation during execution:

```bash
$ libra mv -v old.rs new.rs
Renaming old.rs to new.rs
```

**`-n` / `--dry-run`**

Previews the rename operations without performing them:

```bash
$ libra mv -n old.rs new.rs
Checking rename of 'old.rs' to 'new.rs'
Renaming old.rs to new.rs
```

No filesystem changes or index updates are made in dry-run mode.

**`-f` / `--force`**

Allows overwriting an existing destination. Without this flag, moving to an existing path is an error:

```bash
$ libra mv -f src/old.rs src/new.rs
```

**`-k` / `--skip-errors`**

Skips invalid source moves and applies the remaining valid moves:

```bash
$ libra mv -k missing.rs tracked.rs src/
```

This flag is useful for batch moves where some pathspecs may be absent or otherwise invalid. Invocation-shape errors still fail, such as passing multiple sources with a destination that is not an existing directory.

**`--sparse`**

Accepted and ignored (a no-op) so that third-party scripts written against `git mv --sparse` do not fail to parse. Git's `--sparse` lets you move files that lie outside the sparse-checkout cone; Libra has no sparse-checkout cone (every path is always considered present), so the flag has no effect on the move, the index, or the `MvOutput` JSON. This is an **intentional difference** — the flag is parsed for compatibility, not implemented:

```bash
$ libra mv --sparse old.rs new.rs   # behaves identically to `libra mv old.rs new.rs`
```

Without this flag, an unknown argument would be rejected by the CLI parser and exit with `129` (`LBR-CLI-002`), which would break a `git mv --sparse` invocation embedded in a build/monorepo script.

## Common Commands

```bash
# Rename a file
libra mv old_name.rs new_name.rs

# Move a file into a directory
libra mv utils.rs src/

# Move multiple files into a directory
libra mv a.rs b.rs c.rs src/

# Move a directory into another directory
libra mv old_dir/ parent_dir/

# Preview what would happen
libra mv -n old.rs new.rs

# Force overwrite
libra mv -f src/draft.rs src/final.rs

# Skip invalid sources in a batch
libra mv -k missing.rs tracked.rs src/

# Verbose output
libra mv -v old.rs new.rs
```

## Human Output

Normal move (no flags):

```text
(no output)
```

Verbose mode:

```text
Renaming old.rs to new.rs
```

Dry-run mode:

```text
Checking rename of 'old.rs' to 'new.rs'
Renaming old.rs to new.rs
```

Control characters inside printed path fields are escaped (for example a newline
inside a filename is rendered as `\n`) so the human output remains line-oriented.

Global `--quiet` suppresses dry-run and verbose human output while keeping
warnings and errors on stderr.

## Structured Output

`libra mv` supports the global `--json` and `--machine` flags on successful moves.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success
- dry-run output reports the planned move pairs without changing the filesystem or index

Example:

```json
{
  "ok": true,
  "command": "mv",
  "data": {
    "moves": [
      {
        "source": "old.rs",
        "destination": "new.rs"
      }
    ],
    "index_updates": [
      {
        "source": "old.rs",
        "destination": "new.rs"
      }
    ],
    "dry_run": false,
    "forced": false,
    "skip_errors": false,
    "verbose": false
  }
}
```

Dry-run:

```json
{
  "ok": true,
  "command": "mv",
  "data": {
    "moves": [
      {
        "source": "old.rs",
        "destination": "new.rs"
      }
    ],
    "index_updates": [
      {
        "source": "old.rs",
        "destination": "new.rs"
      }
    ],
    "dry_run": true,
    "forced": false,
    "skip_errors": false,
    "verbose": false
  }
}
```

## Design Rationale

### Why paths-based instead of explicit `--source` / `--dest`?

Libra follows the same convention as Git's `mv` and the Unix `mv` command: the last argument is the destination, and all preceding arguments are sources. This is familiar to every Unix user and avoids the verbosity of named flags for what is fundamentally a positional operation.

The trade-off is that the command requires at least two arguments and the semantics change depending on whether the destination is an existing directory. This is the same trade-off that Unix `mv` and Git `mv` make, and decades of usage have shown it to be intuitive in practice.

### Why is `--sparse` a no-op?

Git's `mv` supports `--sparse` to allow moving files outside the sparse-checkout cone. Libra does not implement sparse checkout, so the flag has no semantic effect. Rather than rejecting it (which would make any `git mv --sparse` script exit non-zero — `129`/`LBR-CLI-002` — and abort a CI pipeline), Libra **accepts and ignores** it. This is recorded as an *intentional difference* in `COMPATIBILITY.md`: the flag is parsed for script compatibility, not implemented. If real sparse-checkout support is ever added, `--sparse` will gain meaning without a breaking change to the surface.

### Submodule cascade rename (out of scope)

`git mv` updates `.gitmodules` when renaming a submodule path. Libra does **not** perform submodule cascade renames — moving a submodule path with `libra mv` moves the working-tree entry and index entry only and does not rewrite `.gitmodules`. This is an intentional out-of-scope decision to keep the VCS core simple.

### Why validate tracked status?

Unlike a plain filesystem `mv`, `libra mv` refuses to move files that are not tracked in the index. This prevents confusion where a user moves a file expecting the rename to be recorded in version control, but the file was never tracked in the first place. If you need to move an untracked file, use the system `mv` command.

### Why refuse conflicted files?

Moving a file that is in a conflicted state (stages 1-3 in the index) would lose conflict information. Libra requires conflicts to be resolved before the file can be moved.

### How does this compare to Git and jj?

Git's `mv` command is similar in design: it moves files in the working tree and updates the index. It supports `-k` to skip move errors and also supports `--sparse`; Libra accepts `--sparse` as a no-op because sparse checkout is not implemented.

jj does not have a `mv` command. Because jj uses automatic snapshotting of the working tree, file moves are detected automatically by the working-copy scanner. Users simply move files with the system `mv` command and jj records the change on the next snapshot. This works well for simple renames but cannot reliably detect moves (as opposed to delete-then-create) for large refactors.

Libra provides an explicit `mv` command (like Git) because its index-based model requires explicit notification of renames to maintain accurate tracking.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Source paths | `<source>...` (positional) | `<source>...` (positional) | N/A (use system `mv`) |
| Destination | Last positional argument | Last positional argument | N/A |
| Verbose | `-v` / `--verbose` | `-v` / `--verbose` | N/A |
| Dry run | `-n` / `--dry-run` | `-n` / `--dry-run` | N/A |
| Force overwrite | `-f` / `--force` | `-f` / `--force` | N/A |
| Structured JSON output | `--json` / `--machine` | N/A | N/A |
| Skip errors | `-k` / `--skip-errors` | `-k` | N/A |
| Sparse checkout | `--sparse` accepted as no-op | `--sparse` | N/A |
| Submodule cascade rename | Out of scope (not performed) | Updates `.gitmodules` | N/A |

Note: jj does not have a dedicated mv command. File renames are detected automatically by the working-copy snapshot mechanism.

## Error Handling

| Scenario | Error Message |
|----------|---------------|
| Fewer than 2 arguments | Usage information printed |
| Source does not exist | `fatal: bad source, source=<src>, destination=<dst>` |
| Source is the same as destination | `fatal: can not move directory into itself` |
| Multiple sources with non-directory destination | `fatal: destination '<dst>' is not a directory` |
| Source not tracked in index | `fatal: not under version control, source=<src>, destination=<dst>` |
| Source has merge conflicts | `fatal: conflicted, source=<src>, destination=<dst>` |
| Destination exists without `--force` | `fatal: destination already exists, source=<src>, destination=<dst>` |
| Directory destination already has source name | `fatal: destination already exists, source=<src>, destination=<dst>` |
| Path outside repository | `fatal: '<path>' is outside of the repository at '<workdir>'` |
| Multiple sources targeting the same path | `fatal: multiple sources moving to the same target path` |
| Filesystem rename failed | `fatal: failed to move, source=<src>, destination=<dst>, error=<err>` |
| Index save failed | `fatal: failed to save index after mv: <err>` |

## Exit Codes

Libra never lets the argument parser exit with a bare `2`; CLI usage errors are
remapped to the stable code table below. Coarse codes are the default; set
`LIBRA_FINE_EXIT_CODES=1` for the fine-grained (`2`–`9`) variants.

| Scenario | Coarse exit | Stable code |
|----------|-------------|-------------|
| Success (including `-n` dry-run and `--sparse` no-op) | `0` | — |
| Too few arguments (`usage:`) | `129` | `LBR-CLI-002` |
| Unknown flag / parse error (located to `mv`) | `129` | `LBR-CLI-002` |
| Source/destination outside the repository | `129` | `LBR-CLI-003` |
| Multiple sources with a non-directory destination | `129` | `LBR-CLI-003` |
| Source does not exist (`bad source`) | `129` | `LBR-CLI-003` |
| Move a directory into itself | `129` | `LBR-CLI-003` |
| Untracked source / duplicate target | `128` | `LBR-CONFLICT-002` |
| Conflicted (unmerged) source | `128` | `LBR-CONFLICT-001` |
| Destination exists without `-f` | `128` | `LBR-CONFLICT-002` |
| Filesystem rename / `create_dir_all` / index-save failure | `128` | (runtime `fatal:`) |
| Not inside a Libra repository | `128` | `LBR-REPO-001` |

The `mv` happy path emits no warnings, so it never triggers the
`--exit-code-on-warning` warning exit (`9`).
