# `libra push`

Send local commits and objects to a remote repository, updating remote refs.
Supports SSH and HTTPS transports, LFS file uploads (HTTP only), fast-forward detection,
force push, dry-run preview, and refspec mapping.

## Synopsis

```
libra push [OPTIONS] [<repository> <refspec>]
```

## Description

`libra push` transfers commits, trees, blobs, and tags from the local repository to a
remote. When invoked without arguments it pushes the current branch to its configured
upstream remote. When a `repository` and `refspec` are given, the specified local ref
is pushed to the named remote ref.

The command negotiates with the remote to determine which objects are missing, packs them
into a single pack file, and sends the pack along with a ref-update request. If the remote
ref has diverged (non-fast-forward), the push is rejected unless `--force` is used.

LFS-tracked files are transparently uploaded during HTTP pushes without requiring a
separate `lfs push` step.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<repository>` | Remote name (e.g. `origin`). Required when `<refspec>` is given. | `libra push origin main` |
| `<refspec>` | Local ref or `<src>:<dst>` mapping. Required when `<repository>` is given. | `libra push origin feature:release` |
| `-u`, `--set-upstream` | Set the upstream tracking branch after a successful push. Requires both `<repository>` and `<refspec>`. | `libra push -u origin feature-x` |
| `-f`, `--force` | Allow non-fast-forward updates that overwrite remote history. | `libra push --force origin main` |
| `-n`, `--dry-run` | Perform negotiation and object collection but skip the actual upload. Reports what would be pushed. | `libra push --dry-run` |
| `--json` | Emit structured JSON envelope to stdout (global flag). | `libra push --json` |
| `--machine` | Compact single-line JSON; suppresses progress (global flag). | `libra push --machine` |
| `--quiet` | Suppress stdout summary; warnings still go to stderr. | `libra push --quiet` |

## Common Commands

```bash
libra push
libra push origin main
libra push -u origin feature-x
libra push --force origin main
libra push --dry-run
libra push origin local_branch:release
libra push --json
```

## Human Output

Default human mode writes progress to `stderr` and the push summary to `stdout`.

Normal push:

```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main
 256 objects pushed (1.2 MiB)
```

New branch:

```text
To git@github.com:user/repo.git
 * [new branch]      feature-x -> feature-x
 12 objects pushed (48.0 KiB)
```

Up-to-date:

```text
Everything up-to-date
```

Force push:

```text
To git@github.com:user/repo.git
 + abc1234...def5678 main -> main (forced update)
 128 objects pushed (512.0 KiB)
warning: force push overwrites remote history
```

Dry-run:

```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main (dry run)
 256 objects would be pushed
```

Set upstream:

```text
To git@github.com:user/repo.git
   abc1234..def5678  main -> main
 256 objects pushed (1.2 MiB)
branch 'main' set up to track 'origin/main'
```

`--quiet` suppresses `stdout` but preserves warnings (e.g. force push) on `stderr`.

## Structured Output (JSON examples)

`libra push` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- progress output is suppressed in JSON/machine mode
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": false
      }
    ],
    "objects_pushed": 256,
    "bytes_pushed": 1258291,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": false,
    "upstream_set": null,
    "warnings": []
  }
}
```

Up-to-date:

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [],
    "objects_pushed": 0,
    "bytes_pushed": 0,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": true,
    "upstream_set": null,
    "warnings": []
  }
}
```

Dry-run:

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": false
      }
    ],
    "objects_pushed": 256,
    "bytes_pushed": 0,
    "lfs_files_uploaded": 0,
    "dry_run": true,
    "up_to_date": false,
    "upstream_set": null,
    "warnings": []
  }
}
```

