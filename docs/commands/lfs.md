# `libra lfs`

Manage Large File Storage for binary and media assets.

## Synopsis

```
libra lfs track [<pattern>...]
libra lfs untrack <path>...
libra lfs locks [--id <ID>] [--path <PATH>] [--limit <N>]
libra lfs lock <path>
libra lfs unlock <path> [--force] [--id <ID>]
libra lfs ls-files [--long] [--size] [--name-only]
libra lfs install
libra lfs uninstall
libra lfs push [<remote>] [<ref>...]
libra lfs fetch [<remote>] [<ref>...]
libra lfs prune [--dry-run]
libra lfs checkout [<path>...]
```

## Description

`libra lfs` provides built-in Large File Storage for managing binary files, media assets, and other large objects that do not diff or merge well. Instead of storing the full file content in the repository, LFS replaces large files with lightweight pointer files and stores the actual content on a dedicated LFS server.

LFS tracking is configured through Libra Attributes (`.libra_attributes`), which maps glob patterns to the LFS filter. The `track` and `untrack` subcommands manage these patterns. File locking prevents concurrent edits to binary files that cannot be merged, with server-side enforcement via the LFS lock API.

**Unlike Git, Libra never installs `git-lfs` smudge/clean filters or pre-push hooks, and never writes `.gitattributes`.** LFS is integrated natively: the LFS client, pointer-file parsing, attribute management, and the `push`/`fetch`/`prune`/`checkout` object-sync flows are all built into the `libra` binary, driven by `.libra_attributes`. This intentional difference is decision **D5** ([`docs/improvement/compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge`](../improvement/compatibility/declined.md)); `install`/`uninstall` exist only as no-op compatibility shims so automation that calls `git lfs install` does not break.

## Options

`libra lfs` has no top-level options. All functionality is accessed through subcommands documented below.

## Subcommands

### `track`

View or add LFS tracking patterns to Libra Attributes.

```bash
# List currently tracked patterns
libra lfs track

# Track all PNG files
libra lfs track "*.png"

# Track multiple patterns
libra lfs track "*.psd" "*.zip" "assets/**"
```

| Argument | Description |
|----------|-------------|
| `<pattern>...` | Optional glob patterns to add. If omitted, lists existing tracked patterns. |

When called without arguments, prints each tracked pattern and the attributes file it was found in:

```text
Listing tracked patterns
    *.png (.libra_attributes)
    *.psd (.libra_attributes)
```

When called with patterns, appends them to the root `.libra_attributes` file, creating the file if it does not exist.

### `untrack`

Remove LFS tracking patterns from Libra Attributes.

```bash
libra lfs untrack "*.png"
```

| Argument | Description |
|----------|-------------|
| `<path>...` | One or more patterns to remove from `.libra_attributes`. |

Removes exact matches of the specified patterns from the attributes file. Files already committed as LFS pointers remain as pointers until re-added normally.

### `locks`

List files currently locked on the LFS server for the current branch.

```bash
# List all locks
libra lfs locks

# Filter by path
libra lfs locks --path assets/logo.png

# Filter by lock ID
libra lfs locks --id 12345

# Limit results
libra lfs locks --limit 10
```

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| ID | `-i` | `--id` | Filter by lock ID. |
| Path | `-p` | `--path` | Filter by file path. |
| Limit | `-l` | `--limit` | Maximum number of locks to return. |

Output format:

```text
assets/logo.png    ID:12345
docs/spec.pdf      ID:12346
```

### `lock`

Lock a file on the LFS server to prevent concurrent edits.

```bash
libra lfs lock assets/logo.png
```

| Argument | Description |
|----------|-------------|
| `<path>` | Path to the file to lock, relative to the repository root. |

The file must exist in the working tree. On success, prints `Locked <path>`. Locking requires push access to the repository.

### `unlock`

Remove a lock from a file on the LFS server.

```bash
# Unlock by path
libra lfs unlock assets/logo.png

# Force unlock (skip working tree check)
libra lfs unlock assets/logo.png --force

# Unlock by ID
libra lfs unlock assets/logo.png --id 12345
```

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Force | `-f` | `--force` | Skip file existence and working-tree cleanliness checks. |
| ID | `-i` | `--id` | Unlock by lock ID instead of looking up the ID from the path. |

