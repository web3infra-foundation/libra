## Git Internal Module

Git-Internal is a high-performance Rust library for Git internal objects, Pack files, and AI-assisted development workflows. It provides comprehensive support for Git's internal object storage format with delta compression, memory management, concurrent processing, and a structured AI object model that captures the full lifecycle of AI-driven code changes — from user intent through planning, execution, validation, and final decision.

## Overview

This library handles Git internal objects and Pack files efficiently, supporting both reading and writing with optimized memory usage and multi-threaded processing. Beyond the standard Git object model (Blob, Tree, Commit, Tag), it introduces a suite of **AI objects** (Intent, Plan, Task, Run, PatchSet, Evidence, Decision, and more) that record and audit every step of an AI agent's interaction with a codebase. These AI objects are stored as content-addressed JSON blobs in the Git object database, enabling reproducibility, auditability, and provenance tracking for AI-generated code changes.

## Quickstart

Decode a pack (offline):

```rust
use std::{fs::File, io::BufReader};
use git_internal::internal::pack::Pack;
use git_internal::hash::{set_hash_kind, HashKind};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // In production, hash kind is configured at the repository level. Set here for demonstration only.
    set_hash_kind(HashKind::Sha1);
    let f = File::open("tests/data/packs/small-sha1.pack")?;
    let mut reader = BufReader::new(f);
    let mut pack = Pack::new(None, Some(64 * 1024 * 1024), None, true);

    pack.decode(&mut reader, |_entry| {
        // Process each decoded object here (MetaAttached<Entry, EntryMeta>).
        // For example, index it, persist it, or feed it into your build pipeline.
    }, None::<fn(git_internal::hash::ObjectHash)>)?;
    Ok(())
}
```

## Modules at a glance

- `hash.rs`: object IDs and hash algorithm selection (thread-local), set once by your app.
- `internal/object`: object parse/serialize — standard Git objects (Blob, Tree, Commit, Tag) and AI objects (Intent, Plan, Task, Run, etc.).
- `internal/index` / `internal/metadata`: .git/index IO, path/offset/CRC metadata.
- `delta` / `zstdelta` / `diff.rs`: delta compression, zstd dictionary delta, line-level diff.
- `internal/pack`: pack decode/encode, waitlist, cache, idx building.
- `protocol/*`: smart protocol + HTTP/SSH adapters, wrapping info-refs/upload-pack/receive-pack.
- Docs: [docs/ARCHITECTURE.md (architecture)](docs/ARCHITECTURE.md), [docs/GIT_OBJECTS.md (objects)](docs/GIT_OBJECTS.md), [docs/GIT_PROTOCOL_GUIDE.md (protocol)](docs/GIT_PROTOCOL_GUIDE.md), [docs/ai.md (AI objects)](docs/ai.md), [docs/intentspec/INTENTSPEC.md (IntentSpec design)](docs/intentspec/INTENTSPEC.md).

## Key Features

### 1. Multi-threaded Processing

- Configurable thread pool for parallel object processing
- Concurrent delta resolution with dependency management
- Asynchronous I/O operations for improved performance

### 2. Advanced Memory Management

- LRU-based memory cache with configurable limits
- Automatic disk spillover for large objects
- Memory usage tracking and optimization
- Heap size calculation for accurate memory accounting

### 3. Delta Compression Support

- Offset Delta : References objects by pack file offset
- Hash Delta : References objects by SHA-1 hash
- Zstd Delta : Enhanced compression using Zstandard algorithm
- Intelligent delta chain resolution

### 4. Streaming Support

- Stream-based pack file processing
- Memory-efficient handling of large pack files
- Support for network streams and file streams

### 5. AI Object Model

A structured object graph that captures the full lifecycle of AI-assisted code changes:

```
 ①  User input
      │
      ▼
 ②  Intent (Draft → Active → Completed)
      │
      ▼
 ③  Plan (steps + context pipeline)
      │
      ▼
 ④  Task (unit of work with acceptance criteria)
      │
      ▼
 ⑤  Run (execution attempt: patching → validating)
      │
      ├── ⑥ ToolInvocation (action log)
      ├── ⑦ PatchSet (candidate diff)
      ├── ⑧ Evidence (test/lint/build results)
      │
      ▼
 ⑨  Decision (commit / retry / abandon / rollback)
      │
      ▼
 ⑩  Intent (Completed, commit hash recorded)
```

| Object | Role |
|--------|------|
| **Intent** | Captures user prompt and AI interpretation; entry/exit point of the workflow |
| **Plan** | Ordered sequence of steps derived from an Intent; supports revision chains |
| **Task** | Stable work identity with constraints and acceptance criteria |
| **Run** | Single execution attempt of a Task; accumulates artifacts |
| **ToolInvocation** | Records each tool call (file read, shell command, etc.) |
| **PatchSet** | Candidate code diff (unified format) relative to a baseline commit |
| **Evidence** | Validation result (test pass/fail, lint output, build log) |
| **Decision** | Terminal verdict on a Run (commit, retry, abandon, rollback) |
| **Provenance** | LLM configuration and token usage metadata for a Run |
| **ContextSnapshot** | Static capture of files/URLs/snippets at Run start |
| **ContextPipeline** | Dynamic sliding-window context accumulated during planning |

