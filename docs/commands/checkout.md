# `libra checkout`

Show the current branch, switch to an existing branch, create and switch to a new branch, or restore paths through the explicit `--` compatibility form.
Compatible with `git checkout` for common branch operations and explicit path restoration.

## Synopsis

```
libra checkout [-f] [<branch>]
libra checkout -b <name>
libra checkout -B <name> [<start-point>]
libra checkout --detach [<commit-ish>]
libra checkout --orphan <name> [<start-point>]
libra checkout (--ours | --theirs) -- <pathspec>...
libra checkout [<tree-ish>] -- <pathspec>...
```

## Description

`libra checkout` is a Git-compatibility surface that delegates to `switch` and `restore` internally. It supports the most common `git checkout` patterns: showing the current branch, switching to an existing branch, creating a new branch with `-b`, force-creating or resetting one with `-B`, detaching HEAD with `--detach`, starting a history-less branch with `--orphan`, forcing a switch over local changes with `-f`, resolving a conflicted path to one side with `--ours`/`--theirs`, auto-tracking remote branches, and restoring paths when an explicit `--` separator is present.

This command exists so that developers migrating from Git can use familiar muscle memory. For new workflows, prefer `libra switch` (for branch operations) and `libra restore` (for file operations), which provide richer error messages, structured JSON output, and clearer semantics.

When checking out a branch name that does not exist locally but matches a remote-tracking branch (e.g., `origin/feature`), Libra automatically creates a local tracking branch, sets upstream, and pulls -- going further than Git's auto-track by also synchronizing content immediately.

Path restoration is only enabled by an explicit `--` separator. Without `--`, `libra checkout <name>` is always branch mode, even when a file has the same name.

The internal `intent` and `agent-traces` branches are protected: `-b`/`-B`/`--orphan`/`--detach` refuse to create, reset, or check out those AI-managed refs (the positional commit-ish is checked revision-aware, so `agent-traces~1` is also refused). `main` is always allowed.

### Branch-control modes

- **`-B <name> [<start-point>]`** — create the branch if absent, or reset it to the start point (default current HEAD) if it already exists, then switch. Records a `checkout` reflog entry.
- **`--detach [<commit-ish>]`** — move HEAD to the resolved commit (default current HEAD) in the detached state, leaving no branch checked out.
- **`--orphan <name> [<start-point>]`** — point HEAD at a new unborn branch (no `reference` row exists until the first commit). The index/worktree are aligned to the start point, matching Git's "as if you had run `checkout <start-point>`". Following Git, `--orphan` writes **no** HEAD reflog entry (the target has no commit OID yet). The first commit on the orphan branch has no parents.
- **`-f` / `--force`** — skip the dirty-working-tree guard and let the target overwrite uncommitted changes (applies to plain switch, `-B`, `--detach`, and `--orphan`).

### Conflicted-path checkout (`--ours` / `--theirs`)

`--ours` / `--theirs` operate only on paths after an explicit `--` and only while those paths are in a merge-conflict state:

- **`--ours`** restores merge stage #2 (our side) into the working tree.
- **`--theirs`** restores merge stage #3 (their side) into the working tree.

