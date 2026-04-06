# `libra reset`

Move `HEAD` and reset the index or working tree depending on the selected mode.

## Synopsis

```
libra reset [<target>] [--soft | --mixed | --hard]
libra reset [<target>] [--] <pathspec>...
```

## Description

`libra reset` moves the HEAD reference to a target commit and optionally resets the index and working tree to match. The three modes control how much state is affected:

- **`--soft`**: moves HEAD only. The index and working tree are untouched, so all differences between the old HEAD and the target appear as staged changes. Useful for squashing commits.
- **`--mixed`** (default): moves HEAD and resets the index. The working tree is untouched, so changes appear as unstaged modifications. Useful for un-staging files.
- **`--hard`**: moves HEAD, resets the index, and restores the working tree. All uncommitted changes are discarded. Useful for fully reverting to a known state.

When pathspecs are provided, the command performs a targeted mixed reset: only the named files are reset in the index to match the target commit, without moving HEAD. This is the primary way to un-stage specific files. Pathspecs are incompatible with `--soft` and `--hard`.

The default target is `HEAD`, making `libra reset` (with no arguments) equivalent to un-staging everything.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<target>` | positional (default: `HEAD`) | Commit, branch, or revision expression to reset to |
| | `--soft` | | Move HEAD only; keep index and working tree |
| | `--mixed` | | Move HEAD and reset index; keep working tree (default) |
| | `--hard` | | Move HEAD, reset index, and restore working tree |
| | `<pathspec>...` | positional (after `--`) | Specific files to reset in the index |

### Flag examples

```bash
# Un-stage everything (mixed reset to HEAD)
libra reset

# Move HEAD back one commit, keep changes staged
libra reset --soft HEAD~1

# Move HEAD back two commits, un-stage changes
libra reset HEAD~2

# Fully revert to a branch tip, discard all changes
libra reset --hard main

# Un-stage a specific file
libra reset HEAD -- src/lib.rs

# Un-stage multiple files
libra reset HEAD -- src/main.rs src/cli.rs

# Reset specific files to a prior commit
libra reset abc1234 -- path/to/file.rs

# JSON output for agents
libra reset --json --hard HEAD~1
```

## Common Commands

```bash
libra reset HEAD~1                    # Move HEAD and reset index to the previous commit
libra reset --soft HEAD~2             # Move HEAD only, keep index and worktree
libra reset --hard main               # Reset HEAD, index, and worktree to branch 'main'
libra reset HEAD -- src/lib.rs        # Unstage a path back to HEAD
libra reset --json --hard HEAD~1      # Structured JSON output for agents
```

## Human Output

Full reset (no pathspecs):

```text
HEAD is now at abc1234 Initial commit
```

Pathspec reset (un-stage specific files):

```text
Unstaged changes after reset:
M	path/to/file
```

## Structured Output (JSON examples)

Full reset:

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "hard",
    "commit": "abc123def456789012345678901234567890abcd",
    "short_commit": "abc123d",
    "subject": "Initial commit",
    "previous_commit": "def456abc789012345678901234567890abcd1234",
    "files_unstaged": 0,
    "files_restored": 1,
    "pathspecs": []
  }
}
```

