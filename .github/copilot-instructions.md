# Libra – Repository Custom Instructions for GitHub Copilot

## What this repo is

This repository (“libra”) implements the core Git engine re-written and extended in Rust: object storage, commit-graph, packfile reader/writer, MIDX/multi-pack index, worktree semantics and transport layer. It’s foundational to the larger monorepo ecosystem at Web3Infra Foundation and is designed for large-scale, multi-client, content-addressed version control.
When generating code or design suggestions, assume this context: high concurrency, large object graphs, cross-crate modular architecture, Git compatibility (SHA-1 & SHA-256), performance/memory sensitivity, and Rust ecosystem conventions.

## Languages & defaults

- Primary language: Rust (edition 2021 or later).
- Async runtime: Tokio. Logging/tracing: tracing crate.
- Error handling: for libraries use thiserror; for binaries/tests/tools use anyhow.
- Serialization: serde; CLI argument parsing: clap.
- Use unsafe only when absolutely necessary — if used, include // SAFETY: comment with invariants and add tests.
- Minor scripting / tooling around the engine may use Python or Bash, but core logic should stay in Rust.

## Build & run

- Local iterative workflow: cargo build -p <crate>, cargo test -p <crate>, cargo bench -p <crate>.
- Integrated build (monorepo context): support for Buck2 or Bazel is expected in the broader org; when prompting for build files, include Buck2 macro snippet.
- For CI/integration: assume buck2 build //libra/... and buck2 test //libra/... as canonical commands.

## Repository architecture & major components

- Crate layout:
    - engine/ — core Git object engine (loose objects, object lookup, packfile read/write).
    - delta/ — delta-chain rewrite engines, multi-pack index support.
    - transport/ — network layer for fetch/push over Git protocol.
    - storage/ — content-addressed storage abstraction, pack caches, object caches.
    - cli/ — command-line utilities, interactive inspection, diagnostics.
    - common/ — shared utilities (hashing, fan-out tables, error types).
- Avoid hard-coding paths or assumptions about repository size; design for millions of objects, multiple packfiles, multi-client concurrency.

## Coding style & quality

- Enforce rustfmt defaults. New/changed code should compile with zero warnings under cargo build --all-targets.
- Treat clippy warnings as errors on new code (e.g., #![deny(clippy::all)] in new crates).
- Avoid unwrap()/expect() in library code. Prefer returning Result<_, _> and propagating context via anyhow::Context or thiserror messages.
- Prefer iterator/slice APIs over heap allocations in hot paths. Use SmallVec, bytes, or no-std-friendly patterns when relevant.
- Document performance expectations for critical code paths (e.g., “expected throughput > X objects/second”, “allocation count < Y per object”).

## Performance & memory

- This engine targets very large repositories — focus on streaming I/O, minimal copying, O(n) algorithms, bounded memory overhead.
- When dealing with packfiles: consider fan-out tables, delta-chain depth, object reuse, object relocation, compression with zstd or deflate.
- Support both SHA-1 and SHA-256 object IDs; avoid assumptions about 20-byte vs 32-byte lengths.
- Provide micro-benchmarks (via criterion) for hot paths; include allocation and throughput metrics. If a change causes a regression (e.g., >5% drop in throughput or >10% increase in allocation count), document it in the PR description and update benchmark results files. Significant regressions (>10% performance drop or allocation increase) should also be noted in CHANGELOG.md.

## Git compatibility & hashing

- Must interoperate with standard Git objects, refs, packfile formats, index formats.
- Support both legacy SHA-1 and new SHA-256 object IDs; design migration paths and dual-stack invariants.
- When generating code proposals: explicitly document trade-offs (compatibility vs performance).
- Avoid assumptions like “object ID is 20 bytes” or “fan-out table always 256 entries” unless clearly parameterized.

## API & CLI guidelines

- Public crates: define stable, versioned APIs; avoid leaking internal pack/graph structures unless explicitly marked unstable.
- CLI tools: default to safe, read-only operations. Provide --dry-run, --json output where appropriate.
- Tools should allow inspection of object graphs, packfiles, deltas, multi-pack indexes, and expose metrics (size, object count, duplicates).

## Testing & quality

- Include unit tests, integration tests, property-based tests (via proptest) especially for object graph, pack behaviour.
- Use insta for snapshot testing when output is textual or structural.
- For concurrency/async code (Tokio), include tests that simulate multiple tasks reading/writing/storage simultaneously.
- For performance-critical modules: include benchmarks (criterion) and ensure no regressions on performance/memory.

## Observability & errors

- Use tracing spans and fields for operations (e.g., object lookup, pack read, delta apply). Avoid logging sensitive data.
- Log errors with context, including object ID, ref name, and any relevant parameters. Error messages should be actionable: propagate context, specify which object ID/ref failed, suggest remediation.
- Provide diagnostics tools/commands for users (e.g., libra inspect-pack, libra delta-stats), wiring logging and metrics.

## Documentation

- Use /// comments on public items; //! at module tops for architecture overviews.
- Provide README sections with architecture diagrams, sequence diagrams (e.g., pack read → object delta apply → commit lookup), and benchmarks summaries.
- Use English for broad audience; internal/international teams may include Chinese remarks but primary docs should be English.
- Suggest code examples for common tasks (e.g., “open packfile”, “iterate objects”, “resolve commit graph”).

## Git workflow & Pull Requests

- Trunk-Based Development: use short-lived branches; merge into main frequently.
- Commit messages: follow Conventional Commits (feat: …, fix: …, perf: …).
- Each PR description should include: problem statement, design decision, performance/alloc benchmark (if applicable), tests added, backward-compatibility implications.
- Update CHANGELOG.md for crates where public API is changed; follow semantic versioning.

## How Copilot should assist

- When the user asks for code: produce Rust snippet first; if build rule required, add Buck2 macro snippet after.
- When user asks for design or architecture advice: list multiple options, each with trade-offs (performance, memory, compatibility, complexity).
- When user asks for tests or benchmarks: include criterion example or proptest snippet.
- When user asks for CLI tool suggestions: include sample clap-derived argument parsing code + usage message + example invocation.
- Always assume the context of large-scale Git internals (objects, packs, deltas, large monorepo) and include that assumption where relevant.

## Non-goals

- Do not propose rewriting the entire Git protocol from scratch unless explicitly requested.
- Do not recommend shifting core logic out of Rust to dynamic languages unless there’s a compelling integration reason.
- Do not ignore compatibility with standard Git, unless the user explicitly states they are targeting a proprietary system only.
