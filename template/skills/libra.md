---
name = "libra"
description = "Project-local guidance for working with this libra-format repository. Seeded by `libra init` / `libra clone`. Customize freely."
version = "1.0.0"
allowed-tools = [
  "read_file",
  "list_dir",
  "grep_files",
  "search_files",
  "web_search",
  "shell",
  "apply_patch",
  "run_libra_vcs",
  "update_plan",
  "submit_task_complete"
]
---

# This project's libra repository

This skill was automatically created when the repository was initialized with `libra init` or cloned with `libra clone`.

**You should edit this file** to capture the specific knowledge, conventions, and workflows that AI agents (and humans) need when working on *this* codebase:
- Important architecture decisions
- Build, test, and lint commands used by the team
- Code style / review rules
- Domain-specific gotchas
- Preferred agent profiles or sub-agent usage

The universal libra knowledge (layout, CLI entry points, test layers, hard rules about `unwrap`, etc.) is always available from the built-in embedded skill. This file is your chance to make the agent *project-smart*.

## Quick reference — libra layout (do not assume .git)

- Metadata: `.libra/libra.db` (SQLite + SeaORM) + `.libra/vault.db`
- Objects/packs/index remain Git-compatible on disk
- Never look for `.git/`

## Essential commands (update these for your project)

```bash
# Format
cargo +nightly fmt --all

# Lint (the real gate)
LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings

# Fast check
LIBRA_SKIP_WEB_BUILD=1 cargo check

# Tests
cargo test --all
cargo test --test <target> -- --test-threads=1
```

## Core rules enforced in this repository

- Rust 2024, 4-space indent, Std/External/Crate import order
- **No `unwrap()`/`expect()` in `src/**`** (tests only; add `// INVARIANT:` comment for the rare infallible case)
- Actionable errors with context (what + which resource + fix hint)
- `_with_conn` suffix for DB helpers that take an existing connection

## When changing public CLI surface

Update in order:
1. `src/cli.rs`
2. `src/command/<name>.rs`
3. `COMPATIBILITY.md`
4. `docs/commands/<name>.md` (must have Examples section)
5. Tests + `tests/INDEX.md`
6. `pub const <CMD>_EXAMPLES`

## Further reading (inside this repo)

- `AGENTS.md` — primary agent guidance
- `Claude.md` — contributor setup, env vars, test layers
- `COMPATIBILITY.md`
- `docs/improvement/agent.md`

Activate with `/skill libra` inside `libra code`.

Happy (agentic) hacking!