Without `--force`, the command verifies that the file exists and the working tree is clean before unlocking. With `--force`, these checks are bypassed -- useful for unlocking files that have been deleted or when the working tree is intentionally dirty.

### `ls-files`

Show information about LFS-tracked files in the index.

```bash
# Default output (short OID, pointer status)
libra lfs ls-files

# Show full 64-character OID
libra lfs ls-files --long

# Include file size
libra lfs ls-files --size

# Show only filenames
libra lfs ls-files --name-only
```

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Long | `-l` | `--long` | Show the entire 64-character OID instead of the first 10 characters. |
| Size | `-s` | `--size` | Show the LFS object size in parentheses at the end of each line. |
| Name only | `-n` | `--name-only` | Show only the tracked file names, without OID or status. |

Output uses `*` after the OID to indicate a full (smudged) object and `-` to indicate an LFS pointer:

```text
a1b2c3d4e5 * assets/logo.png
f6g7h8i9j0 - docs/spec.pdf
```

### `install` / `uninstall`

```bash
libra lfs install
libra lfs uninstall
```

**No-ops.** Git LFS uses `git lfs install` to register global smudge/clean filters and pre-push hooks. Libra has built-in LFS and needs none of that (**D5**), so these subcommands change nothing, print a short notice, and exit 0 — keeping CI scripts that call `git lfs install` working. They must run inside a Libra repository (they go through the standard repository preflight).

### `push`

Upload the LFS objects referenced by the current branch to a remote.

```bash
libra lfs push                 # push current branch's LFS objects to the upstream
libra lfs push origin          # ...to the named remote
libra lfs push origin main     # the remote is always the FIRST positional
```

| Argument | Description |
|----------|-------------|
| `[<remote>]` | Remote name (first positional). Defaults to the current branch upstream. |
| `[<ref>...]` | Additional positionals. Push currently operates on the **current branch only**; an explicit ref that is not the current branch is rejected (`LBR-CLI-003`, exit 129). |

Push scans the current branch's reachable commits, collects the LFS pointers, verifies every referenced object exists locally (a missing object is a hard error, never a silent skip), then uploads via the LFS batch protocol with server-side lock verification. Repositories with no LFS pointers exit 0 without contacting the server.

> Scope note: push is limited to the current branch because lock verification uses the current refspec and index. Push other branches by checking them out first.

### `fetch`

Download LFS objects referenced by remote-tracking refs that are missing from the local cache.

```bash
libra lfs fetch                # fetch missing objects for the current branch
libra lfs fetch origin         # ...from the named remote
libra lfs fetch origin main    # the remote is always the FIRST positional
```

| Argument | Description |
|----------|-------------|
| `[<remote>]` | Remote name (first positional). Defaults to the current branch upstream. |
| `[<ref>...]` | Refs to scan. Each prefers the remote-tracking ref (`<remote>/<ref>`), falling back to a local ref of the same name. |

Each downloaded object lands in a temporary file and is **independently hash-verified** (its SHA-256 must equal the pointer OID) before being atomically renamed into `.libra/lfs/objects/<a>/<b>/<oid>`. A remote 404 (which writes a pointer placeholder) or checksum mismatch never corrupts the object store, and no `.tmp` file is left behind. If no objects are missing, fetch is a no-op and contacts no server.

### `prune`

Delete local LFS objects not referenced by any reachable ref or the index.

```bash
libra lfs prune              # delete unreferenced cached objects
libra lfs prune --dry-run    # report what would be deleted, delete nothing
```

| Flag | Short | Long | Description |
|------|-------|------|-------------|
| Dry run | `-n` | `--dry-run` | List the objects that would be pruned without deleting anything. |

The keep set is every OID referenced by a reachable commit (branches, tags, `HEAD`, and reflog OIDs — ancestry is followed) **plus** the OIDs staged in the current index, so objects you have `add`ed but not yet committed are never deleted. Malformed cache entries (non-64-hex filenames) are skipped, individual removal failures degrade to a warning, and emptied sharding directories are cleaned up. Reports `Pruned <n> files (<size>)`.

