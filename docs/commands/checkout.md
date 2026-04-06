# `libra checkout`

Show the current branch, switch to an existing branch, or create and switch to a new branch.
Compatible with `git checkout` for common branch operations.

## Synopsis

```
libra checkout [<branch>]
libra checkout -b <name>
```

## Description

`libra checkout` is a Git-compatibility surface that delegates to `switch` and `restore` internally. It supports the most common `git checkout` patterns: showing the current branch, switching to an existing branch, creating a new branch with `-b`, and auto-tracking remote branches.

This command exists so that developers migrating from Git can use familiar muscle memory. For new workflows, prefer `libra switch` (for branch operations) and `libra restore` (for file operations), which provide richer error messages, structured JSON output, and clearer semantics.

When checking out a branch name that does not exist locally but matches a remote-tracking branch (e.g., `origin/feature`), Libra automatically creates a local tracking branch, sets upstream, and pulls -- going further than Git's auto-track by also synchronizing content immediately.

## Options

| Flag | Long | Value | Description |
|------|------|-------|-------------|
| | `<branch>` | positional (optional) | Target branch to switch to. Omit to show current branch. |
| `-b` | | `<name>` | Create a new branch from the current HEAD and switch to it |

### Flag examples

```bash
# Show the current branch
libra checkout

# Switch to an existing local branch
libra checkout main

# Create and switch to a new branch
libra checkout -b feature-x

# Auto-track a remote branch (creates local, sets upstream, pulls)
libra checkout feature
```

## Common Commands

```bash
libra checkout                         # Show the current branch
libra checkout main                    # Switch to an existing local branch
libra checkout feature-x               # Switch to another branch
libra checkout -b feature-x            # Create and switch to a new branch
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

`--quiet` suppresses all `stdout` output.

## Structured Output (JSON examples)

`checkout` does not yet have its own structured JSON output model. Structured output for checkout is planned for a future batch. In the meantime, use `libra switch --json` for JSON-compatible branch switching.

When JSON output is added, it will follow the same envelope pattern:

```json
{
  "ok": true,
  "command": "checkout",
  "data": {
    "action": "switch",
    "branch": "main",
    "commit": "abc1234...",
    "created": false,
    "auto_tracked": false
  }
}
```

## Design Rationale

### Why keep checkout as a compatibility command?

Git muscle memory is deeply ingrained. Developers who have used `git checkout` for years will instinctively type `libra checkout main`. Rather than forcing an immediate mental model change, Libra provides `checkout` as a thin wrapper that handles the most common patterns. This lowers the adoption barrier while the recommended `switch`/`restore` split is documented and encouraged.

The command intentionally supports only the branch-switching subset of `git checkout` -- it does not support file restoration (`git checkout -- file`), which is handled by `libra restore`.

### Why hidden / not promoted?

`checkout` is not prominently featured in help output or documentation indexes. It exists for compatibility, not as a primary interface. New Libra users and AI agents should use `switch` and `restore`, which provide:
- Typed error enums with stable error codes (checkout uses raw `CliError`)
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
| Restore files | `git checkout -- file` | Not supported (use `libra restore`) | `jj restore` |
| Detach HEAD | `git checkout <commit>` | Not supported (use `libra switch --detach`) | `jj edit <rev>` |
| Structured output | No | Not yet (planned) | `--template` |

## Error Handling

`checkout` does not yet have its own typed error enum. Errors are surfaced
via `CliError` directly. Key failure scenarios:

| Scenario | Message | Exit |
|----------|---------|------|
| Dirty worktree (unstaged or staged changes) | "local changes would be overwritten by checkout" | 128 |
| Internal branch blocked | "checking out '{name}' branch is not allowed" | 128 |
| Create internal branch blocked | "creating/switching to '{name}' branch is not allowed" | 128 |
| Branch not found (no remote match) | "path specification '{name}' did not match any files known to libra" | 128 |
| Current branch (no-op) | Prints "Already on {branch}" and succeeds | 0 |
| Branch storage query failure | "failed to resolve checkout target: {detail}" | 128 |
| Corrupt branch reference | "failed to resolve checkout target: {detail}" | 128 |
