# `libra push`

`libra push` sends local commits and objects to a remote repository, updating remote refs.
It supports SSH and HTTPS transports, LFS file uploads (HTTP only), fast-forward detection,
force push, dry-run preview, and refspec mapping.

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

## Structured Output

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
| Tracking ref update failed | `LBR-IO-002` | 128 | — |
| Repository state error | `LBR-REPO-002` | 128 | "try 'libra status' to verify" |

### Timeout Policy

- Discovery / connection: 10s connection timeout
- Upload / receive-pack: 10s idle timeout (no data progress triggers timeout)
- Timeouts are mapped to `NetworkUnavailable` with `phase` detail

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Basic push | `git push` | `libra push` |
| Set upstream | `git push -u origin main` | `libra push -u origin main` |
| Force push | `git push --force` | `libra push --force` |
| Dry-run | `git push --dry-run` | `libra push --dry-run` |
| Refspec mapping | `git push origin src:dst` | `libra push origin src:dst` |
| Delete remote branch | `git push origin :branch` | Not supported |
| Push tags | `git push --tags` | Not supported |
| Structured output | No | `--json` / `--machine` |
| Remote name suggestion | No | Fuzzy match "did you mean?" |
| Error hints | Minimal | Every error type has an actionable hint |
| LFS integration | `git lfs push` (separate) | Transparent during HTTP push |
