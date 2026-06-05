# CLAUDE.md

## Project Overview

Libra is an **AI agent–native version control system** written in Rust. It partially implements a Git client with full on-disk format compatibility (`objects`, `index`, `pack`, `pack-index`) while using SQLite for transactional metadata (`config`, `HEAD`, `refs`). It is designed for monorepo/trunk-based development with tiered cloud storage (S3/R2) and a Cloudflare D1/R2 backup path.

The `libra code` command launches an interactive TUI (with a background web server, MCP server, and an automation-control session surface) for collaborative AI-agent and human-driven development. The Git surface is governed by a four-tier compatibility matrix (`supported` / `partial` / `unsupported` / `intentionally-different`) tracked in [`COMPATIBILITY.md`](COMPATIBILITY.md); AI-only commands (`code`, `code-control`, `automation`, `usage`, `graph`, `sandbox`, `agent`, `publish`) are explicitly Libra-only extensions.

The repository also contains a Next.js frontend (`web/`) embedded into the binary via `rust-embed` and a Cloudflare Worker (`worker/`) for read-only `libra publish` hosting.

## Repository Structure

```
src/
├── main.rs                      # Binary entry point (tracing setup, 32 MiB-stack worker thread, tokio runtime)
├── lib.rs                       # Library root, sync/async exec helpers
├── cli.rs                       # Clap CLI definition, subcommand dispatch, hash-kind preflight
├── common_utils.rs, git_protocol.rs, lfs_structs.rs
├── command/                     # All subcommand implementations (~61 top-level modules + 9 under agent/)
│   ├── mod.rs                   # Re-exports, shared helpers (load/save objects, auth)
│   ├── init.rs, clone.rs, add.rs, commit.rs, push.rs, pull.rs, fetch.rs, fsck.rs
│   ├── status.rs, log.rs, show.rs, diff.rs, blame.rs, shortlog.rs, describe.rs
│   ├── branch.rs, tag.rs, switch.rs, checkout.rs, merge.rs, rebase.rs, cherry_pick.rs
│   ├── reset.rs, restore.rs, remove.rs, mv.rs, clean.rs, stash.rs, revert.rs
│   ├── reflog.rs, config.rs, remote.rs, worktree.rs, worktree-fuse.rs, cloud.rs, lfs.rs, lfs_schema.rs
│   ├── bisect.rs, cat_file.rs, ls_remote.rs, show_ref.rs, symbolic_ref.rs, rev_parse.rs, rev_list.rs, grep.rs
│   ├── open.rs, index_pack.rs, db.rs, hooks.rs, web_assets.rs, usage.rs
│   ├── code.rs, code_control.rs, code_control_files.rs   # `libra code` TUI + automation control session
│   ├── automation.rs, graph.rs, sandbox.rs, publish.rs   # AI automation, version-graph, sandbox, Worker publish
│   └── agent/                   # `libra agent` subcommands (checkpoint, clean, doctor, hooks, push, rpc, session, status)
├── internal/                    # Core logic
│   ├── ai/                      # AI Agent Infrastructure
│   │   ├── agent/               # Agent framework: builder, profiles (architect/coder/…), runtime, classifier, budget
│   │   ├── agent_run/           # Run records: budget, context_pack, decision, evidence, patchset, permission, task
│   │   ├── automation/          # Rule-based automation runtime, scheduler, history, executor
│   │   ├── codex/               # Codex protocol/schema bridge (history, model, schema_v2, view)
│   │   ├── commands/            # Agent command parsing & dispatch (+ embedded/*.md command prompts)
│   │   ├── completion/          # CompletionModel trait, request/response, retry/throttle, JSON repair
│   │   ├── context_budget/      # Context window allocator, compaction, frame, handoff, memory anchors
│   │   ├── goal/                # Goal state, supervisor, verifier, spec
│   │   ├── hooks/               # Git hooks integration (config, lifecycle, runner, providers)
│   │   ├── intentspec/          # Intent canonicalisation, draft/repair/validator/review/scope/profiles
│   │   ├── mcp/                 # Model Context Protocol server (server, resource)
│   │   ├── observed_agents/     # External-agent capture adapters (Claude Code, Gemini, …) + redaction/preview
│   │   ├── orchestrator/        # Plan/decide/execute pipeline (planner, decider, executor, verifier, replan, gate, ACL)
│   │   ├── permission/          # Permission rules, evaluation, approved-permission inheritance
│   │   ├── projection/          # Thread/intent projection index, resolver, rebuild scheduler
│   │   ├── prompt/              # Prompt engineering (builder, context, dynamic_context, rules, loader, embedded/*.md)
│   │   ├── providers/           # LLM backends (anthropic, openai, deepseek, gemini, kimi, zhipu, ollama, fake) + factory/transform
│   │   ├── runtime/             # Runtime contracts, phase3/phase4 state machines, hardening, environment, snapshot
│   │   ├── sandbox/             # Command-safety policy, runtime, macOS seatbelt SBPL policies
│   │   ├── session/             # Session state, JSONL store, file history, migration
│   │   ├── skills/              # Skill loader/scanner/parser/dispatcher
│   │   ├── sources/             # External source pool (MCP, OpenAPI) configuration
│   │   ├── tools/               # Tool registry, handlers (apply_patch, shell, read_file, grep, plan, …), semantic-search tools
│   │   ├── usage/               # Usage stats: recorder, pricing, query, format
│   │   └── web/                 # Code-UI bridge (code_ui.rs, headless)
│   ├── tui/                     # Terminal UI (ratatui + crossterm): app, chatwidget, history_cell, slash_command, theme
│   ├── model/                   # Sea-ORM data models (config, config_kv, reference, reflog, object_index, schema_version, ai_*)
│   ├── protocol/                # Network clients (git, https, ssh, lfs, local)
│   ├── publish/                 # Read-only Cloudflare Worker publishing (contract, snapshot, upload, preflight, incremental, ai_export)
│   ├── db/migration.rs          # Versioned schema migrations runner (MigrationRunner)
│   ├── db.rs                    # SQLite database initialization
│   ├── vault.rs                 # libvault-backed secret storage
│   ├── branch.rs, tag.rs, config.rs, head.rs, reflog.rs
│   └── log/                     # Log formatting & date parsing
└── utils/                       # Shared utilities
    ├── client_storage.rs        # Tiered storage (local + S3/R2 with LRU cache)
    ├── storage/                 # Storage backends (local, remote, tiered, publish_storage)
    ├── d1_client.rs             # Cloudflare D1 client
    ├── error.rs, output.rs      # CliError/CliResult + stable error codes; OutputConfig (--json / --machine)
    ├── pager.rs, ignore.rs, lfs.rs, fuse.rs, worktree.rs
    ├── object.rs, object_ext.rs, tree.rs, path.rs, path_ext.rs, storage_ext.rs, text.rs, convert.rs, util.rs
    └── test.rs                  # Test helpers (ChangeDirGuard, setup_with_new_libra_in)

tests/                           # 96 top-level integration test files + 17 tests/compat/ surface guards, layered L1/L2/L3 (see "Test Layers" below)
├── command/                     # Per-command integration tests mirroring real Git workflows
├── compat/                      # Compatibility-surface guards (must be registered as [[test]] in Cargo.toml)
├── harness/, helpers/, fixtures/, data/, objects/
├── ai_*.rs                      # AI agent, runtime, provider, scheduler, goal, projection, context, …
├── code_*.rs                    # `libra code` CLI dispatch, runtime, TUI scenarios, remote SSE/lease/state matrices
├── publish_*.rs                 # Publish snapshot/upload/preflight/refs/redaction/ai_export tests
└── command_test.rs, e2e_mcp_flow.rs, mcp_integration_test.rs, network_remotes_test.rs, storage_r2_test.rs, …

benches/ai_runtime_baseline.rs   # Criterion-style runtime baseline benchmark
examples/                        # `hello_world.rs`, `multi_agent.toml`
build.rs                         # Builds the Next.js web frontend into web/out/ unless LIBRA_SKIP_WEB_BUILD=1
docs/                            # Community docs, contributing guide, agent specs, improvement walkthroughs, error-codes
sql/
├── sqlite_20260309_init.sql                 # SQLite bootstrap schema
├── sqlite_20260415_ai_runtime_contract.sql  # AI runtime contract extension
├── migrations/                              # Versioned forward + matching `_down.sql` migrations (YYYYMMDDNN naming)
└── publish/                                 # Publish Worker D1 schema (0001_publish.sql, …)
template/                        # Git hook templates (pre-commit.sh, pre-commit.ps1, exclude, description) + sandbox seccomp filter (seccomp-default.json)
web/                             # Next.js 16 frontend (pnpm); built into web/out/ and embedded via rust-embed
worker/                          # Cloudflare Worker for read-only publishing (OpenNext + wrangler + playwright)
```

