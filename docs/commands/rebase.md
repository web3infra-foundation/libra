# `libra rebase`

Reapply commits on top of another base tip.

**Alias:** `rb`

## Synopsis

```
libra rebase <upstream>
libra rebase --onto <newbase> <upstream> [<branch>]
libra rebase --root [--onto <newbase>] [<branch>]
libra rebase --continue
libra rebase --abort
libra rebase --skip
```

## Description

`libra rebase` moves a sequence of commits from the current branch onto a new base commit. It finds the common ancestor between the current branch and the specified upstream, collects all commits from that ancestor to the current HEAD, and replays each commit on top of the upstream branch. After all commits are replayed, the current branch reference is updated to point to the final rebased commit.

Each replayed commit preserves its **original author** while recording a fresh **committer** stamped with the current identity (`user.name` / `user.email`), matching Git's rebase semantics.

If a conflict occurs during replay, the rebase stops and reports the conflicting files. The user resolves conflicts manually, stages the resolved files, and then runs `libra rebase --continue` to proceed. Alternatively, `--abort` restores the original branch state and `--skip` discards the current commit and moves on to the next.

Rebase state (the list of remaining and completed commits, the original HEAD, and the target base) is persisted in the SQLite database. This makes rebase state survive process restarts and avoids the fragile file-based state that Git uses. Legacy file-based state from older Libra versions is automatically migrated to the database on first access.

## Options

| Option | Long | Description |
|--------|------|-------------|
| `<upstream>` | | The upstream branch or commit to rebase onto. Required unless `--continue`, `--abort`, or `--skip` is specified. Can be a branch name, commit hash, or any Git reference. |
| `<branch>` | | Optional branch to switch to before starting the rebase. Used with the two- or three-argument form. |
| | `--onto <newbase>` | Replay the `<upstream>..<branch-or-HEAD>` range onto `<newbase>` instead of onto `<upstream>`. |
| | `--root` | Replay the entire branch history from the root commit. Single-root histories are supported; multi-root histories are rejected. |
| | `--autostash` / `--no-autostash` | Stash dirty work before replay and restore the saved stash by OID after completion or abort. Reads `rebase.autoStash`. |
| | `--autosquash` / `--no-autosquash` | Non-interactively move and fold `fixup!`, `squash!`, and `amend!` commits. Reads `rebase.autoSquash`. |
| | `--reapply-cherry-picks` / `--no-reapply-cherry-picks` | Reapply or skip commits whose patch already exists on the target side. |
| | `--keep-empty` / `--no-keep-empty` | Preserve or drop commits that were already empty before replay. Reads `rebase.keepEmpty`. |
| | `--empty=<drop\|keep\|stop>` | Control commits that become empty after replay. Reads `rebase.empty`; `ask` is not supported. |
| `-s` | `--signoff` | Add a deduplicated `Signed-off-by:` trailer to replayed commits. |
| `-S` | `--gpg-sign` / `--no-gpg-sign` | Vault-sign replayed commits. Libra does not call external GnuPG or support keyid selection. |
| | `--continue` | Continue the rebase after resolving conflicts. Mutually exclusive with `--abort`, `--skip`, and `<upstream>`. |
| | `--abort` | Abort the current rebase and restore the original branch to its pre-rebase state. Mutually exclusive with `--continue`, `--skip`, and `<upstream>`. |
| | `--skip` | Skip the current commit and continue with the next commit in the rebase sequence. Mutually exclusive with `--continue`, `--abort`, and `<upstream>`. |

### Option Details

**`<upstream>`**

Start a new rebase, replaying current branch commits onto the specified upstream:

