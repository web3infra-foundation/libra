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

- **Object hash integrity**: Recomputes the SHA1 or SHA256 hash (selected by `core.objectformat` config) of each object and verifies it matches the stored hash
- **Object format validity**: Ensures each object can be parsed correctly (blob, tree, commit, tag)
- **Ref consistency**: Verifies all references point to existing, valid objects
- **Index integrity**: Checks that the staging index file is valid and consistent
- **Cross-reference validation**: Ensures trees reference valid blobs/trees, and commits reference valid trees/parents

This command is essential for:
- Detecting storage corruption or bit rot
- Diagnosing repository issues after crashes or interrupted operations
- Validating repository health before backup or migration
- Investigating suspicious behavior or unexpected errors

## Options

### `-v, --verbose`

Print detailed progress information, including each object being verified and a summary of findings.

```bash
libra fsck --verbose
```

### `--no-cross-ref-check`

Skip cross-reference validation (trees referencing blobs/trees, commits referencing trees/parents).
This makes the check faster but less thorough. Use this when you only need to verify individual
object integrity and don't care about referential consistency.

```bash
libra fsck --no-cross-ref-check
```

### `--no-index-check`

Skip index file validation. The index is checked by default to ensure the staging area is
consistent with the repository state.

```bash
libra fsck --no-index-check
```

### `--objects-only`

Only check objects, skipping refs and index validation. This is a subset of `--no-cross-ref-check`
and `--no-index-check` combined.

```bash
libra fsck --objects-only
```

### `--fix`

Automatically fix issues where possible. Currently supports:

- **Deleting broken refs** that point to nonexistent or invalid objects
- **Rebuilding a corrupted index** from HEAD's tree (falls back to deletion on unborn branches)

```bash
libra fsck --fix
```

Fixed issues are deleted from the database and the index is rebuilt from scratch. Issues that cannot be auto-repaired are left unchanged with a suggestion.

### `[OBJECT]`

Positional argument specifying a single object ID to check. When provided, only that specific
object is verified instead of the entire repository.

```bash
libra fsck 2f24194cb3d41c1ac5b1f40c4c9331a2a40a76a7
```

## Common Commands

```bash
libra fsck                              # Full integrity check
libra fsck --verbose                    # With detailed output
libra fsck --json                       # JSON output for automation
libra fsck <object-id>                  # Check single object
libra fsck --no-cross-ref-check         # Faster, less thorough
libra fsck --objects-only               # Objects only, skip refs/index
```

## Human Output

Default human mode writes the verification summary to `stdout`.

### Clean Repository

```text
Integrity check passed: 4 objects verified
```

### Verbose Output

```text
Checking 4 objects...
Checking object 1/4: 2f24194cb3d41c1ac5b1f40c4c9331a2a40a76a7
Checking object 2/4: 557db03de997c86a4a028e1ebd3a1ceb225be238
Checking object 3/4: 6678874f0d5b658ae5c88b04020c64219f51f743
Checking object 4/4: b0b9fc8f6cc2f8f110306ed7f6d1ce079541b41f

=== Fsck Summary ===
Objects checked: 4
  - OK: 4
  - Corrupted: 0
Refs checked: 3
  - OK: 1
  - Broken: 0
Index valid: true
Cross-reference issues: 0
```

### Single Object Check

```text
Object 2f24194cb3d41c1ac5b1f40c4c9331a2a40a76a7 is valid
Integrity check passed: 1 objects verified
```

### With Errors (Missing Object)

```text
Integrity check FAILED
Objects: 3 checked, 3 OK, 0 corrupted

Issues:
  [ERROR] Tree 6678874f0d5b658ae5c88b04020c64219f51f743 references missing object 557db03de997c86a4a028e1ebd3a1ceb225be238 (test.txt)
```

### With Errors (Broken Ref)

```text
Integrity check FAILED
Objects: 4 checked, 4 OK, 0 corrupted
Refs: 3 checked, 1 OK, 0 broken

Issues:
  [ERROR] Ref 'refs/heads/broken' points to missing object abc123def456...
```

## Structured Output

`libra fsck` supports the global `--json` and `--machine` flags.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success

Example (clean repository):

```json
{
  "ok": true,
  "command": "fsck",
  "data": {
    "objects_checked": 4,
    "objects_ok": 4,
    "objects_corrupted": 0,
    "refs_checked": 3,
    "refs_ok": 1,
    "refs_broken": 0,
    "index_valid": true,
    "cross_ref_issues": 0,
    "overall_status": "ok",
    "issues": []
  }
}
```

