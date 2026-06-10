# `libra revert`

Revert one or more existing commits by creating new inverse commits.

## Synopsis

```bash
libra revert [options] <commit>...
libra revert --continue
libra revert --skip
libra revert --abort
libra revert --quit
```

## Description

`libra revert` applies the inverse of existing commits without rewriting history. Clean reverts create new commits on top of the current `HEAD`; `-n` / `--no-commit` applies a single inverse change to the index and working tree without committing it.

The positional arguments may be individual commit references or double-dot ranges such as `HEAD~3..HEAD`. Ranges are reverted newest-first, matching Git's revert order. Duplicate commits are ignored after their first occurrence.

If a revert conflicts, Libra writes conflict markers, records non-zero index stages, and persists the in-progress operation in `.libra/libra.db` as `revert_sequence`. Resolve the file, run `libra add <path>`, then continue with `libra revert --continue`. Use `--abort`, `--skip`, or `--quit` to control the sequence.

Detached `HEAD` is supported. In detached mode, the generated revert commit advances the detached `HEAD` directly instead of updating a branch.

## Options

### `<commit>...`

One or more commits or `A..B` ranges to revert.

```bash
libra revert HEAD
libra revert abc1234 def5678
libra revert HEAD~3..HEAD
```

### `-n`, `--no-commit`

Apply one commit's inverse change to the index and working tree without creating a commit.

`--no-commit` is intentionally limited to a single commit. Combining it with multiple commits or a range fails with exit 128.

### `-m`, `--mainline <parent-number>`

Select the 1-based parent to use as the mainline when reverting a merge commit.

```bash
libra revert -m 1 <merge-commit>
```

A merge commit requires `-m`. Passing `-m` for a non-merge commit, or selecting a parent outside the merge's parent count, fails with exit 128.

### `-s`, `--signoff`

Append a `Signed-off-by: <name> <email>` trailer to generated revert commits.

### `-e`, `--edit`; `--no-edit`

Accepted for Git-compatible command shape. Libra currently uses the generated message directly; editor integration is deferred.

### `--continue`

Continue an in-progress revert after conflicts have been resolved and staged.

### `--skip`

Drop the currently conflicted commit from the sequence, reset the worktree to the step start, and continue with the remaining commits.

### `--abort`

Cancel the in-progress sequence and reset tracked files plus the index back to the original `HEAD` recorded when the sequence started. Untracked files are preserved.

### `--quit`

Clear the persisted revert sequence while leaving the current working tree and index untouched.

### `--json`

Emit the normal Libra JSON envelope.

## Common Commands

```bash
# Revert the latest commit
libra revert HEAD

# Revert a range newest-first
libra revert HEAD~3..HEAD

# Revert a merge commit relative to the first parent
libra revert -m 1 <merge-commit>

# Resolve a conflict, stage it, and continue
libra add conflicted.txt
libra revert --continue

# Cancel an in-progress revert sequence
libra revert --abort
```

## Human Output

Clean auto-commit:

```text
[def5678] Revert commit abc1234
```

No-commit mode:

```text
Changes staged for revert. Use 'libra commit' to finalize.
```

Sequence controls:

```text
revert sequence continued
revert skipped current commit
revert aborted; HEAD reset to abc1234
revert state cleared; working tree left unchanged
```

## Structured Output

Single clean revert:

```json
{
  "command": "revert",
  "data": {
    "reverted_commit": "abc1234abcdef1234567890abcdef1234567890ab",
    "short_reverted": "abc1234",
    "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
    "short_new": "def5678",
    "no_commit": false,
    "files_changed": 1,
    "reverted_commits": [
      "abc1234abcdef1234567890abcdef1234567890ab"
    ],
    "new_commits": [
      "def5678abcdef1234567890abcdef1234567890ab"
    ]
  }
}
```

`--no-commit` keeps `new_commit` and `short_new` as `null`.

Sequence control output adds `action` and, for `--abort`, `restored_head`.

## Compatibility

`libra revert` is partial Git compatibility. Supported: multiple commits, `A..B` ranges, detached `HEAD`, `-n` for a single commit, merge revert with `-m`, `--signoff`, conflict sequencer controls, JSON output, and quiet output.

Deferred: strategy selection (`--strategy`, `-X`), external GPG signing (`-S` / `--gpg-sign`), `--cleanup`, `--commit`, `--rerere-autoupdate`, `--reference`, editor launch for `--edit`, and Git's full set of `--no-*` aliases.

## Error Handling

| Code | Condition | Hint |
|------|-----------|------|
| `LBR-REPO-001` | Not inside a Libra repository | Initialize with `libra init` or move into a repo |
| `LBR-REPO-003` | Revert state conflict, no sequence, or sequence already active | Finish with `--continue`, `--skip`, `--abort`, or `--quit` |
| `LBR-CLI-003` | Cannot resolve a commit reference | Use `libra log` to find valid commits |
| `LBR-CLI-002` | Invalid arguments, invalid mainline, or `--no-commit` with multiple commits | Adjust the arguments |
| `LBR-CONFLICT-001` | A revert conflict is present | Resolve conflicts, run `libra add`, then `libra revert --continue` |
| `LBR-IO-001` | Failed to load an object or sequence | Check repository integrity |
| `LBR-IO-002` | Failed to save objects, index, sequence, or update `HEAD` | Check filesystem and database writability |
