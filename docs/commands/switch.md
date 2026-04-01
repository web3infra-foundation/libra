# `libra switch`

Switch branches, create and switch to a new branch, or detach HEAD at a specific commit.

## Common Commands

```bash
libra switch main                      # Switch to an existing branch
libra switch -c feature-x              # Create and switch to a new branch
libra switch -c fix-123 abc1234        # Create branch from specific commit
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

Detach HEAD at a commit:

```text
HEAD is now at abc1234
```

Already on the target branch (no-op):

```text
Already on 'main'
```

`--quiet` suppresses all `stdout` output.

## Structured Output

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

### Schema Notes

- `previous_branch` is `null` when HEAD was detached before the switch
- `branch` is `null` when HEAD is now detached (`--detach`)
- `already_on` is `true` when the target branch equals the current branch (no-op)
- `tracking` is present only with `--track`, containing `remote` and `remote_branch`
- `created` is `true` when `--create` or `--track` created a new local branch

## Error Handling

Every `SwitchError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Missing track target | `LBR-CLI-002` | 129 | "provide a remote branch name, for example 'origin/main'." |
| Missing detach target | `LBR-CLI-002` | 129 | "provide a commit, tag, or branch to detach at." |
| Missing branch name | `LBR-CLI-002` | 129 | "provide a branch name." |
| Branch not found | `LBR-CLI-003` | 129 | "create it with 'libra switch -c {name}'." + fuzzy suggestions |
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
| Delegated (branch/restore) | Original code | Original | Original hints |

`switch -c <existing-branch>` currently preserves the original `branch`
command conflict contract through `DelegatedCli`, so that path keeps the branch
command's existing error shape instead of adding the `SwitchError::BranchAlreadyExists`
hint.

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Switch branch | `git switch main` | `libra switch main` |
| Create and switch | `git switch -c feature` | `libra switch -c feature` |
| Create from commit | `git switch -c fix abc1234` | `libra switch -c fix abc1234` |
| Detach HEAD | `git switch --detach v1.0` | `libra switch --detach v1.0` |
| Track remote | `git switch --track origin/main` | `libra switch --track origin/main` |
| Structured output | No | `--json` / `--machine` |
| Fuzzy suggestions | No | Levenshtein-based "did you mean" hints |
| Error hints | Minimal | Most errors include actionable hints |
