# CLAUDE.md

## Project Overview

Libra is an **AI agent–native version control system** written in Rust. It partially implements a Git client with full on-disk format compatibility (`objects`, `index`, `pack`, `pack-index`) while using SQLite for transactional metadata (`config`, `HEAD`, `refs`). It is designed for monorepo/trunk-based development with tiered cloud storage (S3/R2).

The `libra code` command launches an interactive TUI (with a background web server) for collaborative AI-agent and human-driven development. It also supports web-only and stdio/MCP modes for integration with AI clients like Claude Desktop.

## Repository Structure

```
src/
├── main.rs                      # Binary entry point (tokio runtime)
├── lib.rs                       # Library root, sync/async exec helpers
├── cli.rs                       # Clap CLI definition, subcommand dispatch
├── common_utils.rs              # Shared utility functions
├── git_protocol.rs              # Git protocol helpers
├── lfs_structs.rs               # LFS data types (used by command/lfs.rs and protocol/lfs_client.rs)
├── command/                     # All subcommand implementations (38 modules)
│   ├── mod.rs                   # Re-exports, shared helpers (load/save objects, auth)
│   ├── init.rs, clone.rs, add.rs, commit.rs, push.rs, pull.rs, fetch.rs
│   ├── status.rs, log.rs, show.rs, diff.rs, blame.rs, shortlog.rs, describe.rs
│   ├── branch.rs, tag.rs, switch.rs, checkout.rs, merge.rs, rebase.rs, cherry_pick.rs
│   ├── reset.rs, restore.rs, remove.rs, mv.rs, clean.rs, stash.rs, revert.rs
│   ├── reflog.rs, config.rs, remote.rs, worktree.rs, cloud.rs, lfs.rs
│   ├── open.rs, index_pack.rs   # Browser open, pack index operations
│   └── code.rs                  # `libra code` — TUI/Web/MCP entry
├── internal/                    # Core logic
│   ├── ai/                      # AI Agent Infrastructure
│   │   ├── agent/               # Agent framework, builder, profiles, runtime
│   │   ├── providers/           # LLM backends (gemini, openai, anthropic, deepseek, zhipu, ollama)
│   │   ├── tools/               # Tool registry & handlers (ApplyPatch, Shell, ReadFile, Grep, etc.)
│   │   ├── completion/          # CompletionModel trait, request/response types
│   │   ├── mcp/                 # Model Context Protocol server
│   │   ├── session/             # Session state & persistence
│   │   ├── prompt/              # Prompt engineering & templates
│   │   ├── commands/            # Agent command parsing & dispatch
│   │   └── hooks/               # Git hooks integration
│   ├── tui/                     # Terminal UI (ratatui + crossterm)
│   ├── model/                   # Sea-ORM data models (config, reference, reflog, object_index)
│   ├── protocol/                # Network clients (git, https, lfs, local)
│   ├── db.rs                    # SQLite database initialization
│   ├── branch.rs, tag.rs, config.rs, head.rs, reflog.rs
│   └── log/                     # Log formatting & date parsing
└── utils/                       # Shared utilities
    ├── client_storage.rs        # Tiered storage (local + S3/R2 with LRU cache)
    ├── d1_client.rs             # Cloudflare D1 client
    ├── test.rs                  # Test helpers (ChangeDirGuard, setup_with_new_libra_in)
    └── ...                      # Path, object, tree, ignore, LFS utilities

tests/
├── command/                     # Integration tests (one file per command)
│   ├── mod.rs                   # Shared test helpers
│   └── init_test.rs, add_test.rs, commit_test.rs, ...
├── objects/                     # Object-level tests
├── data/                        # Test fixtures (pack files, objects, indices)
├── command_test.rs              # Top-level command integration test
├── e2e_mcp_flow.rs              # MCP end-to-end tests
├── mcp_integration_test.rs      # MCP integration tests
├── ai_agent_test.rs             # AI agent tests
├── ai_chat_agent_test.rs        # AI chat agent tests
├── ai_dag_tool_loop_test.rs     # AI DAG tool loop tests
├── ai_storage_flow_test.rs      # AI storage flow tests
├── intent_flow_test.rs          # Intent flow tests
├── cloud_storage_backup_test.rs # Cloud storage backup tests
└── storage_r2_test.rs           # R2 storage tests

docs/                            # Community docs, contributing guide, agent specs
sql/sqlite_20240331_init.sql     # SQLite schema bootstrap
template/                        # Git hook templates (pre-commit, exclude)
third-party/                     # Buck2/Buckal vendored crate metadata (generated)
platforms/, toolchains/          # Buck2 platform & toolchain configs
```

