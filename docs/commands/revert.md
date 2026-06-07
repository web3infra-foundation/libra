# `libra revert`

Revert some existing commits.

## Synopsis

```
libra revert [-n | --no-commit] [-m | --mainline <parent-number>] [--json] [--quiet] <commit>
```

## Description

`libra revert` creates a new commit that undoes the changes introduced by the specified commit. Unlike `reset`, which rewrites history, `revert` is safe for shared branches because it preserves the original commit and adds a new one on top.

The command works by computing the diff between the target commit and its parent, then applying the inverse of that diff to the current working tree and index. If the resulting state is clean, a new commit is recorded with a message of the form `Revert "<original subject>"`.

Reverting a root commit (one with no parent) produces an empty tree, effectively undoing the initial commit's changes.

To revert a **merge commit** (one with more than one parent), pass `-m <parent-number>` to choose which parent is the mainline; the merge's changes are then computed relative to that parent. See [`-m`, `--mainline`](#-m---mainline-parent-number) below.

The command requires an active branch (not detached HEAD) and accepts exactly one commit reference.

## Options

### `-n`, `--no-commit`

Apply the inverse changes to the index and working tree but do **not** create a new commit. This is useful when you want to inspect the result, combine multiple reverts, or adjust the changes before committing.

```bash
# Stage the revert without committing
libra revert -n abc1234

# Review what changed
libra diff --cached

# Commit with a custom message
libra commit -m "revert abc1234 with adjustments"
```

### `-m`, `--mainline <parent-number>`

Specify the 1-based parent number to treat as the mainline when reverting a **merge commit**. The merge's changes are computed relative to that parent's tree (so `-m 1` undoes everything the merge brought in relative to the first parent), and the generated revert commit still records a single parent (the current `HEAD`).

```bash
# Revert a merge commit, keeping the first-parent line as the baseline
libra revert -m 1 <merge-commit>
```

- A merge commit **requires** `-m`; omitting it fails with exit 128 (`commit <hash> is a merge but no -m option was given`).
- Passing `-m` for a non-merge commit fails with exit 128 (`mainline was specified but commit <hash> is not a merge`).
- An out-of-range parent number fails with exit 128 (`commit <hash> does not have a parent number <n>`).

### `<commit>` (positional, required)

A single commit reference to revert. Can be a full SHA-1 hash, an abbreviated hash, a branch name, `HEAD`, or any ref that resolves to a commit.

```bash
# Revert the most recent commit
libra revert HEAD

# Revert by hash
libra revert abc1234

# Revert the commit a branch points to
libra revert feature-branch
```

### `--json`

Emit machine-readable JSON output instead of human-readable text. See [Structured Output](#structured-output-json-examples) below.

### `--quiet`

Suppress all human-readable output. Exit code still indicates success or failure.

## Common Commands

```bash
# Revert the most recent commit
libra revert HEAD

# Revert a specific commit by hash
libra revert abc1234

# Revert without auto-committing (to edit or combine)
libra revert -n HEAD

# Revert a merge commit relative to its first parent
libra revert -m 1 <merge-commit>

# Revert with JSON output for AI agents or scripts
libra revert --json HEAD
```

## Human Output

When reverting **with** auto-commit (default):

```
[def5678] Revert commit abc1234
```

When reverting **without** auto-commit (`-n`):

```
Changes staged for revert. Use 'libra commit' to finalize.
```

## Structured Output (JSON examples)

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
    "short_new": "def5678",
    "no_commit": false,
    "files_changed": 3
  }
}
```

When `--no-commit` is used, `new_commit` and `short_new` are `null`:

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": null,
    "short_new": null,
    "no_commit": true,
    "files_changed": 3
  }
}
```

## Design Rationale (Why different from Git/jj)

### Why single commit only (no `<commit>...`)?

Git allows `git revert <commit1> <commit2> ...` to revert a sequence of commits. Libra restricts `revert` to a single commit because:

1. **Atomic operations.** Each revert is self-contained: it either succeeds or fails without leaving partial state behind. Multi-commit revert in Git requires sequencer state (`REVERT_HEAD`, `sequencer/`) that can become stale or corrupt if the user abandons the operation.
2. **Explicit is better.** In a trunk-based monorepo workflow, reverting multiple commits is a significant action that deserves deliberate, per-commit attention. Running `libra revert A && libra revert B` makes the intent clear in the reflog and is trivially scriptable.
3. **Agent simplicity.** AI agents can loop over commits and handle each revert result independently, which is simpler than managing sequencer state transitions.

