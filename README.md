## Libra

`Libra` is a partial implementation of a **Git** client, developed using **Rust**. Our goal is not to create a 100% replica of Git (for those interested in such a project, please refer to the [gitoxide](https://github.com/Byron/gitoxide)). Instead, Libra is evolving into an **AI agent–native version control system**. The new `libra code` command starts an interactive TUI (with a background web server) that is designed to be driven by AI agents and humans collaboratively.

## Example
```
$ libra --help
Simulates git commands

Usage: libra <COMMAND>

Commands:
  init     Initialize a new repository
  clone    Clone a repository into a new directory
  code     Start Libra Code interactive TUI (with background web server)
  add      Add file contents to the index
  rm       Remove files from the working tree and from the index
  restore  Restore working tree files
  status   Show the working tree status
  log      Show commit logs
  diff    Show changes between commits, commit and working tree, etc
  branch   List, create, or delete branches
  commit   Record changes to the repository
  switch   Switch branches
  merge    Merge changes
  push     Update remote refs along with associated objects
  fetch    Download objects and refs from another repository
  pull     Fetch from and integrate with another repository or a local branch
  remote   Manage set of tracked repositories
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
## Features

### Clean Code
Our code is designed to be clean and easy to read, 
ensuring that it is both maintainable and understandable for developers of all skill levels.

### Cross-Platform
- [x] Windows
- [x] Linux
- [x] MacOS

### Compatibility with Git
Our implementation is essentially fully compatible with `Git` 
(developed with reference to the `Git` documentation), 
including formats such as `objects`, `index`, `pack`, and `pack-index`. 
Therefore, it can interact seamlessly with `Git` servers (like `push` and `pull`).

### Differences from Git:
While maintaining compatibility with `Git`, we have made some innovations and changes:
we use an `SQLite` database to manage loosely structured files such as `config`, `HEAD`, and `refs`, 
achieving unified management.

## Worktree Management

Libra implements a `worktree` subcommand that is broadly compatible with `git worktree` and allows you to manage multiple working directories attached to the same repository storage (note that, unlike `git worktree remove`, Libra does not delete worktree directories on disk by default):

- `libra worktree add <path>` - create a new linked working tree at `<path>`
- `libra worktree list` - list all registered working trees (including the main worktree)
- `libra worktree lock <path> [--reason <msg>]` - mark a worktree as locked with an optional reason
- `libra worktree unlock <path>` - unlock a previously locked worktree
- `libra worktree move <src> <dest>` - move a worktree directory to a new location
- `libra worktree prune` - prune missing/non-existent worktrees from the registry
- `libra worktree remove <path>` - remove a worktree from the registry without deleting its directory on disk (the main worktree cannot be removed)
- `libra worktree repair` - repair inconsistent worktree state if the registry and directories get out of sync

## Object Storage Configuration

Libra supports using S3-compatible object storage (AWS S3, Cloudflare R2, MinIO, etc.) as an alternative or supplement to local storage. This feature implements a **tiered storage architecture**:

- **Small objects** (< threshold): Stored in both local and remote storage
- **Large objects** (≥ threshold): Stored in remote storage with local LRU cache

### Environment Variables

Configure object storage by setting these environment variables:

| Variable | Description | Required | Default |
|----------|-------------|----------|---------|
| `LIBRA_STORAGE_TYPE` | Storage backend type: `s3` or `r2` | Yes (for S3/R2) | - |
| `LIBRA_STORAGE_BUCKET` | Bucket name | Yes (for S3/R2) | `libra` |
| `LIBRA_STORAGE_ENDPOINT` | S3-compatible endpoint URL (required for R2) | Yes (for R2) | (AWS S3 default) |
| `LIBRA_STORAGE_REGION` | Region for bucket | No | `auto` |
| `LIBRA_STORAGE_ACCESS_KEY` | Access key ID | Yes (for S3/R2) | - |
| `LIBRA_STORAGE_SECRET_KEY` | Secret access key | Yes (for S3/R2) | - |
| `LIBRA_STORAGE_THRESHOLD` | Size threshold in bytes for tiering | No | `1048576` (1MB) |
| `LIBRA_STORAGE_CACHE_SIZE` | Local cache size limit in bytes | No | `209715200` (200MB) |
| `LIBRA_STORAGE_ALLOW_HTTP` | Allow HTTP (not HTTPS) connections | No | `false` |

### Usage Examples

#### Local Storage Only (Default)

```bash
# Simply don't set LIBRA_STORAGE_TYPE
# Or explicitly unset it if previously set
unset LIBRA_STORAGE_TYPE

libra init my-repo
cd my-repo
# Objects will be stored locally in .git/objects
```

#### AWS S3

```bash
export LIBRA_STORAGE_TYPE=s3
export LIBRA_STORAGE_BUCKET=my-libra-bucket
export LIBRA_STORAGE_REGION=us-west-2
export LIBRA_STORAGE_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE
export LIBRA_STORAGE_SECRET_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

libra init my-repo
cd my-repo
# Objects will now be stored in S3
```

#### Cloudflare R2

```bash
export LIBRA_STORAGE_TYPE=r2
export LIBRA_STORAGE_BUCKET=my-libra-bucket
export LIBRA_STORAGE_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
export LIBRA_STORAGE_REGION=auto
export LIBRA_STORAGE_ACCESS_KEY=<r2-access-key>
export LIBRA_STORAGE_SECRET_KEY=<r2-secret-key>

libra clone <remote-url>
```

#### MinIO (Local Testing)

```bash
export LIBRA_STORAGE_TYPE=s3
export LIBRA_STORAGE_BUCKET=libra-test
export LIBRA_STORAGE_ENDPOINT=http://localhost:9000
export LIBRA_STORAGE_ACCESS_KEY=minioadmin
export LIBRA_STORAGE_SECRET_KEY=minioadmin
export LIBRA_STORAGE_ALLOW_HTTP=true

libra init test-repo
```

#### Custom Thresholds

```bash
# Store objects > 512KB in remote storage
export LIBRA_STORAGE_THRESHOLD=524288

# Limit local cache to 500MB
export LIBRA_STORAGE_CACHE_SIZE=524288000
```

### Behavior

1. **Without object storage configuration**: Libra uses local storage only (default behavior)
2. **With object storage configuration**: 
   - Objects smaller than threshold are stored locally and remotely (permanent)
   - Objects larger than threshold are stored remotely with local LRU cache
   - Cache eviction happens when local storage exceeds the cache size limit

### Notes

- Bucket will be created automatically if it doesn't exist (requires appropriate permissions)
- Invalid configuration falls back to local storage with warning messages
- Object paths follow Git's standard structure: `<first-2-chars>/<remaining-chars>`

## CLI Compatibility with Git

This section documents the compatibility between **Libra**’s CLI and **Git** at the level of commands and options, and serves as a roadmap for closing gaps.

### Legend

- **Status**
  - ✅ Implemented and broadly compatible with Git semantics
  - ⚠️ Implemented but behavior may differ from Git, or Libra-specific extension (Git has no direct equivalent)
  - ⛔ Not implemented yet
- **Priority** (for ⛔ items)
  - **P0** – High priority: very common in everyday Git workflows, or important for safety
  - **P1** – Medium priority: advanced workflows or scripting/tooling heavy usage
  - **P2** – Low priority: niche or rarely used options

> Note: The tables below are based on the existing command/flag checklist in this README plus common Git options. If in doubt about exact semantic equality, we err on the conservative side and mark as ⚠️.

---

### Repository Setup: `init`, `clone`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `init` | `<repo_directory>` | Yes | Yes | ✅ | - | Basic repository directory |
|  | `--bare` | Yes | Yes | ✅ | - | Initialize bare repository |
|  | `--template <path>` | Yes | Yes | ✅ | - | Use template directory |
|  | `-b, --initial-branch <name>` | Yes | Yes | ✅ | - | Set initial branch name |
|  | `-q, --quiet` | Yes | Yes | ✅ | - | Suppress output |
|  | `--shared <perm>` | Yes | Yes | ⚠️ | P1 | Supported, but effective semantics may differ from Git in edge cases |
|  | `--separate-git-dir <dir>` | Yes | Yes | ✅ | - | Separate storage directory (Libra: `--separate-libra-dir` is the preferred option name; `--separate-git-dir` is an alias); semantics may differ slightly from Git |
|  | `--object-format <alg>` | Yes | Yes | ✅ | - | Important for SHA‑1/SHA‑256 migration/compatibility |
| `clone` | `<remote_repo>` | Yes | Yes | ✅ | - | Repository URL/path |
|  | `[local_path]` | Yes | Yes | ✅ | - | Target directory |
|  | `-b, --branch <name>` | Yes | Yes | ✅ | - | Check out given branch |
|  | `--depth <n>` | Yes | Yes | ✅ | - | Shallow clone, widely used in CI and large repos |
|  | `--single-branch` | Yes | Yes | ✅ | - | Clone only the specified branch |
|  | `--recurse-submodules` | Yes | No | ⛔ | P1 | Requires submodule support; important in monorepos |
|  | `--bare` | Yes | Yes | ✅ | - | Bare clone for server‑side usage |
|  | `--mirror` | Yes | No | ⛔ | P1 | Full mirror including refs, for replication scenarios |

---

### Working Tree & Index: `add`, `rm`, `restore`, `status`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `add` | `<pathspec...>` | Yes | Yes | ✅ | - | Add files matching pathspec |
|  | `-A, --all` | Yes | Yes | ✅ | - | Add all changes (tracked + untracked) |
|  | `-u, --update` | Yes | Yes | ✅ | - | Add only tracked changes |
|  | `--refresh` | Yes | Yes | ✅ | - | Refresh the index stat info |
|  | `-v, --verbose` | Yes | Yes | ✅ | - | Verbose output |
|  | `-n, --dry-run` | Yes | Yes | ✅ | - | Show what would be added |
|  | `--ignore-errors` | Yes | Yes | ✅ | - | Continue even if some paths fail |
|  | `-f, --force` | Yes | Yes | ✅ | - | Add ignored files |
|  | `-p, --patch` | Yes | No | ⛔ | P0 | Interactive hunk selection; core to many workflows |
|  | `-i, --interactive` | Yes | No | ⛔ | P0 | Interactive mode (`git add -i`) |
|  | `-N, --intent-to-add` | Yes | No | ⛔ | P1 | Mark paths as “to be added” later |
|  | `--chmod=(+x\|-x)` | Yes | No | ⛔ | P1 | Toggle executable bit |
|  | `--renormalize` | Yes | No | ⛔ | P2 | Re‑normalize line endings / attributes |
| `rm` | `<pathspec...>` | Yes | Yes | ✅ | - | Remove files |
|  | `--cached` | Yes | Yes | ✅ | - | Remove only from index |
|  | `-r, --recursive` | Yes | Yes | ✅ | - | Recurse into directories |
|  | `-f, --force` | Yes | Yes | ✅ | - | Force removal |
|  | `--dry-run` | Yes | Yes | ✅ | - | Show what would be removed |
|  | `--ignore-unmatch` | Yes | Yes | ✅ | - | Don’t error if paths don’t match; important for scripts |
|  | `--pathspec-from-file <file>` | Yes | Yes | ✅ | - | Read pathspecs from file |
|  | `--pathspec-file-nul` | Yes | Yes | ✅ | - | NUL‑separated pathspec file |
| `restore` | `<pathspec...>` | Yes | Yes | ✅ | - | Restore paths |
|  | `-s, --source <commit>` | Yes | Yes | ✅ | - | Restore from specific commit |
|  | `-W, --worktree` | Yes | Yes | ✅ | - | Restore working tree only |
|  | `-S, --staged` | Yes | Yes | ✅ | - | Restore index (staged) state |
| `status` | `--porcelain` | Yes | Yes | ✅ | - | Machine‑readable output |
|  | `-s, --short` | Yes | Yes | ✅ | - | Short format |
|  | `--branch` | Yes | Yes | ✅ | - | Show branch info |
|  | `--ignored` | Yes | Yes | ✅ | - | Show ignored files |
|  | `--untracked-files[=no\|normal\|all]` | Yes | Yes | ✅ | - | Control visibility of untracked files |
|  | `--show-stash` | No | Yes | ⚠️ | P1 | Libra extension; only in standard mode |

---

### Commit & History: `commit`, `log`, `tag`, `show`, `reflog`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `commit` | `-m, --message <msg>` | Yes | Yes | ✅ | - | Commit message |
|  | `-F, --file <path>` | Yes | Yes | ✅ | - | Read message from file |
|  | `--allow-empty` | Yes | Yes | ✅ | - | Allow empty commit |
|  | `--conventional` | No | Yes | ⚠️ | P1 | Libra extension for conventional commits |
|  | `--amend` | Yes | Yes | ✅ | - | Amend previous commit |
|  | `-s, --signoff` | Yes | Yes | ✅ | - | Add Signed-off-by |
|  | `--disable-pre` | Approx. `--no-verify` | Yes | ⚠️ | P0 | Behavior should be aligned with Git hook semantics as much as possible |
|  | `-a, --all` | Yes | Yes | ✅ | - | Auto‑stage tracked changes |
|  | `-p, --patch` | Yes | No | ⛔ | P1 | Patch‑mode commit (often paired with `add -p`) |
|  | `--no-verify` | Yes | Yes | ✅ | - | Standard way to skip hooks; coexists with `--disable-pre` |
|  | `--no-edit` | Yes | Yes | ✅ | - | Reuse previous message |
|  | `--author <name>` | Yes | Yes | ⚠️ | P0 | Override author identity (expects "Name <email>" format) |
|  | `--date <when>` | Yes | No | ⛔ | P0 | Override author date |
|  | `-S, --gpg-sign` / `--no-gpg-sign` | Yes | No | ⛔ | P1 | GPG signing support |
| `log` | `-n, --number <n>` | Yes | Yes | ✅ | - | Limit number of commits |
|  | `--oneline` | Yes | Yes | ✅ | - | One‑line output |
|  | `-p, --patch` | Yes | Yes | ✅ | - | Show patch |
|  | `--decorate / --no-decorate` | Yes | Yes | ✅ | - | Show/hide ref decorations |
|  | `[pathspec]` | Yes | Yes | ✅ | - | Restrict to paths |
|  | `--graph` | Yes | Yes | ✅ | - | ASCII commit graph |
|  | `--pretty=<format>` | Yes | Yes | ⚠️ | P0 | Customizable formatting; Libra pretty syntax may differ from Git |
|  | `--abbrev-commit` | Yes | YES | ✅ | - | Shorten commit IDs |
|  | `--name-only / --name-status` | Yes | Yes | ✅ | - | Show changed files, with or without status |
|  | `--stat` | Yes | Yes | ✅ | - | Diffstat summary |
|  | `--since <date> / --until <date>` | Yes | Yes | ✅ | - | Time‑based filtering |
|  | `--author <pattern>` | Yes | Yes | ⚠️ | P0 | Author‑based filtering (substring match; not full Git pattern syntax) |
| `tag` | `<name>` | Yes | Yes | ✅ | - | Lightweight tag |
|  | `-l, --list [pattern]` | Yes | Yes | ✅ | - | List tags |
|  | `-d, --delete <name>` | Yes | Yes | ✅ | - | Delete tags |
|  | `-m, --message <msg>` | Yes | Yes | ✅ | - | Annotated tag message |
|  | `-f, --force` | Yes | Yes | ✅ | - | Force re‑tag |
|  | `-a` | Yes | No | ⛔ | P0 | Explicit annotated tag |
|  | `-s, --sign` | Yes | No | ⛔ | P1 | GPG‑signed tags |
|  | `-u <keyid>` | Yes | No | ⛔ | P1 | Select signing key |
|  | `-n <n>` | Yes | Yes | ✅ | P2 | Show annotation lines |
|  | `-v, --verify` | Yes | No | ⛔ | P1 | Verify tag signatures |
| `show` | (basic usage) | Yes | Yes | ⚠️ | P1 | Core behavior implemented; detailed flag parity needs further audit |
| `reflog` | `show [--pretty=<fmt>]` | Yes | Yes | ⚠️ | P1 | Supported; `--pretty` formatting parity may not be full Git parity |
|  | `delete <selectors...>` | Yes | Yes | ✅ | - | Delete reflog entries |
|  | `exists <ref>` | Yes | Yes | ✅ | - | Check reflog presence |
|  | `expire [--expire=<time>]` | Yes | No | ⛔ | P1 | Cleanup policy for reflogs |

---

### Branching & Checkout: `branch`, `switch`, `checkout`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `branch` | `<new_branch> [commit_hash]` | Yes | Yes | ✅ | - | Create branch from commit or HEAD |
|  | `-D, --delete <branch>` | Yes | Yes | ✅ | - | Force delete branch |
|  | `-d, --delete <branch>` | Yes | Yes | ✅ | - | Safe delete (refuse if unmerged) |
|  | `-u, --set-upstream-to <upstream>` | Yes | Yes | ✅ | - | Set upstream tracking branch |
|  | `--show-current` | Yes | Yes | ✅ | - | Show current branch |
|  | `-m, --move [OLD] [NEW]` | Yes | Yes | ✅ | - | Rename branches |
|  | `-r, --remotes` | Yes | Yes | ✅ | - | List remote branches |
|  | `-a, --all` | Yes | Yes | ✅ | - | List all branches |
|  | `--contains [<commit>]` | Yes | Yes | ✅ | - | Filter branches containing commit (default HEAD) |
|  | `--no-contains [<commit>]` | Yes | Yes | ✅ | - | Filter branches not containing commit (default HEAD) |
|  | `--merged / --no-merged` | Yes | No | ⛔ | P1 | Check merge status into current HEAD |
| `switch` | `<branch>` | Yes | Yes | ✅ | - | Switch to branch |
|  | `-c, --create <new_branch>` | Yes | Yes | ✅ | - | Create and switch |
|  | `-d, --detach` | Yes | Yes | ✅ | - | Detach HEAD |
|  | `-C, --force-create <branch>` | Yes | No | ⛔ | P1 | Force re‑create branch |
|  | `--guess / --no-guess` | Yes | No | ⛔ | P2 | Heuristic branch name guessing |
|  | `--track` | Yes | Yes | ✅ | - | Auto set upstream when switching to remote branch |
|  | `--merge` | Yes | No | ⛔ | P2 | Merge mode on switch |
|  | `--conflict=<style>` | Yes | No | ⛔ | P2 | Conflict marker style |
| `checkout` | `<branch>` | Yes | Yes | ✅ | - | Checkout existing branch |
|  | `-b <new_branch> [start-point]` | Yes | Yes | ✅ | - | Create and checkout branch |
|  | `-B <new_branch> [start-point]` | Yes | No | ⛔ | P1 | Force re‑create branch at start‑point |
|  | `--detach` | Yes | No | ⛔ | P1 | Detach HEAD at given ref |
|  | `-f, --force` | Yes | No | ⛔ | P1 | Discard local changes when switching |

---

### Advanced History Operations: `merge`, `rebase`, `cherry-pick`, `revert`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `merge` | `<branch>` | Yes | Yes | ✅ | - | Basic merge of given branch |
|  | `--no-ff / --ff-only` | Yes | No | ⛔ | P0 | Control merge topology (no‑ff, fast‑forward only) |
|  | `--squash` | Yes | No | ⛔ | P0 | Squash merges into a single commit |
|  | `--commit / --no-commit` | Yes | No | ⛔ | P1 | Whether to auto‑commit after merge |
|  | `-m, --message <msg>` | Yes | No | ⛔ | P1 | Specify merge commit message |
|  | `--strategy <name>` | Yes | No | ⛔ | P1 | Select merge strategy (e.g. `ort`, `recursive`) |
|  | `--strategy-option <opt>` | Yes | No | ⛔ | P2 | Fine‑tune chosen strategy |
| `rebase` | `<upstream>` | Yes | Yes | ✅ | - | Basic rebase onto upstream |
|  | `-i, --interactive` | Yes | No | ⛔ | P1 | Interactive rebase (edit/reorder/squash) |
|  | `--onto <newbase>` | Yes | No | ⛔ | P1 | Rebase onto different base |
|  | `--autostash` | Yes | No | ⛔ | P1 | Stash/unstash automatically around rebase |
|  | `--continue / --abort / --skip` | Yes | Yes | ✅ | - | Essential state machine for resolving rebase conflicts |
| `cherry-pick` | `<commits...>` | Yes | Yes | ✅ | - | Apply one or more commits |
|  | `-n, --no-commit` | Yes | Yes | ✅ | - | Don’t create commit automatically |
|  | `-x` | Yes | No | ⛔ | P1 | Append “(cherry picked from …)” |
|  | `-e, --edit` | Yes | No | ⛔ | P1 | Edit commit message |
|  | `-m, --mainline <parent>` | Yes | No | ⛔ | P1 | Specify parent for merge commits |
|  | `--continue / --abort / --quit` | Yes | No | ⛔ | P0 | Workflow control after conflicts |
| `revert` | `<commit>` | Yes | Yes | ✅ | - | Revert single commit |
|  | `-n, --no-commit` | Yes | Yes | ✅ | - | Don’t auto‑commit |
|  | `--edit / --no-edit` | Yes | No | ⛔ | P1 | Edit or reuse default message |
|  | `-m, --mainline <parent>` | Yes | No | ⛔ | P1 | Revert merge commits |
|  | `--continue / --abort / --quit` | Yes | No | ⛔ | P0 | Multi‑commit revert / conflict handling |

---

### Remote & Network: `remote`, `push`, `fetch`, `pull`, `lfs`

| Command | Option / Form | Git / Git LFS | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `remote` | `add <name> <url>` | Yes | Yes | ✅ | - | Add remote |
|  | `remove <name>` (Git: `rm`) | Yes | Yes | ✅ | - | Remove remote |
|  | `rename <old> <new>` | Yes | Yes | ✅ | - | Rename remote |
|  | `-v` (list with URLs) | Yes | Yes | ✅ | - | List remotes verbosely |
|  | `show` | Yes | Yes | ✅ | - | Show remote details |
|  | `get-url <name> [--push\|--all]` | Yes | Yes | ✅ | - | Print configured URLs |
|  | `set-url <name> <newurl> [--add] [--delete] [--push] [--all]` | Yes | Yes | ✅ | - | Advanced URL management: supports --add/--delete/--push/--all |
|  | `prune <name>` | Yes | Yes | ✅ | - | Prune stale remote-tracking refs (with optional --dry-run) |
|  | `update [<group>\|<remotes>...]` | Yes | No | ⛔ | P2 | Batch remote updates |
| `push` | `<repository> <refspec>` | Yes | Yes | ✅ | - | Basic push |
|  | `-u, --set-upstream` | Yes | Yes | ✅ | - | Set upstream on push |
|  | `--force` | Yes | Yes | ✅ | - | Force push (use carefully) |
|  | `--force-with-lease` | Yes | No | ⛔ | P0 | Safer force push; strongly recommended over bare `--force` |
|  | `--tags / --all` | Yes | No | ⛔ | P1 | Push all tags / all branches |
|  | `--delete` | Yes | No | ⛔ | P1 | Delete remote ref |
|  | `--dry-run` | Yes | Yes | ✅ | - | Simulate push |
| `fetch` | `[<repository>] [<refspec>]` | Yes | Yes | ✅ | - | Basic fetch |
|  | `-a, --all` | Yes | Yes | ✅ | - | Fetch from all remotes |
|  | `--tags` | Yes | No | ⛔ | P1 | Fetch all tags |
|  | `--prune` | Yes | No | ⛔ | P0 | Prune removed remote branches |
|  | `--force` | Yes | No | ⛔ | P1 | Force update of local refs |
|  | `--depth <n> / --shallow-exclude <ref>` | Yes | No | ⛔ | P0 | Shallow fetch; crucial for large repos |
|  | `--multiple` | Yes | No | ⛔ | P2 | Fetch from multiple remotes |
| `pull` | `<repository> <refspec>` | Yes | Yes | ✅ | - | Fetch and integrate |
|  | `--rebase` | Yes | No | ⛔ | P0 | `pull --rebase` workflow |
|  | `--ff-only / --no-ff` | Yes | No | ⛔ | P0 | Control merge mode on pull |
| `lfs` | `track` / `untrack` | Yes (Git LFS) | Yes | ✅ | - | LFS tracking configuration |
|  | `locks` / `lock` / `unlock` | Yes | Yes | ✅ | - | LFS locking |
|  | `install / uninstall` | Yes | No | ⛔ | P1 | Install LFS filters/hooks |
|  | `fetch / pull / push` | Yes | No | ⛔ | P0 | Transfer LFS objects with remotes |
|  | `ls-files` | Yes | Yes | ✅ | - | List LFS‑tracked files |
|  | `env / version` | Yes | No | ⛔ | P2 | Diagnostic info |

---

### Configuration & Maintenance: `config`, `index-pack`

| Command | Option / Form | Git | Libra | Status | Priority (for ⛔) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `config` | `--add <name> <value>` | Yes | Yes | ✅ | - | Add config entry |
|  | `--get <name>` | Yes | Yes | ✅ | - | Get single value |
|  | `--get-all <name>` | Yes | Yes | ✅ | - | Get all values |
|  | `--unset <name>` | Yes | Yes | ✅ | - | Remove entry |
|  | `--unset-all <name>` | Yes | Yes | ✅ | - | Remove all entries |
|  | `-l, --list` | Yes | Yes | ✅ | - | List config |
|  | `--name-only` | Yes | Yes | ✅ | - | Show names only |
|  | `-d, --default <value>` | Yes | Yes | ✅ | - | Default value if missing |
|  | `--global / --system / --local` | Yes | Yes | ✅ | - | Select config scope (system/global/repo) |
|  | `--file <path>` | Yes | No | ⛔ | P1 | Use explicit config file |
|  | `--replace-all` | Yes | No | ⛔ | P1 | Replace all matching entries |
|  | `--type=<bool\|int\|path>` | Yes | No | ⛔ | P2 | Typed config parsing |
| `index-pack` | `<pack_file>` | Yes | Yes | ✅ | - | Operate on given pack file |
|  | `-o <index_file>` | Yes | Yes | ✅ | - | Output index file path |
|  | `--index-version <n>` | Yes | Yes | ✅ | - | Index format version |
|  | `--stdin` | Yes | No | ⛔ | P2 | Read pack from stdin |
|  | `--fix-thin` | Yes | No | ⛔ | P2 | Fix thin packs |
|  | `--verify` | Yes | No | ⛔ | P1 | Validate pack/index correctness |

---

This section is intended to be kept up to date as new flags and commands are implemented. When implementing a new option:

1. Mark the README checklist for that command as `[x]`.
2. Update the corresponding row here:
  - Change Libra to “Yes”.
  - Update **Status** from ⛔ to ✅ or ⚠️ as appropriate.
  - Clear or adjust the **Priority** field.

## Contributing

### Pre-submission Checks
Before submitting a Pull Request, please ensure your code passes the following checks:

```bash
# Run clippy with all warnings treated as errors (warnings will be treated as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Check code formatting (requires nightly toolchain)
cargo +nightly fmt --all --check
```

Both commands must complete without any warnings. The clippy check treats all warnings as errors, and the formatter check ensures code follows the project style guide. Only PRs that pass these checks will be accepted for merge.


If the formatting check fails, you can automatically fix formatting issues by running:

```bash
cargo +nightly fmt --all
```

### Buck2 Build Requirements

This project builds with Buck2. Please install both Buck2 and cargo-buckal before development:

```bash
# Install buck2: download the latest release tarball from
# https://github.com/facebook/buck2/releases, extract the binary,
# and place it in ~/.cargo/bin (ensure ~/.cargo/bin is on PATH).
# Example (replace <tag> and <platform> with the latest for your OS):
wget https://github.com/facebook/buck2/releases/download/<tag>/buck2-<platform>.tar.gz
tar -xzf buck2-<platform>.tar.gz
mv buck2 ~/.cargo/bin/

# Install cargo-buckal (requires Rust toolchain)
cargo install --git https://github.com/buck2hub/cargo-buckal.git
```

Pull Requests must also pass the Buck2 build:

```bash
cargo buckal build
```

When you update dependencies in Cargo.toml, regenerate Buck metadata and third-party lockfiles:

```bash
cargo buckal migrate
```
