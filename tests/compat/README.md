# `tests/compat/` — cross-command compatibility regressions

This directory is the集结点 (collection point) for **cross-command** Git
compatibility regressions defined in
[`docs/improvement/compatibility/`](../../docs/improvement/compatibility/).

The tests in `tests/command/*_test.rs` cover each command's happy path and
error path in isolation. The tests here cover the **outward contract** stated
in [`COMPATIBILITY.md`](../../COMPATIBILITY.md): which subcommands appear in
`--help`, which Git surface flags are exposed, and which Git surface flags are
intentionally absent.

## How these tests run

`tests/compat/*` is selected by the `compat-offline-core` job in
`.github/workflows/base.yml`. The Cargo `[[test]]` integration model means each
top-level file under `tests/` becomes its own test binary. Files placed
directly under `tests/compat/` are reachable only when added as `[[test]]`
entries in `Cargo.toml` (`path = "tests/compat/<name>.rs"`); see Cargo
docs.

For now `tests/compat/` is a planned集结点 — files listed below are
populated by C4 / C5 as those batches land.

## Planned files (filled in by C4 / C5 / C-future)

| File | Owning batch | Coverage |
|------|--------------|----------|
| `stash_subcommand_surface.rs` | C4 | `stash --help` lists `show` / `branch` / `clear`; cross-subcommand JSON schema agreement |
| `bisect_subcommand_surface.rs` | C4 | `bisect --help` lists `run` / `view`; exit-code semantics 0 / 125 / 128 |
| `worktree_delete_dir.rs` | C5 | `worktree remove` with and without `--delete-dir`; dirty-worktree refusal |
| `checkout_alias_help.rs` | C5 | top-level `--help` includes `checkout`; the help banner mentions `prefer switch / restore` |
| `matrix_alignment.rs` | C-future | `COMPATIBILITY.md` ↔ `src/cli.rs::Commands` enum drift detection |

## Authoring guidelines

- **Do** assert outward contracts: `--help` strings, JSON schema keys, exit
  codes that other tools (CI scripts, wrappers) depend on.
- **Don't** duplicate per-command happy/error paths — those belong in
  `tests/command/<name>_test.rs`.
- Use the same test helpers as `tests/command/*` (see
  [`tests/command/mod.rs`](../command/mod.rs)).
- Cross-platform tests (worktree dir deletion, etc.) should annotate
  platform-specific differences with `cfg(unix)` / `cfg(windows)`.