### Merge commit support (`--mainline`)

Git's `--mainline <parent-number>` selects which parent of a merge commit to diff against when computing the inverse. Libra supports this: a merge commit (more than one parent) **requires** `-m <n>`, and the revert is computed relative to the chosen parent's tree. To guard against the "picked the wrong parent" footgun:

1. **No silent default.** Omitting `-m` on a merge commit is a hard error (exit 128), so you must consciously choose the mainline rather than getting an arbitrary changeset.
2. **Symmetric guards.** Passing `-m` on a non-merge commit, or a parent number outside the merge's parent count, is also a hard error — the parent ordering must be explicit and valid.

The generated revert commit still records a single parent (the current `HEAD`), matching Git.

### Why no `--continue`, `--abort`?

Like cherry-pick, Libra's revert is stateless:

1. **No hidden state files.** Git's `REVERT_HEAD` and `sequencer/` directory are implicit state that can confuse users and agents. Libra avoids this entirely.
2. **Conflict resolution is explicit.** When a conflict is detected (a file was modified by a later commit), Libra reports the specific path and error code (`LBR-CONFLICT-001`). The user resolves the conflict, then runs `libra commit`. This is functionally equivalent to `git revert --continue` but without hidden state.
3. **Predictable for automation.** Agents detect the error code, resolve the conflict programmatically, and commit -- no state machine to manage.

### Why conflict detection instead of three-way merge?

Libra's revert uses a simpler conflict model than Git's three-way merge: if the file at the target path has been modified since the commit being reverted, Libra raises a conflict rather than attempting automatic resolution. This is intentionally conservative because:

1. **Safety over convenience.** Automatic merge can silently produce incorrect results when the semantic context of a change has shifted. Failing loudly ensures the user reviews the interaction.
2. **Deterministic behavior.** The same inputs always produce the same output -- either a clean revert or a conflict error, never a "successful" merge that introduced a subtle bug.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Git | jj | Libra |
|-----------|-----|-----|-------|
| Positional commit(s) | `git revert <commit>...` | N/A (uses `jj backout`) | `libra revert <commit>` (single) |
| No-commit mode | `--no-commit` / `-n` | N/A | `--no-commit` / `-n` |
| Edit message | `--edit` / `--no-edit` | N/A | Not supported (use `-n` then `commit -m`) |
| Mainline parent | `--mainline <n>` / `-m <n>` | N/A | `--mainline <n>` / `-m <n>` (required for merge commits) |
| Continue after conflict | `--continue` | N/A | Not supported (resolve then `commit`) |
| Abort in-progress | `--abort` | N/A | Not supported (no sequencer state) |
| Skip current commit | `--skip` | N/A | Not supported |
| Strategy | `--strategy <s>` | N/A | Not supported |
| Strategy option | `-X <option>` | N/A | Not supported |
| GPG sign | `--gpg-sign` / `-S` | N/A | Not supported (planned) |
| JSON output | N/A | N/A | `--json` |
| Quiet mode | `--quiet` | N/A | `--quiet` |
| Files changed count | N/A | N/A | Included in JSON output |

**Note:** jj uses `jj backout -r <rev>` as its equivalent to `git revert`. It creates a new commit that is the inverse of the target revision.

## Error Handling

| Code | Condition | Hint |
|------|-----------|------|
| `LBR-REPO-001` | Not inside a libra repository | Initialize with `libra init` or navigate to a repo |
| `LBR-REPO-003` | HEAD is detached (not on a branch) | Switch to a branch with `libra switch <branch>` |
| `LBR-CLI-003` | Cannot resolve the commit reference | Use `libra log` to find valid commit references |
| `LBR-CLI-002` | Merge commit without `-m`, `-m` on a non-merge commit, or out-of-range parent number (exit 128) | Pass a valid `-m <parent-number>` for merge commits; omit `-m` for non-merge commits |
| `LBR-CONFLICT-001` | File was modified by a later commit, creating a conflict | Resolve conflicts manually, then use `libra commit` |
| `LBR-IO-001` | Failed to load object (commit, tree, blob) | Check repository integrity |
| `LBR-IO-002` | Failed to save object, index, or update HEAD | Check filesystem permissions and repository writability |