Example (with issues):

```json
{
  "ok": false,
  "command": "fsck",
  "data": {
    "objects_checked": 3,
    "objects_ok": 3,
    "objects_corrupted": 0,
    "refs_checked": 3,
    "refs_ok": 1,
    "refs_broken": 0,
    "index_valid": true,
    "cross_ref_issues": 1,
    "overall_status": "invalidformat",
    "failure_mask": 1,
    "failure_categories": ["objects"],
    "issues": [
      {
        "issue_type": "missing_tree_entry",
        "severity": "error",
        "object_id": "557db03de997c86a4a028e1ebd3a1ceb225be238",
        "ref_name": null,
        "message": "Tree 6678874f0d5b658ae5c88b04020c64219f51f743 references missing object 557db03de997c86a4a028e1ebd3a1ceb225be238 (test.txt)",
        "suggestion": "The tree references an object that doesn't exist."
      }
    ]
  }
}
```

### Schema Notes

- `overall_status` is `"ok"`, `"missing"`, `"invalidformat"`, or `"hashmismatch"`
- `failure_mask` is a bitmask of failure categories (see [Exit Code Behavior](#exit-code-behavior)).
  A value of `0` means no failures; `7` means all three categories (objects, refs, index) have issues.
- `failure_categories` is an array of human-readable category names corresponding to the set bits in `failure_mask`.
- `issues` contains detailed problem reports with severity and suggestions
- `issue_type` values:
  - `hash_mismatch`: Object content doesn't match its hash
  - `invalid_format`: Object cannot be parsed
  - `missing_object`: Object referenced but not found
  - `missing_tree_entry`: Tree references missing blob/tree
  - `missing_commit_tree`: Commit references missing tree
  - `missing_parent_commit`: Commit references missing parent
  - `broken_ref`: Reference points to missing object
  - `invalid_ref_hash`: Reference has invalid hash format
  - `index_corruption`: Index file is corrupted

## Design Rationale

### Git-style hash computation (header + content)

Git and Libra compute object hashes as `SHA1(type + ' ' + size + '\0' + content)`, not just
`SHA1(content)`. This design ensures that an object's identity includes its type and size,
preventing type confusion attacks where a malicious actor could substitute a blob for a tree
with the same raw content. The `fsck` command recomputes hashes using this same formula to
verify object integrity.

### Cross-reference validation as optional

Cross-reference validation is thorough but expensive: it requires loading and parsing every
tree and commit object to verify their references. The `--no-cross-ref-check` and `--objects-only`
flags allow users to skip this phase when they only need to verify individual object integrity,
such as in large repositories where full validation would be too slow.

### SQLite-backed refs vs filesystem objects

Libra stores refs (branches, tags) in SQLite for transactional safety, but objects remain as
loose files in the `.libra/objects/` directory (or in pack files). This hybrid approach means
`fsck` must verify both storage layers: filesystem-based object integrity and database-based
ref consistency.

### JSON output for automation

Unlike `git fsck` which only produces human-readable text output, `libra fsck --json` provides
structured output suitable for:
- CI/CD pipelines that need to parse verification results
- AI agents that monitor repository health
- Automated backup systems that validate before archiving
- Monitoring dashboards that track corruption rates over time

## Parameter Comparison: Libra vs Git

| Parameter / Flag | Git | Libra |
| ---------------- | --- | ----- |
| Full integrity check | `git fsck` | `libra fsck` |
| Verbose output | `git fsck --verbose` | `libra fsck --verbose` |
| Check single object | `git fsck <object>` | `libra fsck <object-id>` |
| Skip unreachable | `git fsck --unreachable` | N/A |
| Full/connectivity check | `git fsck --full` | N/A (always full) |
| Strict mode | `git fsck --strict` | N/A |
| JSON output | N/A | `libra fsck --json` |
| Skip cross-refs | N/A | `libra fsck --no-cross-ref-check` |
| Skip index | N/A | `libra fsck --no-index-check` |
| Objects only | N/A | `libra fsck --objects-only` |
| Auto-fix | `git fsck --lost-found` | `libra fsck --fix` (delete broken refs, rebuild index) |

## Exit Code Behavior

The exit code is a **bitmask** encoding all categories of issues found in a single run.
Multiple failure types are OR'd together so scripts can decode the full failure set from one value.

| Bit | Flag | Value | Meaning |
| ---- | ---- | ----- | ------- |
| 0 | `OBJECT_CORRUPT` | 1 | Object hash mismatch or format invalid |
| 1 | `REF_BROKEN` | 2 | Ref points to missing or invalid object |
| 2 | `INDEX_CORRUPT` | 4 | Index file parse error or entry inconsistency |

| Exit Code | Binary | Meaning |
| --------- | ------ | ------- |
| 0 | `0b000` | All checks passed |
| 1 | `0b001` | Object corruption only |
| 2 | `0b010` | Broken refs only |
| 3 | `0b011` | Object corruption + broken refs |
| 4 | `0b100` | Index corruption only |
| 5 | `0b101` | Object corruption + index corruption |
| 6 | `0b110` | Broken refs + index corruption |
| 7 | `0b111` | All three categories have issues |

To decode in shell:

```bash
exit_code=$?
if [ $((exit_code & 1)) -ne 0 ]; then echo "objects corrupted"; fi
if [ $((exit_code & 2)) -ne 0 ]; then echo "refs broken"; fi
if [ $((exit_code & 4)) -ne 0 ]; then echo "index corrupted"; fi
```

## Error Handling

Every error scenario maps to an explicit `StableErrorCode` and a bitmask exit code (see [Exit Code Behavior](#exit-code-behavior)).

| Scenario | Stable Error Code | Exit Bitmask | Hint |
| -------- | ----------------- | ------------ | ---- |
| Hash mismatch / object corrupted | `LBR-REPO-002` | 1 (bit 0) | "Object data is corrupted. Consider restoring from backup or remote." |
| Invalid object format | `LBR-REPO-002` | 1 (bit 0) | "Object has invalid format." |
| Missing tree entry | `LBR-REPO-002` | 1 (bit 0) | "The tree references an object that doesn't exist." |
| Missing commit tree | `LBR-REPO-002` | 1 (bit 0) | "The commit's tree is missing." |
| Missing parent commit | `LBR-REPO-002` | 1 (bit 0) | "Parent commit is missing — history may be incomplete." |
| Broken ref (points to missing object) | `LBR-REPO-002` | 2 (bit 1) | "Update or delete this ref." |
| Invalid ref hash format | `LBR-REPO-002` | 2 (bit 1) | "Delete this corrupted ref." |
| Index parse error | `LBR-REPO-002` | 4 (bit 2) | "The index file is corrupted. Try removing `.libra/index` and re-staging." |
| Index entry missing object | `LBR-REPO-002` | 4 (bit 2) | "Run `libra add <file>` to re-stage this file." |
| Not a repository | `LBR-REPO-001` | 128 | "Run `libra init` to create a repository." |
| Invalid object ID argument | `LBR-CLI-002` | 129 | "Provide a valid hex object hash." |
| Database error | `LBR-INTERNAL-001` | 128 | — |
| I/O error listing objects | `LBR-IO-001` | 128 | — |

When multiple failure categories are present, their bitmask values are OR'd together (e.g. objects corrupted + index corrupted = exit code `5`).

## Compatibility Notes

- Git's `git fsck` checks packed objects and `.git/objects/pack/` directories; Libra currently
  supports loose objects only (pack file support is planned for future versions)
- Git's `--lost-found` option creates refs for dangling objects; Libra's `--fix` deletes broken refs
  and rebuilds a corrupted index from HEAD's tree
- Git stores both objects and refs on the filesystem; Libra uses SQLite for refs, so
  `fsck` must verify database consistency in addition to filesystem integrity
- The JSON output format is unique to Libra and has no Git equivalent

## Usage Examples

### Routine Health Check

```bash
# Quick integrity check
libra fsck

# Before a major operation (rebase, merge)
libra fsck --no-cross-ref-check
```

### Diagnose Corruption

```bash
# Full verbose check with cross-reference validation
libra fsck --verbose

# Export results for analysis
libra fsck --json > fsck-report.json
```

### Verify Specific Object

```bash
# Check a suspicious commit
libra fsck abc123def456...

# Verify a tree object
libra fsck 6678874f0d5b658...
```

### CI/CD Integration

```bash
# Fail pipeline if repository is corrupted
libra fsck --objects-only || exit 1

# Log structured results
libra fsck --json | jq '.data' >> health-log.ndjson
```
