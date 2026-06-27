# `libra index-pack`

Build a `.idx` index file for an existing `.pack` archive.

## Synopsis

```
libra index-pack [OPTIONS] [<PACK_FILE>]
```

## Description

`libra index-pack` reads a Git pack file and generates a corresponding pack
index (`.idx`) file. The index file provides O(1) random access to objects
within the pack by mapping object hashes to byte offsets.

Without `-o`, the output file name is derived by replacing the `.pack` extension
with `.idx`. The default index format is version 1 (SHA-1 fan-out table plus
offset/hash pairs). Version 2 (with CRC32 checksums and support for large
offsets) can be requested with `--index-version 2`.

With `--stdin`, Libra reads pack bytes from standard input. This mode requires
`-o <PATH>` because there is no input file name to derive the index path from.
Libra persists the stdin pack beside the index by replacing the output path's
extension with `.pack`, then builds the requested `.idx` from that saved pack.

With `--keep`, Libra also writes a `.keep` file beside the pack. Bare
`--keep` creates an empty keep file; `--keep=<MSG>` writes the message followed
by a newline, matching Git's keep-file convention.

Git-style `--progress` and `--no-progress` are accepted for script
compatibility. They use Libra's existing global progress mode and do not add a
separate `index-pack` progress stream.

`--fix-thin` is accepted for Git compatibility and is a **no-op**. A *thin* pack
carries `REF_DELTA` objects whose base objects are not in the pack; completing it
means resolving those bases from the repository and appending them. Libra's pack
decoder requires self-contained packs (it has no external-delta-base resolver)
and never produces thin packs, so any pack that indexes successfully already has
no external bases to add — exactly the case where Git's `--fix-thin` also does
nothing. Resolving external delta bases (true thin-pack completion) is not
supported.

This is a low-level plumbing command. It is used internally by `libra fetch` and
`libra clone` after receiving pack data over the wire, and can be invoked
manually to rebuild missing or corrupt index files.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `<PACK_FILE>` | | Path to the `.pack` file to index. Required unless `--stdin` is used. Must end with `.pack` unless `-o` is given. | |
| `--stdin` | | Read pack bytes from standard input. Requires `-o`; writes the pack beside the index with a `.pack` extension. | Off |
| `-o <PATH>` | `-o` | Output path for the generated index file. | `<PACK_FILE>` with `.pack` replaced by `.idx` |
| `--keep[=<MSG>]` | | Create `<PACK_FILE>` with `.keep` extension. If `MSG` is provided, write it followed by a newline. | Not created |
| `--index-version <N>` | | Force the index format version (1 or 2). | `1` |
| `--progress` | | Accept Git-style progress request; maps to Libra's global text progress mode. | Global progress mode |
| `--no-progress` | | Accept Git-style progress suppression; maps to Libra's global no-progress mode. | Global progress mode |
| `--fix-thin` | | Accept Git's thin-pack completion flag. No-op: Libra requires self-contained packs (no external-delta-base resolver) and never produces thin packs, so there is nothing to complete on the packs it indexes. | Off |

### Examples

```bash
# Build an index with default settings (version 1, auto-named)
libra index-pack objects/pack/pack-abc123.pack

# Specify a custom output path
libra index-pack pack-abc123.pack -o /tmp/pack-abc123.idx

# Read a pack stream from stdin and generate /tmp/incoming.idx
cat incoming.pack | libra index-pack --stdin -o /tmp/incoming.idx

# Force version 2 index format
libra index-pack pack-abc123.pack --index-version 2

# Keep the pack from pruning after rebuilding its index
libra index-pack --keep="manual recovery" pack-abc123.pack

# Accept Git-style progress flags used by scripts
libra index-pack --progress pack-abc123.pack
libra index-pack --no-progress pack-abc123.pack

# Accept Git's thin-pack completion flag (no-op on Libra's self-contained packs)
libra index-pack --fix-thin pack-abc123.pack

# JSON output for scripting
libra index-pack pack-abc123.pack --json
```

## Common Commands

```bash
libra index-pack pack-123.pack
libra index-pack pack-123.pack -o pack-123.idx
libra index-pack --stdin -o pack-123.idx
libra index-pack --keep pack-123.pack
libra index-pack --progress pack-123.pack
libra index-pack --no-progress pack-123.pack
libra index-pack --fix-thin pack-123.pack
libra index-pack pack-123.pack --index-version 2
libra index-pack pack-123.pack --json
```

## Human Output

On success, human mode prints the generated index path:

```text
/tmp/pack-123.idx
```

`--quiet` suppresses `stdout`.

