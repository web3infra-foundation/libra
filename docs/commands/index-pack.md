# `libra index-pack`

`libra index-pack` builds a `.idx` file for an existing `.pack` archive.

## Common Commands

```bash
libra index-pack pack-123.pack
libra index-pack pack-123.pack -o pack-123.idx
libra index-pack pack-123.pack --json
```

## Human Output

On success, human mode prints the generated index path:

```text
/tmp/pack-123.idx
```

`--quiet` suppresses `stdout`.

## Structured Output

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

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Pack path does not end with `.pack` | `LBR-CLI-002` | 129 |
| Pack path and index path are identical | `LBR-CLI-002` | 129 |
| Pack file cannot be opened | `LBR-IO-001` | 128 |
| Unsupported index version | `LBR-CLI-002` | 129 |
| Pack contents are invalid or corrupt | `LBR-REPO-002` | 128 |
| Index write failed | `LBR-IO-002` | 128 |
