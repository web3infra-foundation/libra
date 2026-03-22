# Libra Project Context (GEMINI.md)

## Project Overview

Libra is an **AI agent–native version control system** written in Rust. It is designed to be fully compatible with Git's on-disk formats (`objects`, `index`, `pack`, `pack-index`) while utilizing **SQLite** for transactional management of metadata such as `config`, `HEAD`, and `refs`.

### Key Features
- **AI-First Design:** The `libra code` command provides a Terminal User Interface (TUI), Web interface, and Model Context Protocol (MCP) server for seamless AI agent collaboration.
- **Tiered Cloud Storage:** Supports S3-compatible backends (AWS S3, Cloudflare R2) with local LRU caching for large objects.
- **Vault-Backed Security:** Built-in vault for GPG commit signing and SSH key management, stored in an encrypted SQLite database.
- **Cloud Backup:** Integration with Cloudflare D1 (metadata) and R2 (objects) for repository synchronization.
- **Monorepo Optimized:** Designed for trunk-based development, omitting features like submodules/subtrees in favor of cloud-backed large object handling.

## Architecture & Module Organization

- `src/main.rs`: Binary entry point, initializes the Tokio runtime and dispatches CLI commands.
- `src/lib.rs`: Library root, providing sync/async execution helpers (`exec`, `exec_async`).
- `src/cli.rs`: CLI definition using `clap`, handling subcommand dispatch.
- `src/command/`: Subcommand implementations (e.g., `init`, `add`, `commit`, `push`, `code`).
- `src/internal/`: Core logic including AI agent framework, TUI, database models (SeaORM), and network protocols.
- `src/utils/`: Shared utilities for tiered storage, error handling, and testing.
- `tests/`: Extensive integration tests in `tests/command/` mirroring Git workflows.

## Building and Running

### Prerequisites
- **Rust Toolchain:** Stable for building, Nightly for formatting (`rustfmt`).
- **OpenSSL:** Required for network/crypto features (see `README.md` for Windows setup).

### Key Commands
- **Build:** `cargo build`
- **Run CLI:** `cargo run -- <COMMAND>` (e.g., `cargo run -- status`)
- **Interactive Mode:** `cargo run -- code` (TUI), `cargo run -- code --web` (Web), `cargo run -- code --stdio` (MCP).
- **Test:** `cargo test --all` (run all tests), `cargo test <test_name>` (run specific test).
- **Lint:** `cargo clippy --all-targets --all-features -- -D warnings` (**Zero warnings allowed**).
- **Format:** `cargo +nightly fmt --all` (enforces project style).

## Development Conventions

### Coding Style
- **Rust 2024 Edition.**
- **Indentation:** 4 spaces.
- **Naming:** `snake_case` (modules/fns), `PascalCase` (types/traits), `SCREAMING_SNAKE_CASE` (consts).
- **Imports:** Grouped as Standard → External → Crate (enforced by `rustfmt.toml`).

### Error Handling
- **No `unwrap()` or `expect()`:** Forbidden in production code (including startup). Use `Result` and propagate errors with `?`. `unwrap()` is only acceptable in tests or obviously infallible logic with an `// INVARIANT:` comment.
- **User-Friendly Errors:** All errors surfaced to users must be human-readable and actionable. Use `anyhow::Context` to provide high-level context.
- **Library Errors:** Use `thiserror` for domain-specific error types in library modules.

### Testing
- **Integration Tests:** Preferred for new commands, located in `tests/command/`.
- **Isolation:** Use `tempfile::tempdir()` and `utils::test::ChangeDirGuard` to prevent state leakage between tests.
- **Serial Execution:** Mark tests with `#[serial]` if they mutate global/shared state.

### AI Agent Guidelines
- **MCP Server:** `libra code --stdio` allows external agents (like Claude Desktop) to interact with the repository.
- **Structured Output:** All CLI failures provide stable exit codes and JSON error reports when `LIBRA_ERROR_JSON=1` or when `stderr` is not a TTY.

## Key Files
- `README.md`: General overview and usage examples.
- `CLAUDE.md`: Detailed repository structure and developer guide.
- `AGENTS.md`: Guidelines for AI agents reviewing or contributing to the codebase.
- `docs/cli-ux-improvement-plan.md`: Current roadmap for CLI and `config` command enhancements.
- `sql/sqlite_20260309_init.sql`: SQLite database schema for metadata storage.