```bash
$ libra rebase main
Found common ancestor: abc1234
Rebasing 3 commits from `feature` onto `main`...
Applied: def5678 feat: add parser
Applied: 987abcd feat: add lexer
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

**`--onto <newbase>`**

Replay commits selected by `<upstream>` onto an independent new base. If `<branch>` is supplied, Libra first switches to that branch and then rebases it:

```bash
$ libra rebase --onto next main topic
Found common ancestor: abc1234
Rebasing 2 commits from `topic` onto `next`...
Applied: def5678 feat: topic change
Applied: 987abcd test: topic coverage
Successfully rebased branch 'topic' onto '7654321'.
```

**`--continue`**

After resolving conflicts and staging the resolved files, continue the rebase:

```bash
$ libra rebase --continue
Applied: 987abcd feat: add lexer
Rebasing 1 commits from `feature` onto `1234567`...
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

**`--abort`**

Abort the rebase and restore the original branch state:

```bash
$ libra rebase --abort
Rebase aborted. Restored branch 'feature'.
```

**`--skip`**

Skip the current conflicting commit and move to the next one:

```bash
$ libra rebase --skip
Skipped: 987abcd feat: add lexer
Rebasing 1 commits from `feature` onto `1234567`...
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

## Common Commands

```bash
# Rebase current branch onto main
libra rebase main

# Rebase onto a specific commit
libra rebase abc1234

# Replay topic's main..topic commits onto next
libra rebase --onto next main topic

# Replay all commits from the root onto main
libra rebase --root --onto main

# Rebase with dirty-worktree protection
libra rebase --autostash main

# Fold fixup!/squash! commits without an editor
libra rebase --autosquash main

# Keep commits that become empty after replay
libra rebase --empty=keep main

# Add trailers or vault signatures to rewritten commits
libra rebase --signoff main
libra rebase -S main

# Continue after resolving conflicts
libra rebase --continue

# Abort the rebase
libra rebase --abort

# Skip a problematic commit
libra rebase --skip

# Using the alias
libra rb main
```

## Human Output

Normal rebase progress:

```text
Found common ancestor: abc1234
Rebasing 3 commits from `feature` onto `main`...
Applied: def5678 feat: add parser
Applied: 987abcd feat: add lexer
Applied: 13579bd test: add parser tests
Successfully rebased branch 'feature' onto '1234567'.
```

Conflict during rebase:

```text
fatal: rebase stopped while applying 987abcd: feat: add lexer

Hint: conflicted files:
Hint:   src/lexer.rs
Hint: resolve conflicts, stage them, then run 'libra rebase --continue'.
Hint: or run 'libra rebase --skip' / 'libra rebase --abort'.
```

Already up to date:

```text
Current branch is ahead of upstream. No rebase needed.
```

Fast-forward-only case:

```text
Fast-forwarded branch 'feature' to 'main'.
```

Abort:

```text
Rebase aborted. Restored branch 'feature'.
```

## JSON / Machine Output

`--json` and `--machine` are currently supported for successful `rebase <upstream>`, `--abort`, `--continue`, and `--skip` output. CLI/preflight failures, unresolved-conflict `--continue` failures, and structured `rebase <upstream>` conflict stops are rendered through Libra's standard structured error envelope. Deeper replay/conflict-stop error taxonomy is still tracked as follow-up work in the command improvement plan.

Start and complete a replay:

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "start",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "upstream": "main",
    "onto": "fedcba9...",
    "common_ancestor": "0123456...",
    "replay_count": 1,
    "previous_commit": "def5678...",
    "applied_commits": [
      {
        "original_commit": "0123456...",
        "commit": "abc1234...",
        "subject": "Feature adds file"
      }
    ],
    "remaining": 0
  }
}
```

Fast-forward start results use the same envelope with `status: "fast-forwarded"`, `commit` equal to `onto`, and no `applied_commits`. Branches already ahead of upstream return `status: "already-up-to-date"`.

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "abort",
    "status": "aborted",
    "branch": "feature",
    "commit": "abc1234...",
    "previous_commit": "def5678...",
    "restored": true
  }
}
```

Continue after resolving a conflict:

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "continue",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "onto": "fedcba9...",
    "previous_commit": "def5678...",
    "applied_commits": [
      {
        "original_commit": "0123456...",
        "commit": "abc1234...",
        "subject": "Feature modifies conflict.txt"
      }
    ],
    "remaining": 0
  }
}
```

