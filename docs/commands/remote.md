# `libra remote`

Manage configured remotes: list, add, remove, rename, inspect and mutate URLs, and prune stale remote-tracking refs.

## Synopsis

```
libra remote <subcommand> [OPTIONS] [ARGS]
libra remote show
libra remote -v
libra remote add <name> <url>
libra remote remove <name>
libra remote rename <old> <new>
libra remote get-url [--push] [--all] <name>
libra remote set-url [--add | --delete] [--push] [--all] <name> <value>
libra remote prune [--dry-run] <name>
```

## Description

`libra remote` manages the set of named remotes stored in the SQLite configuration
database. Each remote has one or more fetch URLs and optionally separate push URLs.
Subcommands allow full CRUD operations on remotes and their URLs, as well as pruning
stale remote-tracking branches that no longer exist on the remote.

Remote configuration is stored as `remote.<name>.url` and `remote.<name>.pushurl` keys
in the SQLite `config` table, rather than in a flat `.git/config` file. This provides
transactional safety (no partial writes on crash) and makes remote metadata queryable
by agents and tooling.

## Options

### Subcommand: `show`

List configured remote names, one per line.

| Argument | Description |
|----------|-------------|
| (none) | Prints all remote names |

### Subcommand: `-v` (list verbose)

List every remote with its fetch and push URLs.

| Argument | Description |
|----------|-------------|
| (none) | Prints `<name>\t<url> (fetch\|push)` for each URL |

### Subcommand: `add`

Register a new remote.

| Argument | Description | Example |
|----------|-------------|---------|
| `<name>` | Logical name for the remote | `origin` |
| `<url>` | Fetch URL for the remote | `https://example.com/repo.git` |

### Subcommand: `remove`

Delete a remote and all its configuration keys.

| Argument | Description | Example |
|----------|-------------|---------|
| `<name>` | Name of the remote to remove | `origin` |

### Subcommand: `rename`

Rename an existing remote.

| Argument | Description | Example |
|----------|-------------|---------|
| `<old>` | Current name | `origin` |
| `<new>` | New name | `upstream` |

### Subcommand: `get-url`

Print URLs configured for a remote.

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<name>` | Remote name | `origin` |
| `--push` | Print push URLs instead of fetch URLs | `libra remote get-url --push origin` |
| `--all` | Print all configured URLs (not just the first) | `libra remote get-url --all origin` |

### Subcommand: `set-url`

Add, replace, or delete URLs for a remote.

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<name>` | Remote name | `origin` |
| `<value>` | URL value (or substring pattern for `--delete`) | `https://mirror.example.com/repo.git` |
| `--add` | Append a new URL rather than replacing | `libra remote set-url --add origin https://mirror.example.com/repo.git` |
| `--delete` | Remove URLs matching the given substring | `libra remote set-url --delete origin mirror` |
| `--push` | Operate on push URLs (`pushurl`) instead of fetch URLs (`url`) | `libra remote set-url --push origin ssh://git@example.com/repo.git` |
| `--all` | Apply replacement to all matching entries | `libra remote set-url --all origin https://new.example.com/repo.git` |

### Subcommand: `prune`

Delete local remote-tracking branches that no longer exist on the remote.

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<name>` | Remote name | `origin` |
| `--dry-run` | Show what would be pruned without deleting | `libra remote prune --dry-run origin` |

## Common Commands

```bash
libra remote show
libra remote -v
libra remote add origin https://example.com/repo.git
libra remote get-url origin
libra remote get-url --all origin
libra remote set-url --add origin https://mirror.example.com/repo.git
libra remote set-url --add --push origin ssh://git@example.com/repo.git
libra remote prune --dry-run origin
```

## Human Output

- `remote show` prints configured remote names, one per line.
- `remote -v` prints every fetch URL and effective push URL:

```text
origin  https://example.com/repo.git (fetch)
origin  ssh://git@example.com/repo.git (push)
```

- `remote add` prints `Added remote 'origin' -> https://example.com/repo.git`
- `remote remove` prints `Removed remote 'origin'`
- `remote rename` prints `Renamed remote 'origin' to 'upstream'`
- `remote get-url` prints the selected URL set, one per line
- `remote set-url` prints a confirmation describing whether a URL was added, replaced, or deleted
- `remote prune` prints each pruned branch and a final summary; `--dry-run` uses `[would prune]`

```text
 * [would prune] origin/stale-feature
 * [would prune] origin/old-experiment

Would prune 2 stale remote-tracking branch(es).
```

## Structured Output (JSON examples)

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- action-specific payloads are tagged with `data.action`

### Action Schemas

- `add`: `name`, `url`
- `remove`: `name`
- `rename`: `old_name`, `new_name`
- `list`: `verbose`, `remotes[]`
- `urls`: `name`, `push`, `all`, `urls[]`
- `set-url`: `name`, `role`, `mode`, `urls[]`, `removed`
- `prune`: `name`, `dry_run`, `stale_branches[]`

Example (verbose list):

```json
{
  "ok": true,
  "command": "remote",
  "data": {
    "action": "list",
    "verbose": true,
    "remotes": [
      {
        "name": "origin",
        "fetch_urls": ["https://example.com/repo.git"],
        "push_urls": ["ssh://git@example.com/repo.git"]
      }
    ]
  }
}
```

