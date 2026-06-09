# `libra switch`

Switch branches, create and switch to a new branch, or detach HEAD at a specific commit.

**Alias:** `sw`

## Synopsis

```
libra switch <branch>
libra switch -c <name> [<start-point>]
libra switch -C <name> [<start-point>]
libra switch -f <branch>
libra switch --orphan <name>
libra switch [--guess|--no-guess] <branch>
libra switch -d <commit|tag|branch>
libra switch --track <remote/branch>
```

## Description

`libra switch` is the primary command for changing branches. It validates that the working tree is clean before switching, updates HEAD and the index, and restores the working tree to match the target commit. Unlike `libra checkout`, which exists as a Git-compatibility surface, `switch` is the recommended command for branch operations.

The command supports switching to an existing local branch, creating or resetting a branch with `-c`/`-C`, detaching HEAD with `-d`, creating an unborn root branch with `--orphan`, and creating a local tracking branch with `--track` or the default remote-guessing path. When the target branch is already the current branch, the command is a no-op and skips the cleanliness check entirely.

Fuzzy branch name suggestions are provided via Levenshtein distance when a branch is not found, helping catch typos without requiring exact matches.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<branch>` | positional (optional) | Target branch, commit, or remote reference to switch to |
| `-c` | `--create` | `<name>` | Create a new branch and switch to it |
| `-C` | `--force-create` | `<name>` | Create a new branch, or reset it to the start point if it already exists, then switch to it |
| `-f` | `--force` | | Discard tracked working tree changes while switching |
| | `--discard-changes` | | Alias for `--force` |
| | `--orphan` | `<name>` | Create an unborn branch whose first commit has no parents |
| | `--guess` | | Force remote-tracking branch guessing |
| | `--no-guess` | | Disable remote-tracking branch guessing |
| `-d` | `--detach` | | Detach HEAD at the given commit, tag, or branch |
| | `--track` | | Create a local branch tracking the given remote branch and switch to it |

### Flag details

**`-c / --create <name> [start-point]`**: Creates a new branch named `<name>` from `<start-point>` (or HEAD if omitted), then switches to it. Validates the name, checks that no branch with that name already exists, and rejects reserved internal branch names.

**`-C / --force-create <name> [start-point]`**: Like `-c`, but if a branch named `<name>` already exists it is **reset** to the start point (or HEAD) instead of being refused. Reserved/internal branch names are still rejected. Mutually exclusive with `-c`, `-d`, and `--orphan`.

```bash
libra switch -C feature main           # Create or reset feature to main, then switch
```

**`-f / --force / --discard-changes`**: Allows tracked dirty files and staged changes to be discarded while switching. It does not allow untracked files to be overwritten by the target commit.

```bash
libra switch -f main                   # Discard tracked local changes while switching
libra switch --discard-changes main    # Same as --force
```

**`--orphan <name>`**: Points HEAD at a new unborn branch without creating a branch ref yet. The index is cleared and files that were tracked by the previous HEAD are removed from the worktree; existing untracked or ignored files are left alone. The first commit on the orphan branch is a root commit. JSON success keeps the existing schema and reports `commit` as an empty string.

```bash
libra switch --orphan fresh-start
```

```bash
libra switch -c feature-x              # New branch from HEAD
libra switch -c fix-123 abc1234        # New branch from specific commit
libra switch -c release-2.0 main       # New branch from another branch
```

**`-d / --detach`**: Moves HEAD to point directly at a commit rather than a branch. Useful for inspecting historical states or building from tags.

```bash
libra switch --detach v1.0             # Detach at a tag
libra switch --detach abc1234          # Detach at a commit
```

**`--guess / --no-guess`**: By default, `libra switch feature` creates a local branch from a unique remote-tracking branch such as `origin/feature` and sets upstream tracking. `checkout.guess=false` disables that implicit behavior; `--guess` and `--no-guess` override the config. If multiple remotes match, set `checkout.defaultRemote` or use `--track <remote>/<branch>`.

```bash
libra switch feature                   # Guess origin/feature when it is unique
libra config checkout.defaultRemote upstream
libra switch --no-guess feature        # Do not guess; require a local branch
```

**`--track`**: Looks up the remote-tracking reference, creates a local branch with the same name, sets upstream tracking, and switches to it. Conflicts with `--create`, `--force-create`, `--orphan`, and `--detach`.

```bash
libra switch --track origin/main       # Track and switch to remote branch
libra switch --track feature            # Assumes origin/feature
```

## Common Commands

```bash
libra switch main                      # Switch to an existing branch
libra switch -c feature-x              # Create and switch to a new branch
libra switch -c fix-123 abc1234        # Create branch from specific commit
libra switch -C feature-x main         # Create or reset a branch and switch
libra switch -f main                   # Discard tracked changes while switching
libra switch --orphan fresh-start      # Start an unborn root branch
libra switch --guess feature           # Guess a unique remote-tracking branch
libra switch --detach v1.0             # Detach HEAD at a tag
libra switch --track origin/main       # Track and switch to remote branch
libra switch --json main               # Structured JSON output for agents
```

## Human Output

Default human mode writes the result to `stdout`.

Switch to an existing branch:

```text
Switched to branch 'main'
```

Create and switch to a new branch:

```text
Switched to a new branch 'feature'
```

Create a tracking branch:

```text
Switched to a new branch 'feature'
Branch 'feature' set up to track remote branch 'origin/feature'
```

Detach HEAD at a commit:

```text
HEAD is now at abc1234
```

Already on the target branch (no-op):

```text
Already on 'main'
```

`--quiet` suppresses all `stdout` output.

## Structured Output (JSON examples)

`libra switch` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Switch to an existing branch:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": false,
    "detached": false,
    "already_on": false,
    "tracking": null
  }
}
```

