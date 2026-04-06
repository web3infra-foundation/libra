# `libra mv`

Move or rename files and directories.

## Synopsis

```
libra mv [<options>] <source>... <destination>
```

## Description

`libra mv` moves or renames files and directories in the working tree and updates the index accordingly. The last argument is always the destination; all preceding arguments are sources. When there are multiple sources, the destination must be an existing directory.

The command validates that all source paths exist, are tracked in the index, are not in a conflicted state, and reside within the repository working directory. Directory moves are performed as a single filesystem rename, with individual index entries updated for each tracked file within the directory. Untracked files inside a moved directory are carried along by the filesystem rename but are not added to the index.

After all filesystem moves succeed, the index is updated atomically: old entries are removed and new entries (with recalculated blob hashes) are inserted. The index is saved only after all operations complete successfully.

## Options

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Verbose | `-v` | `--verbose` | Print each rename operation as it happens. |
| Dry run | `-n` | `--dry-run` | Show what would be moved without actually performing any moves. |
| Force | `-f` | `--force` | Overwrite an existing destination file instead of reporting an error. Only works for regular files and symlinks; directories cannot be overwritten. |

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

## Design Rationale

### Why paths-based instead of explicit `--source` / `--dest`?

Libra follows the same convention as Git's `mv` and the Unix `mv` command: the last argument is the destination, and all preceding arguments are sources. This is familiar to every Unix user and avoids the verbosity of named flags for what is fundamentally a positional operation.

The trade-off is that the command requires at least two arguments and the semantics change depending on whether the destination is an existing directory. This is the same trade-off that Unix `mv` and Git `mv` make, and decades of usage have shown it to be intuitive in practice.

### Why no `--sparse`?

Git's `mv` supports `--sparse` to allow moving files outside the sparse-checkout cone. Libra does not yet implement sparse checkout, so this flag has no meaning. It will be added if and when sparse checkout support is implemented.

### Why validate tracked status?

Unlike a plain filesystem `mv`, `libra mv` refuses to move files that are not tracked in the index. This prevents confusion where a user moves a file expecting the rename to be recorded in version control, but the file was never tracked in the first place. If you need to move an untracked file, use the system `mv` command.

### Why refuse conflicted files?

Moving a file that is in a conflicted state (stages 1-3 in the index) would lose conflict information. Libra requires conflicts to be resolved before the file can be moved.

### How does this compare to Git and jj?

Git's `mv` command is similar in design: it moves files in the working tree and updates the index. It supports a few additional flags (`-k` to skip move errors, `--sparse`) but is otherwise straightforward.

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
| Skip errors | Not supported | `-k` | N/A |
| Sparse checkout | Not supported | `--sparse` | N/A |

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
