# TUI 改进计划：Local TUI Automation Control

> **本文档面向本地自动化执行（Codex / Claude Code / harness）。**
> 每个 Phase 给出文件路径、改动锚点与验收命令；执行者按 Phase 顺序推进，每完成一个 Task 即跑相应验收命令。"当前实现现状" 段落已标注既有代码位置以避免重复探索。当某个 Task 与列出的代码位置不一致时，以代码现状为准并在 PR 描述中说明。

## Context

`libra code` 默认启动 TUI，并同时启动本地 Web API 与 MCP HTTP server（参见 [src/command/code.rs:485-632](src/command/code.rs)）。Web 侧已有 `/api/code/*` snapshot / SSE / browser controller lease 雏形（[src/internal/ai/web/mod.rs:106-114](src/internal/ai/web/mod.rs)），但普通 provider 的 TUI 仍由终端事件循环独占输入；`libra code --stdio` 是 MCP stdio tool server，不驱动正在运行的 TUI 会话。

本计划定义一个 **Local TUI Automation Control** 能力：让本机自动化脚本、测试 harness、调试工具可以观察并受控驱动当前 TUI 会话。它不是远程 headless server，也不是 thin client 协作协议；v1 只面向 loopback、本地 token 和当前 snapshot SSE。

## Goals

- 为正在运行的 TUI session 提供本地自动化入口：提交消息、响应 pending interaction、取消当前 turn、读取 session snapshot 和 diagnostics。
- 把 Codex 执行后端接入默认 Libra TUI，移除 `libra code --provider codex` 的 Codex 单独 TUI / stdin 交互路径，并移除相关的代码。
- 复用现有 Code UI snapshot / SSE / controller lease 基础设施，不在 v1 引入 typed delta + gap recovery。
- 明确区分新 `libra code-control --stdio` shim 与现有 `libra code --stdio` MCP 模式。
- 本计划新增的 automation 写操作受 loopback、token、controller lease、redaction、audit 约束。

## Non-Goals

- 不支持远程控制、共享公网 API、跨机器协作或持久后台 daemon。
- 不替代 `libra code --web-only`，也不把普通 provider 的 Web-only placeholder 一次性重构掉。
- 不在 v1 承诺 typed delta、Last-Event-ID gap recovery 或完整 Code UI source-of-truth unification；这些属于 `code.md` Implementation Phase 3。
- 不允许自动化通道绕过 TUI/App/Runtime 状态机直接写 Snapshot / Event / Projection。
- 不复用 `libra code --stdio`；该模式继续表示 MCP stdio server。

## Boundary With Existing Plans

| 文档 | 本计划关系 |
|------|------------|
| `docs/improvement/code.md` | 主归属。controller lease、Code UI read model、future typed delta 和 Web takeover 最终应收敛到 Code UI Source Of Truth Unification。 |
| `docs/improvement/agent.md` | 只作为交叉依赖。复用 Step 1.1 安全边界、Step 1.6 approval scope、Step 1.8 / 1.9 session/event 记录、Step 1.10 source/client 隔离原则；不把本计划并入 Agent 主线。 |
| `docs/commands/code.md` | 已补用户命令说明，尤其说明 `code --stdio` 与 `code-control --stdio` 的区别。 |

## 可行性审查结论（本轮修订）

当前版本的主方向合理：它复用现有 snapshot/SSE、loopback gate、controller lease 和 `CodeUiCommandAdapter`，没有提前承诺 typed delta，也把 `code-control --stdio` 与 MCP stdio 分开。落地前必须修正以下实现边界，否则会引入难以调试的状态双写或兼容性回归：

1. **不要扩展 `AppEvent` 承载自动化控制命令。** 现有 `AppEvent::turn_id()` 明确要求每个 variant 都是 turn-scoped；automation respond/cancel/reclaim 不是同一类事件。新增独立 `TuiControlCommand` channel，让 App 主循环在自己的上下文中处理。
2. **turn id 只能由 App 持有。** Adapter 不共享 `next_turn_id`，不生成 turn id，不直接 mutate snapshot；它只发控制命令并等待 App ack。
3. **保留现有 browser controller 兼容性。** `X-Libra-Control-Token` 只约束 automation write/control 路径；既有 browser lease 继续使用 `X-Code-Controller-Token`，除非后续 `code.md` 明确升级浏览器鉴权。
4. **`--control write` 在 `libra code --stdio` 下必须拒绝。** MCP stdio 独占 stdin/stdout，warning 可能污染协议；需要作为 command usage error。
5. **`--control write` 必须拒绝非 loopback bind host。** 不能只靠 handler 检查 remote addr；启动参数中 `--host 0.0.0.0` / 非 loopback 地址应直接失败。
6. **token 每次进程启动重新生成。** 不能复用崩溃遗留 token；已有 0600 常规文件可覆盖写入新 token，symlink/宽权限拒绝。退出清理只是 best-effort。
7. **`control.json` 必须在 web server 绑定成功后写。** 需要真实 `baseUrl`/端口；若 MCP server 后续启动失败，要清理或删除已写出的 info 文件。
8. **TUI controller 不能继续用不可让渡的 `FixedController` 表示。** 当前 fixed controller 会拒绝所有 attach；`--control write` 需要一个可被 automation lease 临时覆盖、reclaim 后回到 TUI 的本地 controller 状态。
9. **验收命令必须符合当前 CLI 校验。** 当前 `--web-only --provider codex` 会被 `validate_mode_args` 拒绝；Phase 1 冒烟用默认 provider，或先单独修复 web-only provider 校验。
10. **Codex 不能继续维护第二套交互式 TUI。** `--provider codex` 的执行后端可以特殊，但输入、approval、渲染和控制权必须统一进入默认 Libra TUI。
11. **同仓库多实例必须显式拒绝。** 默认 token/info 路径固定（`.libra/code/control-token` 与 `.libra/code/control.json`）；若同一仓库已存在另一个 `--control write` 进程，第二个进程**不能默默覆盖**前者的文件而把它的 lease 静默劫持掉。需要在写文件前先获取 advisory file lock 并做 PID liveness 检查，冲突时 fail-fast 并报告既有 PID/URL；显式自定义 `--control-token-file` + `--control-info-file` 到不同路径是允许并发的唯一逃生口（由调用方自行管理冲突）。
12. **跨进程 TUI e2e 必须跑在 PTY 中。** `cargo test` 子进程默认没有交互终端；Phase 6 如果直接 `Command` 启动 `libra code`，会在 CI 中失败或卡住。harness 必须使用 pseudo-terminal（例如 dev-dependency `portable-pty`）启动真实 TUI，并设置固定终端尺寸与 `TERM`。
13. **fake provider 不能直接伪造 approval / user input。** 测试 provider 只能返回 provider-native `CompletionResponse`（text / tool call / error / optional stream delta）；审批、`request_user_input`、plan review 必须由现有 tool loop、sandbox 与 TUI App 真实触发，否则 e2e 覆盖不到生产路径。
14. **control audit 必须贴合现有 `AuditEvent` 结构。** 当前 `AuditEvent` 字段是 `{ trace_id, principal_id, action, policy_version, redacted_summary, at }`；Phase 4 不应假设可以直接写 `{ thread_id, controller_kind, client_id, result }` 字段。需要用 `ControlAuditRecord` 组装 redacted JSON summary，或先显式迁移 `AuditEvent` 并更新所有调用点。
15. **测试 harness 不能依赖 `Drop` 完成断言清理。** 子进程关闭要提供显式 `shutdown()`；`Drop` 只作 best-effort 兜底，测试必须主动调用 shutdown 以保证 PTY、日志与临时 control 文件被收口。
16. **Phase 6 的直接聊天场景必须显式走 `/chat` 或专用测试入口。** 当前普通 provider 的 plain message 会先进入 IntentSpec / Plan review workflow；若场景期望“一次 submit 后立刻得到 assistant 文本”，测试输入必须使用 `/chat ...`，或把 fixture 写成完整 Phase 0/1 计划流程。

## 当前实现现状（Pre-Flight Reference）

### 已具备的能力（直接复用，不重复实现）

| 能力 | 位置 | 说明 |
|------|------|------|
| `/api/code/session` snapshot | [src/internal/ai/web/mod.rs:108, 197](src/internal/ai/web/mod.rs) | loopback-only 读取已就绪 |
| `/api/code/events` SSE | [src/internal/ai/web/mod.rs:109, 206](src/internal/ai/web/mod.rs) | broadcast channel + KeepAlive |
| `/api/code/messages` POST | [src/internal/ai/web/mod.rs:112, 258](src/internal/ai/web/mod.rs) | 已校验 controller token |
| `/api/code/interactions/{id}` POST | [src/internal/ai/web/mod.rs:113, 273](src/internal/ai/web/mod.rs) | 同上 |
| `/api/code/controller/attach`/`detach` | [src/internal/ai/web/mod.rs:110-111, 227-244](src/internal/ai/web/mod.rs) | 仅 browser kind |
| Loopback gate | [src/internal/ai/web/mod.rs:305](src/internal/ai/web/mod.rs) `ensure_loopback_api_request` | 已应用全部 7 个 handler |
| `BrowserControllerLease` 结构 | [src/internal/ai/web/code_ui.rs:570-714](src/internal/ai/web/code_ui.rs) | UUID token, 120s TTL |
| `ensure_browser_write_access` | [src/internal/ai/web/code_ui.rs:745](src/internal/ai/web/code_ui.rs) | controller token 校验逻辑 |
| `CodeUiCommandAdapter` trait | [src/internal/ai/web/code_ui.rs:532](src/internal/ai/web/code_ui.rs) | 写通道抽象（已被 codex / read-only 实现） |
| `CodexCodeUiAdapter` 实现 | [src/internal/ai/codex/mod.rs:1473](src/internal/ai/codex/mod.rs) | 仅 codex 后端走此路径 |
| `ReadOnlyCodeUiAdapter` | [src/internal/ai/web/code_ui.rs:914](src/internal/ai/web/code_ui.rs) | 非 codex provider 占位 |
| Snapshot mutate + broadcast | [src/internal/ai/web/code_ui.rs:362-504](src/internal/ai/web/code_ui.rs) | `mutate`, `broadcast_snapshot` |
| Pending interaction model | [src/internal/ai/web/code_ui.rs:131-167](src/internal/ai/web/code_ui.rs) | id/kind/status/resolved_at |
| `CodeUiControllerKind` 枚举 | [src/internal/ai/web/code_ui.rs:66](src/internal/ai/web/code_ui.rs) | 当前 None/Browser/Tui/Cli |
| AppEvent bus | [src/internal/tui/app_event.rs:136-276](src/internal/tui/app_event.rs) | 只承载 turn-scoped event；不要加入无 turn 的 control command |
| Pending interaction helpers | [src/internal/tui/app.rs:1285-1895](src/internal/tui/app.rs) | user input / approval / managed interaction 可抽 helper 复用 |
| `SecretRedactor` / `AuditEvent` / `AuditSink` | [src/internal/ai/runtime/hardening.rs:145-260](src/internal/ai/runtime/hardening.rs) | 含可直接复用的 `TracingAuditSink` |
| `resolve_storage_root` | [src/command/code.rs:2046](src/command/code.rs) | 解析 `.libra/` 路径 |

### v1 缺口（必须新增）

