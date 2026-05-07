# `libra fsck`

Verify the integrity of objects, refs, and index in a Libra repository.

## Synopsis

```
libra fsck [OPTIONS] [OBJECT]
```

## Description

`libra fsck` verifies the integrity of objects, references, and index files in a Libra repository.
It is analogous to `git fsck` and serves as the primary diagnostic tool for detecting repository
corruption, broken references, or data inconsistencies.

The command performs the following checks:

- **Object hash integrity**: Recomputes the SHA1 or SHA256 hash of each object and verifies it matches the stored hash
- **Object format validity**: Ensures each object can be parsed correctly (blob, tree, commit, tag)
- **Ref consistency**: Verifies all references point to existing, valid objects
- **Index integrity**: Checks that the staging index file is valid and consistent
- **Reachability analysis**: Detects dangling and unreachable objects using BFS from refs, reflogs, and index

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
This flag excludes reflog entries, which may cause more objects to be reported as dangling.

```bash
libra fsck --no-reflogs
```

### `--unreachable`

Report all unreachable objects, not just dangling commits.

```bash
libra fsck --unreachable
```

### `--dangling`, `--no-dangling`

Control reporting of dangling objects. Default is to report dangling commits only.

- `--dangling` or `--dangling=true`: Report all dangling objects (commits, trees, blobs)
- `--no-dangling`: Hide all dangling object reports

```bash
libra fsck --dangling          # Report all dangling objects
libra fsck --no-dangling       # Hide dangling reports
```

### `--name-objects`

Show human-readable names for objects in verbose output. Names are collected from:
- Refs: `refs/heads/master`, `refs/tags/v1.0`
- Reflogs: `HEAD@{1778158193}`, `refs/heads/main@{1778158193}`
- Index: `:path/to/file.txt`

Names are only shown during the connectivity check phase.

```bash
libra fsck --verbose --name-objects
```

Example output:
```
Checking connectivity (6 objects)
Checking 1c59427adc4b205a270d8f810310394962e79a8b (:file2.txt)
Checking 2906c3ede0a129d57a88b3fed7aeb6d17d68ab29 (HEAD, refs/heads/main)
```

### `--lost-found`

Write dangling/unreachable objects to `.libra/lost-found/` directory:
- `lost-found/commit/<hash>`: For commit and tree objects
- `lost-found/other/<hash>`: For blob objects (actual content)

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

## Examples

```bash
# Full integrity check
libra fsck

# Verbose output with object names
libra fsck --verbose --name-objects

# Find dangling objects
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

### Clean Repository

```text
Integrity check passed: 4 objects verified
```

### With Dangling Objects

```text
dangling commit 8ae045f3b2c1d9e7f6a5b4c3d2e1f0a9b8c7d6e5
```

### With Missing Object

```text
missing tree 6678874f0d5b658ae5c88b04020c64219f51f743
```

### With Hash Mismatch

```text
hash mismatch blob 1c59427adc4b205a270d8f810310394962e79a8b
```

### With Root Commits (--root)

```text
root 2906c3ede0a129d57a88b3fed7aeb6d17d68ab29
```

### With Tagged Commits (--tags)

```text
tagged commit 85c5c26f763319a05433663eac5e083e4e55735e (v1.0)
```

## Exit Codes

| Exit Code | Meaning |
| --------- | ------- |
| 0 | All checks passed |
| 1 | Object corruption (hash mismatch or invalid format) |
| 2 | Broken refs (point to missing objects) |
| 4 | Index corruption |

Exit codes are additive: `3` = object corruption + broken refs, `7` = all three categories have issues.

**Note**: `dangling` and `unreachable` objects are informational only and do NOT cause non-zero exit codes.

## Compatibility with Git

| Option | Git | Libra |
| ------ | --- | ----- |
| Full check | `git fsck` | `libra fsck` |
| Verbose | `git fsck --verbose` | `libra fsck --verbose` |
| Skip reflogs | `git fsck --no-reflogs` | `libra fsck --no-reflogs` |
| Show unreachable | `git fsck --unreachable` | `libra fsck --unreachable` |
| Hide dangling | `git fsck --no-dangling` | `libra fsck --no-dangling` |
| Lost+found | `git fsck --lost-found` | `libra fsck --lost-found` |
| Name objects | `git fsck --name-objects` | `libra fsck --name-objects` |
| Connectivity only | `git fsck --connectivity-only` | `libra fsck --connectivity-only` |
| Report roots | N/A | `libra fsck --root` |
| Report tags | N/A | `libra fsck --tags` |
