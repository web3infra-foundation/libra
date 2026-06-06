# `libra pull`

Fetch objects from a remote and integrate the fetched branch into the current branch.

## Synopsis

```text
libra pull [--ff-only | --ff | --no-ff] [--rebase[=<when>]]
          [--squash | --no-squash] [--commit | --no-commit]
          [--autostash | --no-autostash] [--depth <n>]
          [<repository> [<refspec>]]
```

## Description

`libra pull` combines `fetch` and the same merge engine used by `libra merge`. It downloads new objects, updates remote-tracking refs, and then integrates the selected upstream into the current branch.

With `--rebase` (`-r`), the integration step instead replays local-only commits on top of the fetched upstream tip. This is equivalent to `libra fetch` followed by `libra rebase <upstream>`.

With `--ff-only`, pull fetches the upstream but refuses to create a merge commit when local and remote histories have diverged. Fast-forward and already-up-to-date pulls still succeed. `--ff-only` conflicts with `--rebase`.

When invoked with no arguments, the command reads the current branch tracking configuration (`branch.<name>.remote` and `branch.<name>.merge`). When `<repository>` is given alone, the current branch name is used as the remote branch. When both `<repository>` and `<refspec>` are given, the specified remote branch is fetched and merged.

Pull supports already-up-to-date, fast-forward, and single-head three-way merge results. If the local and remote branches conflict, pull returns the merge-owned `LBR-CONFLICT-002` error with `phase: "merge"` and leaves the same merge state that `libra merge` uses. Resolve conflicts with `libra add <path>` and `libra merge --continue`, or run `libra merge --abort`.

On the merge path, pull forwards the merge engine's strategy flags: `--squash`/`--no-squash` (stage a squashed result without committing — finish with `libra commit`), `--commit`/`--no-commit` (stop before the merge commit), `--ff`/`--no-ff` (allow or forbid fast-forward), and `--autostash`/`--no-autostash` (stash a dirty tree before integrating, then restore it). `--depth <n>` performs a shallow fetch before integrating. These merge-only flags may **not** be combined with `--rebase`.

Pull reads the `pull.rebase` and `pull.ff` config keys (command-line flags always override). The fast-forward cascade is `--ff-only` > `--no-ff` > `--ff` > `pull.ff` > `merge.ff`; `--commit`/`--no-commit` fall back to `merge.commit`; `--autostash`/`--no-autostash` fall back to `merge.autoStash`. `--squash` cannot be combined with no-fast-forward (`--no-ff` or `pull.ff=false`), `--commit`, or autostash (`--autostash` or `merge.autoStash=true`) — a squash conflict has no resumable merge state.

**Not supported (rejected, never silently degraded):** `--rebase=merges` / `--rebase=interactive` (the rebase engine only does linear rebase), autostash on the rebase path, and `--unshallow`.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<repository>` | Remote name to pull from. When omitted, uses the current branch's configured upstream. | `libra pull origin` |
| `<refspec>` | Branch name on the remote. Requires `<repository>`. When omitted, uses the current branch name. | `libra pull origin main` |
| `--ff-only` | Refuse to create a merge commit; succeeds only for fast-forward or already-up-to-date pulls. Merge path only. | `libra pull --ff-only` |
| `--ff` | Allow a fast-forward merge; clears `--no-ff` and overrides `pull.ff=false`. Merge path only. | `libra pull --ff` |
| `--no-ff` | Always create a merge commit even when fast-forward is possible. Merge path only. | `libra pull --no-ff` |
| `-r`, `--rebase[=<when>]` | After fetching, rebase the current branch onto the upstream tip instead of merging. `<when>` is `true` (default), `false` (force merge, overriding `pull.rebase`), or `merges`/`interactive` (rejected — unsupported). Use the `=` form for a value. | `libra pull --rebase` / `--rebase=false` |
| `--squash` / `--no-squash` | Stage a squashed merge result without recording a merge commit; finish with `libra commit`. Merge path only. | `libra pull --squash` |
| `--commit` / `--no-commit` | Create the merge commit (overrides `merge.commit=false`) or stop before it. Merge path only. | `libra pull --no-commit` |
| `--autostash` / `--no-autostash` | Stash a dirty working tree before integrating and restore it afterwards (or force off, overriding `merge.autoStash`). Merge path only. | `libra pull --autostash` |
| `--depth <n>` | Shallow-fetch the upstream to `<n>` commits before integrating. | `libra pull --depth 1` |
| `--json` | Emit structured JSON envelope to stdout (global flag). | `libra pull --json` |
| `--machine` | Compact single-line JSON; suppresses progress (global flag). | `libra pull --machine` |
| `--quiet` | Suppress all progress and merge summary output. | `libra pull --quiet` |

## Examples

```
# Pull the current branch's configured upstream (fetch + merge).
libra pull

# Pull a specific remote branch into the current branch.
libra pull origin main

# Fast-forward only — refuse to create a merge commit on divergence.
libra pull --ff-only