## Build & Development Commands

### Essential Commands

```bash
# Format code (requires nightly toolchain)
cargo +nightly fmt --all

# Lint — all warnings must be resolved before committing
cargo clippy --all-targets --all-features -- -D warnings

# Quick compile check
cargo build
cargo check

# Run full test suite
cargo test --all

# Run specific tests
cargo test command::init_test
cargo test add_test

# Run the CLI
cargo run -- <command>          # e.g. cargo run -- status
```

### Buck2 Build (also required for CI)

```bash
cargo buckal build               # Build with Buck2
buck2 build //:libra             # Direct Buck2 invocation
cargo buckal migrate             # Regenerate Buck metadata after Cargo.toml changes
```

### CI Pipeline (`.github/workflows/base.yml`)

All PRs must pass these checks:
1. `cargo +nightly fmt --all --check` — formatting
2. `cargo clippy --all-targets --all-features -- -D warnings` — linting (zero warnings)
3. Redundancy check on `third-party/rust/crates`
4. `buck2 build //:libra` — Buck2 build
5. `cargo test --all` — full test suite

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
- **No `unwrap()` / `expect()` in non-test code**: Always anticipate possible errors and propagate them with `?`. Attach human-readable context via `.context("failed to ...")` or `.with_context(|| format!(...))` so end-users see actionable messages instead of panics. The only exception is when the invariant is provably guaranteed by immediately preceding code and documented with a `// INVARIANT:` comment. In test code, `unwrap()` is acceptable.

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
- **Isolation**: Use `tempfile::tempdir()` and `utils::test::ChangeDirGuard` to isolate state
- **Serial execution**: Mark tests `#[serial]` (from `serial_test` crate) if they mutate shared state
- **Async tests**: Use `#[tokio::test]` (or `flavor = "multi_thread"` when needed)
- **Fixtures**: Keep small and local in `tests/data/`; reuse helpers from `tests/command/mod.rs`
- **Coverage**: Pair new commands/options with at least one end-to-end test plus a focused unit test

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

- All CI checks pass (format, clippy zero-warnings, Buck2 build, tests)
- State intent, linked issues, and tests run
- Include repro steps or sample CLI output for user-visible changes
- Keep changes small and cohesive
- Update README/CLI docs when adding flags or altering behavior

## Key Dependencies

| Category | Crates |
|----------|--------|
| Git internals | `git-internal` |
| CLI | `clap` (derive) |
| Async runtime | `tokio` (multi-thread) |
| Database | `sea-orm` + `sqlx-sqlite` |
| HTTP server | `axum` |
| HTTP client | `reqwest` (rustls) |
| AI/LLM | `rig-core`, `rmcp` (MCP protocol) |
| TUI | `ratatui`, `crossterm` |
| Cloud storage | `object_store` (S3/R2/Azure/GCP) |
| Error handling | `anyhow`, `thiserror` |
| Serialization | `serde`, `serde_json` |
| Logging | `tracing`, `tracing-subscriber` |
| Diff/patch | `similar`, `diffy`, `diffs` |

## Database Schema

SQLite database at `.libra/libra.db` with tables: `config`, `reference`, `reflog`, `rebase_state`, `object_index`. Schema bootstrap in `sql/sqlite_20240331_init.sql`.

## Environment Variables

### AI Providers
| Provider | API Key Env | Base URL Override |
|----------|-------------|-------------------|
| `gemini` | `GEMINI_API_KEY` | — |
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` |
| `anthropic` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` |
| `deepseek` | `DEEPSEEK_API_KEY` | — |
| `zhipu` | `ZHIPU_API_KEY` | `ZHIPU_BASE_URL` |
| `ollama` | — | `OLLAMA_BASE_URL` or `--api-base` |

### Cloud Storage (S3/R2)
`LIBRA_STORAGE_TYPE`, `LIBRA_STORAGE_BUCKET`, `LIBRA_STORAGE_ENDPOINT`, `LIBRA_STORAGE_REGION`, `LIBRA_STORAGE_ACCESS_KEY`, `LIBRA_STORAGE_SECRET_KEY`, `LIBRA_STORAGE_THRESHOLD`, `LIBRA_STORAGE_CACHE_SIZE`, `LIBRA_STORAGE_ALLOW_HTTP` (set to `"true"` to permit non-TLS HTTP endpoints, useful for local/dev S3-compatible stores)

### Cloud Backup (D1/R2)
`LIBRA_D1_ACCOUNT_ID`, `LIBRA_D1_API_TOKEN`, `LIBRA_D1_DATABASE_ID`
