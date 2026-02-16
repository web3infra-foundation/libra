# Repository Guidelines

## Project Structure & Module Organization
- `src/` holds the Rust crate: CLI entry in `src/main.rs`, crate root in `src/lib.rs`, subcommands under `src/command/`, shared primitives in `src/internal/`, utilities in `src/utils/`.
- Integration tests live in `tests/command/` with fixtures in `tests/data/`; `tests/command/mod.rs` re-exports helpers.
- Community docs in `docs/`; schema bootstrap `sql/sqlite_20240331_init.sql`; hooks/templates in `template/`.
- Buck2/Buckal metadata is in `third-party/`; prefer Cargo and regenerate BUCK files when dependencies change.

## Build, Test, and Development Commands
- `cargo +nightly fmt --all` then `cargo clippy --all-targets --all-features` keep formatting and linting aligned (`rustfmt.toml` groups imports by crate).
- `cargo build` or `cargo check` for quick compile checks; `cargo run -- <cmd>` exercises the CLI (e.g., `cargo run -- status` in a temp repo).
- `cargo test` runs the suite; filter with `cargo test command::init_test` or `cargo test add_test`. Integration cases rely on temp dirs; run serial if flaky.
- After editing `Cargo.toml` deps, run `cargo buckal migrate` to sync Buck2 files (see `third-party/README.md`).

## Coding Style & Naming Conventions
- Rust 2024; 4-space indent; snake_case for modules/functions, PascalCase for types, SCREAMING_SNAKE for consts.
- Imports are grouped Std/External/Crate per `rustfmt.toml`; avoid wildcard imports except in tests.
- Prefer `anyhow::Result` for CLI flows and `thiserror` for library errors; keep args parsed via `clap` in `src/command/*`.
- Add short comments only when control flow is non-obvious (e.g., async handling, SQLite migrations).

## Testing Guidelines
- Favor integration coverage in `tests/command/` to mirror Git workflows; use `tempfile::tempdir()` and `utils::test::ChangeDirGuard` to isolate state.
- Keep fixtures small and local; re-use helpers in `tests/command/mod.rs` instead of shelling out directly.
- Mark tests `#[serial]` if they mutate shared state; keep async tests on Tokio (`#[tokio::test]` or `flavor = "multi_thread"` when needed).
- Pair new commands/options with at least one end-to-end test plus a focused unit test where logic is easily isolated.

## Commit & Pull Request Guidelines
- History uses short, typed summaries with optional scope and PR reference, e.g., `feat(status): support porcelain v2 (#82)` or `fix(push): record tracking reflog (#81)`.
- Commits must include DCO and PGP signing: `git commit -S -s -m "feat(...): ..."`; ensure the `Signed-off-by` trailer is present.
- PRs should state intent, linked issues, and tests run (`cargo +nightly fmt`, `cargo clippy`, `cargo test ...`); include repro steps or sample CLI output when touching user-visible behavior.
- Keep changes small and cohesive; update README/CLI docs when adding flags or altering compatibility tables.
