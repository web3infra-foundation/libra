# `libra fetch`

Download objects and update remote-tracking refs from another repository.

## Synopsis

```
libra fetch [OPTIONS] [<repository> [<refspec>]]
```

## Description

`libra fetch` contacts a remote repository, negotiates which objects the local store is
missing, downloads them as a pack file, indexes the pack, and updates the corresponding
remote-tracking refs (e.g. `refs/remotes/origin/main`). It never modifies the working
tree or the current branch -- use `libra pull` or `libra merge` for that.

When invoked with no arguments, it fetches from the current branch's configured upstream.
When `--all` is given, every configured remote is fetched in sequence. When a specific
`<repository>` is named, only that remote is contacted. An optional `<refspec>` narrows
the fetch to a single branch.

Fetch supports SSH, HTTPS, local file, and `git://` transports. Vault-backed SSH keys
are loaded automatically when configured via `vault.ssh.<remote>.privkey`.

## Options

| Flag / Argument | Description | Example |
|-----------------|-------------|---------|
| `<repository>` | Remote name or URL to fetch from. When omitted, uses the current branch's upstream remote. | `libra fetch origin` |
| `<refspec>` | Branch name to fetch. Requires `<repository>`. When omitted, all branches from the remote are fetched. | `libra fetch origin main` |
| `-a`, `--all` | Fetch from every configured remote. Conflicts with `<repository>`. | `libra fetch --all` |
| `--json` | Emit structured JSON envelope to stdout (global flag). | `libra --json fetch origin` |
| `--machine` | Compact single-line JSON; suppresses progress (global flag). | `libra --machine fetch origin` |
| `--progress none` | Suppress NDJSON progress events on stderr in JSON mode. | `libra --json fetch origin --progress none` |
| `--quiet` | Suppress human-readable output. | `libra fetch --quiet` |

## Common Commands

```bash
libra fetch
libra fetch origin
libra fetch origin main
libra fetch --all
libra --json fetch origin
libra --json fetch origin --progress none
```

## Human Output

Successful human mode prints a compact summary:

```text
From /path/to/remote.git
 * [new ref]         origin/main
 32 objects fetched
```

When nothing changed:

```text
From /path/to/remote.git
Already up to date with 'origin'
```

