# `libra status`

`libra status` shows the state of the working tree and staging area: which files are staged
for the next commit, which have modifications not yet staged, and which are untracked. It also
reports the current branch, detached HEAD state, and upstream tracking information.

## Common Commands

```bash
libra status
libra status --short
libra status --json
libra status --exit-code
```

## Human Output

Default human mode writes the status summary to `stdout`.

Clean working tree:

```text
On branch main
nothing to commit, working tree clean
```

With changes:

```text
On branch main
Your branch is ahead of 'origin/main' by 2 commits.
  (use "libra push" to publish your local commits)

Changes to be committed:
        new file:   src/feature.rs
        modified:   src/lib.rs

Changes not staged for commit:
        modified:   README.md

Untracked files:
        notes.txt
```

Detached HEAD:

```text
HEAD detached at abc1234
nothing to commit, working tree clean
```

Short format (`--short`):

```text
A  src/feature.rs
M  src/lib.rs
 M README.md
?? notes.txt
```

`--quiet` suppresses all `stdout` output. Combined with `--exit-code`, it acts as a
silent dirty check (exit 1 if dirty, exit 0 if clean).

## Structured Output

`libra status` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "branch",
      "name": "main"
    },
    "has_commits": true,
    "upstream": {
      "remote_ref": "origin/main",
      "ahead": 2,
      "behind": 0,
      "gone": false
    },
    "staged": {
      "new": ["src/feature.rs"],
      "modified": ["src/lib.rs"],
      "deleted": []
    },
    "unstaged": {
      "modified": ["README.md"],
      "deleted": []
    },
    "untracked": ["notes.txt"],
    "ignored": [],
    "is_clean": false
  }
}
```

Clean working tree:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "branch",
      "name": "main"
    },
    "has_commits": true,
    "upstream": null,
    "staged": {
      "new": [],
      "modified": [],
      "deleted": []
    },
    "unstaged": {
      "modified": [],
      "deleted": []
    },
    "untracked": [],
    "ignored": [],
    "is_clean": true
  }
}
```

Detached HEAD:

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {
      "type": "detached",
      "oid": "abc1234def5678..."
    },
    "has_commits": true,
    "upstream": null,
    "staged": { "new": [], "modified": [], "deleted": [] },
    "unstaged": { "modified": [], "deleted": [] },
    "untracked": [],
    "ignored": [],
    "is_clean": true
  }
}
```

### Schema Notes

- `head.type` is `"branch"` or `"detached"`
- When on a branch, `head.name` is the branch name; when detached, `head.oid` is the commit hash
- `upstream` is `null` when no tracking branch is configured or HEAD is detached
- `upstream.gone` is `true` when the remote tracking branch no longer exists
- `upstream.ahead` / `upstream.behind` are `null` when `gone` is `true`
- `is_clean` is `true` when all staged, unstaged, and untracked lists are empty
- `has_commits` is `false` in a freshly initialized repository with no commits

## Exit Code Behavior

| Flag | Clean | Dirty |
|------|-------|-------|
| (default) | exit 0 | exit 0 |
| `--exit-code` | exit 0 | exit 1 |

`--exit-code` enables a silent dirty check useful for scripting. When combined with
`--quiet`, no output is produced — only the exit code signals the repository state.

## Error Handling

Every `StatusError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Index file corrupted | `LBR-REPO-002` | 128 | "the index file may be corrupted" |
| Invalid path encoding | `LBR-CLI-003` | 129 | "path contains invalid characters" |
| Failed to hash a file | `LBR-IO-001` | 128 | — |
| Cannot list working directory | `LBR-IO-001` | 128 | — |
| Working directory not found | `LBR-REPO-001` | 128 | — |
| Bare repository | `LBR-REPO-003` | 128 | "this operation must be run in a work tree" |

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Show status | `git status` | `libra status` |
| Short format | `git status -s` | `libra status --short` |
| Porcelain | `git status --porcelain` | `libra status --machine` |
| Exit code check | `git diff --exit-code` | `libra status --exit-code` |
| Upstream info | In human output | Human + structured `upstream` object |
| Structured output | No | `--json` / `--machine` |
