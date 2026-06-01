# `libra code-control`

`libra code-control --stdio` is a local automation shim for an already running
`libra code --control write` session. It speaks newline-delimited JSON-RPC 2.0
on stdin/stdout and forwards requests to the loopback `/api/code/*` HTTP/SSE
control surface.

This command is not an MCP server. `libra code --stdio` remains the MCP stdio
transport and does not drive a live TUI session.

## Usage

```bash
libra code-control --stdio \
  --url http://127.0.0.1:3000 \
  --token-file .libra/code/control-token
```

`--url` should come from `.libra/code/control.json`. `--token-file` points at the
process-level token created by `libra code --control write`; the token is sent as
`X-Libra-Control-Token` for write-control HTTP requests.

Methods that require an active controller lease accept `controllerToken` in their
JSON-RPC params. The shim forwards that value as `X-Code-Controller-Token`.

## Methods

| JSON-RPC method | HTTP equivalent |
|-----------------|-----------------|
| `session.get` | `GET /api/code/session` |
| `events.subscribe` | `GET /api/code/events` as JSON-RPC notifications |
| `diagnostics.get` | `GET /api/code/diagnostics` |
| `threads.list` | `GET /api/code/threads` (`limit` / `offset` params optional) |
| `controller.attach` | `POST /api/code/controller/attach` |
| `controller.detach` | `POST /api/code/controller/detach` |
| `message.submit` | `POST /api/code/messages` |
| `task.dispatch` | `POST /api/code/task/dispatch` |
| `interaction.respond` | `POST /api/code/interactions/{id}` |
| `turn.cancel` | `POST /api/code/control/cancel` |
| `goal.start` | `POST /api/code/goal/start` |
| `goal.status` | `GET /api/code/goal/status` |
| `goal.cancel` | `POST /api/code/goal/cancel` |

## Examples

Attach automation:

```json
{"jsonrpc":"2.0","id":1,"method":"controller.attach","params":{"clientId":"local-script","kind":"automation"}}
```

Submit a message after attach returns `controllerToken`:

```json
{"jsonrpc":"2.0","id":2,"method":"message.submit","params":{"controllerToken":"...","text":"/chat hello"}}
```

List active threads:

```json
{"jsonrpc":"2.0","id":3,"method":"threads.list","params":{"limit":20,"offset":0}}
```

Dispatch a sub-agent explicitly:

```json
{"jsonrpc":"2.0","id":4,"method":"task.dispatch","params":{"controllerToken":"...","agent":"explorer","prompt":"grep TODO src/"}}
```

Respond to a pending interaction:

```json
{"jsonrpc":"2.0","id":5,"method":"interaction.respond","params":{"controllerToken":"...","interactionId":"interaction-1","response":{"approved":true}}}
```

Subscribe to events:

```json
{"jsonrpc":"2.0","id":6,"method":"events.subscribe"}
```

The shim first returns `{"subscribed":true}` and then emits notifications:

```json
{"jsonrpc":"2.0","method":"events.notification","params":{"event":"session_updated","data":{}}}
```

## Errors

Malformed JSON maps to JSON-RPC `-32700`. Unknown methods map to `-32601`.
Invalid params map to `-32602`. HTTP 4xx/5xx errors map to `-32000` with
`data.status` and `data.code`, preserving Libra errors such as
`INVALID_CONTROL_TOKEN`, `INVALID_CONTROLLER_TOKEN`, `CONTROLLER_CONFLICT`, and
`INTERACTION_NOT_ACTIVE`.
