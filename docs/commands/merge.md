# `libra merge`

Merge one or more targets into the current branch.

## Synopsis

```text
libra merge <branch> [<branch>...]
libra merge --continue
libra merge --abort
libra merge --quit
```

## Description

`libra merge <branch>` resolves a local branch, commit hash, or remote-tracking ref such as `refs/remotes/origin/main`.

If the current branch can be fast-forwarded, Libra moves the branch pointer to the target commit and restores the index and working tree. If the branches have diverged, Libra performs a single-head three-way merge using the best merge base. Clean multi-head merges are supported for disjoint changes and create an N-parent octopus merge commit.

Clean three-way merges create a two-parent merge commit, update HEAD, rebuild the index, restore the working tree, and write a merge reflog entry. Conflicting three-way merges write conflict markers to the working tree, write unmerged index stages, save Libra merge state, and return `LBR-CONFLICT-002` with hints for `libra merge --continue` and `libra merge --abort`.

Libra supports fast-forward policy flags/config (`--ff-only`, `--no-ff`, `merge.ff`), squash merges, `--no-commit`, custom merge messages, signoff trailers, `-s ours`, `-X ours|theirs`, binary conflict detection, diff3 conflict markers, and simple octopus merges. Deferred Git merge features are listed below and are intentionally not accepted as no-op flags.

## Options

| Option | Description |
|--------|-------------|
| `<branch>...` | Target branches, commits, or remote-tracking refs to merge. Multiple targets use octopus mode and must be clean/disjoint. |
| `--continue` | Finish an in-progress merge after conflicts have been resolved and staged. |
| `--abort` | Restore the pre-merge HEAD, index, and working tree. |
| `--quit` | Remove merge state while leaving the index and working tree untouched. |
| `--ff-only` | Refuse unless the target can fast-forward HEAD. Overrides `merge.ff`. |
| `--no-ff` | Create a merge commit even when a fast-forward is possible. Overrides `merge.ff`. |
| `--squash` | Apply merged changes to the index and working tree without moving HEAD or writing merge state. Cannot be combined with `--no-ff` or `--commit`. |
| `--no-commit` | Stop after a clean real merge with merge state, index, and worktree updated; finish with `libra merge --continue`. Fast-forwards still fast-forward unless `--no-ff` is also used. |
| `--commit` | Explicitly request the default commit-after-clean-merge behavior. |
| `--allow-unrelated-histories` | Permit a two-head merge without a common ancestor. |
| `--autostash`, `--no-autostash` | Stash local changes before merging and reapply them afterward. Honors `merge.autoStash`. A conflict defers reapplication until `--continue`/`--abort`. |
| `-S`, `--gpg-sign` | Sign the merge commit with the Libra vault key. (A key id is not accepted; the vault key is always used.) |
| `--no-gpg-sign` | Do not sign the merge commit (the default). |
| `--verify-signatures`, `--no-verify-signatures` | Require (or skip) a signature on the merged commit before merging. Honors `merge.verifySignatures`. |
| `-m`, `--message <msg>` | Use the provided merge commit message. |
| `-F`, `--file <path>` | Read the merge commit message from a file. |
| `--signoff` | Append a `Signed-off-by` trailer to merge commit messages. |
| `-s ours`, `--strategy ours` | Use Git's `ours` strategy for the merge result. |
| `-X ours`, `-X theirs` | Resolve content/delete conflicts in favor of one side. |
| `--log[=<n>]` | Append up to `n` shortlog entries to the merge commit message (`20` when omitted). |
| `--no-log` | Do not append a shortlog (overrides `--log`). |
| `--no-signoff` | Do not add a `Signed-off-by` trailer (overrides `--signoff`). |
| `--no-squash` | Create a merge commit instead of squashing (the default; overrides `--squash`). |
| `--into-name <name>` | Override the branch name recorded in the auto-generated merge message. |
| `-e`, `--edit`, `--no-edit` | Open (or skip) the merge commit message in `$GIT_EDITOR`/`core.editor`/`$VISUAL`/`$EDITOR`. With no usable editor the message is used unchanged. |
| `--conflict=diff3` | Include base content in conflict markers. `merge.conflictstyle=diff3` is also supported. |
| `--stat`, `-n`/`--no-stat` | Print (or suppress) a diffstat of what the merge brought in. `--summary`/`--no-summary` are accepted aliases. Honors the `merge.stat` config; defaults off so existing output stays stable. |
| `--diff-algorithm <algo>` | Validate the requested content-merge algorithm (`myers`/`histogram`/`patience`/`minimal`). Libra uses a single Myers-style backend. |
| `--ignore-space-change`, `--ignore-all-space`, `--ignore-space-at-eol`, `--ignore-cr-at-eol` | Ignore the named whitespace class when auto-merging text, so a side whose only change is whitespace yields to the side with a real change. |
| `--find-renames`, `--no-renames` | Enable (default) or disable rename detection so an edit on one side follows a rename on the other. Honors `merge.renames`; uses a 50% content-similarity threshold. |
| `--cleanup <mode>` | Validate the message cleanup mode (`strip`/`whitespace`/`verbatim`/`scissors`/`default`). Libra already trims merge messages. |
| `--no-verify` | Accepted for Git compatibility. Libra runs no pre-merge or commit-msg hooks yet, so this has no effect. |
| `--overwrite-ignore`, `--no-overwrite-ignore` | Accepted for Git compatibility; Libra always preserves ignored files during merge. |
| `--rerere-autoupdate`, `--no-rerere-autoupdate` | Accepted for Git compatibility; Libra has no rerere resolution store. |

