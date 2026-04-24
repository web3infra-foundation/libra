# Libra Command Reference

This directory contains detailed documentation for all Libra CLI commands. Each document includes a synopsis, option reference, human and structured (JSON) output examples, design rationale, and a parameter comparison with Git and jj.

## Global Flags

Every Libra command accepts the following global flags:

| Flag | Short | Description |
|------|-------|-------------|
| `--json` | `-J` | Output as JSON (formats: `pretty`, `compact`, `ndjson`) |
| `--machine` | | Strict machine mode (implies `--json=ndjson --no-pager --color=never --quiet`) |
| `--no-pager` | | Disable pager (`less`) |
| `--color` | | When to use colors (`auto`, `never`, `always`) |
| `--quiet` | `-q` | Suppress stdout |
| `--exit-code-on-warning` | | Return exit code 9 on warnings |
| `--progress` | | Control progress output (`json`, `text`, `none`, `auto`) |

## Command Index

### Repository Setup

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra init` | | Create a new Libra repository with SQLite-backed metadata, vault signing, and optional Git import | [init.md](init.md) |
| `libra clone` | | Clone a remote repository with vault bootstrapping, shallow clone, and single-branch support | [clone.md](clone.md) |
| `libra config` | `cfg` | Manage repository-local and user-global configuration with vault-backed secret encryption | [config.md](config.md) |

### Staging & Working Tree

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra add` | | Stage file changes from the working tree into the index | [add.md](add.md) |
| `libra rm` | `remove`, `delete` | Remove files from the working tree and/or the index | [rm.md](rm.md) |
| `libra mv` | | Move or rename files, directories, or symlinks | [mv.md](mv.md) |
| `libra restore` | `unstage` | Restore working tree files or unstage changes from the index | [restore.md](restore.md) |
| `libra clean` | | Remove untracked files from the working tree (requires `-n` or `-f`) | [clean.md](clean.md) |
| `libra stash` | | Save and restore temporary changes with push/pop/list/apply/drop subcommands | [stash.md](stash.md) |
| `libra status` | `st` | Show the state of the working tree, staging area, and upstream tracking | [status.md](status.md) |

### Commits & History

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra commit` | `ci` | Record staged changes as a new commit with optional vault signing and conventional format | [commit.md](commit.md) |
| `libra log` | `hist`, `history` | Show commit history with graph, patch, stat, and custom format support | [log.md](log.md) |
| `libra shortlog` | `slog` | Summarize reachable commits grouped by author | [shortlog.md](shortlog.md) |
| `libra show` | | Display a commit, tag, tree, blob, or `REV:path` content | [show.md](show.md) |
| `libra diff` | | Compare differences between HEAD, index, working tree, or two revisions | [diff.md](diff.md) |
| `libra blame` | | Trace each line of a file to its introducing commit | [blame.md](blame.md) |
| `libra describe` | `desc` | Find the nearest reachable tag and format as `tag-N-g<abbrev>` | [describe.md](describe.md) |
| `libra grep` | | Search for patterns in tracked files with regex, revision, and index support | [grep.md](grep.md) |
| `libra reflog` | | View, delete, or check existence of reference change logs | [reflog.md](reflog.md) |
| `libra rev-list` | | List commit objects reachable from a revision | [rev-list.md](rev-list.md) |
| `libra rev-parse` | | Parse revision names, abbreviate refs, and print repository paths | [rev-parse.md](rev-parse.md) |

### Branching & Navigation

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra branch` | `br` | Create, delete, rename, list, and inspect branches | [branch.md](branch.md) |
| `libra tag` | | Create, list, or delete lightweight and annotated tags | [tag.md](tag.md) |
| `libra switch` | `sw` | Switch branches, create new branches, or detach HEAD with fuzzy suggestions | [switch.md](switch.md) |
| `libra checkout` | | Compatibility surface over `switch` + `restore` (hidden) | [checkout.md](checkout.md) |

