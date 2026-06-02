# AGENTS.md

## What this repo is
- `libra` is a single Rust 2024 crate: a Git-compatible, AI-agent-native VCS. It uses `.libra/libra.db` for config/HEAD/refs/AI runtime tables and `.libra/vault.db` for secrets; do not assume a `.git/` layout.
- The real entry flow is `src/main.rs` (tracing + 32 MiB CLI thread) -> `src/cli.rs::{parse,parse_async}` -> `src/command/*::execute_safe`. `src/lib.rs::{exec,exec_async}` are the embedding API.
- `src/cli.rs` owns clap grammar, schema preflight, global hash-kind pinning from `core.objectformat`, output mode resolution, and dispatch. Touch it when adding/changing public CLI surfaces.
- Major boundaries: `src/command/` subcommands; `src/internal/ai/` agent/runtime/provider/tool/session/MCP/orchestrator stack; `src/internal/protocol/` Git/HTTP/SSH/LFS clients; `src/internal/publish/` Rust publish pipeline; `src/utils/` storage/path/error/output/test helpers.
- `web/` is a Next.js static export embedded into the Rust binary; `worker/` is the OpenNext/Cloudflare Worker for read-only `libra publish` hosting.

## Commands agents commonly guess wrong
- Format: `cargo +nightly fmt --all` (`rustfmt.toml` uses unstable features and Std/External/Crate import grouping). Check-only: `cargo +nightly fmt --all --check`.
- Lint gate: `LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`.
- Fast compile: `LIBRA_SKIP_WEB_BUILD=1 cargo check` or `LIBRA_SKIP_WEB_BUILD=1 cargo build`.
- Default tests: `cargo test --all` (deterministic L1; credential/live layers skip when env is absent).
- Single integration target: `cargo test --test <target> -- --test-threads=1`. Prefer `<target>::<test_fn>` when naming tests in PRs/issues.
- CLI smoke: `cargo run -- <cmd>`.
- Web embed check: `pnpm --dir web install --frozen-lockfile && pnpm --dir web lint && pnpm --dir web build`, then assert no static-export drift with `git status --porcelain -- web/out` (must be empty; the compat-web-check CI job inlines this check).
- Worker checks from `worker/`: `pnpm lint`, `pnpm test`, `pnpm test:miniflare`, `pnpm build`; e2e uses `pnpm e2e:serve` on `127.0.0.1:3127` plus `pnpm e2e`.
- CI-required consistency checks (de-scripted — there is no `scripts/` dir): run `cargo test --test compat_matrix_alignment`, which covers both the `COMPATIBILITY.md` ↔ `src/cli.rs::Commands` drift check and the `docs/commands/code-control.md` ↔ Code UI router coverage check; `web/out` drift is the inline `git status --porcelain -- web/out` in the compat-web-check job. An integration-plan consistency check is still aspirational (see `docs/development/integration-test-plan.md` BASELINE_GAP-INTEG-008).

## Build and generated-output quirks
- `build.rs` runs `pnpm install --frozen-lockfile` and `pnpm run build` in `web/` unless `LIBRA_SKIP_WEB_BUILD=1`; skipped builds create a stub `web/out/index.html`.
- CI uses Node 22 and pnpm 11.1.0 for `web/`. `build.rs` may add `NODE_OPTIONS=--experimental-sqlite` for Node 22/23.
- `compat-web-check` is the only base CI job that must not skip the web build; TUI automation scenarios also set `LIBRA_SKIP_WEB_BUILD=0` because they need the real embedded Next.js app.
- The publish Worker scaffold produced by `libra publish init` makes `worker/wrangler.jsonc` user-owned except LIBRA-MANAGED bindings (`LIBRA_PUBLISH_DB`, `LIBRA_PUBLISH_BUCKET`, `ASSETS`). Never put Cloudflare tokens in wrangler config; use `.dev.vars` or dashboard/wrangler secrets.

