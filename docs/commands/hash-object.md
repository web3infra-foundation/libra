# `libra hash-object`

Compute the Git-compatible object ID for raw file contents or standard input.

```bash
libra hash-object [OPTIONS] <PATH>...
libra hash-object --stdin [OPTIONS]
libra hash-object --stdin-paths [OPTIONS]
```

This implementation supports `blob`, `commit`, `tree`, and `tag` objects. It
hashes raw bytes using the current repository object format and validates
`commit`/`tree`/`tag` input unless `--literally` is used. It does not apply
clean/smudge filters, attributes, CRLF conversion, or LFS pointer conversion.

Read-only hashing does not require a Libra repository and defaults to SHA-1
when no repository object format is available. `-w` / `--write` requires a
repository because it stores the object in the repository object database.

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `<PATH>...` | | File paths to hash |
| `--stdin` | | Read bytes from standard input instead of file paths |
| `--stdin-paths` | | Read newline-delimited file paths from standard input |
| `--write` | `-w` | Store the computed object in the repository object database |
| `--type <TYPE>` | `-t` | Object type to hash: `blob` (default), `commit`, `tree`, or `tag` |
| `--literally` | | Hash the content as-is, without validating that it is a well-formed object of the given type |
| `--path <FILE>` | | Use the given path as the source label for `--stdin` input (does not change the content; Libra has no attribute/filter lookup) |
| `--no-filters` | | Accepted for compatibility but a no-op — Libra applies no clean/smudge or CRLF filters |
| `--json` | | Emit a structured JSON envelope |
| `--machine` | | Emit the same envelope as one compact JSON line |

## Examples

Hash a file without writing the object:

```bash
libra hash-object README.md
```

Hash and write a file as a blob object:

```bash
libra hash-object -w src/main.rs
```

Hash bytes from standard input:

```bash
printf 'hello' | libra hash-object --stdin
```

Hash file paths listed on standard input:

```bash
printf 'a.txt\nb.txt\n' | libra hash-object --stdin-paths
```

Hash and write a commit object:

```bash
libra hash-object -t commit -w commit.txt
```

Hash malformed content as a commit for diagnostics:

```bash
libra hash-object -t commit --literally bad-commit.txt
```

Hash CRLF bytes verbatim while accepting Git-compatible `--no-filters`:

```bash
printf 'a\r\nb\r\n' | libra hash-object --stdin --no-filters
```

## Output

Human output prints one object ID per input:

```text
b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0
```

Structured output:

```json
{
  "ok": true,
  "command": "hash-object",
  "data": {
    "object_type": "blob",
    "write": false,
    "objects": [
      {
        "source": "-",
        "oid": "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0",
        "size": 5,
        "written": false
      }
    ]
  }
}
```

## Compatibility

| Feature | Libra | Git | Jujutsu |
|---------|-------|-----|---------|
| Hash file as blob | `libra hash-object <path>` | `git hash-object <path>` | N/A |
| Read from stdin | `--stdin` | `--stdin` | N/A |
| Read paths from stdin | `--stdin-paths` | `--stdin-paths` | N/A |
| Write object | `-w` / `--write` | `-w` | N/A |
| Select object type | `blob`/`commit`/`tree`/`tag` | `-t <type>` | N/A |
| `--path` source label | Accepted (label only) | `--path` | N/A |
| Clean/smudge & CRLF filters | `--no-filters` accepted as a no-op (no filter infrastructure) | filters | N/A |
| `--filters` | Not implemented | `--filters` | N/A |
| Hash literally invalid objects | `--literally` | `--literally` | N/A |

## Errors

| Condition | Stable code | Exit | Hint |
|-----------|-------------|------|------|
| Unsupported object type | `LBR-CLI-002` | 129 | supported object types: blob, commit, tree, tag |
| Argument parse error or conflicting flags | `LBR-CLI-002` | 129 | inspect `libra hash-object --help` |
| Malformed commit/tree/tag without `--literally` | `LBR-REPO-002` | 128 | pass `--literally` to hash the content as-is |
| Input file cannot be read | `LBR-IO-001` | 128 | Verify the path exists and is readable |
| Object cannot be written | `LBR-IO-002` | 128 | Check object storage permissions and disk space |

`--path` conflicts with `--no-filters` and `--stdin-paths`. `--no-filters` is
allowed with `--stdin-paths` and does not change the bytes being hashed. Libra
maps command-line parse errors into `LBR-CLI-002` with exit 129; this differs
from Git's exact usage exit codes but keeps Libra's stable error taxonomy.