# Rebase local commits onto the fetched upstream instead of merging.
libra pull --rebase

# Force a merge commit even when fast-forward is possible.
libra pull --no-ff

# Stage a squashed merge without committing (finish with `libra commit`).
libra pull --squash

# Stash a dirty tree, integrate, then restore it.
libra pull --autostash

# Shallow-fetch before integrating.
libra pull --depth 1 origin main

# Structured JSON envelope for agents/tooling.
libra pull --json origin main
```

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
- `merge.strategy` is `"fast-forward"`, `"three-way"`, `"squash"`, or `"already-up-to-date"`.
- `merge.commit` is the new HEAD commit after merge; it is `null` when up to date, for `--squash` (staged but not committed), and for `--no-commit` (HEAD left at the recorded merge state).
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
| Fast-forward-only pull | `libra pull --ff-only` | `git pull --ff-only` | N/A |
| Three-way integration | Supported through merge engine | Supported | N/A |
| Rebase on pull | `libra pull --rebase` | `git pull --rebase` | N/A |
| `--rebase=merges`/`interactive` | Rejected (unsupported) | `git pull --rebase=merges` | N/A |
| Force merge commit | `libra pull --no-ff` | `git pull --no-ff` | N/A |
| Squash | `libra pull --squash` (merge path) | `git pull --squash` | N/A |
| Stop before commit | `libra pull --no-commit` | `git pull --no-commit` | N/A |
| Autostash (merge path) | `libra pull --autostash` | `git pull --autostash` | N/A |
| Shallow pull | `libra pull --depth <n>` | `git pull --depth <n>` | N/A |
| Unshallow | Not supported | `git pull --unshallow` | N/A |
| `pull.rebase` / `pull.ff` config | Read (CLI overrides) | Read | N/A |
| Structured output | `--json` / `--machine` | No | No |
| Phase diagnostics | `phase` detail in error JSON | No | No |

## Error Handling

Every `PullError` variant maps to an explicit `StableErrorCode`. Fetch, merge, and rebase sub-errors are forwarded with a `phase` detail for diagnostics.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| HEAD is detached | `LBR-REPO-003` | 128 | "checkout a branch before pulling" |
| No tracking info for branch | `LBR-REPO-003` | 128 | "specify the remote and branch" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to see configured remotes" |
| `--rebase` combined with a merge-only flag (`--squash`/`--commit`/`--no-commit`/`--ff`/`--no-ff`/`--ff-only`/`--autostash`) | `LBR-CLI-002` | 129 | "remove one of the conflicting options and retry" |
| `--squash` combined with no-fast-forward, `--commit`, or autostash (incl. `pull.ff=false` / `merge.autoStash=true`) | `LBR-CLI-002` | 129 | "remove one of the conflicting options and retry" |
| `--rebase=merges` / `--rebase=interactive` (or `pull.rebase=merges`/`interactive`) | `LBR-CLI-003` | 129 | "omit the strategy or use a plain '--rebase' for a linear rebase" |
| Invalid `pull.rebase` / `pull.ff` / `merge.commit` config value | `LBR-CLI-002` | 129 | "set a valid value with 'libra config <key> <value>'" |
| Merge: squash conflict | `LBR-CONFLICT-002` | 128 | "resolve the conflicts, stage the result, then run 'libra commit'" (squash has no `libra merge --continue`) |
| Fetch: network unreachable / timeout | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Fetch: authentication failed | `LBR-AUTH-001` | 128 | "check SSH key or HTTP credentials" |
| Fetch: protocol error | `LBR-NET-002` | 128 | "the remote did not respond correctly" |
| Merge: conflicts, dirty worktree, or untracked overwrite | `LBR-CONFLICT-002` | 128 | "resolve conflicts, then run 'libra merge --continue'" |
| Merge: non-fast-forward rejected by `--ff-only` | `LBR-CONFLICT-002` | 128 | "run 'libra pull' without --ff-only to allow a merge commit" |
| Rebase: conflict during replay | `LBR-CONFLICT-001` | 128 | "resolve conflicts, stage them, then run 'libra rebase --continue'" |
| Rebase: dirty worktree | `LBR-REPO-003` | 128 | "commit or stash your changes before rebasing" |
| Merge: invalid target | `LBR-CLI-003` | 129 | "verify the upstream ref and try again" |
| Merge: unrelated histories or invalid merge state | `LBR-REPO-003` | 128 | "inspect branch history and merge state" |
| Merge: repository corruption | `LBR-REPO-002` | 128 | "inspect repository state and object integrity" |
| Merge: read failure | `LBR-IO-001` | 128 | "check repository metadata and permissions" |
| Merge: write failure | `LBR-IO-002` | 128 | "check filesystem permissions and retry" |

### Phase Detail

When a sub-operation fails, the error JSON includes a `phase` key in the details object (`"fetch"`, `"merge"`, or `"rebase"`) so agents can distinguish which stage failed.
