# `libra gc`

Prune unreachable loose objects and clean stale pack sidecar files.

## Synopsis

```bash
libra gc [--dry-run] [--prune=<date> | --no-prune] [--aggressive] [--auto] [--force]
```

## Description

`libra gc` traces objects reachable from repository references, reflogs, the
index, in-progress operation state, and local AI catalogs, then prunes
unreachable loose objects that match the configured prune cutoff. When cloud
backup is configured, unsynced `object_index` rows retain matching loose objects
as pending backup data; they are reported as retained unreachable objects rather
than counted as reachable graph roots. It also
inspects `.libra/objects/pack/` and removes stale sidecar files such as orphan
`.idx` files when they are old enough and not protected by a matching `.keep`
file.

Valid `.pack` + `.idx` pairs are verified through Libra's existing
`verify-pack`/pack decoding path. Malformed pack groups are retained and
reported instead of blocking unrelated cleanup. If reachable-object traversal is
incomplete, non-dry-run loose-object pruning is skipped for that invocation and
the reason is emitted in `warnings[]`. Libra currently does not rewrite valid
packs, perform delta compression, create cruft packs, expire reflogs, or repack
loose reachable objects.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `--dry-run` | `-n` | Report objects and pack sidecars that would be removed without deleting them | Off |
| `--prune <DATE>` | | Prune unreachable loose objects older than `<DATE>`; supports `now`, `never`, Unix timestamps, RFC3339 timestamps, `YYYY-MM-DD`, and `N.seconds.ago`, `N.minutes.ago`, `N.hours.ago`, `N.days.ago`, `N.weeks.ago`, `N.months.ago`, `N.years.ago` | `2.weeks.ago` |
| `--no-prune` | | Disable pruning and only inspect reachability and pack hygiene | Off |
| `--aggressive` | | Accepted for Git compatibility; Libra does not repack or delta-compress yet | Off |
| `--auto` | | Accepted for Git compatibility; Libra still runs one deterministic local pass | Off |
| `--force` | | Replace an existing `gc.lock` only when it contains a valid PID that is no longer running | Off |
| `--json` | | Emit a structured JSON envelope | Off |
| `--machine` | | Emit the same envelope as one compact JSON line | Off |

## Examples

```bash
libra gc
libra gc --dry-run --prune=now
libra gc --prune=now
libra gc --prune=never --json
```

## Human Output

Human mode prints a loose-object summary and pack-directory statistics:

```text
Enumerating loose objects: 3 scanned, 2 reachable, 1 unreachable.
Pruned 1 loose object(s).
Checked 1 pack(s), containing 42 indexed object(s).
Cleaned 0 stale pack file(s).
```

`--dry-run` switches deletion lines to `Would prune` / `Would clean`.
`--quiet` suppresses stdout while preserving errors and warnings on stderr.

## Structured Output

With `--json`, `libra gc` returns a `gc` envelope containing:

- `loose_objects.scanned`, `reachable`, `unreachable`, `pruned`, and `retained`
- `reachable_objects`
- `unreachable_objects[]` with object id, type, action, and reason
- `pack_files.packs_verified`, `objects_in_packs`, and `stale_files[]`
- `warnings[]` for accepted compatibility flags, stale roots, incomplete traversal, and forced locks

```json
{
  "ok": true,
  "command": "gc",
  "data": {
    "prune": "now",
    "dry_run": false,
    "loose_objects": {
      "scanned": 3,
      "reachable": 2,
      "unreachable": 1,
      "pruned": 1,
      "retained": 0
    },
    "reachable_objects": 2,
    "unreachable_objects": [
      {
        "oid": "0123456789abcdef0123456789abcdef01234567",
        "object_type": "blob",
        "action": "pruned",
        "reason": "unreachable loose object matched prune policy"
      }
    ],
    "pack_files": {
      "directory_exists": true,
      "packs_verified": 1,
      "objects_in_packs": 42,
      "stale_files": []
    },
    "warnings": []
  }
}
```

## Compatibility

The command aligns with Git's core safety rule: reachable objects are retained,
and unreachable loose objects are pruned only when the prune policy allows it.
The implementation is intentionally narrower than `git gc`: it does not perform
full repacking, bitmap generation, commit-graph maintenance, reflog expiration,
or cruft-pack creation.

`.libra/gc.lock` serializes concurrent `libra gc` runs. It is not a repository-wide
write lock: commands that write new objects or update refs do not currently acquire
this lock, so `--prune=now` should be used when no other writer is active. `--force`
only replaces a stale lock when Libra can verify that the recorded PID is no longer
running.

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Keep reachable objects | Supported | Supported | N/A |
| Prune old unreachable loose objects | `--prune <date>` | `--prune=<date>` | N/A |
| Dry run | `-n` / `--dry-run` | `--dry-run` | N/A |
| Disable pruning | `--no-prune` | `--no-prune` | N/A |
| Pack verification | Reuses `verify-pack` for valid pack/index pairs | Repack/verify as part of maintenance | N/A |
| GC lock | `.libra/gc.lock` for concurrent `gc` runs only | Supported | N/A |
| Repack valid objects | Unsupported | Supported | N/A |
| Cruft packs | Unsupported | Supported | N/A |
| Reflog expiration | Unsupported | Supported | N/A |
| JSON output | `--json` / `--machine` | N/A | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Not inside a Libra repository | `LBR-REPO-001` | 128 |
| Invalid prune date | `LBR-CLI-002` | 129 |
| Object storage cannot be read | `LBR-IO-001` | 128 |
| Object directory is a symlink or not a directory | `LBR-REPO-002` | 128 |
| Another GC run holds `gc.lock` | `LBR-CONFLICT-002` | 2 |
| Object or pack sidecar deletion fails | `LBR-IO-002` | 128 |
