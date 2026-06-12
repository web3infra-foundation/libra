# `libra code-control`

`libra code-control --stdio` 是面向已运行 `libra code --control write` 会话的本地自动化适配层。它在 stdin/stdout 上使用换行分隔的 JSON-RPC 2.0，并将请求转发到 loopback `/api/code/*` HTTP/SSE 控制接口。

此命令不是 MCP 服务器。`libra code --stdio` 仍然是 MCP stdio 传输，并且不会驱动正在运行的 TUI 会话。

## 用法

```bash
libra code-control --stdio \
  --url http://127.0.0.1:3000 \
  --token-file .libra/code/control-token
```

`--url` 应来自 `.libra/code/control.json`。`--token-file` 指向由 `libra code --control write` 创建的进程级令牌；该令牌会作为 `X-Libra-Control-Token` 发送，用于写控制 HTTP 请求。

## 方法

| JSON-RPC 方法 | HTTP 等价接口 |
|-----------------|-----------------|
| `session.get` | `GET /api/code/session` |
| `events.subscribe` | 作为 JSON-RPC 通知的 `GET /api/code/events` |
| `diagnostics.get` | `GET /api/code/diagnostics` |
| `controller.attach` | `POST /api/code/controller/attach` |
| `controller.detach` | `POST /api/code/controller/detach` |
| `message.submit` | `POST /api/code/messages` |
| `task.dispatch` | `POST /api/code/task/dispatch` |
| `interaction.respond` | `POST /api/code/interactions/{id}` |
| `turn.cancel` | `POST /api/code/control/cancel` |
| `goal.start` | `POST /api/code/goal/start` |
| `goal.status` | `GET /api/code/goal/status` |
| `goal.cancel` | `POST /api/code/goal/cancel` |

## 示例

附加自动化控制器：

```json
{"jsonrpc":"2.0","id":1,"method":"controller.attach","params":{"clientId":"local-script","kind":"automation"}}
```

在 attach 返回 `controllerToken` 后提交消息：

```json
{"jsonrpc":"2.0","id":2,"method":"message.submit","params":{"controllerToken":"...","text":"/chat hello"}}
```

显式派发子代理：

```json
{"jsonrpc":"2.0","id":3,"method":"task.dispatch","params":{"controllerToken":"...","agent":"explorer","prompt":"grep TODO src/"}}
```

响应待处理交互：

```json
{"jsonrpc":"2.0","id":4,"method":"interaction.respond","params":{"controllerToken":"...","interactionId":"interaction-1","response":{"approved":true}}}
```

订阅事件：

```json
{"jsonrpc":"2.0","id":5,"method":"events.subscribe"}
```

适配层会先返回 `{"subscribed":true}`，然后发出通知：

```json
{"jsonrpc":"2.0","method":"events.notification","params":{"event":"session_updated","data":{}}}
```

## 错误

格式错误的 JSON 映射为 JSON-RPC `-32700`。未知方法映射为 `-32601`。无效参数映射为 `-32602`。HTTP 4xx/5xx 错误映射为带有 `data.status` 和 `data.code` 的 `-32000`，并保留 `INVALID_CONTROL_TOKEN`、`INVALID_CONTROLLER_TOKEN`、`CONTROLLER_CONFLICT` 和 `INTERACTION_NOT_ACTIVE` 等 Libra 错误。