1. `CodeUiControllerKind::Automation` 变体（[code_ui.rs:66](src/internal/ai/web/code_ui.rs)）。
2. `--control` / `--control-token-file` / `--control-info-file` CLI 参数（`CodeArgs` 在 [src/command/code.rs:350](src/command/code.rs)）。
3. control token 文件创建/校验（含 0600 权限）；默认路径 `.libra/code/control-token`。
4. control info 文件写入（`.libra/code/control.json`，无 token）。
5. `X-Libra-Control-Token` HTTP 校验层（独立于 `X-Code-Controller-Token`）。
6. Automation controller lease（参数化既有 `BrowserControllerLease`）。
7. **TUI 端 `CodeUiCommandAdapter` 实现**：把 HTTP write 请求路由到独立 `TuiControlCommand` channel；不能复用 `CodexCodeUiAdapter`（它仅适用 web-only codex 后端，直接发到 codex websocket）。
8. TUI 输入只读化 + 抢回入口（`/control reclaim`）+ controller change SSE 事件。
9. `POST /api/code/control/cancel` 端点（同时把现有 Esc 取消逻辑抽出复用）。
10. `GET /api/code/diagnostics` 端点 + `CodeUiDiagnostics` 类型（按 redaction 规则表填充）。
11. Audit event 接入：attach/detach/submit/respond/cancel 全部写日志（沿用 `TracingAuditSink`）。
12. `libra code-control --stdio` 命令（NDJSON JSON-RPC 2.0 shim，**v1 可推迟**）。
13. `--provider codex` 执行后端接入默认 TUI；删除/隔离 Codex 单独 TUI 与 stdin approval loop。

### 架构关键点（执行前必须理解）

1. **Codex 执行后端与 TUI 解耦**：`CodexCodeUiAdapter`（[codex/mod.rs:1473](src/internal/ai/codex/mod.rs)）只服务 web-only codex，把 HTTP submit/respond 直接发到 codex websocket。TUI 模式下 `--provider codex` 必须启动默认 Libra TUI，并把 Codex app-server 当作 managed execution backend；不能再进入 `agent_codex::execute` 的 stdin/stdout 主循环或 Codex 单独 TUI。TUI 模式下所有 provider 的本地自动化都应走 `TuiCodeUiAdapter` → `TuiControlCommand` → App-owned helper；不要绕过 TUI 状态机，也不要把无 turn 的控制命令塞进 `AppEvent`。
2. **两层鉴权语义**：
   - `X-Libra-Control-Token` = **进程级**"是否允许参与 write 控制面"（长期，进程生命周期）。
   - `X-Code-Controller-Token` = **lease 级**"当前 lease 持有者"（120s TTL）。
   - Phase 1 实现前者，Phase 2 扩展后者覆盖 automation。
3. **observe 默认向后兼容**：今天 `/api/code/session` 在 loopback 上无 token 即可读，`--control observe`（默认值）必须保持此行为；`--control write` 才引入 token 强制。
4. **interaction id 必须当前 active**：自动化 respond 时校验该 id 在 `snapshot.interactions` 中状态为 `Pending`；`Resolved`/`Cancelled` 或不存在时返回 `INTERACTION_NOT_ACTIVE`。
5. **Approval scope 隔离**：现有审批 memo 在 [src/internal/ai/orchestrator/policy.rs](src/internal/ai/orchestrator/policy.rs) 一带；automation 触发的 approval 必须使用独立 scope key（建议 `automation:<thread_id>`），不要继承交互式 session 的"once-allow"。Phase 4 实现时按 grep 找当前 memo key 的写入点统一改造。
6. **Browser 兼容性边界**：automation 是新增 writer kind，不改变缺省 browser attach/body 语义；`kind` 缺省仍为 `"browser"`，旧浏览器客户端不需要读取 control token。
7. **同仓库单实例边界**：默认路径下，`.libra/code/control-token` 与 `.libra/code/control.json` 是单实例 owner contract；通过 advisory file lock `.libra/code/control.lock` + 既有 `control.json.pid` 的 liveness 检查保证 fail-fast。崩溃遗留（lock 已释放、PID 已不在）视为 stale，新实例可清理后接管。要并发跑多个 `--control write` 必须显式提供互不重合的 `--control-token-file` 与 `--control-info-file`；锁文件路径与 info 文件同目录、同 stem，自动跟随用户自定义路径。

## User-Facing Contract

### `libra code` flags

| Flag | 默认值 | 说明 |
|------|--------|------|
| `--control <observe\|write>` | `observe` | `observe` 等价当前默认行为；`write` 启用 token 强制并允许自动化 attach。 |
| `--control-token-file <PATH>` | `.libra/code/control-token` | 写控制 token 文件。Unix / macOS 下必须 `0600`，否则拒绝启用 `--control write`。 |
| `--control-info-file <PATH>` | `.libra/code/control.json` | 本地连接信息文件。**不得**包含 token、token hash、API key、provider request body 或环境变量全集。 |

`control.json` schema：

```json
{
  "version": 1,
  "mode": "write",
  "pid": 12345,
  "baseUrl": "http://127.0.0.1:3000",
  "mcpUrl": "http://127.0.0.1:6789",
  "workingDir": "/path/to/repo",
  "threadId": "11111111-1111-4111-8111-111111111111",
  "startedAt": "2026-04-28T00:00:00Z"
}
```

`threadId` 在新会话尚未持久化时可为 `null`，后续 snapshot/SSE 才是权威来源；`control.json` 只用于发现本地 endpoint。

禁止写入：control token 原文/hash、token 文件路径、provider API key、auth header、credential ref secret、环境变量全集、完整 provider request/response body、未经 redaction 的 shell stdout/stderr。

### New stdio shim

```bash
libra code-control --stdio --url http://127.0.0.1:3000 --token-file .libra/code/control-token
```

约束：
- 本地控制 shim，转发到 HTTP/SSE 控制面。
- NDJSON JSON-RPC 2.0；不是 MCP server，不暴露 MCP tools。
- `libra code --stdio` 继续是 MCP stdio transport，语义不变。

v1 JSON-RPC methods：

| Method | 等价 HTTP |
|--------|-----------|
| `session.get` | `GET /api/code/session` |
| `events.subscribe` | `GET /api/code/events` (SSE → JSON-RPC notification) |
| `controller.attach` | `POST /api/code/controller/attach` |
| `controller.detach` | `POST /api/code/controller/detach` |
| `message.submit` | `POST /api/code/messages` |
| `interaction.respond` | `POST /api/code/interactions/{id}` |
| `turn.cancel` | `POST /api/code/control/cancel` |
| `diagnostics.get` | `GET /api/code/diagnostics` |

## HTTP / SSE API Contract

v1 复用当前 `/api/code/*` 风格，不引入 `/api/v1/threads/*` typed-delta contract。

| Endpoint | Auth Layer | Phase | 说明 |
|----------|-----------|-------|------|
| `GET /api/code/session` | loopback | exists | 当前 snapshot |
| `GET /api/code/events` | loopback | exists | snapshot SSE |
| `POST /api/code/controller/attach` | browser: loopback；automation: loopback + control-token | 2 | 申请 lease；body 可含 `kind: "automation"` |
| `POST /api/code/controller/detach` | browser: loopback + controller-token；automation: loopback + control-token + controller-token | 2 | 主动释放 |
| `POST /api/code/messages` | browser: loopback + controller-token；automation: loopback + control-token + controller-token | 2 | 转 TUI control command |
| `POST /api/code/interactions/{id}` | browser: loopback + controller-token；automation: loopback + control-token + controller-token | 2 | 转 TUI control command |
| `POST /api/code/control/cancel` | automation only: loopback + control-token + controller-token | 2 | 转 TUI control command |
| `GET /api/code/diagnostics` | loopback | 4 | redacted Diagnostics |

## Controller Takeover Semantics

新增 controller kind：

```text
none | tui | browser | automation | cli
```

v1 行为：
- TUI 启动后默认持有 `tui` controller。
- 实现上 `tui` controller 必须是可恢复的本地 owner，不应使用会永久拒绝 attach 的 `FixedController`；automation lease 生效时临时覆盖它，reclaim/lease 过期后回到 `tui`。
- `--control observe` 下自动化只读，不可 attach write。
- `--control write` 下，automation 凭 control token 申请 takeover。
- 自动化持有 lease 时：
  - TUI 输入框只读（参考 [app.rs:800-1000](src/internal/tui/app.rs) 主事件循环），Enter 不再提交。
  - TUI 仍渲染事件、pending interaction、tool progress、diagnostics 摘要。
  - `Ctrl-C` / `/quit` 可结束本地 session（最终控制权）。
  - 提供人工抢回入口 `/control reclaim`；抢回会撤销 automation lease 并 broadcast controller change snapshot。
  - automation 只能响应 active interaction；id 不匹配或已过期返回 `INTERACTION_NOT_ACTIVE`。
- lease TTL 120 秒（沿用 [code_ui.rs:25](src/internal/ai/web/code_ui.rs) `DEFAULT_BROWSER_CONTROLLER_LEASE_SECS`）；写操作刷新 lease。
- 同一时刻只允许一个 writer controller；其他客户端可 observe。

错误码：

| Code | 场景 |
|------|------|
| `CONTROL_DISABLED` | 未启用 `--control write` |
| `LOOPBACK_REQUIRED` | 非 loopback 请求 |
| `MISSING_CONTROL_TOKEN` | 缺 `X-Libra-Control-Token` |
| `INVALID_CONTROL_TOKEN` | control token 不匹配 |
| `MISSING_CONTROLLER_TOKEN` | 缺 `X-Code-Controller-Token` |
| `INVALID_CONTROLLER_TOKEN` | controller token 失效或过期 |
| `CONTROLLER_CONFLICT` | 已有 writer controller |
| `SESSION_BUSY` | 当前状态不接受新 message |
| `INTERACTION_NOT_ACTIVE` | interaction 不存在或已 resolved/cancelled |

## Diagnostics & Audit

### Redaction 规则表

| 字段 / 来源 | v1 处理 |
|-------------|--------|
| Authorization / X-* / Cookie 头 | 全部丢弃 |
| 环境变量字典 | 不输出；仅输出按白名单过滤后的 key 计数 |
| Provider request body | 不输出；输出 `model`、`max_tokens`、`tool_count` |
| Provider response body | 不输出；输出 `finish_reason`、`usage` |
| Shell stdout/stderr | 输出 sha256 + 行数 + 头/尾各 256 字节 redacted excerpt |
| Tool result payload | 经 `redact_workspace_paths_in_output` + `SecretRedactor` |
| API key / token / bearer | 经 `SecretRedactor` 标记替换 |
| control token 原文/hash | 永不输出（既不在 control.json，也不在 logs） |
| token 文件路径 | 不写 control.json / diagnostics；audit 默认不记录 |

### Diagnostics JSON 形态

```json
{
  "pid": 12345,
  "provider": "ollama",
  "model": "gemma4:31b",
  "threadId": "11111111-1111-4111-8111-111111111111",
  "status": "awaiting_interaction",
  "controller": {
    "kind": "automation",
    "ownerLabel": "local-script",
    "leaseExpiresAt": "2026-04-28T00:02:00Z"
  },
  "ports": { "web": 3000, "mcp": 6789 },
  "logFile": "/tmp/libra-code.log",
  "activeInteractionId": "interaction-7",
  "lastError": null
}
```

### Audit Event Schema

沿用 [hardening.rs:178](src/internal/ai/runtime/hardening.rs) `AuditEvent` 与 `TracingAuditSink`（[hardening.rs:256](src/internal/ai/runtime/hardening.rs)），但不要假设 `AuditEvent` 有 control 专属字段。新增一个内部 `ControlAuditRecord`，序列化成 redacted JSON 后写入 `AuditEvent.redacted_summary`；`AuditEvent` 的其它字段按下列规则填充：

