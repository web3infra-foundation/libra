# `libra merge`

Merge one target into the current branch.

## Synopsis

```text
libra merge <branch>
libra merge --continue
libra merge --abort
```

## Description

`libra merge <branch>` resolves a local branch, commit hash, or remote-tracking ref such as `refs/remotes/origin/main`.

If the current branch can be fast-forwarded, Libra moves the branch pointer to the target commit and restores the index and working tree. If the branches have diverged, Libra performs a single-head three-way merge using the merge base.

Clean three-way merges create a two-parent merge commit, update HEAD, rebuild the index, restore the working tree, and write a merge reflog entry. Conflicting three-way merges write conflict markers to the working tree, write unmerged index stages, save Libra merge state, and return `LBR-CONFLICT-002` with hints for `libra merge --continue` and `libra merge --abort`.

Libra still does not implement octopus merges, custom strategies, `--squash`, `--no-ff`, strategy options, message editing, or signature verification.

## Options

| Option | Description |
|--------|-------------|
| `<branch>` | Target branch, commit, or remote-tracking ref to merge. |
| `--continue` | Finish an in-progress merge after conflicts have been resolved and staged. |
| `--abort` | Restore the pre-merge HEAD, index, and working tree. |
| `--json` | Emit a structured success envelope. |
| `--machine` | Emit the same structured envelope as one compact JSON line. |
| `--quiet` | Suppress human success output. |

## Common Commands

```bash
libra merge feature-x
libra merge refs/remotes/origin/main
libra merge --continue
libra merge --abort
libra merge --json feature-x
```

## Conflict Lifecycle

When a merge conflicts:

1. Edit files containing conflict markers.
2. Stage each resolved path with `libra add <path>`.
3. Run `libra merge --continue` to create the two-parent merge commit.

Run `libra merge --abort` before continuing to restore the branch, index, and working tree to the pre-merge commit. `libra status` shows the in-progress merge target and the continue/abort commands while merge state exists.

## Human Output

Fast-forward:

```text
Fast-forward
```

Clean three-way merge:

```text
Merge made by the 'three-way' strategy.
```

Already up to date:

```text
Already up to date.
```

After `--continue`:

```text
Merge completed.
```

After `--abort`:

```text
Merge aborted.
```

Conflict errors are printed through Libra's standard structured error envelope on stderr and include recovery hints.

## JSON / Machine Output

Success output keeps the historical `files_changed` numeric field and adds merge-lifecycle fields only when relevant.

```json
{
  "ok": true,
  "command": "merge",
  "data": {
    "strategy": "three-way",
    "old_commit": "abc1234...",
    "commit": "def5678...",
    "files_changed": 2,
    "up_to_date": false,
    "parents": ["abc1234...", "fedcba9..."]
  }
}
```

Already-up-to-date merges use `strategy: "already-up-to-date"`, `commit: null`, `files_changed: 0`, and `up_to_date: true`.

`--abort` sets `aborted: true`; `--continue` sets `continued: true`. Conflict failures return an error envelope on stderr with `LBR-CONFLICT-002`.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Branch target | `<branch>` (single target) | `<commit>...` (one or more) | N/A (use `jj new`) |
| Fast-forward | Supported | Supported | N/A |
| Single-head three-way | Supported | Supported | N/A |
| Continue / abort | `--continue`, `--abort` | `--continue`, `--abort` | N/A |
| Octopus merge | Not supported | Supported | N/A |
| Squash | Not supported | `--squash` | N/A |
| Custom strategy | Not supported | `--strategy`, `-X` | N/A |
| Commit message | Not supported | `-m <msg>` | N/A |
| Verify signatures | Not supported | `--verify-signatures` | N/A |
| JSON output | `--json` / `--machine` | Not supported | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Missing branch / action | `LBR-CLI-001` | 129 |
| Target ref cannot be resolved | `LBR-CLI-003` | 129 |
| Failed to load merge target/current commit/tree | `LBR-REPO-002` | 128 |
| Unrelated histories | `LBR-REPO-003` | 128 |
| Merge conflicts | `LBR-CONFLICT-002` | 128 |
| Dirty worktree or staged changes | `LBR-CONFLICT-002` | 128 |
| Untracked file would be overwritten | `LBR-CONFLICT-002` | 128 |
| Merge already in progress | `LBR-CONFLICT-002` | 128 |
| No merge in progress for `--continue` / `--abort` | `LBR-REPO-003` | 128 |
| Unresolved conflict stages remain for `--continue` | `LBR-CONFLICT-002` | 128 |
| Failed to read merge state or index | `LBR-IO-001` | 128 |
| Failed to save state, index, tree, commit, HEAD, or worktree | `LBR-IO-002` | 128 |
