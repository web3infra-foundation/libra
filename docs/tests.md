# Libra Test Guide

## Test Layers

Libra tests are organized into three layers. By default, `cargo test --all` runs only L1 tests. L2 and L3 tests are silently skipped when the required environment variables are absent — they never fail due to missing credentials.

| Layer | Dependencies | Trigger | Runs in |
|-------|-------------|---------|---------|
| **L1 — Deterministic** | None (tempdir, in-memory stores, mock models) | `cargo test --all` | Every PR CI |
| **L2 — Network** | GitHub token for temporary repo creation | Set `LIBRA_TEST_GITHUB_TOKEN` | Nightly CI / local |
| **L3 — Live Services** | Real AI API keys or cloud credentials | Set the relevant env vars | Nightly CI / local |

## Running Tests

### L1 Only (default, zero configuration)

```bash
cargo test --all
```

### Full Suite (L1 + L2 + L3)

1. Copy the template:
   ```bash
   cp .env.test.example .env.test
   ```
2. Fill in your credentials in `.env.test`. **Each line must keep the `export` prefix** — without it `source .env.test` only sets shell-local variables that the cargo subprocess cannot see, and L2/L3 tests skip silently.
3. Run:
   ```bash
   source .env.test && cargo test --all
   ```

### Running a Single Layer

```bash
# L2 only — network tests
LIBRA_TEST_GITHUB_TOKEN=ghp_xxx cargo test --all

# L3 only — AI tests
DEEPSEEK_API_KEY=sk-xxx cargo test --all

# L3 only — S3 storage tests (local MinIO)
docker run -d -p 9000:9000 minio/minio server /data
LIBRA_STORAGE_ENDPOINT=http://127.0.0.1:9000 \
LIBRA_STORAGE_BUCKET=test \
LIBRA_STORAGE_ACCESS_KEY=minioadmin \
LIBRA_STORAGE_SECRET_KEY=minioadmin \
LIBRA_STORAGE_ALLOW_HTTP=true \
cargo test --all
```

## Environment Variable Reference

### L2 — Network

