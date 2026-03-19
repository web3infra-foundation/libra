# Libra CLI Error Contract Design

This document consolidates the design intent from RFC [#301](https://github.com/web3infra-foundation/libra/issues/301)
and the initial implementation scope from [#302](https://github.com/web3infra-foundation/libra/issues/302).

It is a development-facing design note. The user-facing reference remains
[`docs/error-codes.md`](../error-codes.md).

## Status

- RFC #301 defines the public failure contract for the Libra CLI.
- PR #302 introduces the first implementation of that contract in the codebase.
- This document records the combined design so future changes can preserve the
  intended external behavior and understand the implementation boundaries.

## Goals

Libra needs a stable, documented failure surface for three audiences:

- Shell and CI users who branch on numeric exit codes.
- Agents, wrappers, and integrations that need stable machine-readable failure
  classification.
- Humans who need clear error messages and actionable recovery guidance.

The design therefore aims to:

- standardize non-zero exits around the primary failure mode
- assign stable symbolic error codes
- emit structured machine-readable error data
- make the contract discoverable from both the CLI and repository docs
- define compatibility rules so the contract can evolve safely

## Non-Goals

This design is specifically about error reporting and failure classification.
It does not define a full machine-readable success-output model for all Libra
commands.

Separately, Libra's newer root-level output flags use a stricter global
contract than Git's per-command flags. In particular, global `--quiet`
suppresses stdout entirely, including primary command results; callers that
still need success payloads should use `--json` / `--machine`.

## Design Summary

The Libra CLI error contract has three layers:

1. Exit code
   Used for coarse shell and CI branching.
2. Stable error code
   Used for durable machine classification by agents and wrappers.
3. Structured error JSON
   Used for machine-readable details and actionable hints.

These three layers are externally visible and must be treated as part of the
public CLI contract.

## Public Contract

### Exit code semantics

Libra must follow these rules:

- Success exits with `0`.
- Failures exit with a non-zero code.
- The exit code represents the most important failure mode for the command.
- Exit codes stay coarse-grained and stable across releases.

Initial exit-code mapping:

| Exit | Category | Primary stable code | Meaning |
| --- | --- | --- | --- |
| `0` | - | - | Success |
| `2` | `cli` | `LBR-CLI-001` | Invalid CLI arguments or unknown subcommand |
| `3` | `repo` | `LBR-REPO-001` | Not a repository or repository metadata/state problem |
| `4` | `conflict` | `LBR-CONFLICT-001` | Unresolved conflict or blocked operation |
| `5` | `network` | `LBR-NET-001` | Remote unreachable, protocol failure, or transport error |
| `6` | `auth` | `LBR-AUTH-001` | Missing credentials, denied permission, or insufficient scope |
| `7` | `io` | `LBR-IO-001` | Filesystem or storage read/write failure |
| `8` | `internal` | `LBR-INTERNAL-001` | Internal unrecoverable error |

> **Note:** The table above is historical. The current default exit-code
> scheme uses `128` for general errors, `129` for usage/CLI errors, and `9`
> for conflicts.

### Stable error code format

Stable error codes use the form:

```text
LBR-<DOMAIN>-<NUMBER>
```

Examples:

```text
LBR-CLI-001
LBR-REPO-001
LBR-CONFLICT-001
```

The symbolic code is intended for machine consumption and documentation. Its
meaning must remain stable once published.

### Structured error JSON

On failure, Libra should emit a single-line JSON object on `stderr`. The final
line of `stderr` should be reserved for this structured record so downstream
tools can parse it reliably.

RFC #301 defines the minimum conceptual payload as:

- code
- category
- message

It may also include:

- hint
- details
- other additive fields that do not break existing consumers

PR #302's initial implementation adopts a richer error report shape so the
machine interface can carry all externally relevant error metadata in one
record. In the implementation, the structured report contains:

- `ok`
- `error_code`
- `category`
- `exit_code`
- `severity`
- `message`
- optional `usage`
- optional `hints`
- optional `details`

This richer envelope should still be understood as the concrete implementation
of the same three-layer contract defined by the RFC.

### Human-readable error output

Structured JSON is not a replacement for human-readable errors. Libra should
continue to present readable error text and hints to interactive users.

The intended model is:

- humans read the visible error text and hints
- shell and CI users branch on the exit code
- agents and wrappers parse the final structured JSON record

## Discoverability and Documentation

The contract must be discoverable from the CLI itself.

Libra should expose:

```bash
libra help error-codes
```

and may also expose:

```bash
libra help errors
```

The repository must also include a complete error-code reference document.
PR #302 delivers this as [`docs/error-codes.md`](../error-codes.md).

That reference should cover:

- the stable exit-code table
- the stable symbolic codes
- how to consume structured error JSON
- usage guidance for shell, CI, and wrappers
- compatibility and change-management rules
- examples of common failure handling patterns

## Shared Implementation Model

PR #302 establishes the first shared implementation path for this contract.

### Central error abstraction

Libra should route user-visible failures through a shared error type rather
than letting each command print ad hoc terminal text.

The implementation centers this behavior in:

- [`src/utils/error.rs`](../../src/utils/error.rs)

This shared layer is responsible for:

- stable code assignment
- category mapping
- exit-code mapping
- user-facing message and hint rendering
- structured JSON rendering

### Command entrypoint model

Commands should expose a safe entrypoint that returns structured failures rather
than printing and exiting directly.

The intended model is:

```text
execute_safe(...) -> CliResult<...>
```

This makes command behavior composable, testable, and consistent with the
shared CLI error contract.

### Legacy compatibility bridge

The codebase still contains legacy paths that emit `fatal:` or `error:` text.
PR #302 keeps migration incremental by providing a compatibility bridge that
converts legacy text into the shared error model.

That bridge allows the project to improve contract consistency without
requiring an all-at-once command rewrite.

### Process boundary behavior

At the process boundary:

- command code returns a structured CLI error
- the shared error layer renders human-readable output and structured JSON
- `main` exits using the mapped stable exit code

This keeps success at `0` while normalizing failures to the stable contract.

## Compatibility Rules

The following changes are compatibility-sensitive and should be treated as
breaking or near-breaking:

- changing the meaning of an existing stable error code
- changing the exit-code mapping for an existing stable error code
- removing a documented stable error code
- changing structured output in a way that breaks known consumers

The following changes are usually backward compatible:

- adding a new stable error code for a new failure mode
- adding optional JSON fields
- improving human-readable wording without changing stable semantics
- adding help aliases or documentation examples

If a published stable code must change, the change should be documented
explicitly as a breaking change.

## Migration Guidance

Before this design, many Libra CLI paths used generic process exits and ad hoc
error text.

The new model keeps the success path unchanged but normalizes failures into:

- stable coarse exit codes
- stable symbolic error codes
- structured JSON records

Consumers that historically branched on generic exit codes should migrate to:

- exit code for coarse flow control
- stable symbolic code for precise automation

## Testing Requirements

Tests should validate the public contract rather than implementation details
only.

At minimum, the test suite should cover:

- success exits with `0`
- representative failures return the expected non-zero exit code
- representative failures emit the expected stable symbolic code
- structured JSON exists and is parseable
- CLI help for `libra help error-codes`
- alignment between CLI help and `docs/error-codes.md`

PR #302 explicitly includes test updates for:

- structured error output behavior
- help-topic discoverability
- contract-level CLI behavior

## What PR #302 Delivers

The initial implementation described in PR #302 includes:

- a shared error abstraction under `src/utils`
- stable symbolic error codes and exit-code mapping
- structured JSON error output
- `libra help error-codes` and `libra help errors`
- a complete reference in `docs/error-codes.md`
- test coverage for error output and help behavior

This implementation should be understood as the first concrete delivery of the
RFC, not the end of the design space. Future work can refine field naming,
expand classification coverage, and standardize more command paths without
changing the core contract.

## Rationale

This design deliberately uses multiple layers because each audience needs a
different abstraction:

- Exit codes are ideal for quick shell branching.
- Stable symbolic codes are durable and documentation-friendly.
- Structured JSON provides precise machine-readable detail.

Using only one of these mechanisms would make the CLI either too coarse for
automation or too inconvenient for normal shell usage.

## Open Questions

RFC #301 leaves several follow-up questions open for future design work:

- Should structured JSON be emitted on every failure by default, or only in
  specific modes?
- Should Libra eventually expose a dedicated JSON error mode?
- Should the project publish a formal JSON schema?
- How should multi-cause failures be represented when more than one category
  plausibly applies?
- Should warnings or partial-success states eventually receive their own stable
  machine-readable taxonomy?

These questions do not block the base contract introduced here.

## Canonical References

- RFC: [#301](https://github.com/web3infra-foundation/libra/issues/301)
- Initial implementation: [#302](https://github.com/web3infra-foundation/libra/issues/302)
- User-facing error-code reference: [`docs/error-codes.md`](../error-codes.md)
- Shared implementation entrypoint: [`src/utils/error.rs`](../../src/utils/error.rs)
