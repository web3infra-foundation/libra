# Libra - Repository Custom Instructions for GitHub Copilot

## What This Repo Is

`libra` is a single Rust 2024 crate: a Git-compatible, AI-agent-native version control system. It does not use a standard `.git/` layout for runtime metadata; local repositories use `.libra/libra.db` for config, HEAD, refs, and AI runtime tables, and `.libra/vault.db` for secrets.

Assume the active architecture is the source tree in this repo, not an older multi-crate design. There are no top-level `engine/`, `delta/`, `transport/`, `storage/`, or `cli/` crates.

## Primary Entry Points

- CLI process: `src/main.rs` initializes tracing and runs the CLI on a 32 MiB thread.
- CLI grammar and dispatch: `src/cli.rs::{parse, parse_async}`.
- Command handlers: `src/command/*::execute_safe` and command-specific helpers.
- Embedding API: `src/lib.rs::{exec, exec_async}`.
- Public CLI surfaces live in `src/cli.rs`; update it when adding or changing commands, flags, output mode behavior, or dispatch.

## Major Boundaries

- `src/command/` contains user-facing subcommands and their clap argument structs.
- `src/internal/ai/` contains agent runtime, provider, tool, session, MCP, orchestration, and related AI infrastructure.
- `src/internal/protocol/` contains Git, HTTP, SSH, and LFS protocol clients.
- `src/internal/publish/` contains the Rust publish pipeline.
- `src/utils/` contains storage, path, error, output, object, worktree, and test helpers.
- `web/` is a Next.js static export embedded into the Rust binary.
- `worker/` is the OpenNext/Cloudflare Worker for read-only `libra publish` hosting.
- `sql/` contains `.libra/libra.db` bootstrap and migrations; `sql/publish/` is separate Worker D1 schema.

## Languages And Defaults

- Rust edition: 2024.
- Async runtime: Tokio.
- CLI parsing: clap.
- Errors: prefer `anyhow::Context` for CLI flows and `thiserror` for domain/library errors.
- Serialization: serde / serde_json.
- Logging/tracing: `tracing` and `tracing-subscriber`.
- Web UI: Next.js in `web/`, static export embedded by `build.rs`.
- Worker: TypeScript/OpenNext/Cloudflare in `worker/`.

## Build, Format, Lint, And Test Commands

- Format: `cargo +nightly fmt --all`.
- Format check: `cargo +nightly fmt --all --check`.
- Fast compile: `LIBRA_SKIP_WEB_BUILD=1 cargo check`.
- Fast build: `LIBRA_SKIP_WEB_BUILD=1 cargo build`.
- Lint gate: `LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`.
- Default tests: `cargo test --all`.
- Single integration target: `cargo test --test <target> -- --test-threads=1`.
- Prefer naming targeted integration tests as `cargo test --test <target> <test_fn> -- --test-threads=1` when possible.
- CLI smoke: `cargo run -- <cmd>`.

Do not suggest plain `cargo fmt` as the formatting command. This repo's `rustfmt.toml` uses unstable nightly formatting features.

## Web And Worker Checks

- `build.rs` runs `pnpm install --frozen-lockfile` and `pnpm run build` in `web/` unless `LIBRA_SKIP_WEB_BUILD=1` is set.
- Skipped web builds create a stub `web/out/index.html`.
- Full web embed check: `pnpm --dir web install --frozen-lockfile && pnpm --dir web lint && pnpm --dir web build`, then ensure no static export drift in `web/out`.
- Worker checks from `worker/`: `pnpm lint`, `pnpm test`, `pnpm test:miniflare`, `pnpm build`.
- Worker e2e uses `pnpm e2e:serve` on `127.0.0.1:3127` plus `pnpm e2e`.
- CI uses Node 22 and pnpm 11.1.0 for `web/`.

## Testing And Feature Gates

- `tests/INDEX.md` is the authoritative index of top-level integration test targets. Update it when adding, renaming, or removing integration tests.
- Files under `tests/compat/` are not auto-discovered by Cargo. Every compat guard needs a `Cargo.toml [[test]]` entry and a row in `tests/compat/README.md`.
- Important consistency guard: `cargo test --test compat_matrix_alignment`.
- Compatibility docs/examples guards include `cargo test --test compat_command_docs_examples_section` and `cargo test --test compat_help_examples_banner`.
- TUI/PTY automation needs all of: `--features test-provider`, `LIBRA_ENABLE_TEST_PROVIDER=1`, and `--test-threads=1`.
- Network smoke: `cargo test --features test-network --test network_remotes_test -- --test-threads=1`.
- Live AI tests require `--features test-live-ai` and credentials such as `DEEPSEEK_API_KEY`.
- Live cloud tests require `--features test-live-cloud` and `LIBRA_D1_*` / `LIBRA_STORAGE_*` credentials.
- `.env.test` lines must keep `export`; otherwise child cargo processes silently miss those variables.
- CLI-level tests should isolate `HOME`, `XDG_CONFIG_HOME`, `LIBRA_CONFIG_GLOBAL_DB`, `LANG`, and `LC_ALL`, preferably using helpers in `tests/command/mod.rs` plus `tempfile::tempdir()` and `utils::test::ChangeDirGuard`.
- Mark tests `#[serial]` if they mutate process cwd, global environment, shared ports, config DBs, or other global state.

