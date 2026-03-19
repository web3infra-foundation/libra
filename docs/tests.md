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
2. Fill in your credentials in `.env.test`.
3. Run:
   ```bash
   source .env.test && cargo test --all
   ```

### Running a Single Layer

```bash
# L2 only — network tests
LIBRA_TEST_GITHUB_TOKEN=ghp_xxx cargo test --all

# L3 only — AI tests
GEMINI_API_KEY=xxx cargo test --all

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
| `GEMINI_API_KEY` | Gemini LLM API calls | [aistudio.google.com/apikey](https://aistudio.google.com/apikey) |

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

Use the `require_env!` macro. If the variable is unset the test prints "skipped" and returns — it does **not** fail.

```rust
use super::require_env;

#[tokio::test]
async fn test_something_with_s3() {
    require_env!("LIBRA_STORAGE_ENDPOINT");
    // test logic ...
}
```

### Test isolation

- Use `tempfile::tempdir()` to isolate filesystem state.
- Use `ChangeDirGuard` (RAII) for safe directory changes.
- CLI-level tests should use `run_libra_command()` or a local `libra_command()` helper that sets `HOME` / `XDG_CONFIG_HOME` to a temp path.
- Mark tests that mutate shared state with `#[serial]`.

### CI

| Workflow | Layers | Trigger |
|----------|--------|---------|
| `base.yml` (PR gate) | L1 only | Every push / PR |
| `live-tests.yml` (nightly) | L1 + L2 + L3 | Cron schedule + manual dispatch |

Nightly CI injects credentials via GitHub Actions secrets.