## Structured Output (JSON examples)

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 1,
    "keep_file": null
  }
}
```

Version 2 example:

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 2,
    "keep_file": null
  }
}
```

Keep-file example:

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/pack-123.pack",
    "index_file": "/tmp/pack-123.idx",
    "index_version": 1,
    "keep_file": "/tmp/pack-123.keep"
  }
}
```

Stdin example:

```json
{
  "ok": true,
  "command": "index-pack",
  "data": {
    "pack_file": "/tmp/stdin-pack.pack",
    "index_file": "/tmp/stdin-pack.idx",
    "index_version": 1,
    "keep_file": null
  }
}
```

## Design Rationale

### Why expose this low-level command?

Pack indexing is a plumbing operation that most users never invoke directly. Libra
exposes it for three reasons:

1. **Debuggability.** When a fetch or clone fails partway through, the user may
   have a valid `.pack` file but no `.idx`. Exposing `index-pack` lets them
   recover without re-downloading.
2. **Agent workflows.** AI agents that manage pack files (e.g., for tiered cloud
   storage with S3/R2) need a programmatic way to generate indices. The `--json`
   output makes this scriptable.
3. **Git compatibility.** Tools and scripts in the Git ecosystem expect
   `index-pack` to exist. Providing it means Libra can be a drop-in replacement
   in CI pipelines that call plumbing commands.

### Why a separate `verify-pack` command?

Git exposes verification through both `verify-pack` and some `index-pack`
workflows. Libra keeps index generation and verification separate:
`index-pack` writes an index, while [`verify-pack`](verify-pack.md) performs a
read-only consistency check between an existing `.idx` and its `.pack`.

### Why limited index versions?

Libra supports version 1 and version 2, which cover the two formats defined in
the Git pack-index specification. Version 1 is compact and sufficient for packs
under 2 GB (offsets are 32-bit). Version 2 adds CRC32 checksums per object and
a 64-bit offset table for large packs. There is no version 3 in the Git spec,
so Libra does not invent one. The default is version 1 for simplicity and
because most Libra-managed packs are well under the 2 GB threshold. Version 1
also avoids the dependency on CRC32 computation, keeping the fast path lean.

### Why does version 1 require SHA-1?

The version 1 index format predates Git's SHA-256 transition and hard-codes
20-byte hash slots. Libra enforces this constraint at runtime: if the
repository is configured for a non-SHA-1 hash, version 1 index generation
fails with a clear error. Version 2 is the path forward for alternative hash
algorithms.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Build index from pack | `libra index-pack <file>` | `git index-pack <file>` | N/A (jj uses its own storage) |
| Custom output path | `-o <path>` | `-o <path>` | N/A |
| Index version | `--index-version 1\|2` (default 1) | `--index-version <N>[,<offset>]` (default 2) | N/A |
| Verify existing index | `libra verify-pack <idx>` | `verify-pack` / `index-pack --verify` | N/A |
| `--stdin` (read pack from stdin) | `--stdin -o <idx>`; stores a same-stem `.pack` beside the idx | Yes | N/A |
| `--fix-thin` (add bases for thin packs) | Accepted no-op (self-contained packs only; no external-base resolver) | Yes | N/A |
| `--keep` (create .keep file) | `--keep[=<MSG>]` | Yes | N/A |
| `--threads` (parallel decompression) | Internal (8 threads) | `--threads=<N>` | N/A |
| Progress flags | `--progress` / `--no-progress` accepted; no dedicated progress stream | `--progress` / `--no-progress` | N/A |
| JSON output | `--json` | No | N/A |
| Max pack size (v1) | ~2 GB (32-bit offsets) | ~2 GB (32-bit offsets) | N/A |
| CRC32 checksums | Version 2 only | Version 2+ | N/A |
| Default hash | SHA-1 | SHA-1 (SHA-256 experimental) | Blake2b (internal) |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Pack path does not end with `.pack` (and no `-o`) | `LBR-CLI-002` | 129 |
| `--stdin` without `-o <PATH>` | `LBR-CLI-002` | 129 |
| `--stdin` combined with `<PACK_FILE>` | `LBR-CLI-002` | 129 |
| Pack path and index path are identical | `LBR-CLI-002` | 129 |
| Keep path and index path are identical | `LBR-CLI-002` | 129 |
| Derived stdin pack file cannot be created | `LBR-IO-002` | 128 |
| Pack file cannot be opened | `LBR-IO-001` | 128 |
| Unsupported index version | `LBR-CLI-002` | 129 |
| Pack contents are invalid or corrupt | `LBR-REPO-002` | 128 |
| Index write failed | `LBR-IO-002` | 128 |
| Keep-file write failed | `LBR-IO-002` | 128 |
