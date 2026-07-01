# `libra pack-objects`

Create a pack from object ids read on stdin.

> **Hidden / internal plumbing.** `pack-objects` is not part of Libra's public
> Git-compatibility surface (it is hidden from `libra --help`). It exists for
> internal and integration use — most users want [`libra repack`](repack.md)
> instead.

## Synopsis

```
libra pack-objects [--stdout] < object-ids
```

## Description

`pack-objects` reads object ids from standard input — one per line, and tolerant
of the `<id> <path>` form printed by `libra rev-list --objects` (only the
leading id is used) — and encodes those objects into a single pack through the
shared pack writer.

By default the pack is written into `.libra/objects/pack/` and the new
`pack-<checksum>` stem is printed to stdout. With `--stdout` the raw pack bytes
are streamed to standard output instead, for piping into `libra index-pack`.

## Options

| Option | Description |
|--------|-------------|
| `--stdout` | Write the raw pack bytes to stdout instead of into `objects/pack`. |

## Exit status

- `0` — a pack was produced.
- `128` — no object ids were supplied on stdin.
- other non-zero — run outside a repository, or a pack could not be encoded.

## Compatibility

A deliberately small subset of `git pack-objects`: input is a plain id list on
stdin (no `--revs`/`--all` history walking), the output pack is always
undeltified, and reachability/thin-pack options are not supported.

## Examples

```
# Pack the objects reachable from HEAD.
libra rev-list --objects HEAD | libra pack-objects

# Stream a pack to stdout and index it in one pipeline.
libra rev-list --objects HEAD | libra pack-objects --stdout | libra index-pack --stdin -o out.idx
```

## See also

- [`libra repack`](repack.md) — the user-facing way to consolidate objects.
- [`libra index-pack`](index-pack.md) — build the index for a pack.
