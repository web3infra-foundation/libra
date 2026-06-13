# `libra prune`

Historical design for pruning unreachable objects from the repository.

> Status: unpublished. `libra prune` is not registered in the public CLI in the
> current release. Running it returns the standard unknown-command error
> (`LBR-CLI-001`). The interface below describes preserved design material, not
> a user-visible command contract.

## Synopsis

```
libra prune [OPTIONS] [HEAD]...
```

## Description

The unpublished design effectively runs `libra fsck --unreachable` using all the refs available in `refs/`, optionally with an additional set of heads specified on the command line, and prunes all unpacked objects unreachable from any of these head objects from the repository. In addition, it prunes the unpacked objects that are also found in packs.

Specifically, unreachable objects found in pack will be kept. For more details about unreachable objects, refer to the `libra fsck --unreachable` documentation.

## Options

### `-n, --dry-run`

Report objects to be removed without actually removing anything.

```bash
$ libra prune -n
d670460b4b4aece5915caf5c68d12f560a9fe3e4 blob
```

### `-v, --verbose`

Report all removed objects.

```bash
$ libra prune -v
d670460b4b4aece5915caf5c68d12f560a9fe3e4 blob
```

### `--expire <TIME>`

Only expire loose objects older than `<TIME>`.

```bash
$ libra prune --expire "2 weeks ago"
$ libra prune --expire 2024-01-01
```

### `[HEAD]...`

In addition to objects reachable from any of our references, keep objects reachable from listed `HEAD`s.

```bash
$ libra prune HEAD~2
$ libra prune v1.0 v1.1
$ libra prune 74689c87fb53b6d666de95efea667d99ba2fa52a
```

## Examples

```bash
# Prune objects unreachable from refs
libra prune

# Prune in dry run mode
libra prune -n

# Only prune expired unreachable objects, with verbose output
libra prune -v --expire "2 weeks ago"

# Only prune expired unreachable objects
libra prune --expire 2024-01-01

# Apart from refs, keep objects reachable from specified heads
libra prune HEAD~2
```

## Human Output

Normal prune (no flags):

```text
(no output)
```

Verbose mode:

```text
d670460b4b4aece5915caf5c68d12f560a9fe3e4 blob
```

Dry-run mode:

```text
d670460b4b4aece5915caf5c68d12f560a9fe3e4 blob
```

Global `--quiet` suppresses dry-run and verbose human output while keeping
warnings and errors on stderr.

## Structured Output

If this command is published in a future release, `libra prune` should support the global `--json` and `--machine` flags on successful prunes.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success
- dry-run output reports the planned prunable objects without actually removing objects.

Example:

```json
{
  "command": "prune",
  "data": {
    "expire": null,
    "heads": [
      "test"
    ],
    "objects": [
      {
        "object_id": "b13c288e945d00a4d16f195b33bf003b53d73dac",
        "object_type": "blob"
      },
      {
        "object_id": "74689c87fb53b6d666de95efea667d99ba2fa52a",
        "object_type": "blob"
      }
    ],
    "dry_run": true,
    "verbose": false
  },
  "ok": true
}
```

## Notes

In most cases, users will not need to call `libra prune` directly, but should instead call `libra gc`, which handles pruning along with many other housekeeping tasks.

 When `libra prune` runs concurrently with another process, There is a risk of it deleting an object that the other process is using but hasn’t created a reference to. This may just cause the other process to fail or may corrupt the repository if the other process later adds a reference to the deleted object. 
 
 Typically, an explicit `--expire` value significantly mitigates this problem. When users really need to directly run this command, it is recommended to attach an expiration value like `--expire 2.weeks.ago`, and run in dry-run mode first to preview objects that will be pruned.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Not inside a Libra repository | `LBR-REPO-001` | 128 |
| Invalid or missing `--expire` value | `LBR-CLI-002` | 129 |
| Ambiguous object name (matches multiple objects) | `LBR-CLI-002` | 129 |
| Invalid `HEAD` argument or object name | `LBR-CLI-003` | 129 |
| Refs/reflogs/HEAD metadata invalid, missing, or points to missing objects | `LBR-REPO-002` | 128 |
| Failed to load commit/tree/tag data or resolve object type | `LBR-REPO-002` | 128 |
| Failed to read objects directory, entries, metadata, or pack indexes | `LBR-IO-001` | 128 |
| Failed to remove loose object or empty prefix directory | `LBR-IO-002` | 128 |
| Internal invariant violated while pruning paths | `LBR-INTERNAL-001` | 128 |