Skip the stopped commit:

```json
{
  "ok": true,
  "command": "rebase",
  "data": {
    "action": "skip",
    "status": "completed",
    "branch": "feature",
    "commit": "abc1234...",
    "onto": "fedcba9...",
    "previous_commit": "def5678...",
    "skipped_commit": "0123456...",
    "skipped_subject": "Feature modifies conflict.txt",
    "remaining": 0
  }
}
```

## Rebase State Persistence

Rebase state is stored in a `rebase_state` SQLite table with the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `head_name` | TEXT | Original branch name being rebased |
| `onto` | TEXT | Commit hash being rebased onto |
| `orig_head` | TEXT | Original HEAD commit before rebase started |
| `current_head` | TEXT | Current new base (HEAD of rebased commits so far) |
| `todo` | TEXT | Remaining commits to replay (newline-separated hashes) |
| `done` | TEXT | Commits already replayed (newline-separated hashes) |
| `stopped_sha` | TEXT (nullable) | Current commit that caused a conflict |
| `autostash_ref` | TEXT (nullable) | Saved autostash commit OID for exact restore/drop |
| `autosquash` | INTEGER | Whether autosquash was enabled for this rebase |
| `reapply_cherry_picks` | INTEGER | Whether redundant cherry-picks are reapplied |
| `keep_empty` | INTEGER | Whether originally empty commits are preserved |
| `empty_mode` | TEXT | `drop`, `keep`, or `stop` for commits that become empty |
| `signoff` | INTEGER | Whether signoff trailers are added during replay |
| `gpg_sign` | INTEGER | Whether replayed commits are vault-signed |

## Design Rationale

### Why no `--interactive` / `-i`?

Git's interactive rebase opens an editor with a list of commits that can be reordered, squashed, edited, or dropped. This is one of Git's most powerful features but is inherently interactive: it requires an editor session and human decision-making at launch time.

Libra targets AI-agent and automation workflows where interactive editor sessions are not feasible. Instead of interactive rebase, Libra encourages breaking complex history rewriting into discrete operations: use `rebase` for linear replay, and (in the future) dedicated commands for squashing or reordering.

### `--onto`

Libra supports Git's non-interactive `--onto <newbase> <upstream> [<branch>]` form. The upstream still defines the replay range, while `<newbase>` is the target parent for the replayed commits. Interactive todo editing remains intentionally unsupported.

### Autosquash And Signing

Libra supports a non-interactive autosquash path. It recognizes `fixup!`, `squash!`, and `amend!` subjects and folds them during replay without opening an editor. Signing is intentionally vault-backed: `-S`/`--gpg-sign` uses Libra's vault signing key and does not accept Git keyid forms such as `-S<keyid>`.

### Why persist state in SQLite?

Git persists rebase state in a `.git/rebase-merge/` directory with one file per field (head-name, onto, orig-head, etc.). This approach is fragile: partial writes can corrupt state, and concurrent access has no protection.

Libra uses SQLite for rebase state persistence, which provides:
- **Atomic writes**: State updates are transactional, preventing partial corruption.
- **Consistent reads**: No torn reads from partially-written files.
- **Schema evolution**: New fields can be added with migrations rather than new files.
- **Single source of truth**: All metadata lives in one database, simplifying backup and restore.

### How does this compare to Git and jj?

Git's rebase is feature-rich with interactive mode, autosquash, `--onto`, `--exec`, `--rebase-merges`, and more. It is one of the most complex commands in Git, with numerous edge cases around conflict resolution and state management.

jj takes a fundamentally different approach: history is immutable by default, and there is no rebase command. Instead, `jj rebase` exists but operates on the revision DAG directly, moving revisions and their descendants to a new parent. Conflicts are recorded in the commit itself rather than stopping the process, so there is no `--continue`/`--abort` flow.