- `trace_id`：优先用当前 session/thread trace id；没有 canonical thread 时用启动时生成的 trace id。
- `principal_id`：`local-tui-control:<controller_kind>:<client_id>`，client id 先经长度限制与 redaction。
- `action`：`controller.attach` / `controller.detach` / `message.submit` / `interaction.respond` / `turn.cancel`。
- `policy_version`：`local-tui-control/v1`。
- `redacted_summary`：JSON string，形态为 `{ "thread_id": "...", "controller_kind": "automation", "client_id": "...", "result": "accepted|error", "error_code": null }`，不得包含 token、headers、provider body 或 env dump。

如果后续决定扩展 `AuditEvent` 本身，必须作为独立 migration/refactor 处理，并同步更新 tool-boundary audit 的所有调用点和测试；Phase 4 默认不做该扩展。

## Implementation Phases

### Phase 0 — Contract（已交付）

本文档即 Phase 0 输出。后续阶段不应再修改 v1 contract（Goals/Non-Goals/HTTP 表/Controller Semantics）。如需修订须先更新本文。

---

### Phase 1 — Security Envelope

**目标**：CLI 参数、token 文件、`X-Libra-Control-Token` 校验 helper 落地；`Automation` 枚举变体加入但 lease 仍仅支持 browser。完成后 automation control 的安全 envelope 有单测保障，现有 browser 行为不变；真正的 automation write 拒绝链在 Phase 2 接入。`--control observe`（默认）行为与今天等价。

#### Task 1.1 — 加 `Automation` 变体

- File: [src/internal/ai/web/code_ui.rs:66](src/internal/ai/web/code_ui.rs)
- 在 `CodeUiControllerKind` 加 `Automation`，序列化字符串 `"automation"`。
- 用 `cargo build` 报错驱动找全所有 match arm（含 codex/mod.rs、mod.rs handler、tests）。
- 不改变已有行为：browser/tui 路径仍走旧函数。

#### Task 1.2 — CLI 参数

- File: [src/command/code.rs:350](src/command/code.rs) `CodeArgs`
- 新增字段：
  ```rust
  #[arg(long, value_enum, default_value_t = ControlMode::Observe)]
  pub control: ControlMode,
  #[arg(long)]
  pub control_token_file: Option<PathBuf>,
  #[arg(long)]
  pub control_info_file: Option<PathBuf>,
  ```
- 在同文件靠近 `CodeArgs` 定义 `pub enum ControlMode { Observe, Write }`（含 clap `ValueEnum`、serde 派生）。
- 与 `--web-only` 不冲突；observe 是 always-on；write 仅在 TUI / web-only 启动 server 的路径生效。
- 与 `--stdio` 的关系：`--control write` 必须被 `validate_mode_args` 拒绝，错误说明 `libra code --stdio` 是 MCP stdio；`--control observe` 可接受但不创建文件、不输出 warning。
- 与 `--host` 的关系：`--control write` 必须要求 `args.host` 是 loopback IP address（例如 `127.0.0.1` / `::1`）；非 loopback bind host 直接 command usage error。
- 单元测试覆盖：
  - `validate_mode_args` 拒绝 `--control write --stdio`。
  - `validate_mode_args` 拒绝 `--control write --host 0.0.0.0`。
  - `validate_mode_args` 接受 `--control write` 的默认 TUI 与默认 web-only 模式。

#### Task 1.3 — Token / info 文件 lifecycle

- New file: `src/command/code_control_files.rs`
- File: [src/command/mod.rs](src/command/mod.rs)，新增 `pub mod code_control_files;`
- 暴露：
  ```rust
  pub struct ControlInfo { /* 与 control.json schema 对应 */ }
  pub struct ControlPaths { pub token: PathBuf, pub info: PathBuf, pub lock: PathBuf }
  pub struct ControlLockGuard { /* RAII：drop 时释放 flock 并 best-effort 删除 lock 文件 */ }
  pub struct LiveInstanceInfo { pub pid: u32, pub base_url: Option<String>, pub started_at: Option<DateTime<Utc>> }

  pub async fn ensure_control_token_file(path: &Path) -> Result<String>;
  pub fn write_control_info(path: &Path, info: &ControlInfo) -> Result<()>;
  pub fn validate_token_file_perms(path: &Path) -> Result<()>; // unix 上 lstat 校验 0600，windows no-op

  pub fn resolve_control_paths(working_dir: &Path, token_override: Option<&Path>, info_override: Option<&Path>) -> ControlPaths;
  pub fn acquire_control_lock(lock_path: &Path) -> Result<ControlLockGuard, ControlLockError>;
  pub fn inspect_existing_instance(info_path: &Path) -> Result<Option<LiveInstanceInfo>>;
  pub fn pid_is_live(pid: u32) -> bool; // pid 0/out-of-range false；unix: kill(pid, 0)；windows: OpenProcess + GetExitCodeProcess
  ```
- `ControlLockError` 至少包含 `AlreadyHeld { existing: Option<LiveInstanceInfo>, info_path: PathBuf, lock_path: PathBuf }` 与 `Io(std::io::Error)`；`AlreadyHeld` 必须可格式化成可操作的 stderr 消息（含 PID、`baseUrl`、修复建议）。
- 实现细节：
  - `resolve_control_paths`：默认 token 路径 `.libra/code/control-token`；默认 info 路径 `.libra/code/control.json`；lock 路径默认与 info 同目录、同 stem，扩展名换为 `.lock`（默认即 `.libra/code/control.lock`）。当用户显式传 `--control-info-file` 自定义路径时，lock 路径同步跟随；这是上文 Option B 并发逃生口的实现基础——不同实例必须落在不同 info 目录或 stem，否则共用 lock 文件即冲突。
  - `acquire_control_lock`：`OpenOptions::new().create(true).read(true).write(true).truncate(false)` 打开 lock 文件；调用 `fs2::FileExt::try_lock_exclusive()`（或等价 advisory lock：`fd-lock` / 直接 `libc::flock(LOCK_EX | LOCK_NB)`；若 `Cargo.toml` 还没相应依赖，需在本任务中添加）。锁失败时尝试 `inspect_existing_instance(info_path)` 填充 `AlreadyHeld.existing` 以提升错误可读性，再返回错误。锁成功后把当前 PID 写入 lock 文件方便人工排查（仅 PID，**不写 token**）。`ControlLockGuard::drop` 释放锁、best-effort 删除 lock 文件；删除失败仅 redacted debug log。
  - `inspect_existing_instance`：读 info JSON，缺字段 / 解析失败时返回 `Ok(None)` 并记 debug log（防止半截文件阻塞新实例）；解析成功且 `pid_is_live(pid)` 为 true 时返回 `Some(...)`，否则视为 stale 返回 `Ok(None)`。
  - `pid_is_live`：先拒绝 `pid == 0`；unix 还必须拒绝 `pid > i32::MAX as u32`，避免 `u32::MAX` cast 成 `-1` 触发进程组/全局探测语义。合法 PID 再用 `nix::sys::signal::kill(Pid::from_raw(pid as i32), None)`，`Ok(())` 视为存活，`ESRCH` 视为已退出，其他错误（`EPERM`）保守视为存活；windows 用 `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, ...)` + `GetExitCodeProcess`，`STILL_ACTIVE` 才视为存活。
  - `ensure_control_token_file`：父目录不存在时 `create_dir_all`；每次 `--control write` 启动都生成新的 32 字节随机 token（base64），不复用旧文件内容。**调用方必须先成功 `acquire_control_lock`**，本函数不再二次防并发，避免活跃实例的 token 被覆盖。
  - token 文件不存在时用 `OpenOptions::new().create_new(true).mode(0o600)` 创建。
  - token 文件已存在时先 `symlink_metadata` 校验：必须是 regular file 且权限正好 `0600`；通过后用 `OpenOptions::write(true).truncate(true).mode(0o600)` 覆盖写入新 token。
  - perm 宽松时返回错误（不自动修复），错误信息包含 `chmod 0600 <path>` 建议。
  - 不允许 follow symlink：unix open 使用 `OpenOptions::custom_flags(libc::O_NOFOLLOW)`；若 open 因 symlink 失败，返回安全错误。
  - 进程正常退出时 best-effort 删除 token 文件、info 文件，再 drop lock guard；崩溃遗留文件在下次启动时被新 token + 新 info 覆盖（lock 自动随 FD 关闭释放）。
- 单元测试覆盖：
  - 全新创建 → token 0600；lock 文件存在但内容仅 PID（无 token）。
  - 已存在 0600 → 覆盖为新 token，旧 token 失效。
  - 已存在 0644 → 拒绝（错误信息包含修复建议 `chmod 0600 <path>`）。
  - symlink → 拒绝且不写 symlink 目标。
  - control.json fixture 不含 token / token hash（golden test）。
  - **多实例 fail-fast**：手动持有 `acquire_control_lock` 返回的 guard，再次调用应返回 `AlreadyHeld`；当 info 文件存在且 PID 存活时，错误消息含 PID 与 `baseUrl`。
  - **stale 接管**：模拟既有 info 文件指向已退出 PID（写一个稳定不存在的 PID，例如 `u32::MAX`）+ lock 文件未被锁 → 新实例可正常 acquire；接管后旧 control.json 被新实例值覆盖。
  - **自定义路径并发**：传两组互不重合的 `--control-token-file` / `--control-info-file`（lock 路径自动分离）→ 两个 guard 都能 acquire 各自 lock；测试明确演示 Option B 逃生口可用。
  - `pid_is_live(0)` / `pid_is_live(u32::MAX)` 返回 false 且不 panic。

#### Task 1.4 — Web 层 control token 校验

- File: [src/internal/ai/web/mod.rs](src/internal/ai/web/mod.rs)
- 在 `WebAppState` 加字段 `automation_control_token: Option<Arc<str>>`（None 表示 automation write disabled；既有 browser write 语义不变）。
- 新 helper：
  ```rust
  fn ensure_automation_control_token(headers: &HeaderMap, expected: Option<&Arc<str>>) -> Result<(), WebApiError>;
  ```
  - `expected` 为 `None` → `CONTROL_DISABLED`。
  - 缺 `X-Libra-Control-Token` → `MISSING_CONTROL_TOKEN`。
  - 不匹配 → `INVALID_CONTROL_TOKEN`。
- 串接顺序：`ensure_loopback_api_request` → 判断请求/lease kind → automation 路径调用 `ensure_automation_control_token` → controller-token 检查。
- Phase 1 只落 helper、state、单测，不改变现有 browser attach/message/respond 行为；Phase 2 在 automation kind 分支接入。
- observe 端点（session/events/diagnostics）**不**接入。

#### Task 1.5 — 启动时把 control 配置接入 runtime

