# `libra prune`

Prune all unreachable objects from the repository.

## Synopsis

```
libra prune [OPTIONS] [HEAD]...
```

## Description

`libra prune` effectively runs `libra fsck --unreachable` using all the refs available in `refs/`, optionally with an additional set of objects specified on the command line, and prunes all unpacked objects unreachable from any of these head objects from the repository. In addition, it prunes the unpacked objects that are also found in packs.

## Options

### `-n, --dry-run`

Report objects to be removed without actually removing anything.

```bash
$ libra prune -n
would prune d670460b4b4aece5915caf5c68d12f560a9fe3e4
```

### `-v, --verbose`

Report all removed objects.

```bash
$ libra prune -v
prune d670460b4b4aece5915caf5c68d12f560a9fe3e4
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
libra prune HEAD~2";
```

## Human Output

Normal prune (no flags):

```text
(no output)
```

Verbose mode:

```text
prune d670460b4b4aece5915caf5c68d12f560a9fe3e4
```

Dry-run mode:

```text
would prune d670460b4b4aece5915caf5c68d12f560a9fe3e4
```

Global `--quiet` suppresses dry-run and verbose human output while keeping
warnings and errors on stderr.

## Structured Output

`libra prune` supports the global `--json` and `--machine` flags on successful prunes.

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- `stderr` stays clean on success
- dry-run output reports the planned prunable objects without actually removing objects.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Not inside a Libra repository | `LBR-REPO-001` | 128 |
| Invalid or missing `--expire` value | `LBR-CLI-002` | 129 |
| Invalid `HEAD` argument or revision | `LBR-CLI-003` | 129 |
| Refs/reflogs/HEAD metadata invalid or points to missing objects | `LBR-REPO-002` | 128 |
| Failed to read objects directory, entries, or metadata | `LBR-IO-001` | 128 |
| Failed to remove loose object or empty prefix directory | `LBR-IO-002` | 128 |
| Internal invariant violated while pruning paths | `LBR-INTERNAL-001` | 128 |