## Structured Output (JSON examples)

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stdout` is reserved for the final envelope only

### Top-Level Schema

- `all`: whether `--all` was used
- `requested_remote`: explicit remote name, or `null` for `--all`
- `refspec`: requested branch/refspec when provided
- `remotes[]`: per-remote fetch results

### Per-Remote Result Schema

- `remote`: logical remote name
- `url`: normalized remote URL/path
- `refs_updated[]`: updated remote-tracking refs
- `objects_fetched`: object count parsed from the received pack

### Refs Updated Schema

- `remote_ref`: fully qualified local remote-tracking ref, e.g. `refs/remotes/origin/main`
- `old_oid`: previous object id, or `null` when the ref is new
- `new_oid`: fetched object id

Example (single remote):

```json
{
  "ok": true,
  "command": "fetch",
  "data": {
    "all": false,
    "requested_remote": "origin",
    "refspec": null,
    "remotes": [
      {
        "remote": "origin",
        "url": "git@github.com:user/repo.git",
        "refs_updated": [
          {
            "remote_ref": "refs/remotes/origin/main",
            "old_oid": "abc1234...",
            "new_oid": "def5678..."
          }
        ],
        "objects_fetched": 32
      }
    ]
  }
}
```

Example (already up to date):

```json
{
  "ok": true,
  "command": "fetch",
  "data": {
    "all": false,
    "requested_remote": "origin",
    "refspec": null,
    "remotes": [
      {
        "remote": "origin",
        "url": "git@github.com:user/repo.git",
        "refs_updated": [],
        "objects_fetched": 0
      }
    ]
  }
}
```

## Progress

- In `--json` mode, progress defaults to NDJSON events on `stderr`
- Use `--progress none` to keep `stderr` quiet in JSON mode
- `--machine` disables progress automatically and keeps `stderr` clean on success

## Design Rationale

### Why no --prune by default?

Git added `fetch.prune = true` as a recommended default because stale remote-tracking
refs accumulate silently. Libra chose not to prune by default for two reasons: (1) pruning
requires an additional round-trip to enumerate the remote's current refs, adding latency to
every fetch, and (2) in agent-driven workflows, stale tracking refs can serve as useful
historical anchors for diffing against a previous remote state. When pruning is desired,
`libra remote prune <name>` provides an explicit, auditable operation. This keeps `fetch`
fast and predictable while giving users a deliberate pruning path.

### Why no --depth/--shallow?

Shallow clones and fetches introduce a "shallow boundary" that breaks many operations
(blame, log, merge-base computation) in subtle ways. Libra targets monorepo and AI-agent
workflows where full history is essential for accurate code understanding. Rather than
supporting a mode that silently degrades downstream commands, Libra omits shallow fetch
entirely. For bandwidth-constrained environments, Libra's tiered cloud storage (S3/R2
with LRU caching) provides a more robust solution than shallow history.

### Why JSON progress on stderr?

Structured progress events (object counts, bytes received) are emitted as NDJSON lines
on stderr so that agent frameworks can parse real-time progress without interfering with
the final result envelope on stdout. This follows the Unix convention of separating status
information (stderr) from data output (stdout). The `--progress none` flag allows callers
that do not need progress to suppress it entirely, and `--machine` mode disables progress
by default for maximum script friendliness.

## Parameter Comparison: Libra vs Git vs jj

| Parameter | Libra | Git | jj |
|-----------|-------|-----|----|
| Fetch upstream | `libra fetch` | `git fetch` | `jj git fetch` |
| Named remote | `libra fetch origin` | `git fetch origin` | `jj git fetch --remote origin` |
| Single branch | `libra fetch origin main` | `git fetch origin main` | `jj git fetch --remote origin --branch main` |
| All remotes | `libra fetch --all` | `git fetch --all` | `jj git fetch --all-remotes` |
| Prune stale refs | `libra remote prune <name>` | `git fetch --prune` | Automatic |
| Shallow fetch | Not supported | `git fetch --depth N` | Not supported |
| Structured output | `--json` / `--machine` | No | No |
| Progress events | NDJSON on stderr | Text on stderr | Text on stderr |

## Error Handling

| Scenario | StableErrorCode | Exit | Hint |
|----------|-----------------|------|------|
| No configured upstream / detached HEAD | `LBR-REPO-003` | 128 | "checkout a branch or specify a remote" |
| Remote not found | `LBR-CLI-003` | 129 | "use 'libra remote -v' to see configured remotes" |
| Remote branch not found | `LBR-CLI-003` | 129 | "verify the remote branch name and try again" |
| Invalid remote spec (missing repo, malformed URL, unsupported scheme) | `LBR-CLI-003` or `LBR-REPO-001` | 129 / 128 | Varies by cause |
| Authentication failure during discovery | `LBR-AUTH-002` | 128 | "check SSH key / HTTP credentials and repository access rights" |
| Network timeout / transport failure | `LBR-NET-001` | 128 | "check network connectivity and retry" |
| Packet / sideband / checksum / pack protocol failure | `LBR-NET-002` | 128 | "the remote did not respond correctly" |
| Object format mismatch | `LBR-REPO-003` | 128 | "remote uses a different hash algorithm" |
| Failed to create pack directory | `LBR-IO-002` | 128 | "check filesystem permissions" |
| Failed to write pack/index/refs | `LBR-IO-002` | 128 | "check filesystem permissions and disk space" |
| Local state corruption | `LBR-REPO-002` | 128 | "inspect repository state and object integrity" |
