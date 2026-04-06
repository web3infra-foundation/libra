# `libra commit`

`libra commit` creates a new commit from staged changes, builds tree and commit objects,
validates messages (including optional conventional commit format and GPG signing via vault),
and updates HEAD and refs.

## Common Commands

```bash
libra commit -m "Add new feature"
libra commit -m "feat: add login" --conventional
libra commit --amend
libra commit --amend --no-edit
libra commit -a -m "Fix typo"
libra commit -F message.txt
libra commit -s -m "Add feature"
libra commit --allow-empty -m "Trigger CI"
libra commit --json -m "Add feature"
```

## Human Output

Default human mode writes the commit summary to `stdout`.

Normal commit:

```text
[main abc1234] Add new feature
 2 files changed (new: 1, modified: 1, deleted: 0)
```

Root commit:

```text
[main (root-commit) abc1234] Initial commit
 1 file changed (new: 1, modified: 0, deleted: 0)
```

`--quiet` suppresses all `stdout` output.

## Structured Output

`libra commit` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- both suppress hook stdout/stderr (piped instead of inherited)
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "short_id": "abc1234",
    "subject": "Add new feature",
    "root_commit": false,
    "amend": false,
    "files_changed": {
      "total": 2,
      "new": 1,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

Root commit:

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "abc1234def5678901234567890abcdef12345678",
    "short_id": "abc1234",
    "subject": "Initial commit",
    "root_commit": true,
    "amend": false,
    "files_changed": {
      "total": 1,
      "new": 1,
      "modified": 0,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

Amend:

```json
{
  "ok": true,
  "command": "commit",
  "data": {
    "head": "main",
    "branch": "main",
    "commit": "def5678abc1234901234567890abcdef12345678",
    "short_id": "def5678",
    "subject": "Amended message",
    "root_commit": false,
    "amend": true,
    "files_changed": {
      "total": 1,
      "new": 0,
      "modified": 1,
      "deleted": 0
    },
    "signoff": false,
    "conventional": null,
    "signed": true
  }
}
```

### Schema Notes

- `head` is the branch name or `"detached"` for backward compatibility
- `branch` is `null` when HEAD is detached; `Some(name)` otherwise
- `conventional` is `true` when `--conventional` was passed and validation succeeded; `null` when not requested
- `signed` is `true` when vault signing is enabled and the commit was GPG-signed
- `signoff` is `true` when `-s` / `--signoff` appended a `Signed-off-by` trailer

## Error Handling

Every `CommitError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Index corrupted | `LBR-REPO-002` | 128 | "the index file may be corrupted; try 'libra status' to verify" |
| Failed to save index | `LBR-IO-002` | 128 | — |
| Nothing to commit (clean) | `LBR-REPO-003` | 128 | "use 'libra add' to stage changes" |
| Nothing to commit (no tracked) | `LBR-REPO-003` | 128 | "create/copy files and use 'libra add' to track" |
| Author identity missing | `LBR-AUTH-001` | 128 | "run 'libra config user.name ...' and 'libra config user.email ...'" |
| No commit to amend | `LBR-REPO-003` | 128 | "create a commit before using --amend" |
| Amend merge commit | `LBR-REPO-003` | 128 | "create a new commit instead of amending a merge commit" |
| Invalid author format | `LBR-CLI-002` | 129 | "expected format: 'Name <email>'" |
| Message file unreadable | `LBR-IO-001` | 128 | — |
| Empty commit message | `LBR-REPO-003` | 128 | "use -m to provide a commit message" |
| Tree creation failed | `LBR-INTERNAL-001` | 128 | Issues URL |
| Object storage failed | `LBR-IO-002` | 128 | — |
| Parent commit missing | `LBR-REPO-002` | 128 | "the parent commit is missing or corrupted" |
| HEAD update failed | `LBR-IO-002` | 128 | — |
| Pre-commit hook failed | `LBR-REPO-003` | 128 | "use --no-verify to bypass the hook" |
| Conventional commit invalid | `LBR-CLI-002` | 129 | "see https://www.conventionalcommits.org for format rules" |
| Vault signing failed | `LBR-AUTH-001` | 128 | "check vault configuration with 'libra config --list'" |
| Auto-stage failed | `LBR-IO-001` | 128 | — |
| Staged changes computation | `LBR-REPO-002` | 128 | "failed to compute staged changes" |

## Feature Comparison: Libra vs Git

| Use Case | Git | Libra |
|----------|-----|-------|
| Basic commit | `git commit -m "msg"` | `libra commit -m "msg"` |
| Amend | `git commit --amend` | `libra commit --amend` |
| Auto-stage | `git commit -a` | `libra commit -a` |
| From file | `git commit -F file` | `libra commit -F file` |
| Signoff | `git commit -s` | `libra commit -s` |
| Conventional check | External tool | `libra commit --conventional` |
| Vault signing | `git commit -S` (GPG) | Automatic when vault enabled |
| Structured output | No | `--json` / `--machine` |
| Error hints | Minimal | Every error type has an actionable hint |