- File: [src/command/code.rs](src/command/code.rs) `execute_tui`（~lines 799）和 `execute_web_only`（~lines 548）
- 流程：
  1. 用 `resolve_control_paths(working_dir, args.control_token_file.as_deref(), args.control_info_file.as_deref())` 算出 `ControlPaths { token, info, lock }`；`code` 子目录不存在时 `create_dir_all`。
  2. 若 `control == Write`：
     - **先**调 `acquire_control_lock(&paths.lock)`：
       - `Err(AlreadyHeld { existing: Some(info), .. })` → 打印一条人类可读 stderr 错误，至少包含既有 PID、`baseUrl`、`info_path`、`lock_path`，并提示 "Stop the existing instance (Ctrl-C / kill <pid>) or pass `--control-token-file` and `--control-info-file` to use separate paths."；返回非零退出码（沿用 `CliError`/`anyhow::bail!` 风格，不 panic）。
       - `Err(AlreadyHeld { existing: None, .. })` → 同样 fail-fast，但消息只能引用 lock 路径与 info 路径，不要伪造 PID。
       - `Err(Io(_))` → 透传错误；不要 fallback 到无锁路径（否则破坏单实例契约）。
       - `Ok(guard)` → 把 guard 持有到进程结束（同生命周期；通过 `tokio::signal::ctrl_c` / shutdown handler 触发 drop）。
     - 调 `ensure_control_token_file(&paths.token)` 取得 token（lock 已成立，覆盖旧 0600 文件即可）。
     - 把 token 注入 `WebServerOptions`（新增字段 `automation_control_token: Option<Arc<str>>`）→ `WebAppState`。
     - web server 启动成功后，用实际 bound addr 调 `write_control_info(&paths.info, &info)` 写 `control.json`（含 `pid = std::process::id()`，**不含** token / token hash / token path）。TUI 模式当前允许 web server 启动失败后继续运行；`--control write` 下必须改为 fail-hard，因为没有 Web endpoint 就没有可用控制面。
     - MCP server 启动成功后更新或重写 `mcpUrl`；若 MCP 启动失败，先删除 `paths.info` 与 `paths.token` 再返回错误（lock guard 在错误返回时一并 drop，自动释放）。
  3. 若 `control == Observe`：不创建 token、不获取 lock；只有显式传 `--control-info-file` 时才写 `mode: "observe"` 的 info 文件，但**不**附带 lock（observe 实例间不互斥）。
  4. 进程退出前 best-effort 清理 control-token 与 control.json，再 drop lock guard 释放 `.lock` 文件；清理失败只写 redacted debug log，不影响退出。
  5. `WebServerOptions` 字段命名为 `automation_control_token`，避免误导 browser controller 必须使用该 token。
- 集成测试覆盖（`tests/code_control_startup_test.rs`，与 Task 1.3 单测互补）：
  - 同一 working dir 启动两个 `--control write` → 第二个进程在标准错误上输出含 PID 的 `CONTROL_INSTANCE_CONFLICT` 类信息并以非零码退出。
  - 第一个实例 SIGKILL 模拟（直接 drop guard 不删除文件） → 第二个实例视为 stale，正常接管，新 control.json 的 PID 与旧不同。
  - 自定义 `--control-info-file=/tmp/a.json` 与 `--control-info-file=/tmp/b.json`（lock 路径分别为 `/tmp/a.lock` 与 `/tmp/b.lock`）→ 双实例并发 OK。
  - `--control observe` 启动两次 → 不互斥（无锁）。

#### Phase 1 验收

```bash
cargo +nightly fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
# 端到端冒烟（脚本化）：
# 1) cargo run -- code --control write --port 0 --web-only &
# 2) 检查 .libra/code/control-token 权限 0600（stat）
# 3) 检查 .libra/code/control.json 不含 token / token-hash 字段
# 4) 检查 .libra/code/control.lock 存在（lsof / fuser 可见被首个进程持有）
# 5) 在同一 working dir 再次 cargo run -- code --control write --port 0 --web-only
#    → 立即非零退出，stderr 含既有 PID 与 baseUrl 提示
# 6) 杀掉首个进程后，第三次启动可成功接管（视为 stale）
# 7) cargo test code_control_files code_control_auth code_control_startup
# 8) 说明：不要在 Phase 1 冒烟使用 --web-only --provider codex；当前 validate_mode_args 会拒绝该组合。
```

---

### Phase 2 — TUI Automation Adapter

**目标**：HTTP write 请求经 `TuiCodeUiAdapter` 翻译成独立 control command；`Automation` lease 全功能；TUI 输入只读 + reclaim；`/api/code/control/cancel` 端点上线。

#### Task 2.1 — 新增 `TuiControlCommand` channel（不要改 `AppEvent`）

- New file: `src/internal/tui/control.rs`
- File: [src/internal/tui/mod.rs](src/internal/tui/mod.rs)，新增 `mod control;`，只按需要 re-export 给 `code.rs` / adapter。
- 新增 domain type，避免 TUI 层依赖 WebApiError：
  ```rust
  pub enum TuiControlCommand {
      SubmitMessage {
          text: String,
          ack: oneshot::Sender<Result<(), TuiControlError>>,
      },
      RespondInteraction {
          interaction_id: String,
          response: CodeUiInteractionResponse,
          ack: oneshot::Sender<Result<(), TuiControlError>>,
      },
      CancelCurrentTurn {
          ack: oneshot::Sender<Result<(), TuiControlError>>,
      },
      ReclaimController {
          ack: oneshot::Sender<Result<(), TuiControlError>>,
      },
  }
  ```
- `TuiControlError` 至少覆盖：`Busy`、`InteractionNotActive`、`UnsupportedInteractionKind`、`ControllerConflict`、`Internal(String)`；由 adapter 映射到 `CodeUiApiError` code。
- 新增 `CancelSource { Esc, SlashQuit, Automation }`，用于 audit、MCP turn decision 和 UI 文案；不需要暴露到 HTTP response。
- File: [src/internal/tui/app.rs](src/internal/tui/app.rs)
  - `AppConfig` 增加 `code_control_rx: Option<UnboundedReceiver<TuiControlCommand>>`。
  - 主 `tokio::select!` 增加 `Some(command) = code_control_rx.recv()` 分支，调用 `handle_tui_control_command(command).await`。
  - `SubmitMessage` 处理必须在 App 内部调用现有 `submit_message(text).await`，让 slash command、plain-message planning workflow、pending revision guard 都走同一入口；App 继续独占 `begin_turn()` / `next_turn_id`。v1 HTTP `CodeUiMessageRequest` 只承载 `text`，不新增 `allowedTools` 字段。
  - 提交前校验本地状态：非 `AgentStatus::Idle`、存在 pending interaction、或正在 revision/plan gate 时返回 `Busy`（HTTP 映射 `SESSION_BUSY`）。
  - `RespondInteraction` 校验 `interaction_id` 与当前 pending state 匹配且 snapshot 中仍为 `Pending`；不匹配返回 `InteractionNotActive`。
  - 抽出 helper：`respond_pending_interaction_from_code_ui(interaction_id, response)`，复用 `submit_user_input_answer`、`submit_exec_approval_decision`、`submit_phase_confirmation_decision`、`submit_managed_interaction_decision` 的底层发送逻辑。不要通过模拟键盘事件实现。
  - 抽出 helper：`cancel_current_turn(source: CancelSource)`，复用 `interrupt_agent_task`、pending interaction cleanup、Code UI status 更新、MCP turn decision 记录。Esc、automation cancel 共用该 helper。
- File: [src/internal/tui/app_event.rs](src/internal/tui/app_event.rs)
  - 不新增 automation/control variant。
  - 保留 `turn_id_is_exposed_for_turn_scoped_events` 测试；该测试应继续证明 `AppEvent` 只承载 turn-scoped events。

#### Task 2.2 — `TuiCodeUiAdapter` 实现

- New file: `src/internal/tui/code_ui_adapter.rs`
- File: [src/internal/tui/mod.rs](src/internal/tui/mod.rs)，新增 `mod code_ui_adapter;` 并 re-export `TuiCodeUiAdapter` 构造函数。
- `impl CodeUiCommandAdapter`（trait 在 [code_ui.rs:532](src/internal/ai/web/code_ui.rs)）
- 内部持有：
  - `control_tx: UnboundedSender<TuiControlCommand>`
  - `snapshot_handle: Arc<CodeUiSession>` 用于读 interaction 状态。
- 方法：
  - `submit_message(text)` → 创建 oneshot → 发 `TuiControlCommand::SubmitMessage` → 等待 App ack（默认 30s timeout）。HTTP 返回只表示 App 已接受/拒绝，不等待 agent 完成。
  - `respond_interaction(id, response)` → 先用 snapshot 快速检查 id 是否 pending（降低无效请求进入 App），再发 command，以 App ack 为准。
  - `cancel_turn()` → 需要先扩展 `CodeUiCommandAdapter` trait 增加默认方法 `cancel_turn()`；默认返回 unsupported，TUI adapter 实现为 command + ack。
- 启动接线：[code.rs:execute_tui](src/command/code.rs) 创建 control channel；`rx` 注入 `AppConfig`，`tx` 注入 `TuiCodeUiAdapter`，再构建 TUI 模式的 `CodeUiRuntimeHandle`。
- TUI 模式下 automation write 永远走 `TuiCodeUiAdapter`；web-only codex 继续用 `CodexCodeUiAdapter`。Codex provider 的默认 TUI 合并由 Task 2.7 完成，不能作为 stretch goal，也不能绕过本地 App 状态机。

#### Task 2.3 — 参数化 lease

- File: [src/internal/ai/web/code_ui.rs:570-714](src/internal/ai/web/code_ui.rs)
- 重命名 `BrowserControllerLease` → `ControllerLease { kind, client_id, token, expires_at }`。
- 调整 `CodeUiControllerRuntimeState`：
  - 保留真正不可让渡的 `fixed`（例如 CLI/web-only 固定 owner）。
  - 新增 `local_tui_owner: Option<FixedController>` 或等价字段；当没有 active automation lease 时，snapshot controller 显示 `Tui`。
  - `--control write` 的 TUI runtime 使用 `local_tui_owner`，不能使用会阻止 attach 的 `fixed`。
- 函数签名调整：
  - `attach_browser_controller` 保留为 browser wrapper。
  - 新增 `attach_controller(kind, client_id, owner_label)`；`kind == Automation` 时要求 runtime 的 automation control 已启用。
  - `detach_browser_controller` 保留为 browser wrapper。
  - 新增 `detach_controller(kind, client_id, token, force)`；`force` 只给本地 TUI reclaim 用。
  - `ensure_browser_write_access` 保留为 browser wrapper。
  - 新增 `ensure_controller_write_access(token)`，返回 active `ControllerLease`，供 HTTP handler 判断是否需要 control token。
- 保留 thin wrapper 以减少 web/mod.rs 改动量；wrapper 内部 hard-code kind。
- attach 冲突：已有 active lease（任何 kind）且 client_id 不同 → `CONTROLLER_CONFLICT`；同 client_id 视为续约。
- 同步 `WebAppState` 与 `code_router`：`POST /api/code/controller/attach` body 接受 `{ "clientId": "...", "kind": "automation" | "browser" }`，缺省保留 `"browser"` 以向后兼容。
- web handler 规则：
  - `kind` 缺省或 `"browser"`：走旧 browser wrapper，不要求 `X-Libra-Control-Token`。
  - `kind == "automation"`：先 `ensure_automation_control_token`，再 `attach_controller(Automation, ...)`。
  - `/messages` / `/interactions/{id}`：先用 controller token 找 lease；若 lease.kind == Automation，再要求 control token；若 lease.kind == Browser，保持旧行为。

#### Task 2.4 — TUI 输入只读 + 抢回

- File: [src/internal/tui/app.rs](src/internal/tui/app.rs)
- 在 `handle_key_event` 中通过 `code_ui_session.snapshot().await` 或 App 内缓存的 controller state 判断 `controller.kind == Automation && lease 未过期`：
  - `Ctrl-C`、`/quit`、`/control reclaim`、滚动与 mux 浏览不受限。
  - 普通 freeform Enter 不提交 message；字符输入只作为本地 slash-command buffer 使用，不进入 agent turn。
  - 状态栏渲染 "Automation in control · /control reclaim"。
