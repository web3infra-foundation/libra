# `libra commit`

Create a new commit from staged changes.

**Alias:** `ci`

## Synopsis

```
libra commit [OPTIONS] -m <MESSAGE>
libra commit [OPTIONS] -F <FILE>
libra commit --amend [--no-edit]
```

## Description

`libra commit` creates a new commit from staged changes, builds tree and commit objects,
validates messages (including optional conventional commit format and GPG signing via vault),
and updates HEAD and refs.

The command reads the index to determine which files are staged, constructs a tree object
hierarchy matching the staged content, creates a commit object with the provided message and
author/committer metadata, and advances the current branch ref. When vault signing is enabled,
the commit is automatically GPG-signed. Pre-commit and commit-msg hooks are executed unless
bypassed with `--no-verify`.

## Options

### `-m, --message <MESSAGE>`

Use the given message as the commit message. Required unless `--no-edit` (with `--amend`) or
`-F` is provided.

```bash
libra commit -m "Add new feature"
```

### `-F, --file <FILE>`

Read the commit message from the given file. Mutually exclusive with `-m` when `--no-edit`
is not in use.

```bash
libra commit -F message.txt
```

### `--amend`

Replace the tip of the current branch by creating a new commit. The new commit has the same
parent(s) as the replaced commit. Cannot amend merge commits (commits with multiple parents).

```bash
libra commit --amend
libra commit --amend -m "Updated message"
```

### `--no-edit`

When used with `--amend`, reuse the message from the original commit without prompting for
changes. Conflicts with `-m` and `-F`.

```bash
libra commit --amend --no-edit
```

### `--conventional`

