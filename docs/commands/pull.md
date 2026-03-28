# `libra pull`

`libra pull` fetches objects from a remote and integrates them into the current branch.
It combines `fetch` and `merge` (fast-forward only) into a single operation, updating
the working tree and remote tracking refs.

## Common Commands

```bash
libra pull
libra pull origin main
libra pull --json
```

## Human Output

Default human mode writes progress to `stderr` and the merge summary to `stdout`.

Fast-forward:

```text
From git@github.com:user/repo.git
   abc1234..def5678  main -> origin/main
Updating abc1234..def5678
Fast-forward
 src/lib.rs | 5 +++++
 3 files changed
```

Already up-to-date:

```text
Already up to date.
```

`--quiet` suppresses all progress and the merge summary.

## Structured Output

`libra pull` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Example (fast-forward):

```json
{
  "ok": true,
  "command": "pull",
  "data": {
    "branch": "main",
    "upstream": "origin/main",
    "fetch": {
      "remote": "origin",
      "url": "git@github.com:user/repo.git",
      "refs_updated": [
        {
          "remote_ref": "refs/remotes/origin/main",
          "old_oid": "abc1234...",
          "new_oid": "def5678..."
        }
      ],
      "objects_fetched": 12
    },
    "merge": {
      "strategy": "fast-forward",
      "commit": "def5678...",
      "files_changed": 3,
      "up_to_date": false
    }
  }
}
```

Already up-to-date:

```json
{
  "ok": true,
  "command": "pull",
  "data": {
    "branch": "main",
    "upstream": "origin/main",
    "fetch": {
      "remote": "origin",
      "url": "git@github.com:user/repo.git",
      "refs_updated": [],
      "objects_fetched": 0
    },
    "merge": {
      "strategy": "already-up-to-date",
      "commit": null,
      "files_changed": 0,
      "up_to_date": true
    }
  }
}
```

### Schema Notes

- `branch` is the current local branch being updated
- `upstream` is the remote tracking branch name (e.g. `"origin/main"`)
- `fetch.refs_updated` lists remote refs that changed during fetch
- `merge.strategy` is `"fast-forward"` or `"already-up-to-date"`
- `merge.commit` is the new HEAD commit after merge; `null` when up-to-date
- `merge.files_changed` is the number of files modified by the merge

## Error Handling

Every `PullError` variant maps to an explicit `StableErrorCode`. Fetch and merge
sub-errors are transparently forwarded with a `phase` detail for diagnostics.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| HEAD is detached | `LBR-REPO-003` | 128 | "checkout a branch before pulling" |
| No tracking info for branch | `LBR-REPO-003` | 128 | "use 'libra branch --set-upstream-to=<remote>/<branch>'" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to see configured remotes" |
| Fetch: network unreachable | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Fetch: authentication failed | `LBR-AUTH-001` | 128 | "check SSH key or HTTP credentials" |
| Fetch: protocol error | `LBR-NET-002` | 128 | "the remote did not respond correctly" |
| Fetch: timeout | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Manual merge required | `LBR-CONFLICT-002` | 128 | "automatic merge is not possible; resolve conflicts manually" |
| Merge: conflict | `LBR-CONFLICT-001` | 128 | "resolve conflicts and commit" |
| Merge: internal error | `LBR-INTERNAL-001` | 128 | Issues URL |

### Phase Detail

When a fetch or merge sub-operation fails, the error JSON includes a `phase` key in the
details object (`"fetch"` or `"merge"`) so agents can distinguish which stage failed.

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Basic pull | `git pull` | `libra pull` |
| Pull from specific remote | `git pull origin main` | `libra pull origin main` |
| Rebase on pull | `git pull --rebase` | Not yet supported |
| Merge strategy | `git pull --no-ff` | Fast-forward only |
| Structured output | No | `--json` / `--machine` |
| Error hints | Minimal | Every error type has an actionable hint |
| Phase diagnostics | No | `phase` detail in error JSON |
