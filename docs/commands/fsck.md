# `libra fsck`

Verify repository integrity by checking objects, refs, and index.

## Synopsis

```
libra fsck [OPTIONS] [OBJECT]
```

## Description

`libra fsck` verifies the integrity of objects, references, and index files in a Libra repository.
It is analogous to `git fsck` and serves as the primary diagnostic tool for detecting repository
corruption, broken references, or data inconsistencies.

Global structured-error flags such as `--json` and `--machine` are honored on
failure paths. For example, invalid object IDs return the standard Libra CLI
error envelope on stderr instead of bypassing the dispatcher.

The command performs the following checks:

- **Object hash integrity**: Recomputes SHA1/SHA256 hash and verifies it matches the stored hash
- **Object format validity**: Validates object structure (blob, tree, commit, tag)
- **Ref consistency**: Verifies all references point to existing, valid objects
- **Index integrity**: Validates index file structure and cross-references entries with object storage
- **Reachability analysis**: Detects dangling and unreachable objects via BFS from refs, reflogs, and index

## Options

### `[OBJECT]`

Check a single object by ID. When not provided, checks all objects in the repository.

```bash
libra fsck 2f24194cb3d41c1ac5b1f40c4c9331a2a40a76a7
```

### `-v, --verbose`

Print detailed progress information during verification.

```bash
libra fsck --verbose
```

### `--no-reflogs`

Skip reflog validation. By default, reflogs are used as starting points for reachability analysis.
Excluding reflogs may cause more objects to be reported as dangling.

```bash
libra fsck --no-reflogs
```

### `--unreachable`

Report all unreachable objects, not just dangling commits.

```bash
libra fsck --unreachable
```

### `--dangling`, `--no-dangling`

Control reporting of dangling objects. Default is to report dangling commits only (matching git fsck behavior).

- `--dangling` or `--dangling=true`: Report dangling commits
- `--no-dangling`: Hide dangling object reports

```bash
libra fsck --dangling          # Report dangling commits (default)
libra fsck --no-dangling       # Hide dangling reports
```

### `--name-objects`

Show human-readable names for objects in verbose output. Names are collected from:
- Refs: `refs/heads/master`, `refs/tags/v1.0`
- Reflogs: `HEAD@{1778158193}`, `refs/heads/main@{1778158193}`
- Index: `:path/to/file.txt`

```bash
libra fsck --verbose --name-objects
```

### `--lost-found`

Write dangling/unreachable objects to `.libra/lost-found/` directory:
- `lost-found/commit/<hash>`: For commit and tree objects (stores hash)
- `lost-found/other/<hash>`: For blob objects (stores actual content)

This option implies `--no-reflogs` for dangling detection, matching `git fsck --lost-found` behavior.

```bash
libra fsck --lost-found
```

### `--root`

Report root commits (commits with no parents).

Output format: `root <commit-hash>`

```bash
libra fsck --root
```

### `--tags`

Report tagged commits.

Output format: `tagged commit <commit-hash> (<tag-name>)`

```bash
libra fsck --tags
```

### `--connectivity-only`

Only check object existence, skip content validation. Significantly faster but does NOT detect:
- Hash mismatches (content corrupted but object exists)
- Format errors (object cannot be parsed)

Still detects missing objects referenced by commits, trees, or refs.

```bash
libra fsck --connectivity-only
```

### `--full` / `--no-full`

`--full` (the default) verifies objects stored in packfiles in addition to loose
objects. Each `.libra/objects/pack/*.idx` (index version 1 and 2) is parsed with
the same validated reader used by `verify-pack`, and the listed objects are
deduplicated against loose objects before verification.

Use `--no-full` to restrict the check to loose objects, skipping packfiles:

```bash
libra fsck --no-full
```

### `--strict`

Apply additional format and graph checks (these are reported as errors, so they
cause a non-zero exit):

- commit author/committer emails must contain `@`, and their timezones must be a
  well-formed `±HHMM` offset within ±1400;
- a commit's tree and parents must exist with the expected object types;
- a tree's entries must exist with object types matching their mode, and be in
  Git's canonical sort order.

```bash
libra fsck --strict
```

Note: this is an intentionally narrowed subset of `git fsck --strict`. The
`.gitmodules`/HFS+/NTFS pathname checks and per-message `fsck.<msg-id>` severity
configuration are not implemented.

## Examples

```bash
# Full integrity check
libra fsck

# Verbose output with object names
libra fsck --verbose --name-objects

# Find dangling commits
libra fsck --dangling

# Write dangling objects to lost-found
libra fsck --lost-found

# Report root commits
libra fsck --root

# Report tagged commits
libra fsck --tags

# Fast connectivity check
libra fsck --connectivity-only

# Check single object
libra fsck abc123def456...
```

## Output Format

### Diagnostic Messages (stdout)

Diagnostic messages are printed to stdout and do NOT cause non-zero exit codes:

```text
missing <type> <object-id>
hash mismatch <type> <object-id>
dangling <type> <object-id>
unreachable <type> <object-id>
```

### Error Messages (stderr)

Error messages are printed to stderr and cause non-zero exit codes:

```text
bad object sha1: <type> <object-id>
bad tree: <object-id>
unknown type: <type> <object-id>
missing author: <object-id>
missing committer: <object-id>
bad ref content: <ref-name>: invalid hash format
index corruption: <details>
```

### Clean Repository

No output (silent success).

### With Dangling Objects

```text
dangling commit 8ae045f3b2c1d9e7f6a5b4c3d2e1f0a9b8c7d6e5
```

### With Missing Object

```text
missing commit 6678874f0d5b658ae5c88b04020c64219f51f743
```

## Exit Codes

| Exit Code | Meaning |
| --------- | ------- |
| 0 | All checks passed, or only dangling/unreachable objects found (informational) |
| 1 | Errors found: hash mismatch, invalid format, missing objects, broken refs, index corruption |
| 1 | Fatal error: not a repository, invalid object ID, database error |

**Note**: 
- `dangling` and `unreachable` are informational only and do NOT cause non-zero exit codes.
- `missing`, `hash_mismatch`, and format errors cause exit code 1.

## Implementation Details

### Check Stages

The fsck command performs checks in the following order:

1. **Directory scan**: Enumerate all loose objects and pack files
2. **Object verification**: Verify hash integrity and format for each object
3. **HEAD validation**: Check HEAD points to a valid ref
4. **Reflog check**: Validate objects referenced in reflog entries
5. **Ref validation**: Verify all refs point to valid objects
6. **Index validation**: Check index file structure and entry integrity
7. **Connectivity check**: Re-verify all objects with optional name resolution
8. **Reachability analysis**: Identify dangling and unreachable objects via BFS
9. **Root commit report**: (with `--root`) List commits with no parents
10. **Tag report**: (with `--tags`) List tagged commits

### Object Types

Libra supports the same object types as Git:

- **blob**: File content
- **tree**: Directory listing with mode, name, and object references
- **commit**: Snapshot metadata with tree, parents, author, committer
- **tag**: Annotated tag with required `object`, `type`, `tag`, and `tagger`
  headers plus an optional message. Missing or malformed tag headers fail fsck
  with tag-specific diagnostics such as `missing tagger`.

### Hash Algorithms

Libra supports both SHA1 and SHA256 hash algorithms, determined by repository configuration.

### Reflog Behavior

By default, objects mentioned in reflogs are considered reachable and not reported as dangling.
Use `--no-reflogs` to exclude reflog entries from reachability analysis.