- 新增 `/control reclaim` slash command：
  - File: [src/internal/tui/slash_command.rs](src/internal/tui/slash_command.rs)，新增 `BuiltinCommand::Control`，`/control reclaim` 作为 args 分支处理。
  - 行为：调 `runtime.reclaim_local_tui_controller()`（或等价的专用 force-detach helper）。不要把 `force=true` 暴露成通用 HTTP detach 参数；force 只允许 App 内部调用，理由是 TUI 物理同机用户是最终控制方。
  - detach 后 broadcast snapshot（kind 回 `Tui`），下一次 automation 写请求拿不到匹配 token → `INVALID_CONTROLLER_TOKEN`。

#### Task 2.5 — `POST /api/code/control/cancel`

- File: [src/internal/ai/web/mod.rs](src/internal/ai/web/mod.rs)
- 加路由：`.route("/control/cancel", post(code_cancel_handler))`。
- handler 鉴权链：loopback → control-token → controller-token → 调 adapter `cancel_turn()`。
- adapter 翻译为 `TuiControlCommand::CancelCurrentTurn`。

#### Task 2.6 — 写请求大小限制

- File: [src/internal/ai/web/mod.rs](src/internal/ai/web/mod.rs)
- 对 `POST /api/code/messages`、`POST /api/code/interactions/{id}`、`POST /api/code/control/cancel` 配置 256KiB body limit；超过返回 `413 PAYLOAD_TOO_LARGE`，错误 body 使用现有 `{ code, message }` 格式。
- 单测覆盖超长 message 不会进入 adapter。

#### Task 2.7 — Codex 执行接入默认 TUI，移除 Codex 单独 TUI

- Files:
  - [src/command/code.rs](src/command/code.rs) `CodeProvider::Codex` 分支、`start_codex_code_ui_runtime`、`run_tui_with_managed_code_runtime`。
  - [src/internal/ai/codex/mod.rs](src/internal/ai/codex/mod.rs) `start_code_ui_runtime` 与旧 `execute` stdin 主循环。
  - [src/internal/tui/app.rs](src/internal/tui/app.rs) managed runtime event / interaction 处理。
- 用户可见 contract：
  - `libra code --provider codex` 总是启动默认 Libra TUI（同一套 composer、bottom pane、status、slash commands、history cells）。
  - Codex app-server 只作为 managed execution backend；不得再由 Codex 侧读取 stdin、打印 approval prompt、或渲染自己的 TUI。
  - `--web-only --provider codex` 仍可保留 `CodexCodeUiAdapter` 路径；本任务只移除交互式 TUI 模式下的 Codex 单独 UI。
- 实现步骤：
  1. 在 `execute_tui` 的 `CodeProvider::Codex` 分支中保留 managed Codex app-server lifecycle，但明确走 `run_tui_with_managed_code_runtime` 和默认 `App`；不调用 `agent_codex::execute`。
  2. 调整 `start_codex_code_ui_runtime` 的 `ui_mode`：TUI 模式传递显式 backend mode（建议 `"managed-tui"`；若 Codex app-server 暂不支持，则使用现有非终端模式并在代码注释说明），避免触发 Codex 自己的 terminal UI / stdin loop。
  3. 在 `agent_codex::start_code_ui_runtime` 保持 WebSocket writer/reader、snapshot publish、approval request 转发；禁止在该 runtime 内直接读 stdin 或写用户可见 stdout。
  4. 将 Codex approval / request-user-input 全部转换为 `CodeUiInteractionRequest`，由默认 TUI 的 pending interaction UI 处理；响应通过 `CodeUiInteractionResponse` / existing codex adapter 写回 app-server。
  5. 若 `agent_codex::execute` 仍保留给 legacy/internal 用途，标记为非 `libra code` 路径，迁移或删除其 stdin approval loop；不得由 `src/command/code.rs` 引用。
  6. 移除 Codex 专属 TUI 文案、提示和测试假设；provider 差异只显示为 provider/model/capabilities。
- 验收：
  - `cargo run -- code --provider codex` 展示默认 Libra TUI chrome；输入框、`/help`、`/quit`、状态栏与其他 provider 一致。
  - 提交消息后只出现一套 transcript 渲染；没有 Codex 侧 stdout prompt、重复 streaming 文本或第二套输入循环。
  - Codex tool approval 在默认 bottom pane 中出现，人工选择后能写回 Codex app-server。
  - `/api/code/session` 和 `/api/code/events` 仍能观察 Codex run snapshot。
  - `rg "agent_codex::execute|stdin_rx|std::io::stdin" src/command src/internal/tui src/internal/ai/codex` 的结果证明 `libra code --provider codex` 路径不再进入 stdin 主循环（legacy 函数若保留需有明确注释和测试隔离）。
- 测试：
  - 新增 `tests/code_codex_default_tui_test.rs` 源码级 routing guard：`libra code --provider codex` 不调用 legacy `agent_codex::execute`，Codex 分支走 `run_tui_with_managed_code_runtime`，TUI/command 路径无 `std::io::stdin` 主循环。
  - 回归测试覆盖 `--web-only --provider codex` 不受默认 TUI 合并影响。

#### Phase 2 验收

```bash
cargo +nightly fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
# 端到端：以 ollama provider 启动 TUI（避免 codex 依赖）
# 1) cargo run -- code --control write --provider ollama
# 2) 另一终端：attach + submit + 观察 TUI 中出现 transcript user 消息 + agent 开始处理
# 3) 在 TUI 内按 Enter 输入文字 → 不提交，状态栏提示 "Automation in control"
# 4) TUI 内输入 /control reclaim → 自动化 lease 失效；下一次 automation 写请求 → INVALID_CONTROLLER_TOKEN
# 5) attach 后再 cancel → TUI 看到 turn 中断（与 Esc 行为一致）
# Codex 默认 TUI：
# 6) cargo run -- code --provider codex
# 7) 确认只出现默认 Libra TUI；Codex execution、approval、snapshot 都经默认 TUI / Code UI runtime。
```

自动化覆盖目标（由 `src/internal/ai/web/code_ui.rs` / `src/internal/tui/code_ui_adapter.rs` 单测与 `tests/code_ui_scenarios.rs` 跨进程场景共同覆盖）：
- attach automation → submit → snapshot.transcript 含新消息。
- attach automation → respond approval → orchestrator 收到 oneshot 回应。
- attach automation → reclaim → 旧 token 失效。
- interaction id 不存在 / 已 resolved → `INTERACTION_NOT_ACTIVE`。
- Codex provider → 默认 TUI routing guard 成功，无 legacy stdin loop。

---

### Phase 3 — `code-control --stdio` Shim

**实现状态（2026-04-30）**：已落地。`libra code-control --stdio` 提供本地 NDJSON JSON-RPC 2.0 bridge；`libra code --stdio` 仍是 MCP stdio transport。

- New file: `src/command/code_control.rs` + 在 [src/command/mod.rs](src/command/mod.rs) 新增 `pub mod code_control;` + 在 [src/cli.rs](src/cli.rs) 新增 `Commands::CodeControl(...)` 分支。
- NDJSON JSON-RPC 2.0 dispatcher 自实现（rmcp 是 MCP-specific，不复用）。
- HTTP backend 用 `reqwest`；SSE notification 由 `src/command/code_control.rs` 内的轻量 parser 读取 `/api/code/events` byte stream。
- Method ↔ HTTP 映射见上文表。
- 错误映射：HTTP 4xx/5xx → JSON-RPC error.data 含 HTTP status + libra error code。
- 验收：
  - mock backend 集成测试覆盖 attach → subscribe → submit → detach。
  - malformed JSON / unknown method / 403 / 409 都有稳定错误。
  - 既有 `libra code --stdio` MCP 测试不变（`tests/e2e_mcp_flow.rs`）。

---

### Phase 4 — Diagnostics & Audit

#### Task 4.1 — `GET /api/code/diagnostics`

- File: [src/internal/ai/web/mod.rs](src/internal/ai/web/mod.rs)
- 加路由（observe，仅 loopback）；handler 调 `runtime.diagnostics()` 返回 `CodeUiDiagnostics`。
- New type: `CodeUiDiagnostics` in [src/internal/ai/web/code_ui.rs](src/internal/ai/web/code_ui.rs)，按 redaction 规则表填充（含 controller 信息）。

#### Task 4.2 — Audit event 接入

- File: [src/internal/ai/web/mod.rs](src/internal/ai/web/mod.rs)
- `WebAppState` 加 `audit_sink: Arc<dyn AuditSink>`（启动时默认 `Arc::new(TracingAuditSink)`，复用 [hardening.rs:256](src/internal/ai/runtime/hardening.rs)）。
- 新增 `ControlAuditRecord` + `append_control_audit(...)` helper，把 control 专属字段 redacted 后写入 `AuditEvent.redacted_summary`；不要直接改 `AuditEvent` 字段结构。
- 在 5 个 write handler 入口/出口 await `append_control_audit(...)`。
- 失败也写 audit（`result: "error", error_code: "..."`）。

#### Task 4.3 — Redaction 规则覆盖与 golden test

- 扩充 `SecretRedactor` markers（[hardening.rs:153-161](src/internal/ai/runtime/hardening.rs)）补 control token 模式。
- 新增 `tests/diagnostics_redaction_test.rs` golden-style integration test，覆盖 diagnostics 中 controller owner / active interaction 等字符串字段会经 `SecretRedactor` 过滤。
- 单测覆盖：env dump / provider body / shell excerpt 全部经规则表过滤。

#### Task 4.4 — Approval scope 隔离

- grep 现有审批 memo key 写入位置（关键词：`approval`, `memo`, `once_allow`，参考 [src/internal/ai/orchestrator/policy.rs](src/internal/ai/orchestrator/policy.rs)）。
- 当 active controller kind == Automation 时，scope key 拼前缀 `automation:`；TUI/Browser 保持原行为。
- 单测验证：交互式 session 的"once allow"在 automation 接管后不被继承。

#### Phase 4 验收

```bash
cargo +nightly fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo test diagnostics_redaction_test
# 手工验证：
# - curl GET /api/code/diagnostics → JSON 不含 token、API key、env dump、provider body
# - tracing 日志中可按 thread_id grep 到 5 类 audit 行
```

---

### Phase 5 — Documentation Deliverables

**目标**：把已完成的 Phase 1–4 行为产出对外契约文档与开发者注释；不接受"代码合入但文档缺失"的 PR。每个文档章节随对应 Phase 一起提交，避免最后一次性补文。

文档语言原则：以仓库内同类文档主语言为准（当前 `docs/` 既有中文也有英文，新文档与最近相邻文件对齐；如新建独立文件，默认中文为主、技术名词保留英文，并允许后续单独 PR 翻译）。

#### Task 5.1 — 用户文档清单

按 audience 分三类文档，落地为下述具体文件：

| 文件 | 类别 | 状态 | 内容要点 |
|------|------|------|---------|
| `docs/commands/code.md` | CLI 参考（既有） | 增补 | 新增 `--control` / `--control-token-file` / `--control-info-file` 三 flag 表；`.libra/code/{control-token,control.json,control.lock}` 文件契约；observe 默认 vs write 显式开启的语义；与 `--stdio`、`--web-only`、`--host` 的互斥规则；`/control reclaim` 内置命令；进程退出 stale 接管行为。 |
| `docs/commands/code-control.md` | CLI 参考（新建） | Phase 3 上线时合并 | 新 subcommand 用法、flag 表、JSON-RPC 2.0 method ↔ HTTP 映射、错误映射、最小 NDJSON 端到端示例。 |
| `docs/automation/local-tui-control.md` | 主指南（新建） | 必交付 | 单一入口主文档，章节见下。 |
| `docs/improvement/code.md` | 改进计划（既有） | 改 1 处 | 在 Phase 3 "Code UI Source Of Truth Unification" 段落加一句 cross-link：本计划引入的 `Automation` lease 与 control token 是该统一的子集，typed delta 迁移时不要破坏其语义。 |

