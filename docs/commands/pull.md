# `libra pull`

Fetch objects from a remote and integrate them into the current branch via fast-forward merge.

## Synopsis

```
libra pull [<repository> [<refspec>]]
```

## Description

`libra pull` combines `fetch` and `merge` (fast-forward only) into a single operation.
It downloads new objects from the remote, updates remote-tracking refs, and advances the
current branch's HEAD if the remote is strictly ahead. The working tree is updated to
match the new HEAD.

When invoked with no arguments, the command reads the current branch's tracking
configuration (`branch.<name>.remote` and `branch.<name>.merge`) to determine which
remote and branch to pull from. When `<repository>` is given alone, the current branch
name is used as the remote branch. When both `<repository>` and `<refspec>` are given,
the specified remote branch is fetched and merged.

Only fast-forward merges are performed. If the remote branch has diverged (i.e., the
local branch has commits not present on the remote), the pull is rejected with an
actionable error suggesting manual fetch-then-merge.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<repository>` | Remote name to pull from. When omitted, uses the current branch's configured upstream. | `libra pull origin` |
| `<refspec>` | Branch name on the remote. Requires `<repository>`. When omitted, uses the current branch name. | `libra pull origin main` |
| `--json` | Emit structured JSON envelope to stdout (global flag). | `libra pull --json` |
| `--machine` | Compact single-line JSON; suppresses progress (global flag). | `libra pull --machine` |
| `--quiet` | Suppress all progress and the merge summary. | `libra pull --quiet` |

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

## Structured Output (JSON examples)

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

## Design Rationale

### Why fast-forward only?

Three-way merge during pull is the single largest source of unintended merge commits in
trunk-based development. A developer pulls, gets an automatic merge commit, pushes, and
the project history accumulates a tangle of "Merge branch 'main' of ..." commits that
convey no semantic information. Libra enforces fast-forward-only pulls to keep history
linear. When the local branch has diverged, the user is directed to fetch and merge (or
rebase) manually, making the decision explicit. This is consistent with Libra's design
philosophy of making destructive or history-altering operations require deliberate action.

### Why no --rebase?

`git pull --rebase` silently rewrites local commits onto the remote tip. While convenient,
it changes commit hashes, which can confuse AI agents that reference specific commits by
SHA, break bisect sessions in progress, and create subtle issues in collaborative
workflows. Libra keeps `pull` as a simple fetch+fast-forward operation. When rebasing is
desired, `libra fetch` followed by `libra rebase` provides the same result with full
visibility into what is happening at each step. This two-command approach also allows
agents to inspect the fetched state before deciding whether to rebase.

### Why no --no-ff?

The `--no-ff` flag in Git forces creation of a merge commit even when a fast-forward is
possible. This is useful in Git for preserving branch topology in the history graph. Libra
takes the opposite stance: pull should never create merge commits. Merge commits are the
domain of `libra merge`, where the user (or agent) explicitly chooses to integrate
diverged branches. Keeping pull limited to fast-forward means the command is always safe
and never modifies the commit graph in unexpected ways.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Basic pull | `libra pull` | `git pull` | N/A (jj uses `jj git fetch` + working copy) |
| Pull from specific remote | `libra pull origin main` | `git pull origin main` | N/A |
| Rebase on pull | Not supported | `git pull --rebase` | N/A (jj rebases automatically) |
| Force merge commit | Not supported | `git pull --no-ff` | N/A |
| Fast-forward only | Default (only mode) | `git pull --ff-only` | N/A |
| Structured output | `--json` / `--machine` | No | No |
| Error hints | Every error type has an actionable hint | Minimal | Minimal |
| Phase diagnostics | `phase` detail in error JSON | No | No |

## Error Handling

Every `PullError` variant maps to an explicit `StableErrorCode`. Fetch and merge
sub-errors are transparently forwarded with a `phase` detail for diagnostics.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| HEAD is detached | `LBR-REPO-003` | 128 | "checkout a branch before pulling" |
| No tracking info for branch | `LBR-REPO-003` | 128 | "use 'libra branch --set-upstream-to=\<remote>/\<branch>'" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to see configured remotes" |
| Fetch: network unreachable | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Fetch: authentication failed | `LBR-AUTH-001` | 128 | "check SSH key or HTTP credentials" |
| Fetch: protocol error | `LBR-NET-002` | 128 | "the remote did not respond correctly" |
| Fetch: timeout | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Manual merge required | `LBR-CONFLICT-002` | 128 | "run 'libra fetch' then merge manually with 'libra merge \<upstream>'" |
| Merge: invalid target | `LBR-CLI-003` | 129 | "verify the upstream ref and try again" |
| Merge: unrelated histories | `LBR-REPO-003` | 128 | "the local and remote branches share no common ancestor" |
| Merge: repository state error | `LBR-REPO-003` | 128 | "the repository state blocks an automatic pull merge" |
| Merge: repository corruption | `LBR-REPO-002` | 128 | "inspect repository state and object integrity" |
| Merge: write failure | `LBR-IO-002` | 128 | "check filesystem permissions and retry" |

### Phase Detail

When a fetch or merge sub-operation fails, the error JSON includes a `phase` key in the
details object (`"fetch"` or `"merge"`) so agents can distinguish which stage failed.
