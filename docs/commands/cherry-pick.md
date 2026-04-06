# `libra cherry-pick`

Apply the changes introduced by some existing commits.

**Alias:** `cp`

## Synopsis

```
libra cherry-pick [-n | --no-commit] [--json] [--quiet] <commit>...
```

## Description

`libra cherry-pick` applies the changes introduced by the specified commits onto the current branch. For each named commit, Libra computes the diff between that commit and its parent, applies the resulting changeset to the current index and working tree, and (unless `--no-commit` is given) records a new commit whose message references the original.

This is useful for selectively applying commits from one branch to another without merging. When multiple commits are supplied they are applied in the order given, each one becoming a new commit on the current branch before the next is processed.

The command requires an active branch (not detached HEAD) and refuses merge commits entirely.

## Options

### `-n`, `--no-commit`

Apply the changes from the source commit to the index and working tree but do **not** create a new commit. This lets you inspect or combine the changes before committing manually with `libra commit`.

**Restriction:** Only a single commit may be specified when `--no-commit` is used. Attempting to pass multiple commits with this flag produces error `LBR-CLI-002`.

```bash
# Stage changes from abc1234 without committing
libra cherry-pick -n abc1234

# Inspect staged changes, then commit manually
libra status
libra commit -m "cherry-picked and adjusted abc1234"
```

### `<commit>...` (positional, required)

One or more commit references to cherry-pick. Each value can be a full SHA-1 hash, an abbreviated hash, a branch name, `HEAD`, or any ref that resolves to a commit. Commits are applied left-to-right.

```bash
# Single commit by hash
libra cherry-pick abc1234

# Multiple commits in order
libra cherry-pick abc1234 def5678 ghi9012
```

### `--json`