`docs/automation/local-tui-control.md` 章节骨架：

1. **Overview** — 一段说明这是 *本机* 自动化控制面，不是远程 API；列出三个使用场景（脚本、harness、调试）。
2. **Security Model** — loopback 约束、token 文件 0600、两层鉴权（control vs controller）、redaction 规则、多实例锁与 stale 接管；显式不防御项（root、ptrace、同用户进程）。
3. **HTTP / SSE API Reference** — 八个端点表 + 鉴权矩阵 + 每端点 request/response/error JSON schema + curl 端到端 attach → submit → respond → cancel → detach 示例（与 `tui.md` 主表逐项一致）。
4. **JSON-RPC 2.0 Reference**（Phase 3 上线后填充；之前显式标注 "deferred to v1.1"） — method 表、payload schema、与 HTTP 错误的映射。
5. **Error Code Reference** — `CONTROL_DISABLED` / `LOOPBACK_REQUIRED` / `MISSING_CONTROL_TOKEN` / `INVALID_CONTROL_TOKEN` / `MISSING_CONTROLLER_TOKEN` / `INVALID_CONTROLLER_TOKEN` / `CONTROLLER_CONFLICT` / `SESSION_BUSY` / `INTERACTION_NOT_ACTIVE` 一栏一码：触发条件 + 修复建议；启动期错误（多实例冲突、非 loopback host、--stdio + write 互斥）单独一节。
6. **Quickstart Recipes** — bash + curl 最小自动化、Python `requests` + `sseclient-py` 订阅事件、`libra code-control --stdio` 在 Phase 3 后的 NDJSON 客户端示例。
7. **Troubleshooting** — `INVALID_CONTROL_TOKEN`（token 过期或被 reclaim）、`CONTROLLER_CONFLICT`（已有 writer）、多实例冲突 stderr 解读、redaction 后 diagnostics 字段为空的合理性。

#### Task 5.2 — 开发者文档清单

新模块顶部加 `//!` module doc，公共类型/方法补 `///`：

| 位置 | 必须说明 |
|------|---------|
| `src/command/code_control_files.rs` 模块 doc | 文件 lifecycle、lock 契约、stale 接管策略、跨平台权限差异。 |
| `src/internal/tui/control.rs` 模块 doc | `TuiControlCommand` 为何独立于 `AppEvent`（无 turn 上下文）、与 App 主循环的 `tokio::select!` 接线方式、`oneshot` ack 的超时假设。 |
| `src/internal/tui/code_ui_adapter.rs` 模块 doc | TUI 模式 `CodeUiCommandAdapter` 的桥接职责；与 `CodexCodeUiAdapter`（web-only codex）的边界。 |
| `CodeUiControllerKind::Automation` 的 `///` | 双 token 鉴权契约（process-level + lease-level）、与 browser kind 的兼容边界。 |
| `attach_controller` / `detach_controller` / `ensure_controller_write_access` 的 `///` | 失败错误码、TTL、`force` 参数语义、wrapper 与底层函数的对应。 |
| `code_router` 注册块附近注释 | 八端点的 auth layer 与 phase 映射（与 HTTP 表保持一致）。 |
| `command/code_control.rs` 模块 doc（Phase 3 时） | NDJSON dispatcher 设计；为何不复用 rmcp。 |

`cargo doc --no-deps --all-features` 必须无 broken intra-doc link warning。

#### Task 5.3 — Phase ↔ 文档时序对齐

随 Phase 同步交付，避免文档滞后：

| Phase | 用户文档动作 | 开发者文档动作 |
|-------|---------|------------|
| 1 | `code.md` 增补 `--control*` 段；`local-tui-control.md` 写 Overview + Security Model（含多实例锁、stale 接管） | `code_control_files.rs` 模块 doc |
| 2 | `local-tui-control.md` HTTP API 段落（端点、鉴权、curl 示例、错误码 7 项）；`code.md` 加 `/control reclaim` 段；`docs/improvement/code.md` cross-link | `tui/control.rs` + `tui/code_ui_adapter.rs` 模块 doc；`attach_controller` 等方法 `///`；`code_router` 注释 |
| 3 | `code-control.md` 新建；`local-tui-control.md` JSON-RPC 段填充；Quickstart 加 stdio 例 | `command/code_control.rs` 模块 doc |
| 4 | `local-tui-control.md` 加 `/diagnostics` + Audit Schema 段、redaction 规则示例；Troubleshooting 加最后两项 | `runtime/hardening.rs` 新增 marker / sink 注释 |
| 6 | `local-tui-control.md` 加 "Writing your own scenario"、PTY harness 复现步骤、artifact 解读 | `tests/harness/` 模块 doc；fake provider 模块 doc；scenario fixture schema 注释 |

#### Task 5.4 — 文档与代码一致性 Lint

新增脚本（建议 `scripts/check_docs_consistency.sh`，CI 可选）或 PR checklist 项，至少覆盖：

- `cargo doc --no-deps --all-features` 无 warning。
- HTTP 路径双向 grep：`rg "/api/code/" docs/automation/local-tui-control.md src/internal/ai/web/mod.rs` 两侧端点列表一致。
- 错误码双向 grep：`rg "CONTROL_DISABLED|LOOPBACK_REQUIRED|MISSING_CONTROL_TOKEN|INVALID_CONTROL_TOKEN|MISSING_CONTROLLER_TOKEN|INVALID_CONTROLLER_TOKEN|CONTROLLER_CONFLICT|SESSION_BUSY|INTERACTION_NOT_ACTIVE" docs/ src/`。
- Header 名双向 grep：`rg "X-Libra-Control-Token|X-Code-Controller-Token" docs/ src/`。
- CLI flag 双向 grep：`cargo run -- code --help` 输出包含的 `--control*` flag 必须在 `docs/commands/code.md` 中出现。
- JSON 字段双向 grep（Phase 3 / 4）：`baseUrl`、`mcpUrl`、`controller.kind`、`leaseExpiresAt`、`activeInteractionId` 在 schema 文档与 Rust 序列化结构一致。

#### Phase 5 验收

```bash
# 文档构建
cargo doc --no-deps --all-features

# 文档 / 代码一致性（可放进 PR checklist）
bash scripts/check_docs_consistency.sh   # 若已落地脚本
# 或人工跑：
rg "/api/code/(session|events|messages|interactions|controller|control/cancel|diagnostics)" docs/ src/internal/ai/web/
rg "X-Libra-Control-Token" docs/ src/

# Markdown lint（如仓库已有配置则跑；无则跳过）
# markdownlint docs/

# CLI 帮助与文档 spot check
cargo run -- code --help | rg -- '--control'
# Phase 3 之后追加：
# cargo run -- code-control --help
```

---

### Phase 6 — Automation-Driven TUI Test Harness

**目标**：把 Phase 1–4 的 control surface 包装成可复用的端到端测试 harness。测试必须启动真实 `libra code` TUI 子进程，通过 HTTP control endpoints 执行 attach / submit / respond / cancel / reclaim，并断言 snapshot、controller、interaction、transcript、diagnostics 与 audit 日志。

**实现状态（2026-04-30）**：Phase 6A 已落地。交付包括 `test-provider` feature、hidden fake provider、`portable-pty` harness、轻量 `Scenario` DSL（`tests/harness/scenario.rs`）、`tests/harness_self_test.rs`、`tests/code_ui_scenarios.rs`（6 个 scenario：basic chat / reclaim / cancel / oversize / unknown interaction / multi-instance conflict）、`tests/fixtures/code_ui/{basic_chat,delayed_chat}.json`、`--port 0` 真实端口写回修复，以及 `tests/code_codex_default_tui_test.rs`（Phase 2 Task 2.7 的源码级 routing guard，无需启动真实 Codex backend）。CI 已通过 `.github/workflows/base.yml` 的 "Run TUI automation scenarios" step 跑这套 scenario + harness 自检 + Codex routing guard，并在失败时上传 `target/code-ui-scenarios/**` 工件。6B 的 Phase 0/1、approval full-flow 和 transcript-level redaction e2e 是未来扩展项，不属于当前 v1 验收；Phase 4 的 diagnostics redaction 已由 `tests/diagnostics_redaction_test.rs` 覆盖。

执行前置：Phase 1（control token + lock）与 Phase 2（automation lease + `TuiCodeUiAdapter`）必须先 land；Phase 4 完成后才启用 audit / diagnostics / redaction 场景。Phase 6 不替代 Phase 2 的 in-process adapter 单测；它只补跨进程、真实 CLI、真实 TUI runtime 的回归覆盖。

#### 关键决策

1. **真实 TUI 必须跑在 PTY 中**：`cargo test` 没有交互终端。harness 使用 dev-dependency `portable-pty` 启动 `libra code --control write --port 0`，设置 `TERM=xterm-256color` 与固定尺寸（建议 120x40），并把 PTY 输出写入 `pty.log`。
2. **直接走 HTTP，不依赖 stdio shim**：Phase 3 的 `code-control --stdio` 可以推迟；harness 的底层 client 直接请求 `/api/code/*` 并轮询 snapshot，避免把两个新系统互相绑定。
3. **fake provider 只模拟 provider 输出**：test-only provider 返回 `CompletionResponse` 的 text / tool_call / error / optional stream delta。它不得直接创建 approval、user input 或 plan-review interaction；这些必须由现有 tool loop、sandbox、App 状态机真实触发。
4. **直接聊天场景显式用 `/chat`**：当前普通 provider 的 plain message 会进入 IntentSpec / Plan workflow。期望“一次 submit 后立刻出现 assistant 文本”的 scenario 必须提交 `/chat ...`；plain message scenario 必须按 Phase 0/1 workflow 编写 fixture。
5. **路径默认隔离，冲突场景例外**：普通 scenario 显式传独立 `--control-token-file` / `--control-info-file` 到 `TempDir`；只有多实例冲突 scenario 使用同一 working dir 的默认路径。
6. **日志由 harness 显式配置**：每个 session 设置 `LIBRA_LOG_FILE=<logs_dir>/libra.log` 与合适的 `LIBRA_LOG`，再从 `libra.log` 断言 audit substring；不要假设存在单独 `audit.log` 或 JSON tracing 格式。
7. **显式 shutdown**：`CodeSession::shutdown()` 负责 graceful quit、等待子进程、收口 PTY reader；`Drop` 只作兜底，测试断言不得依赖 Drop。

#### Task 6.1 — PTY harness 基础库 `CodeSession`

- New file: `tests/harness/mod.rs`
- New file: `tests/harness/code_session.rs`
- `Cargo.toml` dev-dependency：`portable-pty`（仅测试使用）。
- 关键类型：
  ```rust
  pub struct CodeSessionOptions {
      pub fixture: PathBuf,
      pub name: String,
      pub use_default_control_paths: bool,
  }

  pub struct CodeSession {
      child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
      writer: Option<Box<dyn Write + Send>>,
      reader_thread: Option<std::thread::JoinHandle<()>>,
      base_url: String,
      control_token: String,
      controller_token: Option<String>,
      client: reqwest::blocking::Client,
      logs_dir: PathBuf,
      info_path: PathBuf,
      token_path: PathBuf,
  }
  ```
