# `libra op` Development Notes

## Command Goal

`libra op` exposes Libra's command-level operation history. It is a Libra-native
extension rather than a Git command. The current public surface supports:

- `libra op log`
- `libra op show`
- `libra op restore`

## Compatibility

- Tier: `intentionally-different`.
- Rationale: Git has reflog and reset/restore flows, but it does not expose this
  Libra operation-graph model or the command-level restore view used here.

## Implementation

- CLI entry: `src/cli.rs::Commands::Op`.
- Command implementation: `src/command/op.rs`.
- Storage/service layer: `src/internal/operation.rs`.
- Transaction wrapper: `src/internal/operation_wrapper.rs`.
- Operation tables are part of the bootstrap schema and are also ensured by the
  explicit database upgrade path for older repositories.

## Current Behavior

- `op log` lists operations by repository with pagination and exact command
  filtering.
- `op show` resolves an operation id or `@{n}` reference and can print the
  captured view snapshot.
- `op restore` restores HEAD and captured branch refs from a previous operation
  view and records a new successful restore operation. `--dry-run` performs no
  writes.

## Remaining Gaps

- Restore currently updates HEAD and captured branch refs; it does not prune refs
  that were absent from the target view.
- Broader command coverage is incremental. At present, branch creation is wired
  through operation logging as the first command integration target.
