# `libra verify-pack`

Validate a Git pack index (`.idx`) against its matching pack archive (`.pack`).

## Synopsis

```bash
libra verify-pack [OPTIONS] <IDX_FILE>...
```

## Description

`libra verify-pack` is a read-only plumbing command. It parses the pack index,
decodes the corresponding pack file, and verifies that both files agree on:

- index version and structural layout
- fanout table monotonicity and object-name sorting
- index checksum
- pack checksum stored in the index trailer
- object count, object IDs, and offsets
- CRC32 values for version 2 indexes

By default the pack path is derived by replacing the index file extension with
`.pack`. Use `--pack <PACK_FILE>` when the pack archive lives elsewhere.
The command does not require a Libra repository. When run inside a repository,
it uses that repository's object format. Outside a repository, version 2 index
files infer SHA-1 vs SHA-256 from the index layout; version 1 indexes are SHA-1
only.

Compatibility note: this command does not expose Git's `-s` / `--stat-only`
form. `--pack <PACK_FILE>` is a Libra extension and can only be used when
verifying one `<IDX_FILE>`.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `<IDX_FILE>...` | | Pack index files to verify | Required |
| `--pack <PATH>` | | Pack archive to verify against | `<IDX_FILE>` with `.pack` extension |
| `--verbose` | `-v` | Print each indexed object using Git-compatible verbose fields | Off |
| `--json` | | Emit a structured JSON envelope | Off |
| `--machine` | | Emit the same envelope as one compact JSON line | Off |

## Examples

```bash
libra verify-pack objects/pack/pack-abc123.idx
libra verify-pack pack-a.idx pack-b.idx
libra verify-pack --pack /tmp/pack-abc123.pack /tmp/pack-abc123.idx
libra verify-pack -v pack-abc123.idx
libra verify-pack pack-abc123.idx --json
```

## Human Output

Successful non-verbose verification prints one summary line:

```text
objects/pack/pack-abc123.idx: ok
```

Verbose mode prints indexed objects before the summary line using Git's base
field layout:

```text
3b18e512dba79e4c8300dd08aeb37f8e728b8dad blob 12 21 48
objects/pack/pack-abc123.idx: ok
```

The fields are `<oid> <type> <size> <size-in-pack> <offset>`. CRC32 values for
version 2 indexes are validated and remain available in structured output, but
are not printed in human verbose mode.

## Structured Output

```json
{
  "ok": true,
  "command": "verify-pack",
  "data": {
    "idx_file": "objects/pack/pack-abc123.idx",
    "pack_file": "objects/pack/pack-abc123.pack",
    "index_version": 2,
    "object_count": 42,
    "pack_hash": "0123456789abcdef0123456789abcdef01234567",
    "index_hash": "89abcdef0123456789abcdef0123456789abcdef",
    "verified": true
  }
}
```

When multiple index files are verified with `--json`, `data.packs[]` contains
one result object per input index. When `--verbose` is combined with `--json`,
each result's `objects[]` contains `oid`, `object_type`, `size`,
`size_in_pack`, `offset`, and optional `crc32`.

## Compatibility

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Verify pack index | `libra verify-pack <idx>...` | `git verify-pack <idx>...` | N/A |
| Verbose objects | `-v` / `--verbose` | `-v` | N/A |
| Stat-only mode | Unsupported | `-s` / `--stat-only` | N/A |
| Explicit pack path | `--pack <path>` | N/A | N/A |
| JSON output | `--json` / `--machine` | N/A | N/A |
| Version 1 index | Supported for SHA-1 repositories | Supported | N/A |
| Version 2 index | Supported | Supported | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Index file cannot be opened | `LBR-IO-001` | 128 |
| Pack file cannot be opened | `LBR-IO-001` | 128 |
| Index is malformed | `LBR-REPO-002` | 128 |
| Pack is malformed | `LBR-REPO-002` | 128 |
| Index and pack disagree | `LBR-REPO-002` | 128 |