- `spawn(builder)` 流程：
  1. 用 `assert_cmd::cargo::cargo_bin("libra")` 或 `CARGO_BIN_EXE_libra` 找当前测试 binary。
  2. 通过 `portable-pty` openpty，设置 env：`TERM=xterm-256color`、`LIBRA_ENABLE_TEST_PROVIDER=1`、`LIBRA_LOG_FILE=<logs_dir>/libra.log`、`LIBRA_LOG=info,libra::internal::ai::web=debug`。
  3. 启动命令：`libra code --provider fake --fake-fixture <fixture> --control write --port 0 --mcp-port 0 --control-token-file <tmp>/control-token --control-info-file <tmp>/control.json`；多实例冲突 scenario 使用同一 working dir 的默认 control paths。
  4. 轮询 `control.json`（30s 超时、100ms 间隔），解析真实 `baseUrl`；读取 token 文件。
  5. PTY reader 持续写 `logs_dir/pty.log`，失败时保留最近输出供 panic message 使用；`debug_context()` 会同时 dump snapshot、`control.json`、`pty.log` tail 与 `libra.log` tail，并脱敏 token。
- 公开方法：
  - `attach_automation(client_id)`：自动注入 `X-Libra-Control-Token` 并保存返回的 `X-Code-Controller-Token`。
  - `submit_message(text)` / `respond_interaction_expect_error(interaction_id)` / `cancel_turn()` / `write_tui_line("/control reclaim")`。
  - `snapshot()` / `diagnostics()`。
  - `wait_for_snapshot(predicate, timeout)`；超时 dump 最后 snapshot、`pty.log` tail、`libra.log` tail 与 `control.json`。
  - `submit_message_expect_error(text)`、`submit_large_message(bytes)`、`run_default_control_conflict()`。
  - `shutdown()`：先尝试在 PTY 输入 `/quit\r`，5s 内未退出则 kill，随后等待 reader thread 收口。
- 自检测试：
  - `tests/harness_self_test.rs` 用 fake provider + PTY 启动 TUI，等待 `control.json`，获取 snapshot 与 diagnostics，调用 `shutdown()`，断言子进程退出且临时 token/info 文件被清理。

#### Task 6.2 — Fake provider（test-provider feature）

- `Cargo.toml` features 新增 `test-provider = []`。
- New files: `src/internal/ai/providers/fake/{mod.rs, completion.rs, fixture.rs}`。
- File: [src/internal/ai/providers/mod.rs](src/internal/ai/providers/mod.rs) 新增 `#[cfg(feature = "test-provider")] mod fake;`。
- File: [src/command/code.rs](src/command/code.rs)：在 `CodeProvider` 加 hidden `Fake` variant（`#[cfg(feature = "test-provider")]` + clap hidden value），`CodeArgs` 加 hidden `--fake-fixture <PATH>`。
- 安全边界：
  - `validate_mode_args` 中要求 `--provider fake` 必须同时满足 `cfg(feature = "test-provider")`、`--fake-fixture` 存在、`LIBRA_ENABLE_TEST_PROVIDER=1`。缺任一项返回 command usage error。
  - fake provider 不进默认 features；即便 `--all-features` 编译出来，也必须由显式 env 才能运行。
- Fixture schema（provider-native，不直接造 TUI interaction）：
  ```json
  {
    "version": 1,
    "responses": [
      {
        "match": { "last_user_regex": "^hello", "phase": "chat" },
        "events": [
          { "type": "stream_text", "text": "hi " },
          { "type": "text", "text": "there" }
        ]
      },
      {
        "match": { "phase": "phase0" },
        "events": [
          {
            "type": "tool_call",
            "id": "call-input-1",
            "name": "request_user_input",
            "arguments": {
              "questions": [{
                "id": "risk_profile",
                "header": "Risk",
                "question": "Risk level?",
                "options": ["Low", "Medium", "High"]
              }]
            }
          },
          {
            "type": "tool_call",
            "id": "call-intent-1",
            "name": "submit_intent_draft",
            "arguments": { "draft": { "...": "valid IntentDraft fixture" } }
          }
        ]
      }
    ],
    "fallback": { "type": "error", "message": "no fake provider response matched" }
  }
  ```
- 支持 event：`stream_text`、`thinking`、`text`、`tool_call`、`error`、`delay_ms`。`tool_call` 组装为 `AssistantContent::ToolCall`；`stream_text` 只通过 `CompletionRequest.stream_events` 发 delta，最终 response 仍必须包含完整 text 或 tool calls。
- 匹配维度至少包括：`last_user_regex`、`phase`（由 prompt/preamble 分类：`chat` / `phase0` / `phase1` / `execution` / `repair`）、`after_tool_result`（可选，匹配最近 tool result 名称）。不要按全 prompt 字符串精确匹配，避免 prompt 文案微调导致 fixture 全量失效。
- 单测：fixture 解析、fallback、stream delta + final text 一致、tool_call 参数 round-trip、`request_user_input` fixture 经真实 handler 阻塞并由测试释放。

#### Task 6.3 — Scenario DSL

> **状态（2026-04-30）**：已落地轻量 v1。`tests/harness/scenario.rs` 包装 `CodeSession` 的常用 step / assertion，并在失败上下文中追加 scenario 名、step 名、最新 snapshot、`pty.log` tail、`libra.log` tail 与 `control.json`。当前 basic chat scenario 已使用该 DSL；更复杂的 Phase 0/1 / approval builder 留作未来扩展。

- New file: `tests/harness/scenario.rs`
- API 示例：
  ```rust
  let mut scenario = Scenario::new("basic_chat", &mut session);
  scenario
      .step("attach")
      .attach_automation("scenario-basic")?
      .expect_controller_kind("automation")?;
  scenario
      .step("submit direct chat")
      .submit("/chat hello")?
      .expect_transcript_contains("hi there")?
      .expect_status_eq("idle")?;
  ```
- 失败时 error context 必须包含：scenario 名、step 名、最近 snapshot、`pty.log` tail、`libra.log` tail、control.json 内容（确认不含 token）。完整 artifact 保留在 `target/code-ui-scenarios/<scenario>/`。
- Assertion 不解析不存在的 `audit.log`；Phase 4 后使用 `log_contains("action=...")` 或 `redacted_summary` substring 验证 control audit。

#### Task 6.4 — 标准 scenario 套件

- New file: `tests/code_ui_scenarios.rs`
- New dir: `tests/fixtures/code_ui/`
- 每个 scenario 使用 `#[tokio::test(flavor = "multi_thread")] #[serial]`；后续若证明完全隔离再放宽。

| 场景 | Fixture | 阶段 | 主要断言 |
|------|---------|------|---------|
| direct chat submit | `basic_chat.json` | 6A | attach automation → submit `/chat hello` → transcript 含 fake 文本 → status 回 idle |
| automation reclaim | `basic_chat.json` | 6A | attach 后通过 PTY 输入 `/control reclaim` → controller.kind 回 `tui` → 旧 controller token 写请求返回 `INVALID_CONTROLLER_TOKEN` |
| cancel running turn | `delayed_chat.json` | 6A | fake provider delay 中调用 cancel → TUI 回 idle / interrupted 文案出现；不要断言不存在的 `cancelled` status |
| unknown interaction id | `basic_chat.json` | 6A | attach 后 respond 不存在 id → `INTERACTION_NOT_ACTIVE`，session 状态不变 |
| payload too large | 无 fixture | 6A | submit 300KiB body → `PAYLOAD_TOO_LARGE`，adapter 未收到 message |
| multi-instance conflict | `basic_chat.json` | 6A | 同一 working dir + 默认 control paths 启动第二个实例 → 非零退出，stderr/pty log 含 PID 与 `baseUrl` |

6A 是当前 v1 的稳定跨进程 suite。Phase 4 diagnostics redaction 由 `tests/diagnostics_redaction_test.rs` 覆盖；不要求普通 assistant transcript/SSE 文本被 redacted。

未来扩展（不属于当前 v1 验收）：

| 场景 | Fixture | 主要断言 |
|------|---------|---------|
| phase0 user input | `phase0_user_input.json` | plain message 进入 Phase 0 → fake tool_call `request_user_input` → automation respond → fake `submit_intent_draft` → snapshot 出现 `IntentReviewChoice` |
| intent review confirm | `phase1_plan.json` | respond Confirm → fake `submit_plan_draft` → snapshot 出现 post-plan choice |
| approval allow full flow | `approval_allow_plan.json` | Execute Plan 后 fake execution 发 shell tool_call → sandbox approval interaction → automation Allow → tool result recorded |

#### Task 6.5 — CI 接入

- File: `.github/workflows/base.yml`
- 在普通 `cargo test --all` 之后追加独立 step：
  ```yaml
  - name: Run TUI automation scenarios
    env:
      LIBRA_ENABLE_TEST_PROVIDER: "1"
    run: |
      cargo test --features test-provider \
        --test code_ui_scenarios \
        --test harness_self_test \
        --test code_codex_default_tui_test \
        -- --test-threads=1
  - name: Upload scenario artifacts on failure
    if: failure()
    uses: actions/upload-artifact@v4
    with:
      name: code-ui-scenarios
      path: target/code-ui-scenarios/**
      if-no-files-found: ignore
  ```
- harness 自己为每个子进程设置 `LIBRA_LOG_FILE`；CI 不全局设置 `RUST_LOG`，避免日志写进 PTY 干扰 TUI。
- `--test-threads=1` 是保守默认：PTY、临时端口、子进程清理和默认-path conflict scenario 都更容易稳定。若后续并行，必须先把 conflict scenario 单独 serial，并证明普通 scenario 使用 isolated control paths。

#### Task 6.6 — 文档与本地复现

- File: `docs/automation/local-tui-control.md`
  - "Quickstart Recipes" 增 "Writing your own scenario"：fixture schema、Scenario DSL、PTY 注意事项、本地命令。
  - "Troubleshooting" 增 "如何复现 CI scenario 失败"：`LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test code_ui_scenarios --features test-provider -- basic_chat --nocapture`，并说明查看 `target/code-ui-scenarios/<scenario>/pty.log` 与 `libra.log`。
- File: `docs/improvement/tui.md`：Verification Matrix、Risks、Changelog 同步增补。
- 不新增 `libra code-test` subcommand；v1 通过 cargo integration tests 运行。社区需要无 cargo fixture runner 时留 v1.1。

#### 与既有计划的边界

- **Phase 2 单测**：继续覆盖 in-process `TuiControlCommand` / adapter / App ack；Phase 6 只覆盖真实 CLI/TUI/HTTP 跨进程链路。
- **Phase 4 redaction golden**：锁定 diagnostics/control-audit 结构；Phase 6 只验证跨进程暴露面没有泄露，不把 assistant transcript redaction 作为 v1 contract。
- **Phase 5 文档**：Task 5.3 时序表新增 Phase 6 行：用户文档写 scenario/复现段，开发者文档写 `tests/harness/` 与 fake provider module doc。

#### Phase 6 验收

```bash
cargo +nightly fmt --all
cargo clippy --all-targets --all-features -- -D warnings
# fake provider 单测
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider fake
# 基础 PTY harness 自检
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test harness_self_test --features test-provider
# Phase 6A 稳定 scenario
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test code_ui_scenarios --features test-provider -- --test-threads=1
# 单独跑某个 scenario 调试
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test code_ui_scenarios --features test-provider -- basic_chat --nocapture
```

---

## Verification Matrix