Create and switch to a new branch:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature-x",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "created": true,
    "detached": false,
    "already_on": false,
    "tracking": null
  }
}
```

Detach HEAD at a tag or commit:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": null,
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": false,
    "detached": true,
    "already_on": false,
    "tracking": null
  }
}
```

Track and switch to a remote branch:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "feature",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "created": true,
    "detached": false,
    "already_on": false,
    "tracking": {
      "remote": "origin",
      "remote_branch": "feature"
    }
  }
}
```

Create an orphan branch:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234def5678901234567890abcdef12345678",
    "branch": "fresh-start",
    "commit": "",
    "created": true,
    "detached": false,
    "already_on": false,
    "tracking": null
  }
}
```

### Schema Notes

- `previous_branch` is `null` when HEAD was detached before the switch
- `branch` is `null` when HEAD is now detached (`--detach`)
- `commit` is an empty string for `--orphan` because the branch is unborn
- `already_on` is `true` when the target branch equals the current branch (no-op)
- `tracking` is present with `--track` or remote guessing, containing `remote` and `remote_branch`
- `created` is `true` when `--create`, `--force-create`, `--orphan`, `--track`, or remote guessing created a local branch state

## Design Rationale

### Why separate from checkout?

Git's `checkout` is overloaded: it switches branches, restores files, detaches HEAD, and creates branches -- all through the same command with different flag combinations. This makes it difficult for both humans and AI agents to predict behavior. Libra follows Git's own modernization path (introduced in Git 2.23) by splitting `checkout` into `switch` (branch operations) and `restore` (file operations). `libra switch` handles only branch-related state changes, making its behavior predictable and its error messages precise.

Keeping `switch` focused also simplifies structured output: every `SwitchOutput` contains the same fields regardless of the operation mode, so agents can parse results without guessing which schema variant applies.

### Why auto-track remote branches?

When `--track origin/feature` is used, Libra automatically creates a local branch, sets upstream tracking, and switches to it in a single atomic operation. This eliminates the multi-step ceremony of `git fetch && git branch feature origin/feature && git branch -u origin/feature feature && git switch feature`. For AI agents operating in trunk-based workflows, reducing a four-command sequence to one command means fewer failure points and simpler tool orchestration.

The `--track` flag defaults to the `origin` remote when only a branch name is provided (e.g., `libra switch --track feature` assumes `origin/feature`), which matches the most common remote setup.