Libra provides a middle ground: a linear rebase with conflict-stop semantics (familiar to Git users) but with SQLite-backed state persistence for reliability.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Upstream | `<upstream>` (positional) | `<upstream>` (positional) | `-d` / `--destination` |
| Continue | `--continue` | `--continue` | N/A (conflicts stored in commit) |
| Abort | `--abort` | `--abort` | `jj op undo` |
| Skip | `--skip` | `--skip` | N/A |
| Interactive | Not supported | `-i` / `--interactive` | N/A |
| Onto | Supported | `--onto <newbase>` | `-d` with `-s` / `--source` |
| Exec | Not supported | `--exec <cmd>` | N/A |
| Autosquash | Partial: non-interactive folding | `--autosquash` | N/A |
| Rebase merges | Not supported | `--rebase-merges` | Default behavior |
| Autostash | Supported | `--autostash` / `--no-autostash` | N/A |
| Reapply cherry-picks | Supported | `--reapply-cherry-picks` | N/A |
| Root | Partial: single-root histories | `--root` | N/A |
| Keep empty | Supported | `--keep-empty` / `--no-keep-empty` | Default keeps empty |
| Empty after replay | Supported: `drop`/`keep`/`stop` | `--empty=` | N/A |
| Signoff | Supported | `--signoff` | N/A |
| GPG sign | Vault-backed, no keyid | `-S` / `--gpg-sign` | N/A |
| Force rebase | Not supported | `--force-rebase` | N/A |
| Branch | Supported | `<branch>` (third positional) | `-s` / `--source` |
| Revision set | Not supported | N/A | `-r` / `--revisions` |
| State persistence | SQLite database | `.git/rebase-merge/` directory | Not applicable |

Note: jj does not stop on conflicts during rebase. Instead, conflicts are materialized in the commit content and can be resolved later, which eliminates the need for `--continue`/`--abort`/`--skip`.

## Error Handling

`execute_safe` currently returns standard structured `CliError` envelopes for CLI/preflight failures. The deeper replay engine is still a legacy text path and is tracked as pending structured-output work.

| Scenario | StableErrorCode | Exit | Behavior |
|----------|-----------------|------|----------|
| Not a libra repository | `LBR-REPO-001` (RepoNotFound) | 128 | Error with repo-not-found message |
| Missing upstream | `LBR-CLI-002` (CliInvalidArgument) | 129 | Usage error from clap |
| Upstream ref cannot be resolved | `LBR-CLI-003` (CliInvalidTarget) | 129 | Error indicating the ref is not valid |
| `--continue` without rebase in progress | `LBR-REPO-003` (RepoStateInvalid) | 128 | Error indicating no rebase in progress |
| `--continue` with unresolved conflicts | `LBR-CONFLICT-001` (ConflictUnresolved) | 128 | Error indicating conflicts must be staged with `libra add <file>` |
| `--abort` without rebase in progress | `LBR-REPO-003` (RepoStateInvalid) | 128 | Error indicating no rebase in progress |
| `--skip` without rebase in progress | `LBR-REPO-003` (RepoStateInvalid) | 128 | Error indicating no rebase in progress |
| `--skip` without stopped or pending commit | `LBR-REPO-003` (RepoStateInvalid) | 128 | Error indicating there is no commit to skip |
| No common ancestor found | `LBR-CLI-003` (CliInvalidTarget) | 129 | Error refusing to rebase unrelated histories |
| Criss-cross merge bases | `LBR-CONFLICT-002` (ConflictOperationBlocked) | 128 | Error refusing to choose one of multiple best merge bases |
| Conflict during commit replay | pending typed mapping | 128 | Rebase stops, state is saved, user prompted to resolve |
| Failed to create rebased commit | pending typed mapping | 128 | Legacy text error with commit details |
| Failed to update branch reference | pending typed mapping | 128 | Legacy text error with ref update details |