## Build & Development Commands

### Essential Commands

```bash
# Format code (requires nightly toolchain)
cargo +nightly fmt --all

# Lint — all warnings must be resolved before committing (all features on)
cargo clippy --all-targets --all-features -- -D warnings

# Quick compile check (skip the Next.js web build for speed)
LIBRA_SKIP_WEB_BUILD=1 cargo check
LIBRA_SKIP_WEB_BUILD=1 cargo build

# Run full test suite (L1 only by default; L2/L3 auto-skip when env vars are unset)
cargo test --all

# Run specific tests
cargo test command::init_test
cargo test add_test

# Run the CLI
cargo run -- <command>          # e.g. cargo run -- status

# Build the embedded web frontend (normally driven by build.rs)
pnpm --dir web install --frozen-lockfile && pnpm --dir web build
```

### Cargo Features

| Feature | Purpose |
|---------|---------|
| `worktree-fuse` | Enable Unix FUSE-backed worktree commands (Linux/macOS only) |
| `test-network` | Gate L2 tests requiring outbound network but no secrets |
| `test-live-ai` | Gate L3 tests calling real LLM APIs |
| `test-live-cloud` | Gate L3 tests hitting real D1/R2 endpoints |
| `test-provider` | Deterministic hidden provider for local TUI automation tests (requires `LIBRA_ENABLE_TEST_PROVIDER=1`) |
| `subagent-scaffold` | Schema-only sub-agent contract scaffold (CEX-S2-10, gated on CP-4 in production) |

