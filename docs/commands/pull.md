# `libra pull`

Fetch objects from a remote and integrate the fetched branch into the current branch.

## Synopsis

```text
libra pull [--rebase] [<repository> [<refspec>]]
```

## Description

`libra pull` combines `fetch` and the same merge engine used by `libra merge`. It downloads new objects, updates remote-tracking refs, and then integrates the selected upstream into the current branch.

With `--rebase` (`-r`), the integration step instead replays local-only commits on top of the fetched upstream tip. This is equivalent to `libra fetch` followed by `libra rebase <upstream>`.

When invoked with no arguments, the command reads the current branch tracking configuration (`branch.<name>.remote` and `branch.<name>.merge`). When `<repository>` is given alone, the current branch name is used as the remote branch. When both `<repository>` and `<refspec>` are given, the specified remote branch is fetched and merged.

Pull supports already-up-to-date, fast-forward, and single-head three-way merge results. If the local and remote branches conflict, pull returns the merge-owned `LBR-CONFLICT-002` error with `phase: "merge"` and leaves the same merge state that `libra merge` uses. Resolve conflicts with `libra add <path>` and `libra merge --continue`, or run `libra merge --abort`.

`pull` does not implement `--ff-only`, `--squash`, custom merge strategies, or pull-specific strategy flags.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<repository>` | Remote name to pull from. When omitted, uses the current branch's configured upstream. | `libra pull origin` |
| `<refspec>` | Branch name on the remote. Requires `<repository>`. When omitted, uses the current branch name. | `libra pull origin main` |
| `-r`, `--rebase` | After fetching, rebase the current branch onto the upstream tip instead of merging. | `libra pull --rebase` |
| `--json` | Emit structured JSON envelope to stdout (global flag). | `libra pull --json` |
| `--machine` | Compact single-line JSON; suppresses progress (global flag). | `libra pull --machine` |
| `--quiet` | Suppress all progress and merge summary output. | `libra pull --quiet` |

## Human Output

Default human mode writes fetch progress to `stderr` and the pull summary to `stdout`.

Fast-forward:

```text
From git@github.com:user/repo.git
   abc1234..def5678  origin/main
Updating abc1234..def5678
Fast-forward
 3 files changed
```

Clean three-way merge:

```text
From git@github.com:user/repo.git
   abc1234..def5678  origin/main
Updating abc1234..def5678
Merge made by the 'three-way' strategy.
 2 files changed
```

Already up to date:

```text
From git@github.com:user/repo.git
Already up to date.
```

Rebase:

```text
From git@github.com:user/repo.git
   abc1234..def5678  origin/main
Successfully rebased 2 commits onto 'origin/main' (1111111..2222222).
```

`--quiet` suppresses all progress and merge summary output.

## Structured Output

`--json` writes one success envelope to stdout. `--machine` writes the same schema as one compact JSON line. Success leaves stderr clean.

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
      "strategy": "three-way",
      "old_commit": "abc1234...",
      "commit": "def5678...",
      "files_changed": 2,
      "up_to_date": false,
      "parents": ["abc1234...", "fedcba9..."]
    }
  }
}
```

Rebase output omits `merge` and includes `rebase`:

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
    "rebase": {
      "status": "completed",
      "old_commit": "1111111...",
      "commit": "2222222...",
      "replay_count": 2,
      "up_to_date": false
    }
  }
}
```

### Schema Notes

- `branch` is the current local branch being updated.
- `upstream` is the remote tracking branch name, such as `"origin/main"`.
- `fetch.refs_updated` lists remote refs that changed during fetch.
- Exactly one of `merge` or `rebase` is present, depending on whether `--rebase` was passed.
- `merge.old_commit` is the pre-merge `HEAD`; it is `null` on the first pull into an empty local branch.
- `merge.strategy` is `"fast-forward"`, `"three-way"`, or `"already-up-to-date"`.
- `merge.commit` is the new HEAD commit after merge; it is `null` when up to date.
- `merge.parents` appears for successful three-way merge commits.
- `merge.files_changed` is the number of paths changed by the merge result.
- `rebase.status` is `"completed"`, `"fast-forwarded"`, `"already-up-to-date"`, or `"no-commits"`.
- `rebase.replay_count` is the number of local commits replayed onto the upstream tip.
- `rebase.up_to_date` is `true` when the rebase did not move `HEAD`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Basic pull | `libra pull` | `git pull` | N/A (jj uses `jj git fetch` + working copy) |
| Pull from specific remote | `libra pull origin main` | `git pull origin main` | N/A |
| Fast-forward integration | Supported | Supported | N/A |
| Three-way integration | Supported through merge engine | Supported | N/A |
| Rebase on pull | `libra pull --rebase` | `git pull --rebase` | N/A |
| Force merge commit | Not supported | `git pull --no-ff` | N/A |
| Squash | Not supported | `git pull --squash` | N/A |
| Structured output | `--json` / `--machine` | No | No |
| Phase diagnostics | `phase` detail in error JSON | No | No |

## Error Handling

Every `PullError` variant maps to an explicit `StableErrorCode`. Fetch, merge, and rebase sub-errors are forwarded with a `phase` detail for diagnostics.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| HEAD is detached | `LBR-REPO-003` | 128 | "checkout a branch before pulling" |
| No tracking info for branch | `LBR-REPO-003` | 128 | "specify the remote and branch" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to see configured remotes" |
| Fetch: network unreachable / timeout | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Fetch: authentication failed | `LBR-AUTH-001` | 128 | "check SSH key or HTTP credentials" |
| Fetch: protocol error | `LBR-NET-002` | 128 | "the remote did not respond correctly" |
| Merge: conflicts, dirty worktree, or untracked overwrite | `LBR-CONFLICT-002` | 128 | "resolve conflicts, then run 'libra merge --continue'" |
| Rebase: conflict during replay | `LBR-CONFLICT-001` | 128 | "resolve conflicts, stage them, then run 'libra rebase --continue'" |
| Rebase: dirty worktree | `LBR-REPO-003` | 128 | "commit or stash your changes before rebasing" |
| Merge: invalid target | `LBR-CLI-003` | 129 | "verify the upstream ref and try again" |
| Merge: unrelated histories or invalid merge state | `LBR-REPO-003` | 128 | "inspect branch history and merge state" |
| Merge: repository corruption | `LBR-REPO-002` | 128 | "inspect repository state and object integrity" |
| Merge: read failure | `LBR-IO-001` | 128 | "check repository metadata and permissions" |
| Merge: write failure | `LBR-IO-002` | 128 | "check filesystem permissions and retry" |

### Phase Detail

When a sub-operation fails, the error JSON includes a `phase` key in the details object (`"fetch"`, `"merge"`, or `"rebase"`) so agents can distinguish which stage failed.
