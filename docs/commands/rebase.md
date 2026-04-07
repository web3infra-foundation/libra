# `libra rebase`

Reapply commits on top of another base tip.

**Alias:** `rb`

## Synopsis

```
libra rebase <upstream>
libra rebase --continue
libra rebase --abort
libra rebase --skip
```

## Description

`libra rebase` moves a sequence of commits from the current branch onto a new base commit. It finds the common ancestor between the current branch and the specified upstream, collects all commits from that ancestor to the current HEAD, and replays each commit on top of the upstream branch. After all commits are replayed, the current branch reference is updated to point to the final rebased commit.

If a conflict occurs during replay, the rebase stops and reports the conflicting files. The user resolves conflicts manually, stages the resolved files, and then runs `libra rebase --continue` to proceed. Alternatively, `--abort` restores the original branch state and `--skip` discards the current commit and moves on to the next.

Rebase state (the list of remaining and completed commits, the original HEAD, and the target base) is persisted in the SQLite database. This makes rebase state survive process restarts and avoids the fragile file-based state that Git uses. Legacy file-based state from older Libra versions is automatically migrated to the database on first access.

## Options

| Option | Long | Description |
|--------|------|-------------|
| `<upstream>` | | The upstream branch or commit to rebase onto. Required unless `--continue`, `--abort`, or `--skip` is specified. Can be a branch name, commit hash, or any Git reference. |
| | `--continue` | Continue the rebase after resolving conflicts. Mutually exclusive with `--abort`, `--skip`, and `<upstream>`. |
| | `--abort` | Abort the current rebase and restore the original branch to its pre-rebase state. Mutually exclusive with `--continue`, `--skip`, and `<upstream>`. |
| | `--skip` | Skip the current commit and continue with the next commit in the rebase sequence. Mutually exclusive with `--continue`, `--abort`, and `<upstream>`. |

### Option Details

**`<upstream>`**

Start a new rebase, replaying current branch commits onto the specified upstream:

```bash
$ libra rebase main
Rebasing (1/3): feat: add parser
Rebasing (2/3): feat: add lexer
Rebasing (3/3): test: add parser tests
Successfully rebased and updated refs/heads/feature.
```

**`--continue`**

After resolving conflicts and staging the resolved files, continue the rebase:

```bash
$ libra rebase --continue
Rebasing (2/3): feat: add lexer
Rebasing (3/3): test: add parser tests
Successfully rebased and updated refs/heads/feature.
```

**`--abort`**

Abort the rebase and restore the original branch state:

```bash
$ libra rebase --abort
```

**`--skip`**

Skip the current conflicting commit and move to the next one:

```bash
$ libra rebase --skip
Rebasing (3/3): test: add parser tests
Successfully rebased and updated refs/heads/feature.
```

## Common Commands

```bash
# Rebase current branch onto main
libra rebase main

# Rebase onto a specific commit
libra rebase abc1234

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
Rebasing (1/3): feat: add parser
Rebasing (2/3): feat: add lexer
Rebasing (3/3): test: add parser tests
Successfully rebased and updated refs/heads/feature.
```

Conflict during rebase:

```text
Rebasing (2/3): feat: add lexer
CONFLICT: merge conflict in src/lexer.rs
After resolving conflicts, run "libra rebase --continue".
To abort, run "libra rebase --abort".
To skip this commit, run "libra rebase --skip".
```

Already up to date:

```text
Current branch is up to date.
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

## Design Rationale

### Why no `--interactive` / `-i`?

Git's interactive rebase opens an editor with a list of commits that can be reordered, squashed, edited, or dropped. This is one of Git's most powerful features but is inherently interactive: it requires an editor session and human decision-making at launch time.

Libra targets AI-agent and automation workflows where interactive editor sessions are not feasible. Instead of interactive rebase, Libra encourages breaking complex history rewriting into discrete operations: use `rebase` for linear replay, and (in the future) dedicated commands for squashing or reordering.

### Why no `--onto`?

Git's `--onto` flag allows rebasing a subset of commits onto an arbitrary base, independent of the upstream reference. This is a powerful but rarely used feature that creates confusion about the three-argument form (`git rebase --onto <newbase> <upstream> [<branch>]`).

Libra simplifies by always rebasing all commits from the common ancestor to HEAD onto the specified upstream. This covers the vast majority of rebase use cases. The `--onto` flag may be added in the future if there is demand for more precise commit range selection.

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
| Onto | Not supported | `--onto <newbase>` | `-d` with `-s` / `--source` |
| Exec | Not supported | `--exec <cmd>` | N/A |
| Autosquash | Not supported | `--autosquash` | N/A |
| Rebase merges | Not supported | `--rebase-merges` | Default behavior |
| Keep empty | Not supported | `--keep-empty` / `--no-keep-empty` | Default keeps empty |
| Force rebase | Not supported | `--force-rebase` | N/A |
| Branch | Not supported | `<branch>` (third positional) | `-s` / `--source` |
| Revision set | Not supported | N/A | `-r` / `--revisions` |
| State persistence | SQLite database | `.git/rebase-merge/` directory | Not applicable |

Note: jj does not stop on conflicts during rebase. Instead, conflicts are materialized in the commit content and can be resolved later, which eliminates the need for `--continue`/`--abort`/`--skip`.

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Not a libra repository | Error with repo-not-found message |
| Upstream ref cannot be resolved | Error indicating the ref is not valid |
| No common ancestor found | Error refusing to rebase unrelated histories |
| Conflict during commit replay | Rebase stops, state is saved, user prompted to resolve |
| `--continue` without rebase in progress | Error indicating no rebase in progress |
| `--abort` without rebase in progress | Error indicating no rebase in progress |
| Failed to create rebased commit | Error with commit details |
| Failed to update branch reference | Error with ref update details |
