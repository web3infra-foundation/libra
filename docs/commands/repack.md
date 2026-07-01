# `libra repack`

Combine repository objects into a single pack.

## Synopsis

```
libra repack [-a|--all] [-d|--delete] [-q|--quiet]
```

## Description

`repack` encodes repository objects into one new `pack-<checksum>.pack` (with a
matching `pack-<checksum>.idx`) under `.libra/objects/pack/`. It uses the same
shared pack writer as the `maintenance` tasks, so every pack Libra writes on disk
goes through one well-formed encoder — the produced pack round-trips through
`libra index-pack` and `libra verify-pack`.

By default only the reachable objects that are currently **loose** are packed;
objects already stored in an existing pack are left where they are. `--all`
widens the set to every reachable object, producing a single consolidated pack.

Reachability is computed from refs, reflogs and the index — exactly like
`libra maintenance run --task gc` — so an object referenced only by a reflog is
never dropped.

## Options

| Option | Description |
|--------|-------------|
| `-a`, `--all` | Pack all reachable objects, including those already stored in a pack, into a single fresh pack. |
| `-d`, `--delete` | After packing, remove the loose objects that now live in the new pack. Only files whose object id is in the new pack are removed; existing packs are never deleted, so no object is ever left unreferenced. |
| `-q`, `--quiet` | Suppress the informational summary. |

With `--json` / `--machine` the command emits an envelope whose `data` object
carries `pack` (the new pack's name), `objects_packed`, and `loose_removed`.

## Exit status

- `0` — the repack completed (including the no-op case where nothing needed
  packing).
- non-zero — the command was run outside a repository, or a pack could not be
  written.

## Compatibility

This is a focused subset of Git's `git repack`. Libra always writes a single
undeltified pack via its shared writer; delta compression, `--window`/`--depth`
tuning, geometric repacking (`--geometric`), keeping/writing bitmaps, and
removing redundant *packs* (as opposed to loose objects) are not implemented.
See [`COMPATIBILITY.md`](../../COMPATIBILITY.md).

## Examples

```
# Pack the loose reachable objects into a single pack.
libra repack

# Consolidate every reachable object into one pack.
libra repack -a

# Repack everything and drop the loose copies that are now packed.
libra repack -a -d

# Machine-readable summary.
libra --json repack -a -d
```

## See also

- [`libra maintenance`](maintenance.md) — scheduled optimization tasks (its
  `loose-objects` and `incremental-repack` tasks share this writer).
- [`libra index-pack`](index-pack.md) — build the index for an existing pack.
- [`libra verify-pack`](verify-pack.md) — validate a pack and its index.
