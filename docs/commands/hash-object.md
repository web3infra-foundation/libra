# `libra hash-object`

Compute the Git-compatible object ID for raw file contents or standard input.

```bash
libra hash-object [OPTIONS] <PATH>...
libra hash-object --stdin [OPTIONS]
libra hash-object --stdin-paths [OPTIONS]
```

It supports the four Git object types — `blob` (default), `commit`, `tree`, and
`tag` — computing the object id from the `<type> <size>\0<content>` header using the
current repository object format, byte-for-byte identical to `git hash-object -t
<type>`. Without `--literally`, commit/tree/tag content is validated as a well-formed
object of that type (a blob accepts any bytes). It does not apply clean filters,
attributes, or LFS pointer conversion. `--path` is accepted as a Git compatibility
path context and stdin JSON source label; it does not change the hashed bytes until
path-based filters are implemented.

Read-only hashing does not require a Libra repository and defaults to SHA-1
when no repository object format is available. `-w` / `--write` requires a
repository because it stores the object in the repository object database.

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `<PATH>...` | | File paths to hash |
| `--stdin` | | Read bytes from standard input instead of file paths |
| `--stdin-paths` | | Read file paths from standard input (one per line) and hash each |
| `--write` | `-w` | Store the computed object in the repository object database |
| `--type <TYPE>` | `-t` | Object type to hash: `blob` (default), `commit`, `tree`, or `tag` |
| `--literally` | | Hash the bytes as the given type without verifying the content is a well-formed object of that type |
| `--path <PATH>` | | Path context label for compatibility with Git hash-object |
| `--no-filters` | | Explicitly hash raw bytes without path-based filters |
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

Hash file content as a typed object (the id matches `git hash-object -t <type>`); the
content is validated as a well-formed object of that type unless `--literally` is given:

```bash
libra hash-object -t commit commit-payload
libra hash-object -t tag --literally arbitrary-bytes
```

Hash bytes from standard input:

```bash
printf 'hello' | libra hash-object --stdin
```

Hash stdin with a Git-compatible path context label:

```bash
printf 'hello' | libra hash-object --stdin --path README.md
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
| Select object type | `-t blob/commit/tree/tag` | `-t <type>` | N/A |
| Path context | `--path <path>` accepted, no filters applied | `--path <path>` | N/A |
| Disable filters | `--no-filters` accepted | `--no-filters` | N/A |
| Path filters / attributes | Not supported | filters / attributes | N/A |
| Hash literally invalid objects | `--literally` (known types only) | `--literally` (any type string) | N/A |

## Errors

| Condition | Stable code | Exit | Hint |
|-----------|-------------|------|------|
| Object type outside blob/commit/tree/tag | `LBR-CLI-002` | 129 | `hash-object supports blob, commit, tree, and tag` |
| Content is not a well-formed object of `-t <type>` (without `--literally`) | `LBR-CLI-002` | 129 | `pass --literally to hash malformed content without validation` |
| Input file cannot be read | `LBR-IO-001` | 128 | Verify the path exists and is readable |
| Object cannot be written | `LBR-IO-002` | 128 | Check object storage permissions and disk space |