Example (prune dry-run):

```json
{
  "ok": true,
  "command": "remote",
  "data": {
    "action": "prune",
    "name": "origin",
    "dry_run": true,
    "stale_branches": [
      {
        "remote_ref": "refs/remotes/origin/stale-feature",
        "branch": "origin/stale-feature"
      }
    ]
  }
}
```

### Schema Notes

- `list.remotes[].fetch_urls` contains all configured fetch URLs
- `list.remotes[].push_urls` contains effective push URLs; when no explicit `pushurl` is configured it falls back to fetch URLs
- `prune.stale_branches[].branch` is the user-facing short name such as `origin/feature`
- `remote show` currently maps to `action = "list"` with `verbose = false`

## Design Rationale

### Why SQLite-backed remote storage?

Git stores remote configuration in the flat-file `.git/config` using INI-style syntax.
This format is easy to hand-edit but has no transactional guarantees: a crash mid-write
can leave the file truncated or corrupt. Libra stores remotes in SQLite (`config` table),
which provides ACID transactions, concurrent-read safety, and structured queries. An
agent can enumerate all remotes with a single SQL query instead of parsing INI syntax.
The trade-off is that remotes are not directly editable with a text editor, but
`libra remote` subcommands and `libra config` provide full programmatic access.

### Why a `show` subcommand?

Git overloads `git remote` (no subcommand) to list remote names and `git remote -v` for
verbose output. Libra makes listing explicit via `remote show` (names only) and
`remote -v` (verbose with URLs). The `show` subcommand provides a clear, discoverable
entry point for agents that need to enumerate remotes without parsing verbose URL output.
It also avoids the ambiguity of a bare command that means different things depending on
flags.

### Why multi-URL support?

A single remote can have multiple fetch URLs and separate push URLs. This enables
mirror-push workflows (push to GitHub and a self-hosted GitLab simultaneously) and
read-from-cache patterns (fetch from a local mirror, push to the canonical remote).
The `set-url --add` and `set-url --delete` flags manage URL lists without requiring
manual config editing. The `get-url --all` flag exposes the full URL set for inspection.
Push URLs (`pushurl`) take precedence when configured; otherwise, fetch URLs are used
for both fetch and push, matching Git's behavior.

## Parameter Comparison: Libra vs Git vs jj

| Operation | Libra | Git | jj |
|-----------|-------|-----|----|
| List names | `libra remote show` | `git remote` | `jj git remote list` |
| List with URLs | `libra remote -v` | `git remote -v` | `jj git remote list` (always verbose) |
| Add remote | `libra remote add <n> <u>` | `git remote add <n> <u>` | `jj git remote add <n> <u>` |
| Remove remote | `libra remote remove <n>` | `git remote remove <n>` | `jj git remote remove <n>` |
| Rename remote | `libra remote rename <o> <n>` | `git remote rename <o> <n>` | `jj git remote rename <o> <n>` |
| Get URL | `libra remote get-url <n>` | `git remote get-url <n>` | N/A |
| Set URL | `libra remote set-url <n> <u>` | `git remote set-url <n> <u>` | N/A |
| Add extra URL | `libra remote set-url --add <n> <u>` | `git remote set-url --add <n> <u>` | N/A |
| Delete URL | `libra remote set-url --delete <n> <p>` | `git remote set-url --delete <n> <p>` | N/A |
| Push-specific URL | `--push` flag on get-url/set-url | `--push` flag on get-url/set-url | N/A |
| Prune stale refs | `libra remote prune <n>` | `git remote prune <n>` | Automatic |
| Prune dry-run | `libra remote prune --dry-run <n>` | `git remote prune --dry-run <n>` | N/A |
| Storage backend | SQLite (transactional) | Flat file (.git/config) | TOML + oplog |
| Structured output | `--json` / `--machine` | No | No |

## Error Handling

| Scenario | StableErrorCode | Exit | Hint |
|----------|-----------------|------|------|
| Duplicate remote name | `LBR-CONFLICT-002` | 128 | "use 'libra remote -v' to inspect configured remotes" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to inspect configured remotes" |
| No URL configured for remote | `LBR-CLI-003` | 129 | "use 'libra remote get-url --all \<name>' to inspect configured URLs" |
| URL pattern not matched (`set-url --delete`) | `LBR-CLI-003` | 129 | "use 'libra remote get-url --all \<name>' to inspect configured URLs" |
| Failed to read remote config | `LBR-IO-001` | 128 | -- |
| Failed to update remote config | `LBR-IO-002` | 128 | -- |
| Failed to list remote-tracking branches | `LBR-IO-001` | 128 | -- |
| Corrupt remote-tracking branch | `LBR-REPO-002` | 128 | -- |
| Failed to prune remote-tracking branch | `LBR-IO-002` | 128 | -- |
| Remote object format mismatch during prune | `LBR-REPO-003` | 128 | "remote uses a different hash algorithm" |
| Remote discovery / auth / network failure during prune | fetch-aligned network/auth codes | 128 | See `libra fetch` error table |
