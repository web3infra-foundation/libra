# `libra clone`

`libra clone` creates a local copy of a remote repository by fetching objects, configuring
`origin`, and checking out the working tree. It initializes a vault-backed repository and
transparently reuses `run_init()` for the local metadata setup.

## Common Commands

```bash
libra clone git@github.com:user/repo.git
libra clone https://github.com/user/repo.git
libra clone git@github.com:user/repo.git my-dir
libra clone --bare git@github.com:user/repo.git
libra clone -b develop git@github.com:user/repo.git
libra clone --single-branch -b main git@github.com:user/repo.git
libra clone --depth 1 git@github.com:user/repo.git
```

## Human Output

Default human mode writes staged progress to `stderr` and the final summary to `stdout`.

Phases:

- `Connecting to <url> ...`
- `Initializing repository ...`
- `Fetching objects ...`
- `Configuring repository ...`
- `Checking out working copy ...` (non-bare only)

Success output:

```text
Cloned into 'repo'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
```

Bare clone:

```text
Cloned into bare repository '/path/to/repo.git'
  remote: origin → git@github.com:user/repo.git
  branch: main
  signing: enabled
```

Empty remote:

```text
Cloned into 'empty'
  remote: origin → git@github.com:user/empty.git
  signing: enabled

warning: You appear to have cloned an empty repository.
```

`--quiet` suppresses all progress and the final success summary, including warnings.

## Structured Output

`libra clone` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- both suppress progress output and nested init/fetch output
- `stderr` stays clean on success

Example:

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/my-repo",
    "bare": false,
    "remote_url": "git@github.com:user/repo.git",
    "branch": "main",
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": "/Users/eli/.ssh/id_ed25519",
    "shallow": false,
    "warnings": []
  }
}
```

Empty remote returns `"branch": null` and a warning:

```json
{
  "ok": true,
  "command": "clone",
  "data": {
    "path": "/Users/eli/projects/empty-repo",
    "bare": false,
    "remote_url": "git@github.com:user/empty-repo.git",
    "branch": null,
    "object_format": "sha1",
    "repo_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "vault_signing": true,
    "ssh_key_detected": null,
    "shallow": false,
    "warnings": [
      "You appear to have cloned an empty repository."
    ]
  }
}
```

### Schema Notes

- `branch` is the actual checked-out branch; `null` when the remote has no refs
- `shallow` is `true` when `--depth` was used
- `ref_format` and `converted_from` from init are intentionally excluded
- `objects_fetched` / `bytes_received` are not exposed until the fetch improvement lands

## Error Handling

Every `CloneError` variant maps to an explicit `StableErrorCode` — no message substring inference.

| Scenario | Error Code | Exit | Hint |
|----------|-----------|------|------|
| Cannot infer destination path | `LBR-CLI-002` | 129 | "please specify the destination path explicitly" |
| Destination exists and is non-empty | `LBR-CLI-003` | 129 | "choose a different path or empty the directory first" |
| Destination already contains a repo | `LBR-REPO-003` | 128 | "the destination already contains a libra repository" |
| Cannot create destination directory | `LBR-IO-002` | 128 | "check directory permissions and disk space" |
| Local path does not exist | `LBR-REPO-001` | 128 | "use a valid libra repository path or a reachable remote URL" |
| Malformed URL or unsupported scheme | `LBR-CLI-003` | 129 | "check the clone URL or scheme" |
| Authentication / permission denied | `LBR-AUTH-002` | 128 | "check SSH key / HTTP credentials and repository access rights" |
| Network unreachable | `LBR-NET-001` | 128 | "check the remote host, DNS, VPN/proxy, and network connectivity" |
| Protocol / discovery error | `LBR-NET-002` | 128 | "the remote did not complete discovery successfully" |
| Remote branch not found | `LBR-REPO-003` | 128 | "use `-b <branch>` to specify an existing branch" |
| Object format mismatch | `LBR-REPO-003` | 128 | "the remote and local repository use different object formats" |
| Checkout resolve failure | `LBR-REPO-003` | 128 | "working tree checkout target could not be resolved" |
| Checkout read failure | `LBR-IO-001` | 128 | "failed to read repository state while checking out" |
| Checkout write failure | `LBR-IO-002` | 128 | "files could not be written" |
| Checkout LFS download failure | `LBR-NET-001` | 128 | "LFS content transfer failed" |
| Internal invariant | `LBR-INTERNAL-001` | 128 | Issues URL |

Init errors are transparently forwarded through `InitError → CliError`.

### Cleanup Failure Visibility

When clone fails, `cleanup_failed_clone()` attempts to remove the partially created directory.
If cleanup itself fails, the warning is attached to the error via `with_priority_hint()` so it
surfaces in both human and JSON error output instead of being silently swallowed.

### Non-Bare Checkout Is Required For Success

`setup_repository()` uses `execute_checked_typed()` which returns typed `RestoreError` variants.
If checkout fails, the clone reports failure — it does not silently succeed with a broken worktree.

## Vault And Identity

- Clone always initializes with `vault: true`, matching `libra init` defaults
- `vault_signing` and `ssh_key_detected` from init are transparently forwarded to `CloneOutput`
- SSH key detection uses the isolated `HOME` from the init phase

## Feature Comparison: Libra vs Git vs jj

| Use Case | Git | jj | Libra |
|----------|-----|----|-------|
| Basic clone | `git clone <url>` | `jj git clone <url>` | `libra clone <url>` |
| Target directory | `git clone <url> <dir>` | `jj git clone <url> <dir>` | `libra clone <url> <dir>` |
| Bare clone | `git clone --bare <url>` | No direct equivalent | `libra clone --bare <url>` |
| Specific branch | `git clone -b <branch> <url>` | `jj git clone -b <branch> <url>` | `libra clone -b <branch> <url>` |
| Shallow clone | `git clone --depth N <url>` | No direct equivalent | `libra clone --depth N <url>` |
| Single branch | `git clone --single-branch <url>` | No direct equivalent | `libra clone --single-branch <url>` |
| Real-time progress | Progress bar on stderr | Progress on stderr | Phased stderr progress + fetch progress bar |
| Structured output | No | No | `--json` / `--machine` |
| Auth guidance | No | No | SSH key detection + hint |
| Error hints | Minimal | Minimal | Every error type has an actionable hint |