Emit machine-readable JSON output instead of human-readable text. See [Structured Output](#structured-output-json-examples) below.

### `--quiet`

Suppress all human-readable output. Exit code still indicates success or failure.

## Common Commands

```bash
# Cherry-pick a single commit onto the current branch
libra cherry-pick abc1234

# Cherry-pick multiple commits in sequence
libra cherry-pick abc1234 def5678

# Cherry-pick without committing, to edit or combine changes
libra cherry-pick -n abc1234

# Cherry-pick with JSON output for AI agents or scripts
libra cherry-pick --json abc1234
```

## Human Output

When cherry-picking **with** auto-commit (default):

```
[def5678] cherry-picked from abc1234
```

When cherry-picking **without** auto-commit (`-n`):

```
Changes from abc1234 staged. Use 'libra commit' to finalize.
```

## Structured Output (JSON examples)

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234abcdef1234567890abcdef1234567890ab",
        "short_source": "abc1234",
        "new_commit": "def5678abcdef1234567890abcdef1234567890ab",
        "short_new": "def5678"
      }
    ],
    "no_commit": false
  }
}
```

When `--no-commit` is used, `new_commit` and `short_new` are `null`:

```json
{
  "command": "cherry-pick",
  "data": {
    "picked": [
      {
        "source_commit": "abc1234abcdef1234567890abcdef1234567890ab",
        "short_source": "abc1234",
        "new_commit": null,
        "short_new": null
      }
    ],
    "no_commit": true
  }
}
```

## Design Rationale (Why different from Git/jj)

### Why no `--edit` flag?

Git's `--edit` opens an editor so the user can modify the commit message before recording. Libra omits this for two reasons:

1. **Agent-first workflow.** Libra is designed for AI-agent-driven development where interactive editor prompts block automation pipelines. The default message format (`<original message>\n\n(cherry picked from commit <hash>)`) is deterministic and machine-parseable, which is exactly what agents need.
2. **Compose with `--no-commit`.** Users who want to customize the message can use `-n` to stage changes without committing, then run `libra commit -m "custom message"`. This two-step approach is explicit, scriptable, and avoids the complexity of spawning an editor subprocess.

### Why no `--mainline` for merge commits?

Git's `--mainline <parent-number>` allows cherry-picking merge commits by specifying which parent to diff against. Libra rejects merge commits outright because:

1. **Ambiguity is dangerous.** Choosing the wrong parent silently produces a completely different changeset. In a trunk-based monorepo workflow, merge commits are ephemeral integration points, not units of work. The meaningful changes live in the individual commits that were merged.
2. **Simplicity over edge cases.** Supporting `--mainline` adds significant complexity (parent selection, conflict resolution against an arbitrary base) for a use case that rarely arises in trunk-based development. Users can cherry-pick the individual non-merge commits instead.

### Why is `--no-commit` limited to a single commit?

When multiple commits are cherry-picked, each one builds on the result of the previous. Without intermediate commits, the index represents only the cumulative effect of all changes, losing the per-commit attribution. Allowing this would:

1. **Destroy provenance.** The `(cherry picked from commit ...)` trailer would be meaningless since the staged state is a blend of multiple source commits.
2. **Complicate recovery.** If a conflict arises on the third of five commits, there are no intermediate commits to roll back to. Git handles this with `--continue`/`--abort` state files, which Libra intentionally avoids (see below).

### Why no `--continue`, `--abort`, or `--skip`?

Git maintains `.git/CHERRY_PICK_HEAD` and sequencer state files to support multi-step conflict resolution. Libra omits this machinery because:

1. **Stateless by design.** Libra avoids hidden state files that can become stale or corrupt. Each cherry-pick invocation is atomic: it either succeeds completely or fails without partial state.
2. **Explicit conflict resolution.** When a conflict occurs, Libra stages whatever it can and tells the user to resolve conflicts manually, then run `libra commit`. This is the same end result as `git cherry-pick --continue` but without hidden sequencer state.
3. **Agent compatibility.** AI agents can detect the conflict error code (`LBR-CONFLICT-001`), resolve the conflict programmatically, and run `libra commit` -- a simpler protocol than managing `--continue`/`--abort`/`--skip` state transitions.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Git | jj | Libra |
|-----------|-----|-----|-------|
| Positional commits | `git cherry-pick <commit>...` | N/A (uses `jj rebase`) | `libra cherry-pick <commit>...` |
| No-commit mode | `--no-commit` / `-n` | N/A | `--no-commit` / `-n` |
| Edit message | `--edit` / `-e` | N/A | Not supported (use `-n` then `commit -m`) |
| Mainline parent | `--mainline <n>` / `-m <n>` | N/A | Not supported (merge commits rejected) |
| Continue after conflict | `--continue` | N/A | Not supported (resolve then `commit`) |
| Abort in-progress | `--abort` | N/A | Not supported (no sequencer state) |
| Skip current commit | `--skip` | N/A | Not supported |
| Strategy | `--strategy <s>` | N/A | Not supported (single merge strategy) |
| Strategy option | `-X <option>` | N/A | Not supported |
| GPG sign | `--gpg-sign` / `-S` | N/A | Not supported (planned) |
| Allow empty | `--allow-empty` | N/A | Not supported |
| Keep redundant | `--keep-redundant-commits` | N/A | Not supported |
| JSON output | N/A | N/A | `--json` |
| Quiet mode | `--quiet` | `--quiet` | `--quiet` |

**Note:** jj does not have a direct cherry-pick equivalent. The closest operation is `jj rebase -r <rev> -d <dest>`, which moves or copies a commit to a new destination.

## Error Handling

| Code | Condition | Hint |
|------|-----------|------|
| `LBR-REPO-001` | Not inside a libra repository | Initialize with `libra init` or navigate to a repo |
| `LBR-REPO-003` | HEAD is detached (not on a branch) | Switch to a branch with `libra switch <branch>` |
| `LBR-CLI-003` | Cannot resolve a commit reference | Use `libra log` to find valid commit references |
| `LBR-CLI-002` | Multiple commits with `--no-commit`, or merge commit encountered | Use single commit with `-n`; choose non-merge commits |
| `LBR-CONFLICT-001` | Conflict during cherry-pick (e.g. untracked file would be overwritten) | Resolve conflicts manually, then use `libra commit` |
| `LBR-IO-001` | Failed to load an object (commit, tree, index) | Check repository integrity and retry |
| `LBR-IO-002` | Failed to save object, index, or update branch ref | Check filesystem permissions and repository writability |
