# Libra CLI Error Codes

Libra now exposes failures through a stable three-layer contract:

1. `exit code`
   Fast shell/CI branching. `0` means success. Any non-zero value is a failure.
2. `stable error code`
   A machine-stable identifier for agents, wrappers, and higher-level UX.
3. `structured JSON report`
   The last stderr line is JSON and carries category, message, hints, and details.

This contract is implemented in [src/utils/error.rs](/Volumes/Data/GitMono/libra/src/utils/error.rs).

## Output Contract

On failure, Libra writes:

1. A human-readable error block to `stderr`
2. An `Error-Code: ...` line
3. A final JSON line with the structured report

Example:

```text
fatal: not a libra repository (or any of the parent directories): .libra
Error-Code: LBR-REPO-001
Hint: run 'libra init' to create a repository in the current directory.
{"ok":false,"error_code":"LBR-REPO-001","category":"repo","exit_code":3,"severity":"fatal","message":"not a libra repository (or any of the parent directories): .libra","hints":["run 'libra init' to create a repository in the current directory."]}
```

Warnings and progress messages remain plain text. Only failures participate in this contract.

## Exit Codes

| Exit | Meaning | Primary automation use |
| --- | --- | --- |
| `0` | Success | Continue |
| `2` | Usage / invalid target | Fix CLI invocation |
| `3` | Repository / repo state | Fix repository context or state |
| `4` | Conflict / blocked operation | Resolve conflicts or local state |
| `5` | Network / transport | Retry or inspect remote connectivity |
| `6` | Authentication / authorization | Configure identity or credentials |
| `7` | Filesystem / storage I/O | Inspect files, permissions, locks |
| `8` | Internal / invariant | Report bug or unexpected failure |

## Stable Codes

### CLI

| Stable code | Meaning |
| --- | --- |
| `LBR-CLI-001` | Unknown command |
| `LBR-CLI-002` | Invalid or missing CLI arguments |
| `LBR-CLI-003` | Invalid object, revision, pathspec, or move target |

### Repository

| Stable code | Meaning |
| --- | --- |
| `LBR-REPO-001` | Not inside a Libra repository |
| `LBR-REPO-002` | Repository metadata is corrupt or incompatible |
| `LBR-REPO-003` | Repository state blocks the operation |

### Conflict

| Stable code | Meaning |
| --- | --- |
| `LBR-CONFLICT-001` | Unresolved conflict is present |
| `LBR-CONFLICT-002` | Operation blocked to avoid overwriting state |

### Network

| Stable code | Meaning |
| --- | --- |
| `LBR-NET-001` | Remote unreachable / transport unavailable |
| `LBR-NET-002` | Protocol, negotiation, or pack failure |

### Auth

| Stable code | Meaning |
| --- | --- |
| `LBR-AUTH-001` | Missing identity, token, or credential material |
| `LBR-AUTH-002` | Credential present but permission denied |

### I/O

| Stable code | Meaning |
| --- | --- |
| `LBR-IO-001` | Read/open/load failure |
| `LBR-IO-002` | Write/save/update/remove failure |

### Internal

| Stable code | Meaning |
| --- | --- |
| `LBR-INTERNAL-001` | Unexpected internal invariant failure |

## JSON Schema

Every structured failure report includes:

| Field | Type | Meaning |
| --- | --- | --- |
| `ok` | `bool` | Always `false` for error reports |
| `error_code` | `string` | Stable code such as `LBR-REPO-001` |
| `category` | `string` | `cli`, `repo`, `conflict`, `network`, `auth`, `io`, `internal` |
| `exit_code` | `number` | Shell-facing exit code |
| `severity` | `string` | `error` or `fatal` |
| `message` | `string` | User-facing error summary without prefix |
| `usage` | `string?` | Optional usage text for CLI errors |
| `hints` | `string[]` | Optional actionable hints |
| `details` | `object` | Optional structured context |

## Architecture

The design has four layers:

1. `CliError`
   Owns stable code, exit code, hints, details, and rendering.
2. `execute_safe(...) -> CliResult<()>`
   CLI-facing command entrypoints return structured errors instead of printing ad hoc text.
3. `emit_legacy_stderr(...)`
   Compatibility bridge for legacy commands that still produce `fatal:` / `error:` strings.
4. `main`
   Exits with `err.exit_code()` and keeps success at `0`.

This lets Libra migrate incrementally without breaking the stable external contract.

## Testing

Integration tests parse the final JSON stderr line and assert both:

- human-readable text still makes sense
- machine-readable fields are stable

Shared helpers live in [tests/command/mod.rs](/Volumes/Data/GitMono/libra/tests/command/mod.rs).