Validate the commit message against the Conventional Commits specification
(https://www.conventionalcommits.org). The message must match the pattern
`<type>[optional scope]: <description>`. Fails with an error if validation fails.

```bash
libra commit -m "feat: add login" --conventional
libra commit -m "fix(auth): handle expired tokens" --conventional
```

### `-a, --all`

Automatically stage tracked files that have been modified or deleted before committing.
Equivalent to running `libra add -u` before `libra commit`. Does not add new untracked files.

```bash
libra commit -a -m "Fix typo"
```

### `-s, --signoff`

Add a `Signed-off-by` trailer at the end of the commit message, using the committer's
identity.

```bash
libra commit -s -m "Add feature"
```

### `--allow-empty`

Allow creating a commit with no changes (empty diff from parent). Useful for triggering CI
or marking milestones.

```bash
libra commit --allow-empty -m "Trigger CI"
```

### `--disable-pre`

Skip the pre-commit hook only. The commit-msg hook still runs.

```bash
libra commit --disable-pre -m "Quick fix"
```

### `--no-verify`

Skip all pre-commit and commit-msg hooks/validations. Aligns with Git's `--no-verify`
behavior.

```bash
libra commit --no-verify -m "WIP: work in progress"
```

### `--author <AUTHOR>`

Override the commit author. Must use the standard `A U Thor <author@example.com>` format.

```bash
libra commit --author "Jane Doe <jane@example.com>" -m "Patch"
```

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

## Design Rationale

### `--conventional` flag for conventional commits

Git has no built-in support for commit message format validation; teams rely on external
tools like commitlint, husky, or CI checks to enforce Conventional Commits. Libra provides
first-class `--conventional` validation directly in the commit command. This serves two
purposes: (1) it gives immediate feedback at commit time rather than delayed feedback in CI,
and (2) it enables AI agents (which generate commit messages programmatically) to validate
their output without external tooling. The flag is opt-in rather than mandatory, respecting
teams that use different commit message conventions.

### Vault signing by default instead of manual GPG setup

In Git, commit signing requires configuring `user.signingkey`, `gpg.program`, and
`commit.gpgsign` -- a multi-step process that most developers skip. Libra's vault
automatically generates and manages a PGP signing key at repository initialization, so
commits are signed by default with zero configuration. This makes signed commits the norm
rather than the exception, improving supply-chain security for the entire ecosystem. Users
who do not want signing can disable it with `libra config vault.signing false`.

### `--disable-pre` flag

The `--disable-pre` flag skips only the pre-commit hook while still running the commit-msg
hook. This is more granular than Git's `--no-verify`, which skips all hooks. The use case
is when a developer trusts the commit message validation (e.g., conventional commit checks
via commit-msg hook) but wants to skip expensive pre-commit checks (e.g., full test suite,
large linter runs) during rapid iteration. This separation of concerns is intentional: the
commit message is part of the permanent record and should be validated even during quick
iterations.

### `--no-verify` to skip hooks

For cases where all hook validation needs to be bypassed (e.g., emergency fixes, WIP commits),
`--no-verify` skips both pre-commit and commit-msg hooks. This aligns with Git's behavior
and naming convention. The flag name was chosen for Git compatibility so that developers
switching from Git do not need to learn a new flag name.

## Parameter Comparison: Libra vs Git vs jj

| Parameter / Flag | Git | jj | Libra |
|---|---|---|---|
| Commit with message | `git commit -m "msg"` | `jj commit -m "msg"` | `libra commit -m "msg"` |
| Commit from file | `git commit -F file` | N/A | `libra commit -F file` |
| Amend last commit | `git commit --amend` | `jj describe` (edits working copy commit) | `libra commit --amend` |
| Amend without edit | `git commit --amend --no-edit` | `jj describe --no-edit` | `libra commit --amend --no-edit` |
| Auto-stage tracked | `git commit -a` | N/A (automatic tracking) | `libra commit -a` |
| Allow empty commit | `git commit --allow-empty` | `jj commit --allow-empty` | `libra commit --allow-empty` |
| Signoff trailer | `git commit -s` / `--signoff` | N/A | `libra commit -s` / `--signoff` |
| GPG sign commit | `git commit -S` (manual GPG) | N/A (no signing) | Automatic (vault-backed) |
| Override author | `git commit --author="..."` | N/A | `libra commit --author="..."` |
| Conventional check | External tool (commitlint) | N/A | `libra commit --conventional` |
| Skip pre-commit only | N/A | N/A | `libra commit --disable-pre` |
| Skip all hooks | `git commit --no-verify` | N/A | `libra commit --no-verify` |
| Fixup commit | `git commit --fixup=<commit>` | N/A | N/A |
| Squash commit | `git commit --squash=<commit>` | `jj squash` | N/A |
| Interactive message | `git commit` (opens editor) | `jj commit` (opens editor) | N/A (message required via -m or -F) |
| Verbose diff in editor | `git commit -v` | N/A | N/A |
| Reset author date | `git commit --reset-author` | N/A | N/A |
| Cleanup mode | `git commit --cleanup=<mode>` | N/A | N/A |
| Trailer | `git commit --trailer="..."` | N/A | N/A |
| Structured JSON output | N/A | N/A | `--json` / `--machine` |
| Error hints | Minimal | Minimal | Every error type has an actionable hint |

## Error Handling

Every `CommitError` variant maps to an explicit `StableErrorCode`.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Index corrupted | `LBR-REPO-002` | 128 | "the index file may be corrupted; try 'libra status' to verify" |
| Failed to save index | `LBR-IO-002` | 128 | -- |
| Nothing to commit (clean) | `LBR-REPO-003` | 128 | "use 'libra add' to stage changes" |
| Nothing to commit (no tracked) | `LBR-REPO-003` | 128 | "create/copy files and use 'libra add' to track" |
| Author identity missing | `LBR-AUTH-001` | 128 | "run 'libra config user.name ...' and 'libra config user.email ...'" |
| No commit to amend | `LBR-REPO-003` | 128 | "create a commit before using --amend" |
| Amend merge commit | `LBR-REPO-003` | 128 | "create a new commit instead of amending a merge commit" |
| Invalid author format | `LBR-CLI-002` | 129 | "expected format: 'Name <email>'" |
| Message file unreadable | `LBR-IO-001` | 128 | -- |
| Empty commit message | `LBR-REPO-003` | 128 | "use -m to provide a commit message" |
| Tree creation failed | `LBR-INTERNAL-001` | 128 | Issues URL |
| Object storage failed | `LBR-IO-002` | 128 | -- |
| Parent commit missing | `LBR-REPO-002` | 128 | "the parent commit is missing or corrupted" |
| HEAD update failed | `LBR-IO-002` | 128 | -- |
| Pre-commit hook failed | `LBR-REPO-003` | 128 | "use --no-verify to bypass the hook" |
| Conventional commit invalid | `LBR-CLI-002` | 129 | "see https://www.conventionalcommits.org for format rules" |
| Vault signing failed | `LBR-AUTH-001` | 128 | "check vault configuration with 'libra config --list'" |
| Auto-stage failed | `LBR-IO-001` | 128 | -- |
| Staged changes computation | `LBR-REPO-002` | 128 | "failed to compute staged changes" |

## Compatibility Notes

- Libra does not open an editor for interactive message composition; `-m` or `-F` is always required (except with `--amend --no-edit`)
- jj does not have a traditional `commit` command with staging; `jj commit` finalizes the working copy commit
- `--fixup` and `--squash` are not supported; use `libra rebase -i` for commit restructuring
- Vault signing replaces Git's `commit.gpgsign` and `user.signingkey` configuration
- `--cleanup` mode for comment stripping is not supported; messages are used as-is
