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

## Capability Matrix and Decision Ledger

Libra keeps its transactional SQLite metadata (`.libra/libra.db`), vault-backed secrets, and
tiered object storage as the authoritative implementation, while matching the protocol-layer
behavior of common `git clone` flags. **No SQL schema migration is required for clone core**
(metadata stays in SQLite; objects in tiered storage; partial-clone promisor state is recorded
in `config_kv`). The authoritative compatibility levels live in
[`COMPATIBILITY.md`](../../COMPATIBILITY.md); this table summarizes the decisions:

| Capability | Flag | Libra level | Notes |
|---|---|---|---|
| Shallow by count | `--depth` | supported | Reuses `.libra/shallow` boundary + deepen negotiation |
| Shallow by date | `--shallow-since` | supported | `deepen-since`; supersedes plain depth when combined |
| Shallow by ref | `--shallow-exclude` | supported | `deepen-not`; supersedes plain depth when combined |
| Reject shallow source | `--reject-shallow` | supported | Fails (128) on a shallow source |
| All / single branch | `--single-branch` / `--no-single-branch` | supported | Git-style negation, last wins |
| Custom remote name | `-o/--origin` | supported | Names the tracked remote |
| Skip checkout | `-n/--no-checkout` | supported | Metadata only, no working tree |
| Mirror | `--mirror` | partial | Implies bare; writes `+refs/*:refs/*` + `mirror = true`; clones branch heads (tags/exact-`refs/*` mirroring not yet implemented) |
| Reference reuse | `--reference` / `--reference-if-able` | intentionally-different | Copy semantics (no `info/alternates` borrow) |
| Dissociate | `--dissociate` | intentionally-different | Confirms fully-local (copy semantics) |
| Local optimization | `-l/--local` / `--no-hardlinks` | supported | Hardlink (or copy) a local source's objects |
| Shared objects | `-s/--shared` | intentionally-different | Copy semantics, no alternates |
| Parallel jobs | `-j/--jobs` | intentionally-different | Validated 1..=16, reserved/no-op (serial transport) |
| Partial clone | `--filter` | partial | Whitelist specs; promisor config; no lazy backfill |
| Sparse checkout | `--sparse` | declined | See [declined.md#d10](../improvement/compatibility/declined.md#d10-clone---sparse-与顶层-sparse-checkout-命令) |
| Submodules | `--recurse-submodules` | declined | See [declined.md#d4](../improvement/compatibility/declined.md#d4-clone---recurse-submodules) |

**Cloud clones** (`libra+cloud://`) restore a complete published object set from Cloudflare D1/R2
and **fail-fast (exit 129) on every Git shaping flag** above (use `?ref=<branch|tag|full-ref>` in
the URL to select a checkout target) before any clone-domain config lookup or directory creation.
New `StableErrorCode` variants (if any) are recorded in
[`docs/error-codes.md`](../error-codes.md).

## Options

### `<REMOTE_REPO>` (required)

The remote repository URL to clone from. Supports SSH (`git@host:user/repo.git`) and
HTTPS (`https://host/user/repo.git`) protocols, as well as local filesystem paths.
`libra+cloud://` publish sources are recognized and strictly validated. The clone
domain must be configured locally before restore starts; otherwise Libra returns
`LBR-AUTH-001` and does not create the destination directory. Configured cloud
sources resolve the D1 site, repository row, published refs, selected/default
revision, object index, and R2 object availability before creating the target
directory. Restore then initializes a local Libra repo, downloads indexed Git
objects from R2, restores refs metadata, writes origin cloud config, and checks
out the selected/default revision. Cloud sources never fall through to generic
Git discovery.

```bash
libra clone git@github.com:user/repo.git
libra clone https://github.com/user/repo.git
libra clone /path/to/local/repo
libra clone libra+cloud://code.example.com/kepler-ledger
libra clone libra+cloud://code.example.com/repo/rp_8f4c1b
libra clone "libra+cloud://code.example.com/kepler-ledger?ref=refs/tags/v1.0.0"
libra clone "libra+cloud://code.example.com/kepler-ledger?revision=latest"
```

For `libra+cloud://`, the authority is the configured clone domain. The path must be
either `/<slug>` or `/repo/<repo_id>`. Only one selector is allowed: `?ref=<branch|tag|full-ref>`
or `?revision=<oid|latest>`.
The first Cloudflare restore surface does not accept Git transport shaping flags:
`--branch`, `--depth`, `--single-branch`, and `--bare` return `LBR-CLI-002`
before clone-domain config lookup and before creating the destination directory.
Use `?ref=<branch|tag|full-ref>` on the source URL to select a checkout target.

Required clone-domain config keys:

```text
cloud.clone_domains.<domain>.account_id
cloud.clone_domains.<domain>.d1_database_id
cloud.clone_domains.<domain>.r2_bucket
```

Cloud site resolution also requires `LIBRA_D1_API_TOKEN`; Libra reads
`vault.env.LIBRA_D1_API_TOKEN` first, then the exported environment variable, so
the CLI can query the configured D1 database before starting restore.

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
For `libra+cloud://` sources, use `?ref=<branch|tag|full-ref>` in the URL instead;
`--branch` is rejected before restore starts.

```bash
libra clone -b develop git@github.com:user/repo.git
```

### `--single-branch`

Fetch only the history leading to the tip of a single branch (HEAD, or the branch given
by `-b`). Reduces transfer size for large repositories when only one branch is needed.
Only Git remotes support this transport optimization; `libra+cloud://` restore rejects it
because the restored local repository must preserve all published refs.

```bash
libra clone --single-branch -b main git@github.com:user/repo.git
```

### `--no-single-branch`

The opposite of `--single-branch`; clone all branches. This is a Git-style negation: when
both `--single-branch` and `--no-single-branch` appear on the command line, the last one
wins (handled natively by clap's `overrides_with`). It is **not** a usage conflict, so
combining them does not return an error.

```bash
libra clone --no-single-branch git@github.com:user/repo.git
```

### `--bare`

Create a bare repository without a working tree. The destination directory becomes the
object store directly. Useful for central/server-side repositories.
Bare Cloudflare restores are not part of the first restore surface; `libra+cloud://`
currently rejects `--bare` explicitly.

```bash
libra clone --bare git@github.com:user/repo.git
```

### `--depth <N>`

Create a shallow clone with history truncated to the specified number of commits.
`N` must be a positive integer.
Only Git remotes support shallow transfer. Cloudflare restore rejects `--depth`
because it must download the complete published object set.

```bash
libra clone --depth 1 git@github.com:user/repo.git
libra clone --depth 50 git@github.com:user/repo.git
```

### `--shallow-since <time>`

Create a shallow clone with history limited to commits newer than `<time>`. Accepts a date
(`2024-01-01`), an RFC3339 timestamp, a Unix epoch, or a relative form like `2 weeks ago`.
A malformed time is rejected up front with `LBR-CLI-002` (exit 129) before any network
access. May be combined with `--depth`; because `git-upload-pack` rejects sending both a
plain `deepen` and a `deepen-since`/`deepen-not` request together, the time/ref request
supersedes plain depth at the protocol layer. Only Git remotes support shallow transfer;
`libra+cloud://` restore rejects it.

```bash
libra clone --shallow-since 2024-01-01 git@github.com:user/repo.git
```

### `--shallow-exclude <revision>`

Create a shallow clone that excludes history reachable from the given ref or revision
(`deepen-not`). May be **repeated** to exclude multiple refs (one `deepen-not` frame per
value) and combined with `--depth` (the exclude request supersedes plain depth, as with
`--shallow-since`). Only Git remotes support it; `libra+cloud://` restore rejects it.

```bash
libra clone --shallow-exclude refs/tags/v1.0.0 git@github.com:user/repo.git
```

### `--reject-shallow`

Fail (exit 128) if the source repository is itself a shallow repository, rather than
producing a shallow clone of a shallow source. Local sources are detected before any
directory is created; remote sources are detected from the shallow boundaries advertised
during fetch.

```bash
libra clone --reject-shallow git@github.com:user/repo.git
```

### `-o, --origin <name>`

Use `<name>` instead of `origin` for the tracked remote. The remote URL, fetch refspec, and
`branch.<branch>.remote` configuration are all recorded under this name. `libra+cloud://`
restore rejects it.

```bash
libra clone -o upstream git@github.com:user/repo.git
```

### `-n, --no-checkout`

Do not check out HEAD after the clone. Metadata, refs, and config are still written; only
the working-tree checkout (and the `.gitignore` → `.libraignore` conversion that depends on
it) is skipped. `libra+cloud://` restore rejects it.

```bash
libra clone --no-checkout git@github.com:user/repo.git
```

### `--mirror`

Set up a mirror of the source repository. Implies `--bare`, records the mirror refspec
(`+refs/*:refs/*`) and `remote.<name>.mirror = true`, and clones all branch heads.
**Known limitation (partial):** branch refs are stored as remote-tracking
(`refs/remotes/<name>/*`) and tags/other ref namespaces are not yet mirrored at their exact
`refs/*` names, so this is not yet a full Git-style mirror. `libra+cloud://` restore rejects it.

```bash
libra clone --mirror git@github.com:user/repo.git
```

### `--reference <repo>` / `--reference-if-able <repo>`

Reuse objects from a **local** reference repository to reduce work. **Intentionally different
from Git**: because Libra's object reader has no `info/alternates` fallback, these flags use
**copy semantics** — the reference's objects are copied into the new clone's tiered storage and
the clone carries no long-term alternates dependency (no `info/alternates` is written). The source
must be a real (non-symlink) local libra or git repository; a symlinked source is rejected with
exit 128, and the path is length-capped at 4 KiB. `--reference-if-able` degrades to a normal clone
with a warning when the path does not exist, whereas `--reference` fails. `libra+cloud://` rejects both.

```bash
libra clone --reference /srv/mirror/repo git@github.com:user/repo.git
libra clone --reference-if-able /srv/mirror/repo git@github.com:user/repo.git
```

### `--dissociate`

Ensure the clone has no borrow dependency on the reference. With the default copy semantics the
objects are already fully local, so this confirms that state (reported as `dissociated = true` in
JSON) — it never leaves a dangling alternates reference. Requires `--reference` or
`--reference-if-able`; using it alone is a usage error (exit 129).

```bash
libra clone --dissociate --reference /srv/mirror/repo git@github.com:user/repo.git
```

### `-l, --local` / `--no-hardlinks`

When cloning from a **local** repository, reuse its objects directly instead of re-transferring
them: `--local` hardlinks the source's loose objects and pack files into the new clone (sharing
inodes), falling back to a copy across filesystems or when `--no-hardlinks` is given. Symlinked
object sources are rejected (exit 128). If the source is not a local repository, the flag is
ignored with a warning.

```bash
libra clone -l /srv/repos/project.git my-project
libra clone -l --no-hardlinks /srv/repos/project.git my-project
```

### `-s, --shared`

Reuse a local source repository's objects via **copy semantics** (no `info/alternates` borrow,
same as `--reference`/`--shared` elsewhere in Libra). Intentionally different from Git's
alternates-sharing because Libra's object reader has no alternates fallback.

```bash
libra clone -s /srv/repos/project.git my-project
```

### `-j, --jobs <n>`

**Libra extension (reserved).** Validated to the range 1..=16 (0 or >16 exit 129) and retained,
but currently a no-op — Libra's transport is serial and there is no downstream consumer. Git's
`clone --jobs` controls submodule fetches, which Libra does not support, so the name is reserved
for a future transport-concurrency cap.

```bash
libra clone --jobs 4 git@github.com:user/repo.git
```

### `--filter <spec>`

Partial clone: ask the remote to omit objects matching `<spec>` to reduce transfer. Supported
specs (whitelist; unknown specs exit 129, over-long specs are 4 KiB-capped): `blob:none`,
`blob:limit=<n>[kmg]`, and `tree:<depth>`. The clone records promisor config
(`remote.<name>.promisor = true`, `remote.<name>.partialclonefilter = <spec>`) but does **not**
lazily backfill missing objects. Because a non-bare default checkout needs blob contents, pair
`--filter` with `--no-checkout` or `--bare`; otherwise the checkout fails with a clear
partial-clone diagnostic (exit 128) when it hits a filtered-out blob. Requires the server to
allow filtering (`uploadpack.allowFilter`). `libra+cloud://` rejects it.

```bash
libra clone --filter=blob:none --no-checkout git@github.com:user/repo.git
libra clone --filter=tree:0 --bare git@github.com:user/repo.git
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
- `source_kind` and `cloud_site` are omitted for ordinary Git/local clones; `libra+cloud://` clones add them with clone domain, site id, slug, repo id, selected ref, and restored revision
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
(must be a positive integer) and propagated to the fetch protocol layer. Libra additionally
supports date-based `--shallow-since` (`deepen-since`), ref-based `--shallow-exclude`
(`deepen-not`), and `--reject-shallow` to refuse a shallow source. `--depth` may be combined
with `--shallow-since`/`--shallow-exclude`; because `git-upload-pack` rejects sending a plain
`deepen` alongside `deepen-since`/`deepen-not`, the time/ref request supersedes plain depth at
the protocol layer. After a shallow clone, `libra fetch --unshallow` restores complete history
and `libra fetch --deepen N` extends it further.

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

### Metadata write and credential redaction

Clone metadata is written atomically: the branch ref, `HEAD`, and the
`branch.<branch>.merge` / `branch.<branch>.remote` / `remote.<name>.url` /
`remote.<name>.fetch` config entries are all written inside one transaction, so a failure
rolls every entry back (no half-configured repository). An empty remote writes only
`remote.<name>.url` and `remote.<name>.fetch` (no synthetic branch tracking). The fetch
refspec is `+refs/heads/*:refs/remotes/<name>/*` for an ordinary clone and `+refs/*:refs/*`
for `--mirror` (which also records `remote.<name>.mirror = true`).

Credentials embedded in a clone URL (an HTTP(S) token or password) are redacted from every
output and persistence surface — the "Connecting to …" line, the stored `remote.<name>.url`,
the reflog `clone: from <url>` entry, the JSON `remote_url`, and error messages. SSH-style
`git@host` user prefixes are conventional and preserved. The raw URL is used only for the
live transport.

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