> Reachability boundary: the keep set does **not** include remote-tracking branches or git-lfs's `--recent` time window. Make sure unpushed objects are pushed before pruning.

### `checkout`

Restore working-tree pointer files to their full LFS object content from the local cache.

```bash
libra lfs checkout                  # restore all LFS pointer files
libra lfs checkout assets/logo.png  # restore only the given path(s)
```

| Argument | Description |
|----------|-------------|
| `[<path>...]` | Optional paths to restore. Defaults to all LFS-tracked pointer files. |

For each LFS-tracked file that is currently a pointer in the working tree, checkout looks up the cached object, **verifies its hash before overwriting**, and replaces the pointer with the full content. Files already materialized (non-pointers) are skipped, and a missing cache object leaves the pointer untouched with a notice (not a fatal error) — run `libra lfs fetch` first.

## JSON / Machine Output

`--json` and `--machine` are supported for every subcommand. `--json` writes one command envelope to stdout, and `--machine` emits the same envelope as a compact single JSON line. Download/upload progress is suppressed under structured output so stdout carries only the envelope. The `data.action` field identifies the subcommand (`track`, `untrack`, `locks`, `lock`, `unlock`, `ls-files`, `install`, `uninstall`, `push`, `fetch`, `prune`, `checkout`). Subcommand-specific additive fields — all omitted when empty/zero — are: `pushed_oids` (push), `fetched_oids` (fetch), `pruned_files` + `size_freed` + `dry_run` (prune), and `restored_paths` (checkout).

Tracking patterns:

```json
{
  "ok": true,
  "command": "lfs",
  "data": {
    "action": "track",
    "patterns": ["*.png"]
  }
}
```

Listing LFS files:

```json
{
  "ok": true,
  "command": "lfs",
  "data": {
    "action": "ls-files",
    "show_size": true,
    "files": [
      {
        "path": "assets/logo.png",
        "oid": "a1b2c3d4e5",
        "marker": "-",
        "size": 1024,
        "display_size": " (1.00 KiB)"
      }
    ]
  }
}
```

Lock operations include `path`, `id` when available, `refspec`, or a `locks` array for `lfs locks`.

## Common Commands

```bash
# Set up LFS tracking for common binary types
libra lfs track "*.png" "*.jpg" "*.gif" "*.pdf" "*.zip"

# Check what is being tracked
libra lfs track

# See all LFS files with sizes
libra lfs ls-files --size

# Lock a file before editing
libra lfs lock assets/hero-image.psd

# Check current locks
libra lfs locks

# Unlock after committing changes
libra lfs unlock assets/hero-image.psd

# Stop tracking a pattern
libra lfs untrack "*.gif"

# Sync LFS objects with a remote
libra lfs push origin
libra lfs fetch origin

# Reclaim disk by pruning unreferenced objects (preview first)
libra lfs prune --dry-run
libra lfs prune

# Materialize pointer files back to full content
libra lfs checkout
```

## Design Rationale

### Why built-in LFS instead of a separate extension?

Git LFS is a separate binary that hooks into Git via smudge/clean filters and a custom transfer agent. This architecture has several pain points:
- **Installation friction**: Every developer must install `git-lfs` and run `git lfs install` to configure filters. Forgetting this step silently commits pointer files as regular blobs.
- **Filter misconfiguration**: Smudge/clean filter setup is fragile. A `.gitattributes` typo or missing filter config leads to corrupted checkouts where pointer files appear instead of content.
- **Transfer complexity**: Git LFS intercepts `git push`/`git pull` via pre-push hooks and custom transfer protocols, adding failure modes that are difficult to debug.

Libra integrates LFS at the binary level: the pointer format, attribute parsing, batch API client, and lock management are all compiled in. `libra add` automatically detects LFS-tracked patterns and creates pointer files. `libra checkout` automatically smudges pointers back to full content. No hooks, no filters, no separate installation.

### Why file locking?

Binary files (PSDs, compiled assets, large datasets) cannot be merged. When two developers edit the same binary file, one of them will lose their work on merge. File locking provides server-side coordination: `libra lfs lock` claims exclusive edit rights, and `libra lfs unlock` releases them. The `locks` subcommand lets developers see who has locked what before starting work.

