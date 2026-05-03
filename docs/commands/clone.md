# `libra clone`

Clone a repository into a new directory.

## Synopsis

```
libra clone [OPTIONS] <REMOTE_REPO> [LOCAL_PATH]
```

## Description

`libra clone` creates a local copy of a remote repository by fetching objects, configuring
`origin`, and checking out the working tree. It initializes a vault-backed repository and
transparently reuses `run_init()` for the local metadata setup.

Cloning fetches all objects and refs from the remote, creates a `.libra` directory with a
SQLite-backed metadata store, sets up the `origin` remote, and checks out the default branch
(or the branch specified with `-b`). Vault signing is always bootstrapped during clone,
matching `libra init` defaults. For non-bare clones, any checked-out `.gitignore` files are
copied to matching `.libraignore` files so Libra ignore rules work immediately.

For bare clones, no working tree checkout is performed and the repository directory itself
becomes the object store. Bare clones do not create `.libraignore`.

## Options

### `<REMOTE_REPO>` (required)

The remote repository URL to clone from. Supports SSH (`git@host:user/repo.git`) and
HTTPS (`https://host/user/repo.git`) protocols, as well as local filesystem paths.

```bash
libra clone git@github.com:user/repo.git
libra clone https://github.com/user/repo.git
libra clone /path/to/local/repo
```

### `[LOCAL_PATH]`

Optional destination directory. When omitted, Libra infers the directory name from the
repository URL (e.g., `repo` from `repo.git`). If inference fails, an error is returned
asking the user to specify the path explicitly.

```bash
libra clone git@github.com:user/repo.git my-dir
```

### `-b, --branch <NAME>`

Check out `<NAME>` instead of the remote's HEAD. The branch must exist on the remote;
otherwise a "remote branch not found" error is raised.

```bash
libra clone -b develop git@github.com:user/repo.git
```

### `--single-branch`

Fetch only the history leading to the tip of a single branch (HEAD, or the branch given
by `-b`). Reduces transfer size for large repositories when only one branch is needed.

```bash
libra clone --single-branch -b main git@github.com:user/repo.git
```

### `--bare`

Create a bare repository without a working tree. The destination directory becomes the
object store directly. Useful for central/server-side repositories.

```bash
libra clone --bare git@github.com:user/repo.git
```

### `--depth <N>`

Create a shallow clone with history truncated to the specified number of commits.
`N` must be a positive integer.

```bash
libra clone --depth 1 git@github.com:user/repo.git
libra clone --depth 50 git@github.com:user/repo.git
```

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
  remote: origin -> git@github.com:user/repo.git
  branch: main
  signing: enabled

Tip: using existing SSH key at ~/.ssh/id_ed25519
```

Bare clone:

```text
Cloned into bare repository '/path/to/repo.git'
  remote: origin -> git@github.com:user/repo.git
  branch: main
  signing: enabled
```

Empty remote:

```text
Cloned into 'empty'
  remote: origin -> git@github.com:user/empty.git
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

## Design Rationale

### No `--recurse-submodules`

Git's submodule system (`--recurse-submodules`) is a frequent source of developer friction:
submodules require separate fetch/checkout cycles, create nested `.git` directories, and
break many tools that assume a single worktree. Libra does not implement submodules. For
monorepo workflows, all code lives in a single repository. For multi-repo composition, Libra
encourages explicit dependency management (package managers, vendoring) rather than embedding
repositories within repositories. This keeps the clone operation simple and predictable.

### Vault bootstrapping during clone

Libra initializes vault-backed signing during clone by reusing the same `run_init()` path
as `libra init`. This means every cloned repository is immediately ready for signed commits
without additional setup. Git requires users to manually configure GPG/SSH signing after
cloning, which means most cloned repositories produce unsigned commits by default. By
bootstrapping the vault at clone time, Libra ensures that the security posture of a cloned
repository matches that of a freshly initialized one.

### Ignore file conversion

Libra uses `.libraignore` for its ignore policy. During non-bare clone, every checked-out
`.gitignore` is copied to a sibling `.libraignore`. Existing user-owned `.libraignore` files
are preserved and surfaced as warnings; the original `.gitignore` files remain untouched.

### `--depth` for shallow clones

Shallow clones are essential for CI/CD pipelines and large monorepos where full history is
unnecessary. Libra supports `--depth N` with the same semantics as Git: the history is
truncated to the specified number of commits. The depth value is validated at parse time
(must be a positive integer) and propagated to the fetch protocol layer. Unlike Git, Libra
does not yet support `--shallow-since` or `--shallow-exclude` for date-based or ref-based
shallow boundaries, keeping the initial implementation focused and predictable.

### `--sparse` is intentionally unsupported

