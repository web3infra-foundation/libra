# `libra index-pack`

Build a `.idx` index file for an existing `.pack` archive.

## Synopsis

```
libra index-pack [OPTIONS] <PACK_FILE>
```

## Description

`libra index-pack` reads a Git pack file and generates a corresponding pack
index (`.idx`) file. The index file provides O(1) random access to objects
within the pack by mapping object hashes to byte offsets.

Without `-o`, the output file name is derived by replacing the `.pack` extension
with `.idx`. The default index format is version 1 (SHA-1 fan-out table plus
offset/hash pairs). Version 2 (with CRC32 checksums and support for large
offsets) can be requested with `--index-version 2`.

This is a low-level plumbing command. It is used internally by `libra fetch` and
`libra clone` after receiving pack data over the wire, and can be invoked
manually to rebuild missing or corrupt index files.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `<PACK_FILE>` | | Path to the `.pack` file to index (required). Must end with `.pack` unless `-o` is given. | |
| `-o <PATH>` | `-o` | Output path for the generated index file. | `<PACK_FILE>` with `.pack` replaced by `.idx` |
| `--index-version <N>` | | Force the index format version (1 or 2). | `1` |

### Examples

```bash
# Build an index with default settings (version 1, auto-named)
libra index-pack objects/pack/pack-abc123.pack

# Specify a custom output path
libra index-pack pack-abc123.pack -o /tmp/pack-abc123.idx

# Force version 2 index format
libra index-pack pack-abc123.pack --index-version 2

# JSON output for scripting
libra index-pack pack-abc123.pack --json
```

## Common Commands

```bash
libra index-pack pack-123.pack
libra index-pack pack-123.pack -o pack-123.idx
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
    "index_version": 1
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
    "index_version": 2
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

### Why no `--verify`?

Git's `index-pack --verify` re-reads an existing `.idx` file and checks it
against the pack for consistency. Libra does not yet implement this because the
primary use case (generating indices) is covered, and verification can be done
by regenerating the index and comparing checksums. A dedicated `--verify` flag
is a natural future addition once there is demand from agent or CI workflows.

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
| Verify existing index | Not implemented | `--verify` | N/A |
| `--stdin` (read pack from stdin) | Not implemented | Yes | N/A |
| `--fix-thin` (add bases for thin packs) | Not implemented | Yes | N/A |
| `--keep` (create .keep file) | Not implemented | Yes | N/A |
| `--threads` (parallel decompression) | Internal (8 threads) | `--threads=<N>` | N/A |
| Progress output | Not implemented | `--progress` / `--no-progress` | N/A |
| JSON output | `--json` | No | N/A |
| Max pack size (v1) | ~2 GB (32-bit offsets) | ~2 GB (32-bit offsets) | N/A |
| CRC32 checksums | Version 2 only | Version 2+ | N/A |
| Default hash | SHA-1 | SHA-1 (SHA-256 experimental) | Blake2b (internal) |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Pack path does not end with `.pack` (and no `-o`) | `LBR-CLI-002` | 129 |
| Pack path and index path are identical | `LBR-CLI-002` | 129 |
| Pack file cannot be opened | `LBR-IO-001` | 128 |
| Unsupported index version | `LBR-CLI-002` | 129 |
| Pack contents are invalid or corrupt | `LBR-REPO-002` | 128 |
| Index write failed | `LBR-IO-002` | 128 |