The `--force` flag on `unlock` is an escape hatch for administrators to release stale locks (e.g., when the lock holder is on vacation or has left the team).

### Why check working-tree cleanliness on unlock?

Unlocking a file while the working tree is dirty could indicate that the developer has uncommitted LFS changes that would be lost if someone else immediately locks and modifies the file. The cleanliness check is a safety reminder to commit before releasing the lock. `--force` bypasses this for cases where the dirty state is unrelated to the locked file.

## Parameter Comparison: Libra vs Git (git-lfs) vs jj

| Parameter | Libra | Git (git-lfs) | jj |
|-----------|-------|---------------|-----|
| Track patterns | `libra lfs track <pattern>` | `git lfs track <pattern>` | Not available |
| Untrack patterns | `libra lfs untrack <pattern>` | `git lfs untrack <pattern>` | Not available |
| List tracked patterns | `libra lfs track` (no args) | `git lfs track` (no args) | Not available |
| List locks | `libra lfs locks` | `git lfs locks` | Not available |
| Lock a file | `libra lfs lock <path>` | `git lfs lock <path>` | Not available |
| Unlock a file | `libra lfs unlock <path>` | `git lfs unlock <path>` | Not available |
| Force unlock | `--force` | `--force` | Not available |
| List LFS files | `libra lfs ls-files` | `git lfs ls-files` | Not available |
| Long OID | `--long` | `--long` | Not available |
| File size | `--size` | `--size` | Not available |
| Name only | `--name-only` | `--name-only` | Not available |
| Install / uninstall | `libra lfs install` / `uninstall` (no-op shims) | `git lfs install` / `uninstall` (registers filters/hooks) | Not available |
| Push objects | `libra lfs push [<remote>]` (current branch) | `git lfs push <remote> <ref>` | Not available |
| Fetch objects | `libra lfs fetch [<remote>] [<ref>...]` | `git lfs fetch <remote> <ref>` | Not available |
| Prune cache | `libra lfs prune [--dry-run]` (refs+tags+HEAD+reflog+index) | `git lfs prune` (+`--recent` window) | Not available |
| Checkout pointers | `libra lfs checkout [<path>...]` | `git lfs checkout` | Not available |
| Installation required | Built-in | Separate `git-lfs` install + `git lfs install` | Not available |
| Attributes file | `.libra_attributes` | `.gitattributes` | Not available |
| Filter configuration | Automatic | Manual (smudge/clean filters) | Not available |

Note: jj does not currently have LFS support. Large file management in jj repositories requires using Git's LFS infrastructure via jj's Git backend.

## Error Handling

| Scenario | StableErrorCode | Description |
|----------|-----------------|-------------|
| `lock` on non-existent path | `CliInvalidTarget` | The specified file does not exist in the working tree. |
| `lock` without push access | `AuthPermissionDenied` | The user lacks push permissions on the repository. |
| `lock` on already-locked file | `ConflictOperationBlocked` | A lock already exists for the specified path. |
| `unlock` on non-existent path (no `--force`) | `CliInvalidTarget` | The specified file does not exist. |
| `unlock` with dirty working tree (no `--force`) | `ConflictOperationBlocked` | The working tree has uncommitted changes. |
| `unlock` on file with no lock | `RepoStateInvalid` | No lock was found for the specified path. |
| `unlock` without push access | `AuthPermissionDenied` | The user lacks push permissions. |
| Failed to read/write `.libra_attributes` | IO error | The attributes file could not be read or written. |
| Failed to load index | IO error | The repository index is corrupted or missing. |
| LFS server communication failure | `NetworkUnavailable` | The LFS server was unreachable or a download/upload failed. |
| `push` of a non-current branch | `CliInvalidTarget` | Push operates on the current branch only; check out the target branch first. |
| `push` with an object missing from the local cache | `RepoStateInvalid` | A referenced LFS object is absent locally; run `libra lfs fetch` or restore it. |
| `push`/`fetch` with an unknown remote | `NetworkUnavailable` | No URL is configured for the requested remote. |
| `checkout` against a corrupt cached object | `RepoCorrupt` | The cached object failed hash verification and was not used to overwrite the pointer. |