### CI Pipeline (`.github/workflows/base.yml`)

All PRs must pass these jobs on the `[self-hosted]` runner pool:
1. **compat-rustfmt** — `cargo +nightly fmt --all --check`
2. **compat-clippy** — `cargo clippy --all-targets --all-features -- -D warnings` (with `LIBRA_SKIP_WEB_BUILD=1`)
3. **compat-web-check** — `pnpm --dir web lint` + `pnpm --dir web build`, then an inline `git status --porcelain -- web/out` drift check so `web/out/` cannot drift from `WebAssets`
4. **compat-redundancy** — directory-shape check on `third-party/rust/crates`
5. **compat-offline-core** — `cargo test --test compat_matrix_alignment` + `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` + `cargo test --all` + TUI automation matrices under `--features test-provider` and `--test-threads=1`
6. **compat-network-remotes** — `cargo test --features test-network --test network_remotes_test`

Additional workflows: `codeql.yml` (security analysis), `model-generation-nightly.yml` (nightly model-generation matrix), `release.yml` (release pipeline).

## Test Layers

Libra tests are organised into three layers — `cargo test --all` runs L1 only; L2/L3 are silently skipped when their env vars are unset. See `docs/tests.md` for the canonical guide.

| Layer | Dependencies | Trigger |
|-------|--------------|---------|
| **L1 — Deterministic** | None (tempdir, in-memory stores, mock models) | `cargo test --all` |
| **L2 — Network** | GitHub token for temporary repo creation | `LIBRA_TEST_GITHUB_TOKEN` + `LIBRA_TEST_GITHUB_NAMESPACE` |
| **L3 — Live Services** | Real AI API keys (`DEEPSEEK_API_KEY`, `MOONSHOT_API_KEY`, …) or cloud credentials (`LIBRA_D1_*`, `LIBRA_STORAGE_*`, `LIBRA_TEST_S3_*`) | Set the relevant env vars |

