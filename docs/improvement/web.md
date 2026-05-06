# Libra Code Web UI 接入计划

## Context

当前 `libra code` 的 Web UI 已经完成静态页面设计，但前端仍主要消费 `web/src/lib/mock/*`，聊天输入、终端、workflow、summary、diff 和 thread 列表都是本地 stub 状态。Rust 侧已经有一部分可复用的 Code UI 协议与运行时（含已落地的细节）：

- [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 提供静态资源（`WebAssets` 嵌入 `web/out/`）服务、`/api/repo`、`/api/health`，以及 `/api/code/*` HTTP/SSE 路由：`/session`、`/events`、`/diagnostics`、`/controller/attach|detach`、`/messages`、`/interactions/{id}`、`/control/cancel`。所有 `/api/*` 入口都经 `ensure_loopback_api_request` 强制 loopback；写控制再叠加 256 KiB body limit（`enforce_code_write_body_limit`）和 `AuditSink` 审计。`code_router()` 内的注释（[mod.rs:130-138](../../src/internal/ai/web/mod.rs:130)）已经把每条路由的鉴权矩阵写清。
- [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 已定义 `CodeUiSessionSnapshot`（含 `transcript / plans / tasks / toolCalls / patchsets / interactions`）、`CodeUiCapabilities`（8 个布尔位：`messageInput`、`streamingText`、`planUpdates`、`toolCalls`、`patchsets`、`interactiveApprovals`、`structuredQuestions`、`providerSessionResume`）、`CodeUiControllerState`（含 `loopbackOnly` 标志）、`CodeUiInitialController`（`Unclaimed` / `Fixed` / `LocalTui` 三种初始 controller 模式）、`ControllerLease`（默认 120s，`X-Code-Controller-Token`），以及 `CodeUiInteractionKind` 五种 kind：`approval`、`sandboxApproval`、`requestUserInput`、`intentReviewChoice`、`postPlanChoice`。所有 JSON 字段都是 `camelCase`（`#[serde(rename_all = "camelCase")]`），事件流是 `broadcast::channel(256)`——前端必须能处理 lag/重连。
- [src/internal/tui/code_ui_adapter.rs](../../src/internal/tui/code_ui_adapter.rs) 把 HTTP 写请求桥接到 TUI 主循环的 `TuiControlCommand`，但目前在 TUI 模式下构建 runtime 时 `browser_write_enabled = false`（[code.rs:2104-2113](../../src/command/code.rs:2104)），即只有 `Automation` 控制器（带 `X-Libra-Control-Token`）能写。浏览器还没有 lease 入口。
- [src/internal/ai/codex/mod.rs](../../src/internal/ai/codex/mod.rs) 已为 `--web-only --provider codex` 提供可写 managed Code UI runtime（`CodexCodeUiAdapter` + `start_managed_codex_server`，`browser_write_enabled = true`，`Unclaimed` controller），并通过 `ensure_loopback_browser_control_host`([code.rs:1410](../../src/command/code.rs:1410)) 拒绝非 loopback `--host`。
- [src/command/code.rs](../../src/command/code.rs) 中普通 provider 的 `--web-only` 仍走 `build_placeholder_web_code_ui_runtime()`（[code.rs:1427](../../src/command/code.rs:1427)），只显示只读 placeholder；TUI 模式下 Web server 可观察 session，但浏览器页面没有接入这些接口。
- 既有测试入口：[tests/ai_code_ui_projection_test.rs](../../tests/ai_code_ui_projection_test.rs)（`snapshot_from_thread_bundle` golden）、[tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs)（含写控制 + lease 场景，gated by `--features test-provider`）、[tests/harness/code_session.rs](../../tests/harness/code_session.rs)（PTY 化 + HTTP 控制 harness）、[tests/code_codex_default_tui_test.rs](../../tests/code_codex_default_tui_test.rs)。

本文目标是把现有页面从"可看的 mock shell"推进到"可驱动真实 `libra code` session 的浏览器 UI"。本文是 [agent.md](agent.md) 的 Code UI Source of Truth / Local TUI Automation Control 的前端落地补充，不替代 Agent runtime 主计划。

---

## 目标与非目标

**目标：**
- 前端以 Rust `CodeUiSessionSnapshot` 为唯一运行时数据源，首屏加载 `GET /api/code/session`，后续通过 `GET /api/code/events` SSE 增量刷新。
- 聊天、Intent/Plan review、post-plan choice、approval、request-user-input 通过现有 `/api/code/messages` 与 `/api/code/interactions/{id}` 写回真实 session。
- workflow / summary / diff / terminal 不再依赖 mock fixture，而是从 transcript、plans、tasks、tool_calls、patchsets、interactions 和 diagnostics 派生。
- 普通 TUI session、`--web-only --provider codex`、resume 后的 session snapshot 都有回归测试。
- 保持静态导出：`web/next.config.ts` 的 `output: "export"` 不变，Rust 继续嵌入 `web/out/`。

**非目标：**
- 不在本计划内开放远程公网写控制。Code UI v1 仍以 loopback 为安全边界。
- 不把 browser UI 变成独立多用户协作产品；同一 session 仍只有一个 active controller lease。
- 不重新设计 `CodeUiSessionSnapshot` 为 typed delta 协议。SSE v1 继续发送完整 snapshot，typed delta 后续单独规划。
- 不把 web terminal 做成任意 shell。终端面板先只展示 agent sandbox/tool/event 输出，命令执行继续通过 agent approval/sandbox 路径。

---

## 当前差距

| 区域 | 当前状态 | 接入缺口 |
|------|----------|----------|
| 前端数据源 | `Chat`、`Sidebar`、`Workflow`、`SummaryView`、`ReviewView`、`Terminal`、`GitTimeline`、`PhaseStrip`、`Cards`、`Message`、`ThreadItem`、`workflow/types.ts` 共 13 处 import `@/lib/mock`（`grep "@/lib/mock"` 全量列表） | 新增 live client/store，并把组件改成 props-driven |
| 硬编码文案 | [chat.tsx:122-128](../../web/src/components/workspace/chat/chat.tsx:122) 的 thread 标题/branch/Phase chip、[workflow.tsx:64-68,87](../../web/src/components/workspace/workflow/workflow.tsx:64) 的 token 计数与 footer "5 events · 2 PatchSets"、[sidebar.tsx:152-155](../../web/src/components/workspace/sidebar/sidebar.tsx:152) 的 "web3infra / libra · main · clean" 都是字面量 | 需统一从 `snapshot` + `/api/repo` 派生；空态显式渲染 |
| 会话读取 | Rust 已有 `GET /api/code/session` 与 SSE `/api/code/events`，事件 `broadcast::channel(256)` | 前端没有 fetch、SSE reconnect、`Lagged`/断线后的全量 re-fetch、snapshot 校验、错误态 |
| 写控制 | Rust 已有 controller attach/detach、message submit、interaction respond、turn cancel；写路径有 256 KiB body limit + 审计 | 前端没有 controller lease 管理，也没有按 8 个 `capabilities` 布尔位禁用不可写动作；没有 lease 续期/检测过期策略 |
| TUI 模式浏览器写 | `browser_write_enabled=false`（[code.rs:2104-2113](../../src/command/code.rs:2104)），TUI 是 `LocalTui` 初始 controller | 需要在 loopback 时打开 browser write（要么自动启用，要么新增 `--browser-control` flag，详见 Open Questions #1） |
| 普通 provider web-only | 非 Codex `--web-only` 返回 placeholder read-only snapshot（[code.rs:1427](../../src/command/code.rs:1427)） | 需要抽出 headless generic code runtime，不能继续依赖 TUI event loop |
| Workflow 映射 | TUI 已上报 plan、task、tool call、patchset、interaction 的 snapshot；`tasks` 字段当前前端没用 | 前端仍按 mock 的 Phase 0-4 卡片结构展示，缺少真实状态映射、`tasks` 渲染和空态 |
| Thread 列表 | mock `THREADS` | 当前 Code UI API 没有 thread list endpoint；先显示当前 thread，历史 thread list 后续接 projection/session store |
| 测试 | 已有：[ai_code_ui_projection_test.rs](../../tests/ai_code_ui_projection_test.rs)、[code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs)、[tests/harness/code_session.rs](../../tests/harness/code_session.rs) | 前端缺 API client 单测、组件空态测试、真实 `libra code` 浏览器 smoke；headless web-only 还没有 e2e 覆盖 |
| 静态产物 | `web/next.config.ts` 已 `output: "export"`、`images.unoptimized`、`trailingSlash: true`；`web/out/` 通过 `rust-embed` 编入二进制（[web_assets.rs](../../src/command/web_assets.rs)） | CI 需要在 `cargo build` 前强制 `pnpm --dir web build`，并把 `web/out/` 变更纳入 review |

---

## 架构约定

### Source of Truth

Rust `CodeUiSessionSnapshot` 是浏览器运行时唯一事实源。前端可以定义自己的 view model，但不能让 `web/src/lib/mock/types.ts` 继续充当 wire contract。

建议新增：

- `web/src/lib/code-ui/types.ts`：手写或生成的 wire types，字段命名严格匹配 Rust `camelCase` JSON。
- `web/src/lib/code-ui/client.ts`：`getSession()`、`subscribeEvents()`、`attachController()`、`submitMessage()`、`respondInteraction()`、`detachController()`。
- `web/src/lib/code-ui/store.tsx`：React context + reducer，负责 snapshot、connection state、controller token、last error。
- `web/src/lib/code-ui/view-model.ts`：把 snapshot 派生为 `ChatMessage[]`、workflow cards、diff files、terminal rows、header meta。

### API 边界

所有 `/api/*` 路由在 Rust 侧都强制 loopback（`ensure_loopback_api_request`，由 `ConnectInfo` 校验），即使 `--host 0.0.0.0` 也只接受 127.0.0.1 / ::1 的请求；写控制再叠加 256 KiB body limit + 审计。前端只需要保证不发非 loopback 请求即可，不需要再做 host 检查。

| 操作 | Endpoint | 前端行为 |
|------|----------|----------|
| 仓库元信息 | `GET /api/repo` | 返回 `{ id, name, description }`，用于 sidebar header / chat header；不需要 SSE |
| 仓库 git 状态 | `GET /api/repo/status` | **新增**：服务端复用 `build_status_json()`（[status.rs:662](../../src/command/status.rs:662)）输出与 `libra status --json` 一致的 shape；前端在首屏 + 收到任意 `session_updated`/`status_changed` SSE 时 debounce(2–5s) fetch；用于 Summary.branch、Sidebar repo 行的 dirty/ahead/behind 标记 |
| Thread 列表 | `GET /api/code/threads?limit&offset` | **新增**：服务端调 [`ThreadProjection::list_active(db, limit, offset)`](../../src/internal/ai/projection/thread.rs:425)，返回 `{ items: [{ id, title, archived, currentIntentId, createdAt, updatedAt }], nextOffset }`；前端用于 Sidebar 历史 thread 列表与 search；当前 active thread 高亮由 `snapshot.threadId` 比对得到 |
| 初始加载 | `GET /api/code/session` | 成功后渲染真实 snapshot；404 `CODE_UI_UNAVAILABLE` 显示无活动 session 空态 |
| 实时更新 | `GET /api/code/events` | SSE 断线/`Lagged` 后退避重连，重连成功立即 `GET /api/code/session` 拉一次完整快照；事件类型至少包含 `session_updated` / `status_changed` / `controller_changed` |
| 取得写 lease | `POST /api/code/controller/attach` | browser kind 默认只发 `{ clientId, kind: "browser" }`；保存返回的 `controllerToken` 到内存，**不**落 localStorage；lease 过期时间 `leaseExpiresAt` 用于提前触发续期或在下一次写动作前重新 attach |
| 发送消息 | `POST /api/code/messages` | header `X-Code-Controller-Token`；body `{ "text": draft }`；body 上限 256 KiB，超限会被中间件直接拒绝 `PAYLOAD_TOO_LARGE` |
| 响应交互 | `POST /api/code/interactions/{id}` | 根据 `CodeUiInteractionKind` 构造 `CodeUiInteractionResponse`，可选字段：`approved` / `applyToFuture` / `selectedOption` / `note` / `answers`(map) |
| 释放 lease | `POST /api/code/controller/detach` | 页面卸载或用户释放控制时 best-effort 调用；失败不阻塞关闭 |
| 诊断 | `GET /api/code/diagnostics` | 只用于状态/错误面板；服务端已用 `SecretRedactor` 脱敏 |
| 取消当前 turn | `POST /api/code/control/cancel` | **Phase 2 改造**：放开 `Browser` controller 也能调用，与 TUI `Esc` 按键操作对齐；`Browser` controller 仅校验 `X-Code-Controller-Token`（lease），`Automation` 仍需要 `X-Libra-Control-Token` + lease；handler 内移除 `AUTOMATION_CONTROLLER_REQUIRED` 早返回，按 `lease.kind` 分支鉴权 |

> 头部命名上要严格区分两类 token：
> - `X-Code-Controller-Token`：lease token，attach 后下发，用于 browser/automation 写控制；
> - `X-Libra-Control-Token`：进程级 control token，仅在 `--control write` 自动化场景下由本地工具持有，浏览器永远不要使用它。

### Controller 策略

- 默认首屏只观察，不自动抢占 controller。
- 当用户第一次点击 Send、approval、Confirm / Execute / Modify / Cancel 时，再 attach browser controller。
- attach 失败 `CONTROLLER_CONFLICT` 时，UI 显示当前 `controller.kind` / `controller.ownerLabel` / `controller.reason` 与只读状态，不重试抢占。`LocalTui` initial controller 下浏览器永远拿不到 lease（除非未来 TUI 主动释放）。
- controller token 只保存在内存。刷新页面需要重新 attach。
- TUI 模式要启用 browser 写入时，Rust 侧需要把 `start_codex_code_ui_runtime` / `build_tui_code_ui_runtime` 当前硬编码的 `browser_write_enabled = false` 改为 loopback 时启用（或受 `--browser-control` flag 控制）。`ensure_loopback_browser_control_host` 已经在 `--web-only --provider codex` 路径就绪，TUI 路径需要复用同样的校验。

---

## 分阶段实施

### Phase 0：冻结 Web UI v1 契约

**目标：** 先把前端要消费的 JSON 契约固定下来，避免边做 UI 边改字段。

**任务：**
- 在 `web/src/lib/code-ui/types.ts` 定义 `CodeUiSessionSnapshot`、`CodeUiEventEnvelope`、`CodeUiInteractionRequest`、`CodeUiInteractionResponse`、`CodeUiControllerState`、`CodeUiCapabilities`、`CodeUiInteractionKind`、`CodeUiTranscriptEntryKind`、`CodeUiPlanSnapshot`、`CodeUiTaskSnapshot`、`CodeUiToolCallSnapshot`、`CodeUiPatchsetSnapshot` 等 wire types。命名严格匹配 Rust 的 `camelCase`（`#[serde(rename_all = "camelCase")]`）。
- 在 `docs/commands/code.md` 或本计划后续更新中补一张 Code UI snapshot 字段表（只记录对前端稳定的字段），并显式列出 8 个 capability flag、5 个 interaction kind、3 种 initial controller。
- Rust 侧：在 [tests/ai_code_ui_projection_test.rs](../../tests/ai_code_ui_projection_test.rs) 已有 `snapshot_from_thread_bundle` golden 的基础上，补齐针对 `CodeUiSessionSnapshot` JSON 形态的 serde round-trip 单测（覆盖 `threadId`、`capabilities`、`controller`（含 `loopbackOnly`）、`transcript[].kind`、`plans[].steps[].status`、`tasks`、`toolCalls`、`patchsets[].changes[].diff`、`interactions[].kind/options`、`updatedAt` 时区）。
- 校验 `CodeUiEventEnvelope.type` 字面量集合（至少 `session_updated`、`status_changed`、`controller_changed`），将允许的事件类型常量化在前后端共享。

**验收：**
- `cargo test --features test-provider code_ui`
- `pnpm --dir web lint && pnpm --dir web build`（验证 `output: "export"` 静态产物可以编译）。
- 前端 type 名称不再从 `@/lib/mock/types` 复用 wire contract。

### Phase 1：只读 live UI

**目标：** 不引入写控制，先让页面展示真实 session。

**任务：**
- 新增 `CodeUiProvider`（`web/src/lib/code-ui/store.tsx`），首屏并行 `GET /api/repo` + `GET /api/repo/status` + `GET /api/code/session`，再连接 `/api/code/events`。
- Rust 侧新增 `GET /api/repo/status` handler：复用 [collect_status_data + build_status_json](../../src/command/status.rs:662)，handler 自身 ≤30 行；同样走 `ensure_loopback_api_request` 校验；返回的 JSON 与 `libra status --json` 保持字节级一致。
- SSE 处理：监听 `session_updated`/`status_changed`/`controller_changed`；遇 `EventSource` 断线、`Lagged`（`broadcast::channel(256)` 落后）或 5xx 时，按指数退避重连并在重连成功后立即重新拉一次 `/api/code/session`，避免 partial state。
- 把 `Chat` 改为渲染 `snapshot.transcript`；支持 `userMessage`、`assistantMessage`、`toolCall`、`planSummary`、`diff`、`infoNote` 六种 `kind`，并尊重 `streaming` 标志。
- 替换硬编码文案：`chat.tsx:122-128` 的 thread 标题/branch/Phase chip → 从 snapshot.transcript / repo / status 派生；`workflow.tsx:64-68,87` 的 token 计数与 footer 计数 → 从 snapshot 衍生（暂时无 token 数据时显示空态而非占位数字）；`sidebar.tsx:152-155` 的 repo 行 → 用 `/api/repo` 返回值。
- `Workflow` 先渲染 `snapshot.plans`、`snapshot.tasks`、`snapshot.toolCalls`、`snapshot.patchsets` 的真实摘要；没有数据时显示紧凑空态。注意 `tasks` 字段当前 mock 没有对应展示，需要新增轻量行。
- `ReviewView` 从 `snapshot.patchsets[].changes[].diff` 解析 unified diff；解析失败时降级展示原始 diff 文本与解析错误。
- `Terminal` 先改为只读 event log：从 `snapshot.toolCalls` + `snapshot.transcript`（`infoNote` / `toolCall`） + `snapshot.status` 派生 meta/info/run/pass/fail 行，不再依赖 `TERMINAL_LINES`。

**验收：**
- 启动 `libra code --control observe`（或默认 TUI 模式）后打开浏览器，能看到真实 user/assistant/tool/plan 更新。
- 刷新页面后从 `GET /api/code/session` 恢复当前状态。
- 模拟 SSE 断开（kill connection、退避场景）后 UI 显示 reconnecting，重连成功能恢复到最新 snapshot。
- 决定不保留 mock fixture：Phase 1 PR 中**直接删除** `web/src/lib/mock/` 目录（13 处 import 全部迁移完毕后），同时清理 `web/src/components/workspace/workflow/types.ts` 中对 `@/lib/mock` 的 type 引用。`pnpm --dir web build` 中找到任何 `@/lib/mock` 残留视为回归。
- 关闭 Code UI runtime（无 session）时浏览器看到明确的 "无活动 session" 空态而非崩溃。

### Phase 2：接入浏览器写控制

**目标：** 浏览器可以驱动已有 runtime：发送消息、回答 review/approval/user input。

**任务：**
- 前端实现 lazy attach：首次写动作调用 `POST /api/code/controller/attach`（kind: `browser`），保存 `controllerToken` + `leaseExpiresAt`；之后所有写请求带 `X-Code-Controller-Token`。
- `Composer` 改为调用 `submitMessage()`，发送期间根据 `snapshot.status === "thinking" | "executingTool" | "awaitingInteraction"` 与 `capabilities.messageInput` 禁用重复提交；body 长度限制 256 KiB（提前在客户端校验，避免 `PAYLOAD_TOO_LARGE`）。
- Cancel parity：在 chat header / workflow footer 加 "Cancel turn" 按钮，调 `POST /api/code/control/cancel`，与 TUI 的 `Esc` 行为对齐；按钮 enable 条件 `snapshot.status ∈ { thinking, executingTool, awaitingInteraction }` 且 `controller.canWrite`。Rust 侧 [code_cancel_handler](../../src/internal/ai/web/mod.rs:430) 移除"automation only"早返回，按 `lease.kind` 分支：browser 只校 lease token，automation 继续要求 `X-Libra-Control-Token`；audit `turn.cancel` 事件按 controller kind 区分。
- 新增 `InteractionPanel`（或复用 workflow footer），按 `CodeUiInteractionKind` 渲染：
  - `intentReviewChoice`：Confirm / Modify / Cancel（来自 `options[]`）
  - `postPlanChoice`：Execute Plan / Modify Plan / Cancel，并显示 network policy 等 `metadata`
  - `approval` / `sandboxApproval`：选 `selectedOption` + 可选 `applyToFuture`（`acceptAll`/`declineAll`/`no`）
  - `requestUserInput`：根据 `metadata` 渲染单选/多选/自由文本，回填 `answers: { [questionId]: string[] }`
- Rust：新增 `--browser-control <off|loopback>` flag（与 `--control` 平行，独立 enum），统一 TUI 与 web-only 两条路径：
  - TUI 模式默认 `off`，与现状一致；显式传 `loopback` 才把 [code.rs:2097-2113](../../src/command/code.rs:2097) / [code.rs:1893-1904](../../src/command/code.rs:1893) 的 `browser_write_enabled` 翻成 `true`，并复用 `ensure_loopback_browser_control_host` 校验（同时保持 `LocalTui` 为初始 controller —— TUI 用户必须主动让出 lease 浏览器才能写）。
  - `--web-only --provider codex` 默认 `loopback`，保留"开箱即用"语义；显式 `off` 时 attach 直接拒绝。
  - `--web-only` + 非 Codex provider 的 placeholder runtime 永远 `off`（capability 都关）。
  - 启动 banner 与 control info 文件加 `browser_control: off | loopback` 字段；attach 在 `off` 状态下返回新错误码 `BROWSER_CONTROL_DISABLED`，message 直接给出"重启时加 `--browser-control loopback` 即可"。
  - clap 校验：`--browser-control` 与 `--stdio` 互斥；`loopback` 与非 loopback `--host` 互斥（早失败优于运行时拒绝）。
- 页面卸载（`beforeunload` / `visibilitychange=hidden`）时 best-effort `POST /controller/detach`；`leaseExpiresAt` 接近时主动续 attach；写请求遇 `MISSING_CONTROLLER_TOKEN` / `INVALID_CONTROLLER_TOKEN` 时清 token 并自动重新 attach 一次。

**验收：**
- `--web-only --provider codex`：浏览器发送消息后 Codex runtime 产生真实响应；audit log（`local-tui-control:browser:<clientId>`）出现 `message.submit accepted` 行。
- 普通 provider TUI 模式（loopback host + browser write 已开关）：浏览器提交 `/chat ...` 能进入同一 TUI session，TUI 让出 lease 后浏览器可顺利 attach。
- 浏览器能完成 IntentSpec confirm、Plan execute/modify/cancel、shell approval、request-user-input 至少各一个场景。
- `CONTROLLER_CONFLICT`、`MISSING_CONTROLLER_TOKEN`、`INVALID_CONTROLLER_TOKEN`、`PAYLOAD_TOO_LARGE`、`CODE_UI_UNAVAILABLE`、`LOOPBACK_REQUIRED`、`BROWSER_CONTROL_DISABLED` 都有可读 UI 错误。
- 非 loopback `--host` 下 attach 直接被拒，UI 给出 "本机/loopback 才能写控制" 的解释，不重试。
- `--browser-control` 测试矩阵：`{off, loopback} × {loopback host, 0.0.0.0 host} × {TUI mode, web-only Codex, web-only generic}` 至少覆盖到核心 6 组合。

### Phase 3：普通 provider 的 headless web-only runtime

**目标：** `libra code --web-only --provider <non-codex>` 不再是 placeholder，而是能在无 TUI 环境中运行同一 agent workflow。

**任务：**
- 从 `App` 中抽出不依赖 ratatui 的 session driver：turn 状态、plain message → IntentSpec/Plan workflow、tool loop、approval/request-user-input、session persistence、Code UI snapshot update。
- 新建 `HeadlessCodeRuntime`，实现 `CodeUiProviderAdapter`，复用 `ToolRegistry`、`ToolLoopConfig`、`SessionStore`、`UsageRecorder`、`ApprovalStore`。
- `execute_web_only()` 对非 Codex provider 构建真实 completion model 和 headless runtime，删除或降级 `build_placeholder_web_code_ui_runtime()`。
- headless runtime 中所有 mutating tool 仍走 sandbox/approval；approval 通过 `CodeUiInteractionRequest` 交给浏览器。
- 保持 MCP server 与 web runtime 共享同一个 `LibraMcpServer` / history object store。

**验收：**
- `libra code --web-only --provider ollama --model <model>` 可以在浏览器完成 Phase 0 → Phase 1 → Execute Plan 的最小流程。
- 无 terminal 时不调用 `tui_init()`，不会进入 alternate screen。
- `--resume <thread_id>` 在 web-only headless 模式恢复 transcript、plan、pending interaction。
- `--host 0.0.0.0` 下只允许 observe，写控制必须有明确 auth 方案后才能开放；本阶段不绕过 loopback 限制。

### Phase 4：补齐页面功能映射

**目标：** 让页面所有主要区域都对应真实功能，而不是仅显示低保真 snapshot。

**任务：**
- Sidebar：
  - 显示当前 thread 和 repo info（`/api/repo` + `/api/repo/status`）。
  - 历史 thread list 走新端点 `GET /api/code/threads?limit&offset`：默认 `limit=50, offset=0`，按 `updatedAt desc`；search 框前端先做 client-side substring filter，超过 `limit` 后再考虑加 `q=` 参数；当前 active thread 通过 `snapshot.threadId === thread.id` 高亮。
  - "New thread" 按钮 v1 暂不实现（无对应 backend 入口），UI 上保留按钮但点击后给 toast "本机 TUI 中创建新 thread"，并附 CLI 提示。
- Workflow：
  - 把 `plans[].steps[].status`、`tasks[].status`、`toolCalls[].status`、`patchsets[].status` 映射到 Phase strip。
  - Plan card 支持 `pending/running/completed/failed` 状态；点击 step 展示 `summary` / `details` / `metadata`。
  - `tasks[]` 单列展示 scheduler 状态（mock 当前完全没有这个区域，需要新增）。
- Summary：
  - 从 snapshot 派生 progress、artifacts、todo。
  - branch state 改为消费 `GET /api/repo/status`，字段映射如下（与 `libra status --json` 同源，避免重新发明 schema）：
    - `branch.name` ← `head.name`（`head.type === "detached"` 时显示 `detached @ <oid7>`）
    - `branch.upstream` ← `upstream.remote_ref`（无 upstream 时显示 "no upstream"）
    - `branch.ahead` / `branch.behind` ← `upstream.ahead` / `upstream.behind`
    - `branch.dirty` ← `!is_clean`
    - `branch.changes` ← 文件计数（`staged.* + unstaged.* + untracked` 的总和），mock 的 `+812 −214` 行级 stat **v1 不做**（如要做，再扩 `build_status_json` 加 shortstat）
  - **PR 字段 v1 砍掉**，UI 上整列移除（不显示空字符串），未来若引入 `gh` 集成再单开 `/api/repo/pr` 端点。
  - 刷新策略：首屏拉一次；之后在 `session_updated`/`status_changed` SSE 触发后 debounce 5s fetch；Summary 头加一个手动 refresh 按钮兜底（覆盖"另一终端 commit"场景）。
- Diff：
  - 统一 diff parser，支持多文件 patchset、binary/no diff、large diff collapse；解析失败 fail-open 显示原始 diff 与错误。
- Terminal：
  - 按 `toolCalls` 和 transcript metadata 分 Sandbox / Tools / Agent 三个 tab。
  - 不提供直接 shell prompt；如需要运行命令，通过 agent message 或后续受控 command interaction。
- Settings：
  - 显示 provider/model/context/network/approval policy；可修改项必须先有后端 endpoint，否则只读。
- Capability gating：所有可写 UI 控件都按 `snapshot.capabilities.*` 8 个 flag 显式启用/禁用；`controller.canWrite === false` 时整层置灰并显示当前 owner。

**验收：**
- 页面无硬编码 demo 文案；没有真实数据时显示明确空态（包括 token 计数、footer 计数）。
- PatchSet diff、tool call result、plan review choice、approval prompt 都能从同一个 snapshot 重建。
- 长 transcript、长 diff、长 tool output 不阻塞主线程，不造成布局错位（建议长 diff > 2000 行折叠、tool output > 200 KB 截断 + 加载更多按钮）。
- capability flag 改变（例如 provider 不支持 streaming）时，对应控件立即变成只读/禁用。

### Phase 5：测试、文档与发布门

**目标：** 把 Web UI 接入纳入 CI 与 release checklist。

**任务：**
- 前端：
  - `web/src/lib/code-ui/client.test.ts` 覆盖 fetch、SSE event parse、reconnect、`Lagged`-recover、error mapping、controller token header 注入。
  - 组件测试覆盖：无 session 空态、只读 controller、`LocalTui` 占用下浏览器禁用写、pending interaction（5 种 kind）、streaming transcript、long diff、`PAYLOAD_TOO_LARGE`。
  - `pnpm --dir web build` 失败应在 CI 中阻断（保证 `web/out/` 与 `WebAssets` 嵌入资源一致）。
- Rust：
  - 扩展 [tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs)，增加 browser-like flow：session load、SSE wait、attach、submit、respond、detach、控制冲突、loopback 拒绝、`LocalTui` 抢占、`PAYLOAD_TOO_LARGE`。
  - [tests/harness/code_session.rs](../../tests/harness/code_session.rs) 增加 browser controller 路径（含 `X-Code-Controller-Token`），与现有 automation `X-Libra-Control-Token` 路径并列。
  - 增加 headless web-only non-Codex scenario（依赖 Phase 3 的 `HeadlessCodeRuntime`）。
- 浏览器 smoke：
  - 用 Playwright 或现有 browser harness 打开 `http://127.0.0.1:<port>`，断言页面不再出现 mock 的 thread title (`agent/optimistic-mutate`、`Add optimistic updates to useMutation`)，且发送消息后 snapshot 更新。
- 文档：
  - 重写 `web/README.md`：移除 create-next-app 默认说明（当前还是 `npm run dev` 模板），写清 `pnpm dev` / `pnpm build` / 静态 export / Rust `WebAssets` 嵌入路径 / 本地 live API（`/api/code/*`、`/api/repo`）。
  - 更新 `docs/commands/code.md` 的 Web mode 限制，明确 browser write 的 loopback/auth 边界、controller token 与 control token 的区别、256 KiB body 限制、audit 行为。

**验收命令：**

```bash
pnpm --dir web lint
pnpm --dir web build
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test code_ui_scenarios \
  --test harness_self_test \
  --test code_codex_default_tui_test \
  -- --test-threads=1
```

---

## PR 切片建议

| PR | 范围 | 可独立验证 |
|----|------|------------|
| PR 1 | Phase 0 + API client skeleton + type contract | lint + serde golden |
| PR 2 | Phase 1 只读 live chat/header/workflow 基线 | TUI observe 手工/自动 smoke |
| PR 3 | Phase 2 browser controller + composer + interaction panel | Codex web-only + generic TUI write scenario |
| PR 4 | Phase 4 workflow/summary/diff/terminal 完整映射 | component tests + long data fixtures |
| PR 5 | Phase 3 headless generic web-only runtime | non-Codex web-only e2e |
| PR 6 | docs + CI hardening + web README 收口 | 全量验收矩阵 |

Phase 3 工程风险最大，可以在 PR 2/3/4 之后并行设计，但不要阻塞“浏览器观察/控制已有 TUI session”的短路径。

---

## 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 前端直接依赖 mock types | wire contract 漂移，接入后大量返工 | Phase 0 先建立 `code-ui/types.ts`，mock 只做 dev fixture |
| 浏览器写控制暴露到非 loopback | 本地 workspace 被远程网页驱动 | 服务端 `ensure_loopback_api_request` + `ensure_loopback_browser_control_host` 已是双层防御；TUI 路径打开 browser write 时也必须复用 |
| TUI 与 browser 同时写入 | turn 状态错乱、approval 响应错配 | 单 controller lease（`LocalTui` 默认占用）；TUI 可 reclaim；提交按钮按 `status/capabilities/controller.canWrite` 三个维度禁用 |
| 误用 `X-Libra-Control-Token` | 浏览器附带进程级 control token 等同于自动化提权 | 前端只能持有 lease token；浏览器代码里禁出现 `X-Libra-Control-Token` 字面量（lint rule 守护） |
| Headless runtime 复制 TUI 逻辑 | TUI/web 行为分叉 | 抽 session driver，不复制 `App` 内的业务状态机 |
| SSE 全量 snapshot 过大 / `Lagged` | 长会话下卡顿或丢事件 | v1 先做节流、large output collapse；遇 `Lagged`/断线一定要 fallback 到 `GET /api/code/session`；typed delta 单独规划 |
| 256 KiB body limit | 长 prompt 被服务端拒 | 客户端预校验 + UI 提示；超长内容引导分片或上传文件 interaction（后续） |
| Diff 解析失败 | PatchSet 页面空白或崩溃 | parser fail-open，显示原始 diff 文本与错误提示 |
| Web build 未同步嵌入资源 | Rust server 继续服务旧页面 | CI 强制 `pnpm --dir web build` 并检查 `web/out/` 变更 |
| 审计漏失 | 浏览器写动作未走 audit sink | 前端必须经统一 client；新加的写路径必须在 [tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs) 中验证 audit log 出现 |

---

## Open Questions

- ~~是否新增显式 `--browser-control` flag~~ → **已决定**：新增 `--browser-control <off|loopback>`，TUI 默认 `off`，web-only Codex 默认 `loopback`，详见 Phase 2。
- ~~mock 文件去留~~ → **已决定**：Phase 1 PR 完成迁移后直接删除 `web/src/lib/mock/`，详见 Phase 1 验收。
- ~~`Summary.branch` 数据来源~~ → **已决定**：新建 `GET /api/repo/status` 复用 `build_status_json()`；PR 字段 v1 不做；详见 Phase 1 / Phase 4 与 API 表。
- ~~`Thread list` 来源~~ → **已决定**：后端新增 `GET /api/code/threads`，复用 [`ThreadProjection::list_active`](../../src/internal/ai/projection/thread.rs:425)；"New thread" v1 不做，UI 引导回 CLI；详见 API 表与 Phase 4 Sidebar。
- ~~`/api/code/control/cancel` 浏览器入口~~ → **已决定**：放开 `Browser` controller 调用，与 TUI `Esc` 同等水平；详见 API 表与 Phase 2。
- ~~`--host 0.0.0.0` 的 Web UI 行为~~ → **已决定**：服务端检测到非 loopback 请求时由 `static_handler` 改派一张独立静态说明页（设计交由 Claude Design 出稿，brief 见下方 [Remote Access Notice 页面 brief](#remote-access-notice-页面-brief)）。

---

## Remote Access Notice 页面 brief

下面这段是给 Claude Design 用的产出说明，描述要做什么、为谁做、怎么落到 Rust embed 资产里。设计稿不限定具体视觉，但要落在文末的硬约束之内。

### 用途与触发条件

`libra code` 的 web server 默认绑 loopback；当用户传 `--host 0.0.0.0`（或其它非 loopback 地址）后，远程 IP 仍然能加载静态前端资源，但所有 `/api/*` 请求会被 [`ensure_loopback_api_request`](../../src/internal/ai/web/mod.rs:505) 以 `LOOPBACK_REQUIRED` 拒绝。结果：远端浏览器看到的是一个永远在转圈、所有请求 403 的空 SPA。

要做的事：在 [`static_handler`](../../src/internal/ai/web/mod.rs:156) 里加一段判断，如果 `ConnectInfo<SocketAddr>` 的 IP 不是 loopback，就把所有 HTML 请求改派成单一的 `remote-notice/index.html`（与 SPA 同样从 `web/out/` 嵌入）。该页面纯静态、不做任何 fetch、不暴露 session 信息。

### 受众与目标

**受众**：从另一台机器（或容器、远程 dev box）通过 IP 访问这台 libra 进程的工程师/PM/QA。他们大概率不知道 libra 的 loopback-only 安全模型，看到空白页会以为服务挂了。

**首要目标**：让对方在 10 秒内理解三件事 ——
1. 这不是 bug；这是有意的安全设计。
2. 想看 UI 必须在跑 libra 的那台机器上，或者通过 SSH 端口转发。
3. 给一两条可以立即复制的命令。

**次要目标**：传达"libra 是有人盯着安全边界的项目"的专业感，不要看上去像 nginx 默认 403 页。

### 必含内容块

按页面从上到下：

1. **品牌行**：libra brand mark + 文字 "Libra Code"，左上角，字号克制。
2. **状态徽标**：一个小的标签条，例如 `Server: 0.0.0.0:<port> · Your IP: <client-ip> · Mode: loopback-only`。`<port>` 与 `<client-ip>` 由 Rust 在 serve 时注入两个占位符替换（`__LIBRA_BIND__`、`__LIBRA_REMOTE__`），不要做 JS 替换。
3. **主标题**：一句话，例如 "This Libra session only accepts connections from the host machine."（中英双语；中文版独立 HTML，路径 `remote-notice/zh-CN/index.html`，按 `Accept-Language` 选择）。
4. **解释段落（≤ 60 字 / ≤ 30 中文字）**：说明 libra 的写控制走 loopback，远程访问需要走 SSH 隧道。**不**展示具体安全细节、token、内部架构。
5. **"在本机打开" 卡片**：单一命令（`open http://127.0.0.1:<port>` / `xdg-open …` / `start …`），辅助说明哪台机器是"本机"。
6. **"从另一台机器打开" 卡片**：一条 SSH 端口转发命令模板：
   ```
   ssh -L <port>:127.0.0.1:<port> user@host
   # 然后访问 http://127.0.0.1:<port>
   ```
   `<port>` / `user@host` 同样用占位符。
7. **"为什么这么设计" 折叠区**：一句话总结 + 链到 `docs/commands/code.md` 的 Web mode 章节。默认折起，不抢主流程。
8. **页脚**：libra 版本号 + 当前 commit hash（构建时注入到 `__LIBRA_VERSION__` / `__LIBRA_COMMIT__` 占位符），右下角微弱呈现。

### 视觉风格

- 与现有 web UI 同源 token：`bg-paper`、`bg-paper-2`、`text-ink`、`text-ink-2`、`text-ink-3`、`border-rule`、`accent`、`accent-soft`、`accent-line`、`mono`（参考 [globals.css](../../web/src/app/globals.css) 与 brand mark）。
- 内联 SVG brand mark，禁止 `<img>` 外链。
- 单页 < 30 KB（含内联 CSS、SVG），无外部字体（用 system stack）。
- 支持 `prefers-color-scheme: dark`；最小 360px 宽手机屏可读。
- 不做动画、不做装饰性图形；命令块用 1px 边 + 等宽字体，模仿现有 chat 内 mono 块的样式。

### 硬约束（违反等于回退）

- **零 JavaScript**：不能 fetch、不能读 cookie、不能 `<script>`。
- **零外部请求**：所有 CSS、SVG、字体 inline；不引 CDN。
- **零数据泄露**：不暴露 thread id、provider 名、API key 提示、文件路径、用户名以外的任何 session 信息（`<client-ip>` 是远端自己的 IP，可以显示）。
- **静态文件路径**：`web/out/remote-notice/index.html`（英文）+ `web/out/remote-notice/zh-CN/index.html`（中文）；`pnpm --dir web build` 后 `WebAssets` 自动嵌入。
- **占位符约定**：`__LIBRA_BIND__`、`__LIBRA_REMOTE__`、`__LIBRA_VERSION__`、`__LIBRA_COMMIT__`。Rust 在响应阶段做字符串替换，不引模板引擎。
- **路由规则**：服务端 `static_handler` 在判定为非 loopback 时，对所有 `text/html` 请求统一返回 notice 页；CSS/JS/字体等 asset 请求继续返回 404，避免 SPA bootstrap。
- **可访问性**：单 `<main>` 区域，`<h1>` 唯一，所有交互（折叠、链接）键盘可达；颜色对比度 ≥ AA。

### 验收

- 用 `curl -H "Host: <bind-ip>" http://<bind-ip>:<port>/` 抓的 HTML 含 "loopback" 关键字，且 `<` script `>` 标签零次出现。
- `pnpm --dir web build` 后 `web/out/remote-notice/index.html` 大小 ≤ 30 KB。
- 中文版 `web/out/remote-notice/zh-CN/index.html` 同样 ≤ 30 KB，文案专业不机翻。
- `cargo test` 中加一条用例：模拟非 loopback `ConnectInfo` 请求 `/`，断言返回 200 + `text/html` + body 含特定字符串（避免回归把 SPA 漏过去）。

---

---

## 完成定义

当以下条件全部满足时，Web UI 接入可视为完成：

- `web/src/lib/mock/*` 不再参与生产页面运行路径（`grep -rn "@/lib/mock" web/src/components` 应只剩 dev fallback / 测试 fixture）。
- 浏览器刷新后能从 Rust snapshot 恢复真实 session（含 `transcript / plans / tasks / toolCalls / patchsets / interactions / controller`）。
- 浏览器能发送消息并响应所有 v1 interaction kind（`approval` / `sandboxApproval` / `requestUserInput` / `intentReviewChoice` / `postPlanChoice`）。
- 8 个 capability flag 都被 UI 正确尊重，不可写场景一律置灰。
- 普通 TUI、Codex web-only、普通 provider headless web-only 都有自动化覆盖（含 audit log 断言）。
- 文档明确 Web mode 能力边界、loopback 写控制、`X-Code-Controller-Token` vs `X-Libra-Control-Token` 的分工、256 KiB body limit 与验收命令。