| 测试 | 命令 | 覆盖 |
|------|------|------|
| 格式 | `cargo +nightly fmt --all --check` | 风格 |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | warning-free |
| 全量 | `cargo test --all` | 所有测试 |
| Phase 1 单测 | `cargo test code_control_files` | token 文件 lifecycle |
| Phase 2 集成 | `cargo test code_ui_automation` | attach/submit/respond/reclaim/cancel |
| Codex 默认 TUI | `cargo test --test code_codex_default_tui_test` | Phase 2 Task 2.7 的源码级 routing guard：`agent_codex::execute` 不被 `libra code` 调用、Codex 分支走 `run_tui_with_managed_code_runtime`、TUI/command 路径无 `std::io::stdin` |
| Phase 3 集成 | `cargo test code_control_stdio` | NDJSON shim（如执行） |
| Phase 4 redactor | `cargo test --test ai_hardening_contract_test secret_redactor_removes_common_token_shapes`；`cargo test --test diagnostics_redaction_test` | `SecretRedactor::default_runtime()` 覆盖 OpenAI key / bearer / password / generic token / control-token 五类 marker；diagnostics controller owner / active interaction 字段脱敏 |
| Phase 5 文档构建 | `cargo doc --no-deps --all-features` | intra-doc link、模块文档无 warning |
| Phase 5 一致性 | `rg "/api/code/" docs/ src/internal/ai/web/`、`rg "X-Libra-Control-Token" docs/ src/`、`cargo run -- code --help \| rg -- '--control'` | HTTP 路径、header、CLI flag 双向一致 |
| Phase 6 fake provider | `LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider fake` | provider-native fixture 解析、stream/text/tool_call/error 回放 |
| Phase 6 PTY harness 自检 | `LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --test harness_self_test --features test-provider` | PTY spawn、control.json 发现、sync shutdown、临时文件清理 |
| Phase 6 scenario | `LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider --test code_ui_scenarios --test harness_self_test --test code_codex_default_tui_test -- --test-threads=1` | Phase 6A submit/respond/cancel/reclaim/oversize/多实例 + harness 自检 + Codex routing guard；Phase 4 后补 transcript-level redaction e2e |
| Phase 6 CI | `.github/workflows/base.yml` 的 "Run TUI automation scenarios" step | CI 自动跑上一行命令；失败时 upload `target/code-ui-scenarios/**` 工件 |
| 回归 | `cargo test command_test e2e_mcp_flow` | 既有命令、MCP stdio |

## Risks And Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| 自动化控制被误认为远程 API | 用户暴露本地控制面 | 名称 Local TUI Automation Control；默认 observe；automation write 强制 loopback + token。 |
| token 泄露到 control.json / diagnostics | 本地权限边界失效 | control.json schema 白名单字段；diagnostics golden test。 |
| stdio 语义和 MCP stdio 混淆 | client 配置错误、协议不兼容 | 新命令 `code-control --stdio`；文档显式区分。 |
| v1 event contract 过度承诺 | 后续 Code UI Phase 3 迁移被绑定 | v1 复用 snapshot SSE，typed delta 显式 deferred。 |
| automation lease 抢占人工输入 | 用户失去本地控制感 | TUI 保留 Ctrl-C / /quit / reclaim；lease 短 TTL；snapshot 显示 owner。 |
| 写通道绕过 Runtime | session/projection 不一致 | 所有 automation write 必经 `TuiControlCommand` → App helper；adapter 不直接 mutate snapshot、不共享 turn id。 |
| **Adapter 抽象错位** | 普通 provider TUI 模式下 HTTP write 找不到出口 | 显式新增 `TuiCodeUiAdapter`，TUI 模式 automation 均走 App；Phase 2 验收强制 ollama provider。 |
| Codex 默认 TUI 合并后出现双输入循环 | 用户看到重复输出、approval 被错误消费 | `--provider codex` 只允许默认 App 读输入；Codex runtime 禁止 stdin/stdout 用户交互；源码级 routing guard 覆盖默认 TUI 路径不会进入 legacy stdin loop。 |
| TUI 继续使用不可让渡 fixed controller | automation attach 永远 `CONTROLLER_CONFLICT` | Phase 2 把 TUI owner 从 fixed state 拆成可恢复 `local_tui_owner`。 |
| symlink 攻击 token 文件 | 任意文件被覆盖/读取 | `O_NOFOLLOW` + 权限 lstat 校验；已有 0600 regular file 只覆盖写入新 token。 |
| 同仓库多实例覆盖 token/info | 前一个实例的 lease 被静默劫持，原自动化客户端突然 401 或被路由到错进程 | 默认路径下 `acquire_control_lock` advisory lock + `control.json.pid` liveness 检查；冲突时 fail-fast 报告既有 PID/`baseUrl`；并发场景必须显式给出互不重合的 `--control-token-file` + `--control-info-file`（lock 路径自动跟随 info 路径分离）。 |
| approval memo 跨 controller 泄露 | automation 触发 once-allow 被人工继承（反向亦然） | scope key 加 controller kind 前缀（Phase 4） |
| 文档滞后导致契约漂移 | 用户用错 flag、客户端 hardcode 错路径或错 header、错误码语义不可发现 | Phase 5 把文档作为强约束交付：随 Phase 同步上线，`cargo doc` 无 warning，HTTP/header/flag/错误码双向 grep 一致；不接受"代码合入但文档缺失"的 PR。 |
| e2e harness flake（PTY、端口、子进程时序、临时文件竞争） | Phase 6 scenario 间歇性失败，CI 信号被噪音稀释 | scenario 串行（`--test-threads=1`）；每 scenario 独立 `TempDir` + `--port 0`；`spawn` 30s 超时 + 100ms 轮询；失败 dump snapshot/`control.json`/`pty.log`/`libra.log` 到 artifact；harness 自检测试覆盖 PTY spawn 与 sync shutdown。 |
| fake provider 与真实 provider 行为漂移 | scenario 全绿但真实 provider 上线后回归 | fake provider 只输出 provider-native `CompletionResponse` text/tool_call/error，不直接伪造 App interaction；Phase 2 端到端冒烟仍用 ollama provider；Phase 6 不替代手工冒烟。 |
| test-provider 被误用于真实会话 | 隐藏测试 provider 在 `--all-features` binary 中可被解析 | `Fake` provider 和 `--fake-fixture` clap hidden；`validate_mode_args` 强制 `LIBRA_ENABLE_TEST_PROVIDER=1` + fixture；默认 features 不包含该 provider。 |

## Open Questions

执行过程中如答案与下文假设冲突，先在 PR 描述说明再调整代码：

1. **Lease slot 共享 vs 分离**：v1 采用单 slot（同一时刻一个 writer，简化 takeover）。如需 browser-观察 + automation-写并存，留 v2。
2. **`/control reclaim` 二次确认**：v1 不需要（同机用户已物理控制键盘）。
3. **control token rotate**：v1 不提供在线 rotate；每次进程启动生成新 token 并覆盖旧文件，退出清理 best-effort。
4. **Windows 权限**：v1 unix-only enforce 0600；windows 上仅 best-effort（NTFS ACL 复杂，留 issue）。
5. **observe SSE 节流**：v1 不加；依赖 broadcast channel 默认 backpressure。
6. **Codex legacy `agent_codex::execute` 是否删除**：v1 至少从 `libra code --provider codex` 路径移除；若仍保留给内部/legacy 用途，必须标记 deprecated 并有测试证明默认 TUI 路径不引用它。
7. **Browser 写通道是否也升级 control token**：v1 不升级；保持旧 browser controller 兼容性，后续若要加强浏览器鉴权应进入 `code.md`。
8. **多实例策略**：v1 采用 Option A——默认路径下单仓库单实例，advisory lock + PID liveness 检查 fail-fast。Option B（per-PID 文件名）作为客户端发现复杂度的反例不进 v1；用户要并发跑必须显式提供互不重合的 `--control-token-file` + `--control-info-file`，由调用方自己决定如何让自动化客户端找到正确端口。

## Changelog

| 日期 | 作者 | 变更摘要 |
|------|------|----------|
| 2026-04-28 | Codex | 新建 Local TUI Automation Control 独立改进计划；收口命名、loopback-only write、token 文件权限、stdio shim、snapshot SSE v1、controller takeover、diagnostics redaction / audit 边界。 |
| 2026-04-28 | Claude Code | 重写为可执行版本：补充"当前实现现状"映射表、架构关键点（adapter 抽象错位）、两层鉴权语义、redaction 规则表、Audit Schema、每 Phase 拆分到文件级 Task + 验收命令、Open Questions、symlink/approval memo 风险条目。 |
| 2026-04-28 | Codex | 本轮可行性修订：新增审查结论；将自动化控制从 `AppEvent` 改为 `TuiControlCommand`；明确 App 独占 turn id、browser 兼容、stdio write 拒绝、token 每进程重建、TUI controller 可让渡、Phase 1 冒烟命令修正。 |
| 2026-04-28 | Codex | 加入 Codex 默认 TUI 合并任务：`--provider codex` 使用默认 Libra TUI，Codex app-server 仅作执行后端，移除交互式路径中的 Codex 单独 TUI / stdin approval loop。 |
| 2026-04-28 | Claude Code | 修复多实例并发冲突：默认路径下加 `.libra/code/control.lock` advisory file lock + `control.json.pid` liveness 检查 fail-fast；扩 Task 1.3 helpers / 测试、Task 1.5 启动流程、Phase 1 冒烟、Risks、Open Questions；显式自定义 token/info 路径作为 Option B 并发逃生口。 |
| 2026-04-28 | Claude Code | 新增 Phase 5 — Documentation Deliverables：用户文档清单（`code.md` 增补、`code-control.md` 新建、`local-tui-control.md` 主指南、`code.md` cross-link）、开发者文档清单（模块/方法 docstring）、Phase ↔ 文档时序对齐表、文档与代码一致性 lint；同步更新 Verification Matrix 与 Risks。 |
| 2026-04-29 | Claude Code | 新增并收敛 Phase 6：跨进程 e2e 改为 PTY 启动真实 TUI；fake provider 只返回 provider-native text/tool_call/error，不直接伪造 approval/user input；补 `LIBRA_ENABLE_TEST_PROVIDER` 安全门、显式 shutdown、`pty.log`/`libra.log` artifact、6A/6B scenario 分层；同步更新 Verification Matrix 与 Risks。 |
| 2026-04-30 | Codex | 完成 Phase 6A：新增 hidden fake provider、PTY `CodeSession` harness、harness self-test、direct chat/reclaim/cancel/oversize scenarios；修复 `--port 0` 写回真实端口、raw PTY Enter 写入、fake fixture `delayMs` 解析与超限 body 稳定 413。 |
| 2026-04-30 | Claude Code | 闭环 doc↔code gap：(1) `.github/workflows/base.yml` 新增 "Run TUI automation scenarios" step + 失败工件上传，CI 实际跑 `--features test-provider` scenario+harness 自检+Codex routing guard；(2) 新增 `tests/code_codex_default_tui_test.rs`（4 个源码级 routing guard：`agent_codex::execute` 不被 `libra code` 调用、Codex 分支走 `run_tui_with_managed_code_runtime`、`#[deprecated]` marker 保留、TUI/command 路径无 `std::io::stdin`），无需启动真实 Codex backend；(3) Phase 6 实现状态段落补完整 scenario 清单与 CI 现状；(4) Verification Matrix 修正 Codex / Phase 4 redactor 行指向真实测试名，新增 Phase 6 CI 行；(5) 不改动 Phase 1/2/4 已落地代码。 |
| 2026-04-30 | Codex | 补齐剩余 TUI doc↔code gap：`CodeUiDiagnostics` 字符串字段接入 `SecretRedactor` 并新增 `tests/diagnostics_redaction_test.rs`；新增轻量 `tests/harness/scenario.rs` DSL 并接入 basic chat scenario；CI scenario step 移除全局 `RUST_LOG`；文档同步改为当前 v1 已落地范围，6B scenario 明确为未来扩展。 |