Remote guessing applies the same tracking setup automatically when a plain branch name has exactly one remote-tracking match. It is on by default to match Git; use `checkout.guess=false` or `--no-guess` when scripts require local-only branch resolution.

### Why fuzzy suggestions?

When a branch name is not found, Libra computes Levenshtein distance against all known branches and suggests matches within edit distance 2. This catches common typos (`faeture` instead of `feature`) without requiring glob patterns or regex. The suggestions appear as actionable hints in the error output, reducing round-trips for both human users and AI agents that can parse hint text.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Switch branch | `git switch main` | `libra switch main` | `jj edit <rev>` |
| Create and switch | `git switch -c feature` | `libra switch -c feature` | `jj new -m "feature"` + `jj branch create feature` |
| Create from commit | `git switch -c fix abc1234` | `libra switch -c fix abc1234` | `jj new abc1234` + `jj branch create fix` |
| Detach HEAD | `git switch --detach v1.0` | `libra switch --detach v1.0` | `jj edit <rev>` (always detached-like) |
| Track remote | `git switch --track origin/main` | `libra switch --track origin/main` | N/A (jj tracks all remotes) |
| Guess remote | `git switch feature` | `libra switch feature` when a unique remote-tracking branch exists | N/A |
| Force switch | `git switch -f main` | `libra switch -f main` | N/A |
| Force create | `git switch -C feature` | `libra switch -C feature` | N/A |
| Orphan branch | `git switch --orphan <name>` | `libra switch --orphan <name>` | `jj new root()` |
| Structured output | No | `--json` / `--machine` | `--template` |
| Fuzzy suggestions | No | Levenshtein-based "did you mean" hints | No |
| Clean-state validation | Warns but proceeds (sometimes) | Blocks switch with actionable error | No dirty state concept |

## Error Handling

Every `SwitchError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Missing track target | `LBR-CLI-002` | 129 | "provide a remote branch name, for example 'origin/main'." |
| Missing detach target | `LBR-CLI-002` | 129 | "provide a commit, tag, or branch to detach at." |
| Missing branch name | `LBR-CLI-002` | 129 | "provide a branch name." |
| Branch not found | `LBR-CLI-003` | 129 | "create it with 'libra switch -c {name}'." + fuzzy suggestions |
| Guess matched multiple remotes | `LBR-CLI-003` | 128 | "set checkout.defaultRemote or pass --no-guess and use --track explicitly." |
| Got remote branch | `LBR-CLI-003` | 129 | "use 'libra switch --track ...' to create a local tracking branch." |
| Remote branch not found | `LBR-CLI-003` | 129 | "Run 'libra fetch {remote}' to update remote-tracking branches." |
| Invalid remote branch | `LBR-CLI-003` | 129 | "expected format: 'remote/branch'." |
| Branch already exists | `LBR-CONFLICT-002` | 128 | "use 'libra switch {name}' if you meant the existing local branch." |
| Internal branch blocked | `LBR-CLI-003` | 129 | -- |
| Unstaged changes | `LBR-REPO-003` | 128 | "commit or stash your changes before switching." |
| Uncommitted changes | `LBR-REPO-003` | 128 | "commit or stash your changes before switching." |
| Untracked file would be overwritten | `LBR-CONFLICT-002` | 128 | "move or remove it before switching." |
| Status check failed | `LBR-IO-001` | 128 | -- |
| Commit resolve failed | `LBR-CLI-003` | 129 | "check the revision name and try again." |
| Branch creation failed | `LBR-IO-002` | 128 | -- |
| HEAD update failed | `LBR-IO-002` | 128 | -- |
| Orphan cleanup failed | `LBR-IO-002` | 128 | "inspect the worktree, then retry or restore files manually." |
| Delegated (branch/restore) | Original code | Original | Original hints |

`switch -c <existing-branch>` currently preserves the original `branch`
command conflict contract through `DelegatedCli`, so that path keeps the branch
command's existing error shape instead of adding the `SwitchError::BranchAlreadyExists`
hint.
