# Local TUI Automation Control

Local TUI Automation Control lets scripts and test harnesses discover and authenticate to the `libra code` web control surface running on the same machine. It is not a remote collaboration protocol and must stay loopback-only.

## Security Model

`--control observe` is the default. It preserves the existing behavior: loopback clients can read the current Code UI snapshot and SSE stream without a token, and Libra does not create token, info, or lock files unless `--control-info-file` is explicitly supplied.

`--control write` enables a process-level control token:

- Token file: `.libra/code/control-token` by default.
- Info file: `.libra/code/control.json` by default.
- Lock file: `.libra/code/control.lock` by default.

The token file contains a fresh random token for the current process. On Unix/macOS, Libra refuses to use an existing token path unless it is a regular file with exactly `0600` permissions. Symlinks are rejected.

`control.json` is written only after the web server has bound its real address. It contains endpoint discovery metadata such as `baseUrl`, optional `mcpUrl`, `pid`, `workingDir`, optional `threadId`, and `startedAt`. It must not contain the control token, token hashes, token paths, provider credentials, auth headers, environment dumps, or provider request/response bodies.

## Multi-Instance Behavior

Default write-control paths are single-owner per repository. A process must acquire the advisory lock before generating the token or writing `control.json`. If another live process owns the lock, startup fails with `CONTROL_INSTANCE_CONFLICT` and includes the existing PID and URL when `control.json` is readable.

Stale `control.json` files from crashed processes do not block startup once the lock is released and the recorded PID is no longer live. To run multiple local write-control instances intentionally, pass distinct `--control-token-file` and `--control-info-file` paths for each process; the lock path follows the info file stem.

## HTTP Auth Boundary

Automation write requests use `X-Libra-Control-Token` as the process-level authorization header. Browser controller compatibility is preserved: existing browser clients keep using `X-Code-Controller-Token` for their lease and do not need to know the control token.

To take control, an automation client sends:

```http
POST /api/code/controller/attach
X-Libra-Control-Token: <token file contents>
Content-Type: application/json

{ "clientId": "local-script", "kind": "automation" }
```

The response includes a lease token. Subsequent write requests must include both:

- `X-Libra-Control-Token`: process-level local authorization.
- `X-Code-Controller-Token`: active controller lease authorization.

The local TUI remains the final owner. `/control reclaim` force-detaches the automation lease and returns the visible controller to `tui`; old automation lease tokens then fail with `INVALID_CONTROLLER_TOKEN` or `CONTROLLER_CONFLICT`.

## Write Endpoints

Automation write control currently covers:

- `POST /api/code/messages`
- `POST /api/code/interactions/{id}`
- `POST /api/code/controller/detach`
- `POST /api/code/control/cancel`

Write request bodies are limited to 256KiB. `GET /api/code/session`, `GET /api/code/events`, and `GET /api/code/diagnostics` remain observe-only loopback endpoints. Diagnostics are generated from a whitelist of session fields and must not include the control token, controller token, auth headers, provider request bodies, or environment dumps.

Control attach, detach, submit, respond, and cancel operations emit `local-tui-control/v1` audit events. Control-specific fields are serialized into the existing `AuditEvent.redacted_summary` JSON string; the audit event schema itself is unchanged.

`libra code-control --stdio --url <baseUrl> --token-file <path>` provides the local NDJSON JSON-RPC bridge for automation clients. It is distinct from `libra code --stdio`, which remains the MCP stdio transport.

## Writing Your Own Scenario

The cross-process TUI harness lives under `tests/harness/` and starts a real `libra code` process inside a pseudo-terminal. It uses the hidden `test-provider` feature and fixture files under `tests/fixtures/code_ui/`; each scenario writes artifacts to `target/code-ui-scenarios/<scenario>/`.

Run the stable scenario suite with:

```bash
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test code_ui_scenarios \
  --test harness_self_test \
  --test code_codex_default_tui_test \
  -- --test-threads=1
```

To add a scenario, create a fixture that returns provider-native text, tool calls, errors, optional stream deltas, and optional `delayMs`, then spawn `CodeSession` with that fixture. Use `Scenario::new(...).step(...)` from `tests/harness/scenario.rs` for common attach/submit/wait assertions. Submit direct chat prompts with `/chat ...`; plain prompts enter the normal IntentSpec and plan workflow.

Useful artifacts:

- `control.json`: discovered `baseUrl`/`mcpUrl`, never the token.
- `pty.log`: terminal rendering and typed local commands such as `/control reclaim`.
- `libra.log`: runtime logs and `local-tui-control/v1` audit events.

## Troubleshooting

To reproduce one failing scenario locally:

```bash
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test code_ui_scenarios --features test-provider -- basic_chat --nocapture
```

If a test fails, inspect `target/code-ui-scenarios/<scenario>/pty.log`, `libra.log`, and `control.json`. A `:0` URL in `control.json` means the server did not write back the bound OS port; a lingering token or info file means the TUI did not exit cleanly.