All AI objects share a common `Header` (UUID, timestamps, creator) and are serialized as JSON. See [docs/ai.md](docs/ai.md) for the full lifecycle, field-level documentation, and usage examples.

For authoring AI-driven code changes, see **[IntentSpec](docs/intentspec/INTENTSPEC.md)** — a machine-readable intent contract format (JSON Schema Draft 2020-12) that drives the orchestrator to produce a Task DAG, enforce verification gates, and bind SLSA provenance artefacts to every change. The doc includes the complete schema, field-by-field reference, and three worked examples (low / medium / high-assurance).

## Core Algorithms

### Pack Decoding Algorithm

1. Read and validate pack header (PACK signature, version, object count)
2. For each object in the pack:
   a. Parse object header (type, size)
   b. Handle based on object type:
      - Base objects: Decompress and store directly
      - Delta objects: Add to waitlist until base is available
   c. Resolve delta chains when base objects become available
3. Verify pack checksum

### Delta Resolution Strategy

- Waitlist Management : Delta objects wait for their base objects
- Dependency Tracking : Maintains offset and hash-based dependency maps
- Chain Resolution : Recursively applies delta operations
- Memory Optimization : Calculates expanded object sizes to prevent OOM

### Cache Management

- Two-tier Caching : Memory cache with disk spillover
- LRU Eviction : Least recently used objects are evicted first
- Size-based Limits : Configurable memory limits with accurate tracking
- Async Persistence : Background threads handle disk operations

### Object Processing Pipeline

```
Input Stream → Header Parsing → Object Decoding → Delta Resolution → Cache Storage → Output
                     ↓              ↓              ↓              ↓
                Validation    Decompression   Waitlist Mgmt   Memory Mgmt
```

## Performance Optimizations

### Memory Allocator Recommendations

> [!TIP]
> Here are some performance tips that you can use to significantly improve performance when using `git-internal` crates as a dependency.

In certain versions of Rust, using `HashMap` on Windows can lead to performance issues. This is due to the allocation strategy of the internal heap memory allocator. To mitigate these performance issues on Windows, you can use [mimalloc](https://github.com/microsoft/mimalloc). (See [this issue](https://github.com/rust-lang/rust/issues/121747) for more details.)

On other platforms, you can also experiment with [jemalloc](https://github.com/jemalloc/jemalloc) or [mimalloc](https://github.com/microsoft/mimalloc) to potentially improve performance.

A simple approach:

1. Change Cargo.toml to use mimalloc on Windows and jemalloc on other platforms.

   ```toml
   [target.'cfg(not(windows))'.dependencies]
   jemallocator = "0.5.4"

   [target.'cfg(windows)'.dependencies]
   mimalloc = "0.1.43"
   ```

2. Add `#[global_allocator]` to the main.rs file of the program to specify the allocator.

   ```rust
   #[cfg(not(target_os = "windows"))]
   #[global_allocator]
   static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

   #[cfg(target_os = "windows")]
   #[global_allocator]
   static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
   ```

### Concurrent Processing

- Configurable thread pools for CPU-intensive operations
- Lock-free data structures where possible (DashMap for waitlists)
- Parallel delta application using Rayon

### 3. I/O Optimization

- Buffered reading with configurable buffer sizes
- Asynchronous file operations for cache persistence
- Stream-based processing to minimize memory footprint

### Benchmark

**TODO**

## Contributing

### Pre-submission Checks

Before submitting a Pull Request, please ensure your code passes the following checks:

```bash
# Run clippy with all warnings treated as errors (warnings will be treated as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Check code formatting (requires nightly toolchain)
cargo +nightly fmt --all --check
```

**Both commands must complete without any warnings.** The clippy check treats all warnings as errors, and the formatter check ensures code follows the project style guide. Only PRs that pass these checks will be accepted for merge.

If the formatting check fails, you can automatically fix formatting issues by running:

```bash
cargo +nightly fmt --all
```

### Buck2 Build Requirements

This project builds with Buck2. Please install both Buck2 and `cargo-buckal` before development:

```bash
# Install buck2: download the latest release tarball from
# https://github.com/facebook/buck2/releases, extract the binary,
# and place it in ~/.cargo/bin (ensure ~/.cargo/bin is on PATH).
# Example (replace <tag> and <platform> with the latest for your OS):
wget https://github.com/facebook/buck2/releases/download/<tag>/buck2-<platform>.tar.gz
tar -xzf buck2-<platform>.tar.gz
mv buck2 ~/.cargo/bin/

# Install cargo-buckal (requires Rust toolchain)
cargo install --git https://github.com/buck2hub/cargo-buckal.git
```

Pull Requests must also pass the Buck2 build:

```bash
cargo buckal build
```

When you update dependencies in Cargo.toml, regenerate Buck metadata and third-party lockfiles:

```bash
cargo buckal migrate
```

