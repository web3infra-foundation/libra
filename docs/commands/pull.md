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
   abc1234..def5678  origin/main
Updating abc1234..def5678
Fast-forward
 3 files changed
```

Already up-to-date:

```text
From git@github.com:user/repo.git
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
      "old_commit": "abc1234...",
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
      "old_commit": "def5678...",
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
- `merge.old_commit` is the pre-merge `HEAD`; it is `null` on the first pull into an empty local branch
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
| Merge: invalid target | `LBR-CLI-003` | 129 | "verify the upstream ref and try again" |
| Merge: repository state error | `LBR-REPO-003` | 128 | "the repository state blocks an automatic pull merge" |
| Merge: repository corruption | `LBR-REPO-002` | 128 | "inspect repository state and object integrity" |
| Merge: write failure | `LBR-IO-002` | 128 | "check filesystem permissions and retry" |

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
