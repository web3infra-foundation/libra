# `libra checkout`

Show the current branch, switch to an existing branch, or create and switch to a new branch.
Compatible with `git checkout` for common branch operations.

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

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Show current branch | `git branch --show-current` | `libra checkout` |
| Switch branch | `git checkout main` | `libra checkout main` |
| Create and switch | `git checkout -b feature` | `libra checkout -b feature` |
| Auto-track remote | `git checkout feature` (auto-creates tracking) | `libra checkout feature` (auto-creates tracking + pulls) |
| Structured output | No | Not yet (planned) |