Progress output is controlled by the global `--progress=<json\|text\|none\|auto>` flag rather than a merge-specific `--progress` toggle.
| `--json` | Emit a structured success envelope. |
| `--machine` | Emit the same structured envelope as one compact JSON line. |
| `--quiet` | Suppress human success output. |

## Supported Merge Config

| Key | Values | Behavior |
|-----|--------|----------|
| `merge.ff` | `true`/`false`/`only` | Default/true allows fast-forward, false behaves like `--no-ff`, only behaves like `--ff-only`. CLI flags override config. |
| `merge.conflictstyle` | `merge`/`diff3` | Selects default conflict marker style. |
| `merge.stat` | `true`/`false` | When true, print a diffstat after a successful merge (off by default; `--stat`/`--no-stat` override). |
| `merge.autoStash` | `true`/`false` | When true, autostash local changes around every merge (off by default; `--autostash`/`--no-autostash` override). |
| `merge.verifySignatures` | `true`/`false` | When true, require the merged commit to be signed (off by default; `--verify-signatures`/`--no-verify-signatures` override). |
| `merge.renames` | `true`/`false` | Enable rename detection (on by default; `--find-renames`/`--no-renames` override). |

`merge.commit` is intentionally absent because stock Git does not define that config key.

## Common Commands

```bash
libra merge feature-x
libra merge left right
libra merge refs/remotes/origin/main
libra merge --ff-only feature-x
libra merge --no-ff --no-commit feature-x
libra merge --squash feature-x
libra merge --continue
libra merge --abort
libra merge --quit
libra merge --json feature-x
```

## Conflict Lifecycle

When a merge conflicts:

1. Edit files containing conflict markers.
2. Stage each resolved path with `libra add <path>`.
3. Run `libra merge --continue` to create the two-parent merge commit.

Run `libra merge --abort` before continuing to restore the branch, index, and working tree to the pre-merge commit. `libra status` shows the in-progress merge target and the continue/abort commands while merge state exists.

## Deferred Git Merge Features

The following Git flags are not implemented and are not accepted as ignored no-ops: custom merge drivers, custom strategies beyond `ours`, subtree strategy, and advanced octopus conflict resolution. Full cryptographic verification of signatures is reduced to a presence check; `--verify-signatures` confirms a signature exists rather than validating it against a keyring. Rename detection handles the clean rename-plus-edit case (and falls back to a delete/modify conflict when the relocated merge would itself conflict); directory renames are not tracked.

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
| Branch target | `<branch>...` (clean octopus supported) | `<commit>...` (one or more) | N/A (use `jj new`) |
| Fast-forward | Supported | Supported | N/A |
| Single-head three-way | Supported | Supported | N/A |
| Continue / abort | `--continue`, `--abort` | `--continue`, `--abort` | N/A |
| Octopus merge | Clean disjoint changes supported; conflicts refused | Supported | N/A |
| Squash | Supported | `--squash` | N/A |
| Custom strategy | `ours` and `-X ours/theirs` only | `--strategy`, `-X` | N/A |
| Commit message | `-m`, `-F`, `--log`, `--signoff` | `-m <msg>` | N/A |
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
