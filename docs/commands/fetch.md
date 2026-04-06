# `libra fetch`

Download objects and update remote-tracking refs from another repository.

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

## Structured Output

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

## Progress

- In `--json` mode, progress defaults to NDJSON events on `stderr`
- Use `--progress none` to keep `stderr` quiet in JSON mode
- `--machine` disables progress automatically and keeps `stderr` clean on success

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| No configured upstream / detached HEAD | `LBR-REPO-003` | 128 |
| Remote not found / invalid branch / invalid remote spec | `LBR-CLI-003` or `LBR-REPO-001` | 129 / 128 |
| Authentication failure during discovery | `LBR-AUTH-002` | 128 |
| Network timeout / transport failure | `LBR-NET-001` | 128 |
| Packet / sideband / checksum / pack protocol failure | `LBR-NET-002` | 128 |
| Object format mismatch | `LBR-REPO-003` | 128 |
| Failed to write pack/index/refs | `LBR-IO-002` | 128 |