## Tests and feature gates
- `tests/INDEX.md` is the authoritative list of every integration `--test` target. Add/update its row when adding, renaming, or removing a top-level integration target.
- Files under `tests/compat/` are not auto-discovered by Cargo; every compat guard needs a `Cargo.toml [[test]]` entry and a row in `tests/compat/README.md`.
- TUI/PTY automation needs all three: `--features test-provider`, `LIBRA_ENABLE_TEST_PROVIDER=1`, and `--test-threads=1`. CI runs at least `code_ui_scenarios`, `harness_self_test`, `code_codex_default_tui_test`, `code_ui_remote_lease_matrix`, and `code_ui_remote_sse_matrix` this way.
- Network smoke: `cargo test --features test-network --test network_remotes_test -- --test-threads=1`.
- Live AI: `cargo test --features test-live-ai --test ai_agent_test --test ai_chat_agent_test -- --test-threads=1 --nocapture` with `DEEPSEEK_API_KEY`; live cloud uses `--features test-live-cloud` plus `LIBRA_D1_*` and `LIBRA_STORAGE_*` credentials.
- `.env.test` lines must keep `export`; otherwise `source .env.test` sets shell-local vars and cargo child processes silently skip L2/L3 tests.
- Use `tempfile::tempdir()` plus `utils::test::ChangeDirGuard`; CLI-level tests should use helpers in `tests/command/mod.rs` so `HOME`, `XDG_CONFIG_HOME`, `LIBRA_CONFIG_GLOBAL_DB`, `LANG`, and `LC_ALL` are isolated.
- Mark tests `#[serial]` if they mutate process cwd, global env, shared ports, config DBs, or other global state.

## Public-surface change checklist
- New or changed command: update `src/cli.rs`, the matching `src/command/<name>.rs`, `COMPATIBILITY.md`, command docs under `docs/commands/`, tests under `tests/command/`, and `tests/INDEX.md`.
- Every visible command/help surface must render examples (`pub const <CMD>_EXAMPLES` wired through clap `after_help`) and every `docs/commands/<name>.md` page needs `## Examples` or `## Common Commands`.
- New stable error codes in `src/utils/error.rs` must be documented in `docs/error-codes.md`; `libra help error-codes` includes that doc at compile time.
- If changing compatibility semantics, run `cargo test --test compat_matrix_alignment` and update declined/intentional notes under `docs/improvement/compatibility/` when relevant.
- If changing SQL, update bootstrap/migrations under `sql/`; `sql/publish/` is the Worker D1 schema and is independent from `.libra/libra.db` runtime schema.

## Code conventions that are enforced here
- Do not add `unwrap()` or `expect()` in production `src/**` paths. Existing compat guards scan broadly; tests may use them, and truly infallible production cases need a short `// INVARIANT:` comment.
- User-facing errors must be actionable: include what failed, the affected path/ref/object/resource, and a fix hint when known. Prefer `anyhow::Context` for CLI flows and `thiserror` for domain/library errors.
- Command modules should expose clap args and structured `execute_safe`-style handlers; document externally visible side effects and error mapping on command entry points.
- Database helpers that accept an existing connection follow the `_with_conn` naming pattern to preserve transaction safety.
- For AI provider work, keep provider-specific code under `src/internal/ai/providers/<provider>/` and satisfy common contracts in `completion/`; fake/deterministic provider paths are for tests.

## Review bias to preserve
- Review with high recall for security, data loss/corruption, auth/tenancy, migrations, external APIs, concurrency, retries/idempotency, hot-path performance, and missing tests/docs for changed behavior.
- Treat production `unwrap()`/`expect()`, silent failure paths, unsafe secret/PII logging, missing validation at trust boundaries, and unbounded network/loop/retry/resource behavior as material findings.
- `worktree remove` intentionally does not delete directories by default; `lfs` intentionally uses `.libra_attributes`; `submodule`/`subtree` are intentionally out of scope. Check `COMPATIBILITY.md` before assuming Git parity.

## Instruction-file precedence
- Root `AGENTS.md` is the primary OpenCode/agent guidance. `.codex/AGENTS.md` should stay a thin pointer here; it currently contains stale copied fragments, so prefer updating this file first.
- `CLAUDE.md` is detailed and mostly current. `.github/copilot-instructions.md` is stale about architecture (mentions non-existent `engine/`, `delta/`, `transport/` top-level crates); trust Cargo/source over that file.