Either way the path collapses to a clean stage #0 index entry (preserving its mode) and the remaining conflict stages are dropped. Running `--ours`/`--theirs` on a path that is not conflicted is an error — the file is never silently rewritten. `--ours` and `--theirs` are mutually exclusive, and both require a pathspec after `--`.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<branch>` | positional (optional) | Target branch to switch to. Omit to show current branch. |
| `-b` | | `<name>` | Create a new branch from the current HEAD and switch to it |
| `-B` | | `<name>` | Create the branch, or reset it to the start point (or current HEAD), then switch |
| | `--detach` | | Detach HEAD at the given commit-ish (or current HEAD) instead of switching to a branch |
| | `--orphan` | `<name>` | Create a new unborn branch whose first commit will have no parents |
| | `--ours` | | On a conflicted path, check out our side of the merge (stage #2); requires `-- <path>` |
| | `--theirs` | | On a conflicted path, check out their side of the merge (stage #3); requires `-- <path>` |
| `-f` | `--force` | | Force checkout: proceed even when the working tree has changes that would be overwritten |
| | `[<tree-ish>] -- <pathspec>...` | positional | Restore paths. Without `<tree-ish>`, restores the worktree from the index. With `<tree-ish>`, restores both index and worktree from that source. |

### Flag examples

```bash
# Show the current branch
libra checkout

# Switch to an existing local branch
libra checkout main

# Create and switch to a new branch
libra checkout -b feature-x

# Create or reset a branch to the current HEAD, then switch
libra checkout -B feature-x

# Detach HEAD at a previous commit
libra checkout --detach HEAD~1

# Start a new history-less branch
libra checkout --orphan fresh-start

# Force a switch, discarding uncommitted local changes
libra checkout -f main

# Resolve a conflicted path to our / their side
libra checkout --ours -- src/conflicted.rs
libra checkout --theirs -- src/conflicted.rs

# Auto-track a remote branch (creates local, sets upstream, pulls)
libra checkout feature

# Restore a path from the index to the worktree
libra checkout -- src/main.rs

# Restore a path from HEAD to both index and worktree
libra checkout HEAD -- src/main.rs
```

## Common Commands

```bash
libra checkout                         # Show the current branch
libra checkout main                    # Switch to an existing local branch
libra checkout feature-x               # Switch to another branch
libra checkout -b feature-x            # Create and switch to a new branch
libra checkout -B feature-x            # Create or reset a branch to HEAD, then switch
libra checkout --detach HEAD~1         # Detach HEAD at a commit
libra checkout --orphan fresh          # Start a new unborn branch (no history)
libra checkout -f main                 # Force switch, discarding local changes
libra checkout --ours -- file.txt      # Take our side of a conflicted path
libra checkout --theirs -- file.txt    # Take their side of a conflicted path
libra checkout -- file.txt             # Restore file from index to worktree
libra checkout HEAD -- file.txt        # Restore file from HEAD to index + worktree
libra --json checkout main             # Structured compatibility output
libra checkout --quiet main            # Switch without informational stdout
```

## Human Output

Default human mode writes the result to `stdout`.

Show current branch:

```text
Current branch is main.
```

Show detached HEAD:

```text
HEAD detached at abc1234d
```

Switch to an existing branch:

```text
Switched to branch 'main'
```

Create and switch to a new branch:

```text
Switched to a new branch 'feature-x'
```

Auto-track a remote branch:

```text
branch 'feature' set up to track 'origin/feature'.
Switched to a new branch 'feature'
Branch 'feature' set up to track remote branch 'origin/feature'
```

Depending on the remote state, the follow-up `pull` step may emit additional
synchronization output.

Already on the target branch (no-op):

```text
Already on main
```

Path restore:

```text
Updated 1 path(s) from HEAD
```

`--quiet` suppresses all `stdout` output.

## Structured Output (JSON)

`checkout` supports `--json` and `--machine` for the compatibility surface. `--json` emits a normal command envelope; `--machine` emits the same envelope as one NDJSON line. Nested `restore`, branch-upstream, and pull output is suppressed so stdout contains only the checkout result.

Example for switching to an existing local branch:

```json
{
  "ok": true,
  "command": "checkout",
  "data": {
    "action": "switch",
    "previous_branch": "main",
    "previous_commit": "abc1234...",
    "branch": "feature-x",
    "commit": "def5678...",
    "short_commit": "def5678a",
    "switched": true,
    "created": false,
    "pulled": false,
    "already_on": false,
    "detached": false,
    "tracking": null
  }
}
```

| Action | When emitted |
|--------|--------------|
| `show-current` | `libra checkout` with no branch |
| `already-on` | Target branch is already checked out |
| `switch` | Existing local branch checkout |
| `create` | `checkout -b <branch>`, or `-B`/`--orphan` creating a new branch |
| `reset` | `checkout -B <branch>` resetting an already-existing branch (sets `reset: true`) |
| `detach` | `checkout --detach [<commit-ish>]` (sets `detached: true`) |
| `track` | Local branch is created from `origin/<branch>` and pull is attempted |
| `restore-paths` | Explicit `checkout [<tree-ish>] -- <pathspec>...` path restoration, including `--ours`/`--theirs` |

`--orphan` sets `action: "create"` with `orphan: true` and a null `commit` (the branch is unborn). Remote auto-track output sets `created: true`, `pulled: true`, and includes `tracking.remote` plus `tracking.remote_branch`.

For richer branch workflows, `libra switch --json ...` remains the preferred structured command. For file workflows, `libra restore --json ...` remains preferred; checkout path mode is only a Git-compatible alias.

Example for path restoration:

```json
{
  "ok": true,
  "command": "checkout",
  "data": {
    "action": "restore-paths",
    "previous_branch": "main",
    "branch": "main",
    "switched": false,
    "restore": {
      "source": "HEAD",
      "worktree": true,
      "staged": true,
      "restored_files": ["src/main.rs"],
      "deleted_files": []
    }
  }
}
```

## Design Rationale

### Why keep checkout as a compatibility command?

Git muscle memory is deeply ingrained. Developers who have used `git checkout` for years will instinctively type `libra checkout main`. Rather than forcing an immediate mental model change, Libra provides `checkout` as a thin wrapper that handles the most common patterns. This lowers the adoption barrier while the recommended `switch`/`restore` split is documented and encouraged.

The command intentionally keeps file restoration behind Git's explicit `--` separator. Plain `libra checkout <name>` remains branch mode; `libra checkout -- <path>` and `libra checkout <tree-ish> -- <path>` are compatibility aliases for the corresponding `restore` operations.

### Visible compatibility surface (post-C5)

`checkout` is exposed in top-level help (`libra --help`) as a compatibility
surface — it is **no longer hidden**. New users coming from Git can find it
without surprise, but the help banner and the command index both steer
day-to-day usage to `switch` (branch navigation) and `restore` (file
restoration). `switch` and `restore` provide:

- Typed command-specific error enums and stable error codes
- Structured JSON output (`--json` / `--machine`)
- Fuzzy branch suggestions on typos
- Explicit semantics (no ambiguity between "switch branch" and "restore file")

### Why auto-pull on remote branch?

When `libra checkout feature` finds `origin/feature` but no local `feature` branch, it creates the local branch, sets upstream tracking, and immediately pulls. This goes beyond Git's behavior (which only creates the tracking branch without pulling). The rationale:

- **Trunk-based development**: in Libra's target workflow, checking out a remote branch implies intent to work on it, so having the latest content is almost always desired.
- **Fewer commands for agents**: an AI agent checking out a remote branch wants working content immediately, not an empty tracking branch that requires a separate `pull`.
- **Fail-fast**: if the pull fails (network error, merge conflict), the user learns immediately rather than discovering stale content later.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Git | Libra | jj |
|---------|-----|-------|----|
| Show current branch | `git branch --show-current` | `libra checkout` (no args) | `jj log -r @` |
| Switch branch | `git checkout main` | `libra checkout main` | `jj edit <rev>` |
| Create and switch | `git checkout -b feature` | `libra checkout -b feature` | `jj new` + `jj branch create` |
| Auto-track remote | `git checkout feature` (creates tracking) | `libra checkout feature` (creates tracking + pulls) | N/A |
| Restore files | `git checkout -- file` | `libra checkout -- file` (prefer `libra restore file`) | `jj restore` |
| Restore files from revision | `git checkout HEAD -- file` | `libra checkout HEAD -- file` (prefer `libra restore --source HEAD -S -W file`) | `jj restore --from <revision>` |
| Create or reset branch | `git checkout -B feature [start]` | `libra checkout -B feature [start]` | `jj branch set` |
| Detach HEAD | `git checkout <commit>` | `libra checkout --detach <commit>` | `jj edit <rev>` |
| Orphan branch | `git checkout --orphan name` | `libra checkout --orphan name` | `jj new --no-edit root()` |
| Force switch | `git checkout -f branch` | `libra checkout -f branch` | N/A |
| Resolve conflicted path | `git checkout --ours/--theirs -- file` | `libra checkout --ours/--theirs -- file` | `jj resolve` |
| Structured output | No | `--json` / `--machine` for branch compatibility actions | `--template` |

## Error Handling

`checkout` has a typed `CheckoutError` for checkout-owned failures and delegates path restore failures to `restore` while preserving stable codes.

Exit codes follow Libra's coarse class contract: `Cli`-class stable codes
(`LBR-CLI-002` / `LBR-CLI-003`) exit **129**; `Repo` / `Conflict` / `Io` /
`RepoCorrupt` classes exit **128**; clap parse failures exit **2**, except
clap argument conflicts for a present subcommand (e.g. `--detach -b`,
`--ours --theirs`), which Libra remaps to `command_usage` → **129**.

| Scenario | Stable code | Message | Exit |
|----------|-------------|---------|------|
| Dirty worktree (unstaged or staged changes), no `-f` | `LBR-REPO-003` | "local changes would be overwritten by checkout" | 128 |
| Untracked file would be overwritten | `LBR-CONFLICT-002` | "local changes would be overwritten by checkout: {path}" | 128 |
| Internal (`intent`/`agent-traces`) branch checkout blocked | `LBR-CLI-003` | "checking out '{name}' branch is not allowed" | 129 |
| Create/reset internal branch blocked | `LBR-CLI-003` | "creating/switching to '{name}' branch is not allowed" | 129 |
| Branch not found (no remote match) | `LBR-CLI-003` | "branch '{name}' not found" | 129 |
| Pathspec not matched in path mode | `LBR-CLI-003` | "path specification '{path}' did not match any files known to libra" | 129 |
| `-b` combined with path mode | `LBR-CLI-002` | "checkout path mode cannot be combined with -b" | 129 |
| `--ours`/`--theirs` without a pathspec | `LBR-CLI-002` | "'--ours' requires a pathspec after '--' ..." | 129 |
| `--ours`/`--theirs` on a non-conflicted path | `LBR-CONFLICT-002` | "path '{path}' is not in a merge conflict state" | 128 |
| `--ours --theirs` together (clap conflict) | `LBR-CLI-002` | usage error (remapped from clap conflict) | 129 |
| Index/object read failure (conflict-stage checkout) | `LBR-IO-001` | "failed to read index/object for '{path}': {detail}" | 128 |
| Worktree/index write failure | `LBR-IO-002` | "failed to write '{path}': {detail}" | 128 |
| HEAD/branch reference write failure | `LBR-IO-002` | "failed to update HEAD/branch reference: {detail}" | 128 |
| Current branch (no-op) | N/A | Prints "Already on {branch}" and succeeds | 0 |
| Branch storage query failure | `LBR-IO-001` | "failed to resolve checkout target: {detail}" | 128 |
| Corrupt branch reference | `LBR-REPO-002` | "failed to resolve checkout target: {detail}" | 128 |