### History Manipulation

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra reset` | | Move HEAD and optionally reset index or working directory | [reset.md](reset.md) |
| `libra merge` | | Fast-forward merge a branch into the current branch | [merge.md](merge.md) |
| `libra rebase` | `rb` | Reapply commits on top of another base tip with conflict resolution | [rebase.md](rebase.md) |
| `libra cherry-pick` | `cp` | Apply changes from existing commits onto the current branch | [cherry-pick.md](cherry-pick.md) |
| `libra revert` | | Create a new commit that undoes changes from a specified commit | [revert.md](revert.md) |
| `libra bisect` | | Binary search to find the commit that introduced a bug (hidden) | [bisect.md](bisect.md) |

### Remote Operations

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra remote` | | Manage remote repositories: add, remove, rename, inspect URLs, prune stale refs | [remote.md](remote.md) |
| `libra fetch` | | Download objects and update remote-tracking refs from one or all remotes | [fetch.md](fetch.md) |
| `libra push` | | Send local commits and objects to a remote with LFS integration | [push.md](push.md) |
| `libra pull` | | Fetch and fast-forward merge into the current branch | [pull.md](pull.md) |
| `libra open` | | Open the repository's remote URL in the system browser | [open.md](open.md) |
| `libra lfs` | | Manage Large File Storage: track, lock, unlock, list LFS files | [lfs.md](lfs.md) |

### Cloud & Storage

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra cloud` | | Cloud backup and restore operations via Cloudflare D1/R2 | [cloud.md](cloud.md) |
| `libra worktree` | `wt` | Manage multiple working trees attached to the repository | [worktree.md](worktree.md) |

### AI & Development

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra code` | | Interactive TUI with AI agent, web server, and MCP integration | [code.md](code.md) |

### Low-Level & Inspection

| Command | Alias | Description | Doc |
|---------|-------|-------------|-----|
| `libra cat-file` | | Inspect Git objects and AI objects by type, size, or pretty-printed content | [cat-file.md](cat-file.md) |
| `libra show-ref` | | List local refs (branches, tags, HEAD) and their object IDs | [show-ref.md](show-ref.md) |
| `libra index-pack` | | Build a `.idx` pack index file for an existing `.pack` archive (hidden) | [index-pack.md](index-pack.md) |

## Structured Output Envelope

All commands that support `--json` / `--machine` return a consistent JSON envelope:

```json
{
  "ok": true,
  "command": "<command-name>",
  "data": { ... }
}
```

On error:

```json
{
  "ok": false,
  "command": "<command-name>",
  "error": {
    "code": "LBR-XXX-NNN",
    "message": "Human-readable error description",
    "hint": "Suggested fix or next step"
  }
}
```

## Error Code Namespaces

| Prefix | Domain |
|--------|--------|
| `LBR-REPO-*` | Repository state errors (not a repo, corrupt objects, missing refs) |
| `LBR-CLI-*` | CLI argument validation errors (invalid flags, missing required args) |
| `LBR-NET-*` | Network and transport errors (auth failure, timeout, DNS) |
| `LBR-FS-*` | Filesystem errors (permission denied, disk full, path encoding) |
| `LBR-IDX-*` | Index/staging area errors (corrupt index, lock contention) |
| `LBR-OBJ-*` | Object storage errors (missing object, hash mismatch) |
| `LBR-VAULT-*` | Vault and encryption errors (unseal failure, key generation) |

## Design Philosophy

Libra's command-line interface is designed with these principles:

1. **Git compatibility where it makes sense** — Most commands mirror Git's flag names and behavior so existing muscle memory transfers directly.
2. **Structured output as a first-class citizen** — Every command supports `--json` and `--machine` for CI/CD pipelines and AI agent consumption.
3. **SQLite over flat files** — Refs, config, and metadata are stored in SQLite for transactional consistency and atomic updates.
4. **Security by default** — Vault-backed signing and secret encryption are enabled by default, not opt-in.
5. **Explicit over implicit** — Commands like `clean` require `-f` or `-n`; `status --exit-code` is an explicit opt-in rather than Git's ambiguous exit code behavior.
6. **Actionable errors** — Every error includes a stable code (`LBR-*`), a human-readable message, and a hint for resolution.
7. **AI-native development** — The `libra code` command integrates AI agents directly into the version control workflow with multi-provider support and MCP protocol.
8. **Cloud-native storage** — Built-in tiered storage (S3/R2) and cloud backup (D1/R2) for distributed monorepo workflows.