| Variable | Purpose | How to obtain |
|----------|---------|---------------|
| `LIBRA_TEST_GITHUB_TOKEN` | GitHub API auth for creating / deleting temporary test repos | [github.com/settings/tokens](https://github.com/settings/tokens) — scope: `public_repo` |
| `LIBRA_TEST_GITHUB_NAMESPACE` | GitHub user or org that owns the temporary repo | Your GitHub username or an org you control |

A single temporary repo named `libra-test-<4-6 random letters>` is created under `https://github.com/<namespace>/`, shared by all L2 tests, and deleted after the full suite completes. Only repos matching the `libra-test-` prefix are ever deleted.

### L3 — AI

| Variable | Purpose | How to obtain |
|----------|---------|---------------|
| `DEEPSEEK_API_KEY` | DeepSeek LLM API calls (`deepseek-v4-flash` model) | [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) |

The L3 AI tests live in `tests/ai_agent_test.rs` and `tests/ai_chat_agent_test.rs` and exercise the agent runtime against the live DeepSeek API. The flash tier is chosen because it's the cheapest model that still supports tool-use; tests must not silently switch to a costlier tier without an explicit per-test override. Setting `DEEPSEEK_API_KEY` alone activates the gate — no separate `LIBRA_AI_LIVE_*` opt-in is needed.

### L3 — Cloud Feature Tests (D1 + R2)

Tests the full cloud sync/restore workflow, D1 metadata CRUD, and R2 object storage.

| Variable | Purpose |
|----------|---------|
| `LIBRA_D1_ACCOUNT_ID` | Cloudflare account ID |
| `LIBRA_D1_API_TOKEN` | D1 API token |
| `LIBRA_D1_DATABASE_ID` | D1 database ID |
| `LIBRA_STORAGE_ENDPOINT` | R2 S3 API endpoint |
| `LIBRA_STORAGE_BUCKET` | R2 bucket name |
| `LIBRA_STORAGE_ACCESS_KEY` | R2 access key |
| `LIBRA_STORAGE_SECRET_KEY` | R2 secret key |
| `LIBRA_STORAGE_REGION` | Region (`auto` for R2) |

### L3 — S3 Storage Object Tests

Tests S3 protocol features (PUT/GET/DELETE, multipart, list, tiered storage). Uses a **separate** S3-compatible endpoint — can be AWS S3, MinIO, or any S3-compatible service (independent of the R2 config above).

| Variable | Purpose | Default |
|----------|---------|---------|
| `LIBRA_TEST_S3_ENDPOINT` | S3-compatible endpoint URL | — |
| `LIBRA_TEST_S3_BUCKET` | Bucket name | — |
| `LIBRA_TEST_S3_ACCESS_KEY` | Access key ID | — |
| `LIBRA_TEST_S3_SECRET_KEY` | Secret access key | — |
| `LIBRA_TEST_S3_REGION` | Region | — |
| `LIBRA_TEST_S3_ALLOW_HTTP` | Allow plain HTTP endpoints (for local dev) | — |

**Endpoint examples:**

| Provider | Endpoint format |
|----------|----------------|
| AWS S3 | `https://s3.<region>.amazonaws.com` |
| Local MinIO | `http://127.0.0.1:9000` (set `LIBRA_TEST_S3_ALLOW_HTTP=true`) |

## Writing Tests

### Gating tests that require network or credentials

Define a small `env_var_is_set` helper (or reuse one from the test
file you are extending — see `tests/cloud_storage_backup_test.rs:30`)
and pair it with an early `eprintln!("skipped (set ...)")` return so
missing vars print a skip notice and do **not** fail the test.

```rust
fn env_var_is_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

#[tokio::test]
async fn test_something_with_s3() {
    if !env_var_is_set("LIBRA_STORAGE_ENDPOINT") {
        eprintln!("skipped (set --features test-live-cloud and LIBRA_STORAGE_*)");
        return;
    }
    // test logic ...
}
```

The skip notice should name both the Cargo feature flag (e.g.
`--features test-live-cloud`) and the env-var prefix (e.g.
`LIBRA_STORAGE_*`) so contributors landing on a CI log can tell at
a glance what is missing.

### Test isolation

- Use `tempfile::tempdir()` to isolate filesystem state.
- Use `ChangeDirGuard` (RAII) for safe directory changes.
- CLI-level tests should use `run_libra_command()` or a local `libra_command()` helper that sets `HOME` / `XDG_CONFIG_HOME` to a temp path.
- Mark tests that mutate shared state with `#[serial]`.

### Compatibility-surface tests (`tests/compat/`)

The `tests/compat/` directory holds **cross-cutting** regression guards
that pin contracts spanning multiple commands or doc artefacts (see
[`tests/compat/README.md`](../tests/compat/README.md) for the file
inventory and per-file ownership).

Two operational rules differ from the rest of the suite:

1. **Cargo registration is mandatory.** Cargo's default `tests/*.rs`
   discovery only walks files directly under `tests/`. Files placed
   under `tests/compat/` are reachable **only** when registered as a
   `[[test]]` entry in `Cargo.toml` with an explicit `path =
   "tests/compat/<name>.rs"`. A new file that compiles but is missing
   the entry will silently never run.
2. **Membership reflects an intent, not just a category.** Most compat
   guards exist because a specific failure mode previously hit
   production or a Codex review caught it before it did. When adding
   a new compat guard, also add a one-line entry to the inventory
   table in `tests/compat/README.md` so future readers can map the
   guard to its owning batch / version.

All `tests/compat/*` files run in CI under the same `compat-offline-core`
job that runs L1.

### CI

| Workflow | Layers | Trigger |
|----------|--------|---------|
| `base.yml` (PR gate) | L1 + `tests/compat/*` (`compat-offline-core` job); a single L2 file — `tests/network_remotes_test.rs` — via the `compat-network-remotes` job under `--features test-network` | Every push / PR |
| `model-generation-nightly.yml` (nightly) | One L3 file — `tests/code_ui_remote_model_generation_matrix.rs` — under `LIBRA_RUN_LIVE=1` + `DEEPSEEK_API_KEY` | Daily 03:00 UTC + manual dispatch |

Other L2 / L3 surfaces (full GitHub-namespace tests, S3/D1 round-trips, broader live-AI suites) are not wired into a scheduled workflow; run them locally by sourcing `.env.test` and invoking `cargo test --all` with the corresponding feature flag.