Pathspec reset:

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "mixed",
    "commit": "abc123def456789012345678901234567890abcd",
    "short_commit": "abc123d",
    "subject": "Initial commit",
    "previous_commit": null,
    "files_unstaged": 2,
    "files_restored": 0,
    "pathspecs": ["src/lib.rs", "src/cli.rs"]
  }
}
```

### Schema Notes

- When `pathspecs` is non-empty, the command performs a mixed reset on the specified paths only, without moving HEAD.
- `previous_commit` is `null` for pathspec-only resets (HEAD does not move).
- `files_restored` counts tracked files rewritten or removed by `--hard`; on a clean repository, `reset --hard HEAD` can report `0`.
- `files_unstaged` counts files whose index entries were reset during mixed/pathspec resets.
- `subject` is the first line of the target commit message.

## Design Rationale

### Why reject pathspecs with --soft/--hard?

- **`--soft` + pathspecs**: `--soft` by definition only moves HEAD and touches nothing else. Resetting individual file index entries contradicts the "HEAD only" contract. If you want to un-stage specific files, use the default mixed mode: `libra reset HEAD -- file`.
- **`--hard` + pathspecs**: `--hard` restores the entire working tree to match the target commit. Selectively restoring only some files while leaving others in a different state would create a confusing hybrid that is neither "fully reset" nor "index-only reset." For selective file restoration, use `libra restore --source <commit> -- file`.

This restriction makes the three modes unambiguous: soft touches HEAD, mixed touches HEAD + index, hard touches HEAD + index + worktree. Pathspecs operate orthogonally at the index level only.

### Why default to mixed?

Mixed mode is the safest general-purpose reset: it un-stages changes without discarding work. A developer who runs `libra reset HEAD~1` without thinking about modes gets their changes preserved in the working tree as unstaged modifications. This matches Git's default and is the least surprising behavior for the most common use case (un-staging files or amending a commit).

### Why no --merge/--keep?

Git's `--merge` and `--keep` modes attempt to preserve uncommitted changes during a reset by performing a three-way merge between the old HEAD, the new HEAD, and the working tree. These modes are:

- **Rarely used**: most developers use `--soft`, `--mixed`, or `--hard` exclusively. The merge/keep modes add complexity for a niche use case.
- **Difficult to reason about**: the three-way merge during reset can produce conflicts, leaving the repository in a state that is neither "reset" nor "unchanged." This is confusing for both humans and AI agents.
- **Replaceable by explicit workflows**: the same result is achievable with `libra stash && libra reset --hard <target> && libra stash pop`, which makes each step visible and debuggable.

Libra favors explicit, composable commands over implicit multi-step operations hidden behind a single flag.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Mixed reset (default) | `git reset <target>` | `libra reset <target>` | N/A (jj has no staging area) |
| Soft reset | `git reset --soft <target>` | `libra reset --soft <target>` | N/A |
| Hard reset | `git reset --hard <target>` | `libra reset --hard <target>` | `jj restore --from <rev>` |
| Un-stage files | `git reset HEAD -- <file>` | `libra reset HEAD -- <file>` | N/A (no staging area) |
| Merge reset | `git reset --merge <target>` | Not supported | N/A |
| Keep reset | `git reset --keep <target>` | Not supported | N/A |
| Default target | HEAD | HEAD | N/A |
| Structured output | No | `--json` / `--machine` | `--template` |
| Pathspec + soft | Allowed (un-stages) | Rejected (`LBR-CLI-002`) | N/A |
| Pathspec + hard | Rejected | Rejected (`LBR-CLI-002`) | N/A |
| Rollback on failure | No | Attempts index rollback | N/A (operation log undo) |

## Error Handling

| Scenario | Error Code | Hint |
|----------|-----------|------|
| Not a libra repository | `LBR-REPO-001` | "run 'libra init' to create a repository in the current directory." |
| Invalid revision | `LBR-CLI-003` | "check the revision name and try again." |
| HEAD is unborn | `LBR-REPO-003` | "create a commit first before resetting HEAD." |
| Failed to resolve HEAD | `LBR-IO-001` | "check whether the repository database is readable." |
| HEAD reference corrupt | `LBR-REPO-002` | "the HEAD reference or branch metadata may be corrupted." |
| Object load failure | `LBR-REPO-002` | "the object store may be corrupted." |
| Index load failure | `LBR-REPO-002` | "the index file may be corrupted." |
| Index save failure | `LBR-IO-002` | -- |
| HEAD update failure | `LBR-IO-002` | -- |
| Working tree read failure | `LBR-IO-001` | -- |
| Working tree restore failure | `LBR-IO-002` | -- |
| Invalid path encoding | `LBR-CLI-002` | "rename the path or invoke libra from a path representable as UTF-8." |
| `--soft` with pathspecs | `LBR-CLI-002` | "--soft only moves HEAD; use --mixed to reset index for specific paths." |
| `--hard` with pathspecs | `LBR-CLI-002` | "--hard updates the working tree; omit pathspecs or use --mixed for specific paths." |
| Pathspec not matched | `LBR-CLI-003` | "check the path and try again." |
| Rollback failure | (primary code) | (primary hint) |