Force push:

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": true
      }
    ],
    "objects_pushed": 128,
    "bytes_pushed": 524288,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": false,
    "upstream_set": null,
    "warnings": ["force push overwrites remote history"]
  }
}
```

Set upstream:

```json
{
  "ok": true,
  "command": "push",
  "data": {
    "remote": "origin",
    "url": "git@github.com:user/repo.git",
    "updates": [
      {
        "local_ref": "refs/heads/main",
        "remote_ref": "refs/heads/main",
        "old_oid": "abc1234...",
        "new_oid": "def5678...",
        "forced": false
      }
    ],
    "objects_pushed": 256,
    "bytes_pushed": 1258291,
    "lfs_files_uploaded": 0,
    "dry_run": false,
    "up_to_date": false,
    "upstream_set": "origin/main",
    "warnings": []
  }
}
```

### Schema Notes

- `updates` lists each ref update; empty when up-to-date
- `old_oid` is `null` for new branches (no previous remote ref)
- `forced` is `true` when the update required `--force` (non-fast-forward)
- `bytes_pushed` is the pack data size in bytes; `0` for dry-run
- `lfs_files_uploaded` counts LFS objects transferred (HTTP transport only)
- `upstream_set` is non-null when `-u` / `--set-upstream` was used
- `warnings` contains force push warnings or other advisory messages

## Refspec Semantics

Three forms are supported in this version:

| Invocation | Meaning |
|-----------|---------|
| `libra push` | Push current branch to its configured tracking remote |
| `libra push origin main` | Push local `refs/heads/main` to remote `refs/heads/main` |
| `libra push origin local:release` | Push local `refs/heads/local` to remote `refs/heads/release` |

Delete syntax (`:ref`), empty source (`src:`), and multi-refspec are not supported.
Invalid forms return `InvalidRefspec` with exit 129.

## Design Rationale

### Why require an explicit repository+refspec pair?

Git allows `git push origin` (push current branch to same-named remote branch) and treats
`repository` and `refspec` as independent optional arguments with complex defaulting rules
(`push.default`, `remote.pushDefault`, branch tracking config). This flexibility is a
well-known source of accidental pushes to the wrong branch. Libra takes a deliberately
restrictive stance: when you name a remote you must also name the ref. The bare
`libra push` form (no arguments) uses the tracking configuration, which is unambiguous.
This eliminates an entire class of "I accidentally pushed to production" mistakes without
reducing the expressiveness of the command for scripted or agent-driven workflows.

### Why no --tags?

`git push --tags` pushes all local tags to the remote, which is a frequent source of
namespace pollution in monorepos. Tags in Libra are intended to be managed explicitly
via `libra tag` and pushed as part of normal refspec operations. A dedicated `--tags`
flag would encourage bulk-pushing every local tag, conflicting with Libra's design goal
of deliberate, minimal ref updates. If tag pushing is needed, the refspec syntax
(`libra push origin v1.0:v1.0`) provides explicit control.

### Why integrated LFS push?

Git LFS requires a separate binary (`git-lfs`) and a post-push hook to upload large files.
This two-phase design means LFS failures can leave the remote in an inconsistent state
where commits reference LFS pointers whose backing objects have not arrived. Libra detects
LFS pointer blobs during the object-collection phase and uploads them inline during the
HTTP push transaction. This ensures atomicity: either all objects (including LFS) arrive,
or the push fails cleanly. The integration is transparent -- users do not need to install
or configure a separate LFS tool.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Basic push | `libra push` | `git push` | `jj git push` |
| Named remote + ref | `libra push origin main` | `git push origin main` | `jj git push --remote origin --branch main` |
| Set upstream | `libra push -u origin main` | `git push -u origin main` | N/A (jj tracks bookmarks) |
| Force push | `libra push --force` | `git push --force` | `jj git push --allow-new` |
| Dry-run | `libra push --dry-run` | `git push --dry-run` | `jj git push --dry-run` |
| Refspec mapping | `libra push origin src:dst` | `git push origin src:dst` | N/A |
| Delete remote branch | Not supported | `git push origin :branch` | `jj git push --delete branch` |
| Push tags | Not supported | `git push --tags` | N/A |
| Structured output | `--json` / `--machine` | No | No |
| Remote name suggestion | Fuzzy match "did you mean?" | No | No |
| Error hints | Every error type has an actionable hint | Minimal | Minimal |
| LFS integration | Transparent during HTTP push | `git lfs push` (separate) | N/A |

## Error Handling

Every `PushError` variant maps to an explicit `StableErrorCode`. Remote name typos
trigger a fuzzy match suggestion via edit distance.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| HEAD is detached | `LBR-REPO-003` | 128 | "checkout a branch before pushing" |
| No remote configured | `LBR-REPO-003` | 128 | "use 'libra remote add' to configure a remote" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v'" + fuzzy "did you mean?" |
| Invalid refspec | `LBR-CLI-002` | 129 | "use '\<name>' or '\<src>:\<dst>'" |
| Source ref not found | `LBR-CLI-003` | 129 | "verify the local branch/ref exists" |
| Local file remote | `LBR-CLI-003` | 129 | "push supports network remotes only" |
| Invalid remote URL | `LBR-CLI-002` | 129 | "check the remote URL" |
| Authentication failed | `LBR-AUTH-001` | 128 | "check SSH key or HTTP credentials" |
| Discovery failed | `LBR-NET-001` | 128 | "check the remote URL and network connectivity" |
| Network timeout | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Non-fast-forward | `LBR-CONFLICT-002` | 128 | "pull first, or use --force (data loss risk)" |
| Object collection failed | `LBR-INTERNAL-001` | 128 | Issues URL |
| Pack encoding failed | `LBR-INTERNAL-001` | 128 | Issues URL |
| Remote unpack failed | `LBR-NET-002` | 128 | "retry or check server logs" |
| Remote ref update rejected | `LBR-NET-002` | 128 | "check branch protection rules" |
| Network error | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| LFS upload failed | `LBR-NET-001` | 128 | "check LFS endpoint configuration" |
| Tracking ref update failed | `LBR-IO-002` | 128 | -- |
| Repository state error | `LBR-REPO-002` | 128 | "try 'libra status' to verify" |

### Timeout Policy

- Discovery / connection: 10s connection timeout
- Upload / receive-pack: 10s idle timeout (no data progress triggers timeout)
- Timeouts are mapped to `NetworkUnavailable` with `phase` detail
