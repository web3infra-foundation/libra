# `libra merge`

Merge a branch into the current branch.

## Synopsis

```
libra merge <branch>
```

## Description

`libra merge` merges the specified branch into the current branch using fast-forward only. If the current branch's HEAD is an ancestor of the target branch, the branch pointer is moved forward to the target commit and the working tree is updated. If a fast-forward is not possible (i.e., the branches have diverged), the command refuses to merge and exits with an error.

The merge target can be a local branch name, a commit hash, or a remote tracking branch reference (e.g., `refs/remotes/origin/main`). The command resolves remote branch references through the branch store and falls back to generic commit resolution for other ref formats.

After advancing the branch pointer, the command runs `restore --worktree --staged .` to update the working tree and index to match the new HEAD.

## Options

| Option | Description |
|--------|-------------|
| `<branch>` | Required positional argument. The branch to merge into the current branch. Can be a local branch, commit hash, or remote branch reference. |

## Common Commands

```bash
# Fast-forward merge a local branch
libra merge feature-x

# Merge a remote tracking branch
libra merge refs/remotes/origin/main

# Merge a specific commit
libra merge abc1234
```

## Human Output

When the merge is a fast-forward:

```text
Fast-forward
```

When already up to date:

```text
Already up to date.
```

When fast-forward is not possible:

```text
error: Not possible to fast-forward merge, try merge manually
```

## Design Rationale

### Why fast-forward only?

Libra targets trunk-based and AI-agent workflows where the merge topology should remain linear. Fast-forward merges produce a clean, linear history that is easy for both humans and automated tools to traverse. Three-way merge commits introduce diamond shapes in the DAG that complicate log traversal, blame attribution, and automated bisection.

By restricting to fast-forward, Libra pushes users toward rebasing divergent branches before merging, which keeps history linear by default. This is a deliberate trade-off: it sacrifices the ability to preserve exact branch topology in exchange for a simpler, more predictable history.

### Why no `--no-ff`, `--squash`, or `--abort`?

- **`--no-ff`**: Forces a merge commit even when fast-forward is possible. This is useful for preserving branch structure in feature-branch workflows, but works against Libra's preference for linear history.
- **`--squash`**: Collapses all branch commits into a single staged change. Libra does not yet implement this because it requires staging without committing, which overlaps with rebase squash workflows.
- **`--abort`**: Aborts an in-progress merge with conflicts. Since Libra does not perform three-way merges, there is no conflict state to abort from.

These flags may be added in the future as the merge engine matures to support recursive and three-way strategies.

### Why a single branch argument?

Git's `merge` accepts multiple branches for octopus merges. Libra accepts exactly one branch because fast-forward merges are inherently a two-commit operation (current HEAD and target). This simplification eliminates an entire class of edge cases around multi-parent merge commits.

### How does this compare to Git and jj?

Git's `merge` command supports numerous strategies (`recursive`, `ort`, `octopus`, `ours`, `subtree`) and flags for controlling merge commit creation, conflict resolution, and commit messages. It is one of the most complex commands in Git.

jj does not have a `merge` command at all. Instead, `jj new A B` creates a new change with multiple parents, which implicitly creates a merge. The merge is resolved lazily when the change is materialized. This is a fundamentally different model where merges are first-class revisions rather than special operations.

Libra sits between these extremes: it provides an explicit `merge` command (familiar to Git users) but constrains it to the simplest possible strategy (fast-forward only), deferring complex merge resolution to rebase workflows.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Branch target | `<branch>` (required, single) | `<commit>...` (one or more) | N/A (use `jj new`) |
| Fast-forward only | Always (only strategy) | Default when possible | N/A |
| No fast-forward | Not supported | `--no-ff` | N/A |
| Squash | Not supported | `--squash` | N/A |
| Abort | Not supported | `--abort` | N/A |
| Continue | Not supported | `--continue` | N/A |
| Strategy | Fast-forward only | `--strategy` (`recursive`, `ort`, `octopus`, etc.) | N/A |
| Strategy option | Not supported | `-X <option>` | N/A |
| Commit message | Not supported | `-m <msg>` | N/A |
| Verify signatures | Not supported | `--verify-signatures` | N/A |
| Stat output | Not supported | `--stat` | N/A |
| JSON output | Not supported | Not supported | N/A |

Note: jj does not have a dedicated merge command. Merges are created implicitly via `jj new` with multiple parent revisions.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Target ref cannot be resolved | `LBR-CLI-003` (CliInvalidTarget) | 129 |
| Failed to load merge target commit | `LBR-REPO-002` (RepoCorrupt) | 128 |
| Failed to load current commit | `LBR-REPO-002` (RepoCorrupt) | 128 |
| Failed to inspect merge history | `LBR-REPO-002` (RepoCorrupt) | 128 |
| Failed to load tree object | `LBR-REPO-002` (RepoCorrupt) | 128 |
| Unrelated histories (no common ancestor) | `LBR-REPO-003` (RepoStateInvalid) | 128 |
| Non-fast-forward merge required | `LBR-CONFLICT-002` (ConflictOperationBlocked) | 128 |
| Failed to resolve HEAD state | `LBR-IO-001` (IoReadFailed) | 128 |
| Failed to update HEAD during merge | `LBR-IO-002` (IoWriteFailed) | 128 |
| Failed to restore working tree after merge | `LBR-IO-002` (IoWriteFailed) | 128 |
