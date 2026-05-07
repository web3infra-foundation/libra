# Code UI Remote Matrix Test Data

This directory contains machine-readable data for the planned Libra Code TUI Remote L2 matrix tests.

The JSON files are intentionally separate from `tests/harness/` so adding a new protocol scenario can be a data-only change. Paths in `fixture.path` are repo-root relative.

Files:

- `lease_cases.json`: controller lease lifecycle and token validation.
- `sse_cases.json`: `/api/code/events` event-stream contract.
- `generation_cases.json`: full Code service generation flow, including file writes and verification commands.
- `model_generation_cases.json`: live model-backed generation flow. Defaults to the repository `.env.test`; includes both a small Rust source-file task and a full `linked` Cargo CLI project task with fmt, clippy, test, config, and `.libraignore` checks.
- `state_cases.json`: busy state, body limits, concurrency, and streaming state.
- `security_cases.json`: diagnostics redaction, loopback, `/threads`, and audit checks.
- `provider_fixtures/*.json`: fake provider fixtures used by matrix cases that need streaming or tool-call behavior beyond the existing `tests/fixtures/code_ui/*.json`.

The future Rust runner should deserialize these files into `RemoteCase` values and map `op` values to typed harness actions. It should reject unknown `op`, `auth`, `token`, and `assertions` values so stale data fails fast.