Gate L2 / L3 tests with the small `env_var_is_set(name) -> bool` helper (see e.g. [`tests/cloud_storage_backup_test.rs:30`](tests/cloud_storage_backup_test.rs)) followed by an early `eprintln!("skipped (...)")` return when a required var is unset — missing vars print "skipped", never fail. Copy `.env.test.example` → `.env.test` and `source` it before running the full suite (the `export` prefix is required).

## Black-box CLI integration tests (Git-compatible `libra` commands)

Separate from Cargo `--test` targets: compiled `libra` in isolated temp repos, driven by [`tools/integration-runner/`](tools/integration-runner/).

| Artifact | Role |
|----------|------|
| [`docs/development/integration-scenarios.yaml`](docs/development/integration-scenarios.yaml) | Scenario registry (wave, gh_required, assertion categories) |
| [`docs/development/integration-scenarios/<id>.md`](docs/development/integration-scenarios/) | Per-scenario steps/assertions (edit the file for the command you change) |
| [`docs/development/integration-test-plan.md`](docs/development/integration-test-plan.md) | Coverage matrix §2.3, isolation §3, PR Test Plan §8 |
| `tools/integration-runner/src/scenarios/<id>.rs` + [`registry.rs`](tools/integration-runner/src/registry.rs) | Automated execution |
| [`docs/development/integration-scenarios/README.md` Command → Scenario Map](docs/development/integration-scenarios/README.md#命令--场景映射command--scenario-map) | Which scenario owns each Git-compat command (use to find what to update) |

**Changing a Git-compat command** — changing any Git-compat command obliges you to keep the integration test scheme **and** the command docs in sync. First find the command's owner scenario in the [Command → Scenario Map](docs/development/integration-scenarios/README.md#命令--场景映射command--scenario-map), then sync, in order: `src/cli.rs` + `src/command/<name>.rs` → `COMPATIBILITY.md` → `docs/commands/<name>.md` → plan §2.3 matrix → owner `integration-scenarios/<id>.md` + yaml → runner scenario(s) → `tests/command/` as needed → `cargo test --test compat_matrix_alignment` → `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` → `run --only <owner-ids>` (add `run-live` for real-remote changes). A new Git-compat command must add a map row + at least one `cli.<cmd>-smoke` scenario; none may go uncovered.

**Commands**: `check-plan`; `run --waves 0,1,2`; `run --only cli.<id>`; `run-live --only live.*` (needs `gh`). Not part of `cargo test --all`.

## Coding Conventions

### Language & Style

- **Rust edition 2024**, 4-space indentation
- **Naming**: `snake_case` for modules/functions, `PascalCase` for types/traits, `SCREAMING_SNAKE_CASE` for constants
- **Imports**: Grouped as Standard → External → Crate per `rustfmt.toml` (`group_imports = "StdExternalCrate"`, `imports_granularity = "Crate"`); avoid wildcard imports except in tests

### Error Handling

- **CLI flows**: Use `anyhow::Result` for flexible error propagation
- **Library code**: Use `thiserror` with domain-specific error enums (e.g., `InitError`, `GitError`)
- **Command handlers**: `execute(args)` is the public async entry; may return early without Result for simple CLI feedback
- **Database operations**: `_with_conn` suffix for transaction-safe variants accepting `ConnectionTrait`
- **Avoid `unwrap()` / `expect()`**: Prefer returning `Result` and propagating errors with `?`, attaching human-readable context via `.context("...")` or `.with_context(|| format!(...))` so end-users see actionable messages instead of panics. `unwrap()`/`expect()` are acceptable only in **unit/integration tests** and where the logic is **obviously infallible** (e.g., compile-time-known constants) with a brief `// INVARIANT:` comment. All other code — including program startup and initialization — must handle errors gracefully and return actionable messages.
- **User-friendly error messages**: All errors surfaced to the user must be human-readable and actionable. Avoid exposing raw internal errors; wrap them with context that explains *what went wrong*, *which resource was affected* (path, ref, object ID), and *how to fix it*.

### Patterns

- **Command structure**: Each command in `src/command/<name>.rs` with an `Args` struct (clap derive) and `async fn execute(args)`
- **Extension traits**: `TreeExt`, `CommitExt`, `BlobExt` add methods to git-internal types
- **Builder pattern**: Used for `AgentBuilder`, with validation in builder methods returning `Result`
- **Guard pattern (RAII)**: `ChangeDirGuard` for safe directory changes in tests
- **Provider pattern**: Each AI provider has `mod.rs` + `client.rs` + `completion.rs`

### Documentation

- Module-level `//!` doc comments explaining purpose
- Function-level `///` with `# Arguments`, `# Returns`, `# Example` sections where helpful
- Architecture notes as block comments (`/* ... */`) for complex patterns like `_with_conn`
- Add comments only when control flow is non-obvious (async handling, SQLite migrations)

## Testing Guidelines

- **Integration tests** in `tests/command/` mirror real Git workflows; prefer these for new commands
- **Compatibility-surface tests** in `tests/compat/` guard against regressions in CLI flag/help wording, declined-feature drift, and the production `unwrap()` audit. Each `*.rs` under `tests/compat/` must be registered as a `[[test]]` entry in `Cargo.toml` (Cargo's default discovery only picks up files directly under `tests/`). New compat guards must also add a row to the inventory table in [`tests/compat/README.md`](tests/compat/README.md). See [`docs/tests.md`](docs/tests.md) `Compatibility-surface tests` section for the full convention.
- **Cross-cutting `--help` EXAMPLES contract**: every visible command in `src/cli.rs::Commands` ships with a `pub const <CMD>_EXAMPLES` constant wired via `#[command(after_help = …)]` (or `after_help = command::<name>::<CMD>_EXAMPLES` on the parent subcommand binding in `cli.rs` for `Subcommand`-style commands). Three compat guards protect this contract: `compat_help_examples_banner` (every `<cmd> --help` renders an EXAMPLES section), `cli::tests::root_after_help_lists_every_visible_command` (every non-hidden command appears in a Command Groups row), and `compat_command_docs_examples_section` (every `docs/commands/<name>.md` page carries an Examples / Common Commands heading). New commands must satisfy all three.
- **Isolation**: Use `tempfile::tempdir()` and `utils::test::ChangeDirGuard` to isolate state
- **Serial execution**: Mark tests `#[serial]` (from `serial_test` crate) if they mutate shared state
- **Async tests**: Use `#[tokio::test]` (or `flavor = "multi_thread"` when needed)
- **Fixtures**: Keep small and local in `tests/data/` and `tests/fixtures/`; reuse helpers from `tests/command/mod.rs`, `tests/harness/`, and `tests/helpers/`
- **Gating**: Use the `env_var_is_set(name)` helper pattern (see `tests/cloud_storage_backup_test.rs:30`) plus an early `eprintln!("skipped (set ...)")` return so missing vars print a skip notice and do not fail the test. Match the L1/L2/L3 layering and the matching `test-network` / `test-live-ai` / `test-live-cloud` Cargo features
- **Coverage**: Pair new commands/options with at least one end-to-end test plus a focused unit test, and an entry in `COMPATIBILITY.md` if you change the Git surface. New `StableErrorCode` variants must also be added to `docs/error-codes.md` (the `compat_error_codes_doc_sync` guard fails the build otherwise).

## Commit & PR Conventions

### Commit Messages

Use typed summaries with optional scope:
```
feat(status): support porcelain v2 (#82)
fix(push): record tracking reflog (#81)
refactor(ai): extract completion trait
test(merge): add three-way merge coverage
docs(readme): update provider table
```

### PR Requirements

- All CI checks pass (format, clippy zero-warnings, tests)
- State intent, linked issues, and tests run
- Include repro steps or sample CLI output for user-visible changes
- Keep changes small and cohesive
- Update README/CLI docs when adding flags or altering behavior

## Key Dependencies

| Category | Crates |
|----------|--------|
| Git internals | `git-internal` |
| CLI | `clap` (derive) |
| Async runtime | `tokio` (multi-thread), `async-stream`, `tokio-stream` |
| Database | `sea-orm` + `sqlx-sqlite` |
| HTTP server | `axum`, `tower`, `tower-http`, `hyper-util` |
| HTTP client | `reqwest` (rustls), `tokio-tungstenite` |
| AI/LLM | `rig-core` (rmcp/rustls features), `rmcp` (MCP protocol), `dagrs` (DAG scheduler) |
| TUI | `ratatui`, `crossterm`, `unicode-width`, `pulldown-cmark` (markdown render) |
| Cloud storage | `object_store` (S3/R2/Azure/GCP), `lru-mem` |
| Embedded assets | `rust-embed`, `mime_guess` |
| Code analysis | `tree-sitter` + `tree-sitter-bash`, `tree-sitter-rust` |
| FUSE worktrees (Unix) | `libfuse-fs`, `rfuse3`, `pager`, `libc` |
| Secret storage | `libvault` (SQLite-backed) |
| Error handling | `anyhow`, `thiserror` |
| Serialization | `serde`, `serde_json`, `toml` |
| Logging | `tracing`, `tracing-subscriber` |
| Diff/patch | `diffy`, `diffs`, `similar` |
| Dev/test | `serial_test`, `testcontainers`, `assert_cmd`, `tempfile`, `gag`, `portable-pty`, `pgp` |

## Database Schema

SQLite database at `.libra/libra.db`. Tables fall into three groups:

- **Git core**: `config`, `config_kv`, `reference`, `reflog`, `rebase_state`, `object_index`, `schema_version`
- **AI threads & scheduling**: `ai_thread`, `ai_thread_participant`, `ai_thread_intent`, `ai_thread_provider_metadata`, `ai_scheduler_state`, `ai_scheduler_plan_head`, `ai_scheduler_selected_plan`, `ai_live_context_window`
- **AI runtime contracts**: `ai_index_intent_plan`, `ai_index_intent_task`, `ai_index_intent_context_frame`, `ai_index_plan_step_task`, `ai_index_run_event`, `ai_index_run_patchset`, `ai_index_task_run`, `ai_decision_proposal`, `ai_risk_score_breakdown`, `ai_validation_report`

Bootstrap files: `sql/sqlite_20260309_init.sql` (core + AI baseline) and `sql/sqlite_20260415_ai_runtime_contract.sql` (runtime-contract extension).

**Versioned migrations** live under `sql/migrations/` and are applied by `internal::db::migration::MigrationRunner`. Filenames follow `YYYYMMDDNN_<snake_case_name>.sql` (forward) with optional matching `*_down.sql` (rollback). Forward DDL must be idempotent (`CREATE TABLE IF NOT EXISTS …`). See `sql/migrations/README.md`.

The publish Worker uses its own D1 schema in `sql/publish/` (`0001_publish.sql`, `0002_publish_digest_check.sql`, `0003_publish_max_preview_trigger_replace.sql`, `0004_publish_refs_index.sql`).

## Environment Variables

### AI Providers
| Provider | API Key Env | Base URL Override |
|----------|-------------|-------------------|
| `gemini` | `GEMINI_API_KEY` | — |
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` |
| `anthropic` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` |
| `deepseek` | `DEEPSEEK_API_KEY` | `--api-base` only (no env var) |
| `kimi` | `MOONSHOT_API_KEY` | `MOONSHOT_BASE_URL` |
| `zhipu` | `ZHIPU_API_KEY` | `ZHIPU_BASE_URL` |
| `ollama` | — | `OLLAMA_BASE_URL` or `--api-base` |

### Cloud Storage (S3/R2)
`LIBRA_STORAGE_TYPE`, `LIBRA_STORAGE_BUCKET`, `LIBRA_STORAGE_ENDPOINT`, `LIBRA_STORAGE_REGION`, `LIBRA_STORAGE_ACCESS_KEY`, `LIBRA_STORAGE_SECRET_KEY`, `LIBRA_STORAGE_THRESHOLD`, `LIBRA_STORAGE_CACHE_SIZE`, `LIBRA_STORAGE_ALLOW_HTTP` (set to `"true"` to permit non-TLS HTTP endpoints, useful for local/dev S3-compatible stores)

### Cloud Backup (D1/R2)
`LIBRA_D1_ACCOUNT_ID`, `LIBRA_D1_API_TOKEN`, `LIBRA_D1_DATABASE_ID`

### Build & Runtime
- `LIBRA_SKIP_WEB_BUILD=1` — skip the Next.js web build in `build.rs` (set by every CI job except `compat-web-check`)
- `LIBRA_LOG`, `RUST_LOG` — `tracing-subscriber` env filter
- `LIBRA_LOG_FILE` — append-mode tracing sink path
- `LIBRA_PAGER` — pager override (falls back to system `PAGER` then `less`)
- `LIBRA_NO_HIDE_PASSWORD` — show password prompts in plain text (debugging)
- `LIBRA_CONFIG_GLOBAL_DB` — override the global config SQLite path
- `LIBRA_COMMITTER_NAME` / `LIBRA_COMMITTER_EMAIL` — committer identity overrides
- `LIBRA_SSH_COMMAND`, `LIBRA_SSH_STRICT_HOST_KEY_CHECKING` — SSH protocol tuning
- `LIBRA_CODE_LEASE_DURATION_MS` — `libra code` automation lease length
- `LIBRA_SANDBOX_ENFORCEMENT`, `LIBRA_SANDBOX_NETWORK_DISABLED`, `LIBRA_LINUX_SANDBOX_EXE`, `LIBRA_USE_LINUX_SANDBOX_BWRAP` — sandbox toggles (`docs/improvement/sandbox.md`)
- `LIBRA_ERROR_JSON`, `LIBRA_FINE_EXIT_CODES` — stable-error-code surface toggles

The following are baked-in constants (no env-var override) — listed
here so contributors do not waste time trying to set them at runtime:

- `LIBRA_VCS_TIMEOUT_SECONDS` (`src/internal/ai/mcp/resource.rs:86`) —
  MCP-side AI-VCS tool timeout, currently fixed at 120 s.
- `LIBRA_VCS_DEFAULT_APPROVAL_SCOPE` (`src/internal/ai/sources/mcp.rs:28`)
  — default approval scope for `run_libra_vcs`, currently `interactive`.
- `LIBRA_ISSUES_URL` (`src/utils/error.rs:59`) — canonical GitHub
  issues URL appended to internal-invariant error hints.

### Tests
- `LIBRA_TEST_GITHUB_TOKEN`, `LIBRA_TEST_GITHUB_NAMESPACE` — L2 GitHub gate (creates/deletes a temporary `libra-test-*` repo)
- `LIBRA_TEST_S3_ENDPOINT`, `LIBRA_TEST_S3_BUCKET`, `LIBRA_TEST_S3_ACCESS_KEY`, `LIBRA_TEST_S3_SECRET_KEY`, `LIBRA_TEST_S3_REGION`, `LIBRA_TEST_S3_ALLOW_HTTP` — L3 S3 protocol gate (separate from the R2 backup credentials above)
- `LIBRA_PUBLISH_LIVE_WORKER_ORIGIN`, `LIBRA_PUBLISH_LIVE_CLONE_DOMAIN`, `LIBRA_PUBLISH_LIVE_SLUG`, `LIBRA_PUBLISH_LIVE_FILE_PATH` — `publish_live` deploy-smoke gate
- `LIBRA_TEST_MEGA_SERVER` — LFS protocol live-server gate
- `LIBRA_ENABLE_TEST_PROVIDER` — activate the `test-provider` deterministic LLM for TUI scenarios (required alongside `--features test-provider`)
- `LIBRA_TEST_LOG`, `LIBRA_TEST_HOME`, `LIBRA_TEST_ENV` — test-only logging/home/sentinel overrides