Sparse-checkout (`git clone --sparse`, `git sparse-checkout`) is intentionally not
implemented. Sparse cone/skip-worktree relies on Git-managed worktree configuration,
while Libra has migrated config / HEAD / refs to SQLite. The bridge is not free, and
the audit-driven decision is to keep `--sparse` deferred until there is a concrete
monorepo subtree-checkout requirement that cannot be met by tiered cloud storage.
See [`docs/improvement/compatibility/declined.md`](../improvement/compatibility/declined.md)
entry **D10** for the restart conditions.

### `--recurse-submodules` is intentionally unsupported

Per the broader product boundary on submodules (no submodule subcommand surface),
`clone --recurse-submodules` is also unsupported. See
[`docs/improvement/compatibility/declined.md`](../improvement/compatibility/declined.md)
entries **D1** (submodule) and **D4** (clone --recurse-submodules) for restart
conditions.

### `--single-branch` flag

When combined with `--branch`, `--single-branch` reduces the data transferred during clone
by fetching only the specified branch's history. This is particularly useful for large
repositories with many long-lived branches where only one branch is needed for the current
workflow (e.g., CI building a specific release branch). Git supports this as well; jj does
not, because its operation-log model fetches all refs by design.

## Parameter Comparison: Libra vs Git vs jj

| Parameter / Flag | Git | jj | Libra |
|---|---|---|---|
| Remote URL (positional) | `git clone <url>` | `jj git clone <url>` | `libra clone <url>` |
| Destination directory | `git clone <url> <dir>` | `jj git clone <url> <dir>` | `libra clone <url> <dir>` |
| Specific branch | `-b` / `--branch` | `-b` / `--branch` (jj 0.17+) | `-b` / `--branch` |
| Single branch | `--single-branch` | N/A | `--single-branch` |
| Bare clone | `--bare` | N/A | `--bare` |
| Shallow clone (depth) | `--depth <n>` | N/A | `--depth <n>` |
| Shallow since date | `--shallow-since=<date>` | N/A | N/A |
| Shallow exclude | `--shallow-exclude=<rev>` | N/A | N/A |
| Mirror clone | `--mirror` | N/A | N/A |
| Reference repository | `--reference <repo>` | N/A | N/A |
| Dissociate from reference | `--dissociate` | N/A | N/A |
| No hardlinks | `--no-hardlinks` | N/A | N/A |
| Recurse submodules | `--recurse-submodules` | N/A | N/A (no submodules) |
| Shallow submodules | `--shallow-submodules` | N/A | N/A |
| Separate git dir | `--separate-git-dir=<dir>` | N/A | N/A (removed) |
| Template directory | `--template=<dir>` | N/A | N/A (handled by init internally) |
| Quiet mode | `-q` / `--quiet` | `--quiet` | `--quiet` (global flag) |
| Verbose / progress | `--progress` / `--verbose` | N/A | Phased stderr progress (default) |
| No checkout | `-n` / `--no-checkout` | N/A | N/A (bare implies no checkout) |
| Sparse checkout | `--sparse` | N/A | N/A |
| Filter (partial clone) | `--filter=<spec>` | N/A | N/A |
| Bundle URI | `--bundle-uri=<uri>` | N/A | N/A |
| Vault signing bootstrap | N/A | N/A | Always enabled (matches init) |
| SSH key detection | N/A | N/A | Automatic detection + hint |
| Structured JSON output | N/A | N/A | `--json` / `--machine` |
| Error hints | Minimal messages | Minimal messages | Every error type has an actionable hint |

## Error Handling

Every `CloneError` variant maps to an explicit `StableErrorCode` -- no message substring inference.

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

Init errors are transparently forwarded through `InitError -> CliError`.

### Cleanup Failure Visibility

When clone fails, `cleanup_failed_clone()` attempts to remove the partially created directory.
If cleanup itself fails, the warning is attached to the error via `with_priority_hint()` so it
surfaces in both human and JSON error output instead of being silently swallowed.

### Non-Bare Checkout Is Required For Success

`setup_repository()` uses `execute_checked_typed()` which returns typed `RestoreError` variants.
If checkout fails, the clone reports failure -- it does not silently succeed with a broken worktree.

## Vault And Identity

- Clone always initializes with `vault: true`, matching `libra init` defaults
- `vault_signing` and `ssh_key_detected` from init are transparently forwarded to `CloneOutput`
- SSH key detection uses the isolated `HOME` from the init phase

## Compatibility Notes

- `--recurse-submodules` is not supported; Libra does not implement submodules
- `--mirror` and `--reference` are not supported
- Clone always bootstraps vault signing; use `libra config` to disable after cloning if needed
- The `--depth` value must be a positive integer; zero or negative values are rejected at parse time
- `--no-checkout` is not available as a separate flag; use `--bare` for repositories without a working tree