## Public Surface Checklist

When adding or changing a visible command, flag, help surface, output format, compatibility behavior, or stable error:

- Update `src/cli.rs`.
- Update the matching `src/command/<name>.rs`.
- Update `COMPATIBILITY.md`.
- Update command docs under `docs/commands/`.
- Update examples via the command's `pub const <CMD>_EXAMPLES` and clap `after_help` wiring.
- Ensure every `docs/commands/<name>.md` page has `## Examples` or `## Common Commands`.
- Update tests under `tests/command/` and `tests/INDEX.md` as needed.
- For compat tests under `tests/compat/`, update `Cargo.toml [[test]]` and `tests/compat/README.md`.
- New stable error codes in `src/utils/error.rs` must be documented in `docs/error-codes.md`; `libra help error-codes` includes that doc at compile time.
- If changing SQL, update bootstrap or migrations under `sql/`; remember `sql/publish/` is for Worker D1 and is independent from runtime `.libra/libra.db`.

## Code Quality Rules

- Do not add `unwrap()` or `expect()` in production `src/**` paths.
- Tests may use `unwrap()` / `expect()`, but production code should return `Result` and propagate actionable errors.
- Truly infallible production cases need a brief `// INVARIANT:` comment.
- User-facing errors must explain what failed, which path/ref/object/resource was affected, and what the user can do next when known.
- Command modules should expose clap args and structured `execute_safe`-style handlers.
- Document externally visible side effects and error mapping on command entry points.
- Database helpers that accept an existing connection should use the `_with_conn` naming pattern to preserve transaction safety.
- Keep provider-specific AI code under `src/internal/ai/providers/<provider>/` and satisfy common contracts in `completion/`.
- Fake or deterministic provider paths are for tests, not production behavior.

## Security And Data Safety Bias

- Prioritize security, data loss/corruption, auth/tenancy, migrations, external APIs, concurrency, retries/idempotency, hot-path performance, and missing tests/docs.
- Treat production `unwrap()` / `expect()`, silent failure paths, unsafe secret or PII logging, missing validation at trust boundaries, and unbounded network/loop/retry/resource behavior as material issues.
- Never log secrets from `.libra/vault.db`, cloud credentials, tokens, or provider API keys.
- Never put Cloudflare tokens in `worker/wrangler.jsonc`; use `.dev.vars`, dashboard secrets, or wrangler secrets.
- The publish Worker scaffold from `libra publish init` makes `worker/wrangler.jsonc` user-owned except LIBRA-MANAGED bindings: `LIBRA_PUBLISH_DB`, `LIBRA_PUBLISH_BUCKET`, and `ASSETS`.

## Git Compatibility Notes

- Libra is Git-compatible but intentionally differs in some areas. Check `COMPATIBILITY.md` before assuming Git parity.
- `worktree remove` intentionally does not delete directories by default.
- `lfs` intentionally uses `.libra_attributes`.
- `submodule` and `subtree` are intentionally out of scope.
- Object format is globally pinned from `core.objectformat`; avoid hard-coding SHA-1-only assumptions.

## Performance Guidance

- Favor streaming I/O, bounded buffers, and batched operations for object traversal, pack operations, status scans, LFS transfer, and network protocol code.
- Avoid unbounded retries, unbounded caches, repeated full scans, and loading large pathspecs or blobs into memory without a clear cap.
- When touching hot paths, add or update tests and, where practical, benchmark or smoke-test large repository behavior.

## How Copilot Should Assist

- Use the real current module layout from this file and `AGENTS.md`; do not invent multi-crate architecture.
- For command changes, suggest the public-surface checklist above rather than only changing the Rust handler.
- For test suggestions, prefer concrete repo commands and exact integration target names.
- For AI/provider work, keep provider-specific code isolated and include deterministic test coverage.
- For web or worker work, preserve the existing Next.js/OpenNext/Cloudflare setup and include the relevant pnpm checks.
- For reviews, lead with findings: security, correctness, data loss, compatibility drift, missing tests, and performance risks.

## Non-Goals

- Do not propose moving core logic out of Rust.
- Do not propose a standard `.git/` runtime layout unless explicitly asked; Libra's `.libra` SQLite-backed model is core to this repo.
- Do not silently paper over unsupported Git features. Unsupported or intentional differences need explicit errors, docs, and compatibility notes.
- Do not introduce broad backwards-compatibility shims unless there is a concrete persisted-data, shipped-behavior, external-consumer, or explicit user requirement.
