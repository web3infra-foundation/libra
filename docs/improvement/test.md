# Libra Code 完整测试范围规划（AI 可落地版）

## 概述

`libra code` 是仓库目前最复杂的子命令：一条入口同时拉起 TUI、HTTP/SSE Web 服务、MCP 服务、AI Agent 工具循环、Codex 旁路、Local TUI Automation Control。代码侧近 30 个 commit 几乎全部围绕 `web/code-ui/orchestrator`，但测试侧仅覆盖到部分维度：

- L0 单元/契约：headless / projection / wire / codex 默认 TUI 守卫，以及 tool loop / session jsonl / redaction / wire 序列化等通过 Code 路径间接受益的测试。
- L2 PTY+HTTP smoke：[tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs) 已覆盖 attach/detach/cancel/conflict/oversize 等 13 条场景。
- L2 数据驱动：[tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs) 仅 4 条 lease case 在跑；`tests/data/code_ui_remote/` 下 SSE/state/security/generation/model_generation 五个矩阵的 JSON 已就位但**没有 Rust runner**。
- L3 真实模型：仅 AI 子系统层面有 ollama gate 与 deepseek 直连测试，**未与 `libra code` 服务路径打通**。

本文目标：给出整个 `libra code` 功能的端到端测试范围地图。第 5 节按“功能面 × 测试分层 × 现状/缺口/优先级”组织 20 项纵深面；第 6 节内嵌已有的 L2 远端矩阵方案；第 7–10 节给出统一的覆盖矩阵、文件清单、验证命令和 12 PR Wave roadmap。本文是规划文档，实施按 Wave 拆分 PR；每个 PR 保持现有 smoke 不回归。

---

## 前置依赖与基线核查

### 2.1 已就位的基础设施（无需改动即可使用）

| 组件 | 状态 | 路径/说明 |
|---|---|---|
| 数据驱动矩阵骨架 | ✅ 就位 | `tests/harness/matrix.rs`：已定义 `CaseFile`、`Case`、`Step`（Attach/Detach/Submit/Sleep/WaitSnapshot）、`AuthMode`、`TokenSource`、`TokenSlot`、断言求值器 |
| PTY Session Harness | ✅ 就位 | `tests/harness/code_session.rs`：`CodeSession::spawn()`、`snapshot()`、`matrix_attach/detach/submit()`、`with_control_observe()`、`with_lease_duration_ms()` |
| Lease TTL override | ✅ 就位 | `src/internal/ai/web/code_ui.rs` 有 `test_lease_duration_override()`；`src/command/code.rs` 在 `cfg(feature = "test-provider")` 下解析 `LIBRA_CODE_LEASE_DURATION_MS` |
| 全部 JSON 数据文件 | ✅ 就位 | `tests/data/code_ui_remote/` 下 6 个矩阵 JSON + `provider_fixtures/` 下 4 个 fake fixture |
| 现有 L2 smoke | ✅ 就位 | `tests/code_ui_scenarios.rs` 13 条 case（10 条 `#[cfg(feature = "test-provider")]` + 3 条 browser） |
| 现有 L2 lease | ⚠️ 部分 | `tests/code_ui_remote_lease_matrix.rs` 仅 macro 了 4 条 case（共 9 条） |
| Inline 网络层测试 | ⚠️ 部分 | `src/internal/ai/web/mod.rs` `mod tests` 已有 loopback、control auth、body limit、audit 单测 |
| Executor 单元测试 | ⚠️ 部分 | `src/internal/ai/orchestrator/executor.rs` `mod tests` 已有 mock model 和基础 tool loop 测试 |
| WS 库 | ✅ 就位 | `tokio-tungstenite = "0.29.0"` 已在 `Cargo.toml` `[dependencies]`，可用于 Codex mock WS |

### 2.2 缺失的前置依赖（必须在对应 Wave 开始前解决）

| 依赖 | 影响 Wave | 解决方式 | 备注 |
|---|---|---|---|
| `insta` | PR 10（TUI 快照） | `Cargo.toml [dev-dependencies]` 新增 `insta = "1.40"` | 用于 `ratatui::backend::TestBackend` 快照比对；若团队不希望引入新依赖，可降级为手动 `Buffer::cell` 断言 |
| `httpmock` | PR 10（Provider boot） | `Cargo.toml [dev-dependencies]` 新增 `httpmock = "2.0"` | 用于 stub 各 provider 的首次 completion HTTP 请求；可用 `axum::Server` 手写 mock 替代，但 `httpmock` 更轻量 |
| `event_stream.rs` | PR 1 / PR 4（SSE） | **新建文件** `tests/harness/event_stream.rs` | 当前 harness 没有 SSE reader；这是 SSE 矩阵的硬性阻塞项 |

> **AI 编码落地原则**：如果一个 Wave 的前置依赖未解决，必须在该 Wave 的 PR 描述中把“添加依赖”列为首条 commit，或把该 Wave 整体后移到依赖就绪之后。

### 2.3 可行性分级

| 分级 | 含义 | 包含项 |
|---|---|---|
| 🟢 可直接编码 | 文件/类型已存在，只需扩写或新增同结构文件 | Lease 扩 9/9、State/Security/Generation runner、inline loopback test、error code L0、executor 扩写 |
| 🟡 需新增中等复杂度组件 | 需要新建文件或新增 enum variant，但有明确模式可参考 | `event_stream.rs`、matrix 新增 `Step`/`ProviderRef` variant、`CodeSession` 新增 HTTP helper、CLI dispatch L1 |
| 🔴 需架构决策或外部依赖 | 依赖未安装、或涉及跨模块可见性改动、或 CI 秘钥配置 | TUI 快照（缺 `insta`）、Provider boot（缺 `httpmock`）、Model generation L3（需 `.env.test` + nightly CI）、Codex mock WS（虽可行但工作量大） |

---

## 范围与分层

**纳入：**

- `src/command/code.rs` 暴露的全部 CLI surface（TUI / `--web` / `--stdio`、provider 矩阵、`--control` / `--browser-control`、`--env-file`、`--port` / `--mcp-port`、`--resume` 等）。
- `src/internal/ai/web/code_ui.rs` + `src/internal/ai/web/mod.rs` 的 HTTP/SSE 协议、controller lease 状态机、redaction、audit。
- TUI 渲染层（`src/internal/tui/`）由 `libra code` 路径触发的部分。
- Tool 注册与执行（`src/internal/ai/tools/`），仅限通过 `libra code` 调用的入口语义（context / approval policy / network 矩阵）。
- `src/internal/ai/orchestrator/`、`src/internal/ai/codex/` 与 Code 路径直接耦合的 gate / executor / sandbox / approval。
- `src/internal/ai/mcp/` 在 `libra code` 启动后暴露的服务。
- 真实 provider（DeepSeek 默认）通过 `.env.test` 驱动 `libra code` 完整生成流程。

**不纳入（已有专项或与 Code 解耦）：**

- AI 子系统的纯算法/数据结构单元测试（已有 30+ 文件覆盖）。
- Git core 命令（init/commit/push…）、cloud storage、LFS、protocol。
- Codex CLI 自身行为（仅在与 `libra code` 衔接处覆盖）。

**测试分层：**

| 层 | 标识 | 边界 | feature gate / env |
|---|---|---|---|
| L0 | 模块内 `#[test]` | 纯函数；`cargo test --lib` 命中 | 默认 |
| L1 | `tests/*.rs` 进程内集成 | 直接调用 `code_router()` / `HeadlessCodeRuntime` / `LibraMcpServer`，不起子进程 | 默认 / `test-provider` |
| L2 | PTY + HTTP/SSE 子进程 | 通过 [tests/harness/code_session.rs](../../tests/harness/code_session.rs) spawn `libra code` 二进制 | `test-provider` |
| L3 | 真实模型 | 通过仓库根 `.env.test` 注入 DeepSeek 凭证，跑完整 `apply_patch` + 编译/测试链路 | `test-provider` + `LIBRA_RUN_LIVE=1` + 网络 |

CI 默认门：L0+L1 必跑；L2 在 `test-provider` 下必跑；L3 仅 nightly / `LIBRA_RUN_LIVE=1` 跑。Cargo features 见 [Cargo.toml](../../Cargo.toml) 的 `test-provider`、`test-network`、`test-live-ai`、`test-live-cloud`。

---

## 现状基线

### 既有基线对比

| 领域 | 当前已有 | 备注 |
|---|---|---|
| L2 smoke | [tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs) 13 条 case | automation 和 browser 两条写通道的端到端场景 |
| Harness browser helper | [tests/harness/code_session.rs](../../tests/harness/code_session.rs) 已有 `with_browser_control_loopback()`、`attach_browser()`、browser submit/cancel/detach/oversize/respond_interaction helper | 删除“待建 browser helper”假设 |
| Control mode | CLI 只有 `observe` / `write`，见 [src/command/code.rs](../../src/command/code.rs) 的 `ControlMode` | 原方案里的 `--control read` 不存在，应改为默认 `observe`；`CodeSessionOptions::with_control_observe()` 已存在 |
| Fake provider | [src/internal/ai/providers/fake/fixture.rs](../../src/internal/ai/providers/fake/fixture.rs) 已支持 `text`、`tool_call`、`error` 和 `stream` delta | tool case 的风险不是缺 `tool_call` variant，而是需要避免重复命中同一 tool_call 造成循环 |
| Env file | [src/command/code.rs](../../src/command/code.rs) 已支持 `--env-file` 且 TUI mode 接受 `.env.test` | 真实模型矩阵要默认传仓库根目录 `.env.test`，不能只跑 fake provider |
| Loopback inline test | [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 的 `mod tests` 已能直接使用 private `code_router()` / `code_write_router()` | 不需要把 `build_router` 提升为 `pub(crate)`；但**缺少**带 `ConnectInfo` 的路由级 403 测试 |
| SSE lagged | `/events` 使用 `BroadcastStream` 后对 lagged error 静默丢弃，不会发 `event: lagged` | P0 不做强行制造 lag 的跨进程测试，只测重连和 initial replay |
| Lease TTL override | [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 已新增 `CodeUiRuntimeOptions` 与 `test_lease_duration_override()`，在 `cfg(feature = "test-provider")` 下读 `LIBRA_CODE_LEASE_DURATION_MS` | 已就位，矩阵直接消费即可 |
| Tool-loop max_turns | [src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) 已把 Implementation 24→48、Analysis 24→32 | 边界值测试需要新增 |

### 本次扩写新增覆盖面

| 维度 | 当前 | 扩写后 |
|---|---|---|
| CLI 模式 / 互斥 / 解析 | 隐式触发 | L1 dispatch 测试覆盖每条模式与互斥错误码 |
| Provider boot / flag passthrough | 仅 fake + ollama gate | 每个 provider httpmock smoke；thinking/reasoning/stream 字段 passthrough |
| HTTP 读路由 + loopback gate 顺序 | 缺 | inline route test + security 矩阵 |
| HTTP 写路由 lease 矩阵 | 4/9 | 9/9 + observe 模式 |
| SSE | 无 runner | event_stream harness + 7 条 case |
| Local TUI Control 锁 / 审计 | 1 条 instance conflict | advisory lock、stale PID、audit redaction L0 |
| TUI 渲染 | 无 | TestBackend + insta 快照（需先加 `insta` 依赖） |
| Tool ACL × context × policy | 隐式 | dev/review/research × 5 种 approval-policy 矩阵 |
| Apply-Patch 文件生成(fake) | 无 runner | generation_cases.json 全量 + 失败分支 |
| Approval / Interaction E2E | 无 | accept/reject/apply_to_future/并发 pending |
| Orchestrator gate 边界 | 一条 review rejection | max_turns 47/49、workspace 越界、network deny、FUSE 回退 |
| Codex 旁路运行时 | 静态守卫 | mock WS app-server + plan-first 拦截 |
| MCP 双入口一致 | 单端 | HTTP + stdio 双入口同 thread 一致 |
| Session resume / kill 重启 | 无 | SIGTERM kill 重启，transcript 完整恢复 |
| 性能 smoke | 无 | 100k transcript / 长 SSE / 并发 / threads（`#[ignore]`） |
| 真实模型生成 | 无 | DeepSeek `deepseek-v4-flash` 完整闭环 |
| 错误码契约 | 散落 | L0 single-source mapping + 文档注脚 |

---

## 纵深功能面 → 测试策略

每节体例：**现状（✅/⚠️/❌）→ 缺口 → 优先级 → 测试位置**。

### 5.1 CLI 解析与模式分发

- **现状 ⚠️**：仅通过 `code_ui_scenarios.rs` 隐式触发 TUI 模式；`--web` / `--stdio` / `--resume` / `--cwd` / `--repo` / `--env-file` 没有专项断言。
- **缺口**：
  - `--web` 无终端、`--stdio` 无 web、`--mcp-port 0` 端口动态分配。
  - `--resume <thread_id>` 取回历史 transcript（连接 `.libra/objects/`）。
  - `--env-file` 同时驱动 provider 凭证 + `LIBRA_*` 行为开关。
  - 互斥校验：同时给 `--web` 和 `--stdio` 必须报错。
  - `--plan-mode` 默认值矩阵（Codex=true，其它=false）。
- **优先级**：P0。
- **测试位置**：**L1 新增** `tests/code_cli_dispatch_test.rs`，直接调用 `Code::Args::parse_from(...)` 加部分 `execute()` 短路路径，断言模式选择与错误。
- **AI 落地提示**：`src/command/code.rs` 的 `CodeArgs` 已用 `clap` derive。测试只需 `CodeArgs::try_parse_from(["code", "--web"])` 等组合，断言 `Ok` / `Err` 和具体错误消息。不要 spawn 子进程。

### 5.2 Provider 配置与启动

- **现状 ⚠️**：fake provider 完全覆盖；DeepSeek/OpenAI/Anthropic/Gemini/Kimi/Zhipu/Ollama/Codex 在 Code 路径下只有 ollama L3 gate 和 codex 静态守卫。
- **缺口**：
  - 每个 provider 至少一条 boot smoke：CLI → 客户端构造 → 第一次 completion 请求成功（`httpmock` 返回 stub）。
  - DeepSeek `--deepseek-thinking` / `--deepseek-reasoning-effort` / `--deepseek-stream` 透传到请求 body。
  - Kimi `--kimi-thinking` / `--kimi-stream`、Ollama `--ollama-thinking` / `--ollama-compact-tools` 同上。
  - 缺 API Key 时的可读错误（`anyhow::Context` 链）。
  - `--api-base` 覆盖默认 base URL。
- **优先级**：P1（Codex 除外，见 5.13）。
- **测试位置**：**L1 新增** `tests/code_provider_boot_test.rs` + `tests/code_provider_flag_passthrough_test.rs`，用 `httpmock` 起本地 stub，捕获 outgoing request body。
- **AI 落地提示**：**前置阻塞项**——`Cargo.toml` 必须先加 `httpmock`。若团队不希望引入 `httpmock`，可用 `tests/helpers/` 中已有的 `mock_completion_model.rs` 模式（手写 `axum` oneshot）替代，但需额外工作量。

### 5.3 HTTP 读路由 + loopback gate

- **现状 ⚠️**：`/session` 由 `code_ui_scenarios.rs` 间接验证；`/diagnostics`、`/threads` 几乎无专项；`/events` 完全未跑。
- **缺口**：
  - `/session` 完整 schema（含 `controller`、`status`、`activeInteractionId`、`patchsets`）。
  - `/diagnostics` 字段齐全且全部 secret 字段经 redactor。
  - `/threads?limit/offset` 边界：`limit=abc` → 400 `INVALID_QUERY_PARAM`；`limit=10000` → clamp 到 200；空集 → `[]`。
  - 任意读路由 non-loopback 客户端 → 403 `LOOPBACK_REQUIRED`（loopback 校验**先于** body/token 校验，错误码顺序固定）。
- **优先级**：P0。
- **测试位置**：**L1 新增** [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) `mod tests` 内 inline test：用 `code_router()` + `ConnectInfo(192.0.2.10:4000)` 直击各路由 → 403。无需修改 `build_router` 可见性。**L2 配合** `code_ui_remote_security_matrix.rs` 复用 `security_cases.json`。
- **AI 落地提示**：参考 `mod tests` 中已有的 `code_write_body_limit_returns_json_error` 模式：`app.oneshot(request).await`。对读路由需要构造带 `ConnectInfo` 扩展的 `Request`，可用 `tower::ServiceExt::oneshot` + `axum::extract::connect_info::MockConnectInfo` 或直接在 `Request` 扩展中插入 `SocketAddr`。

### 5.4 HTTP 写路由 + Controller Lease 状态机

- **现状 ⚠️**：lease 矩阵 4 条 case 已跑；[tests/data/code_ui_remote/lease_cases.json](../../tests/data/code_ui_remote/lease_cases.json) 共 9 条 P0，runner 仅 macroed 4 条；`/messages` 大 body / busy / 422 在 `code_ui_scenarios.rs` 已覆盖；`/interactions/{id}` 仅 `INTERACTION_NOT_ACTIVE` 一条。
- **缺口**：
  - 把 lease_cases.json 9 条全部接入 `lease_case!()` 宏。
  - 短 TTL 真过期重新 attach（`LIBRA_CODE_LEASE_DURATION_MS` 已落地）。
  - `--control observe` 下 automation attach → 403 `CONTROL_DISABLED`；`CodeSessionOptions::with_control_observe()` **已存在**，只需在矩阵中配置 `"control": "observe"`。
  - 同 client 续约 vs 不同 client conflict 的 token 失效顺序。
  - `/interactions/{id}` 写路径：approval 接受/拒绝、`selected_option`、`answers` 字段、`apply_to_future` 缓存、超时。
  - `/control/cancel` idle 期间的 422 / `SESSION_BUSY` 文档化。
- **优先级**：P0。
- **测试位置**：**L2 扩展** lease matrix 9/9；**L1 新增** approval interaction state machine 单测（在 `code_ui.rs` 内直接构造 pending interaction 驱动）。
- **AI 落地提示**：`tests/code_ui_remote_lease_matrix.rs` 已有 `lease_case!` 宏，新增 case 只需在文件底部加一行 `lease_case!(lease_xxx);`，对应 `lease_cases.json` 中已有的 case name。已存在的 5 条未接入 case 名称见该 JSON。

### 5.5 SSE 事件流

- **现状 ❌**：完全无 Rust 客户端；[tests/data/code_ui_remote/sse_cases.json](../../tests/data/code_ui_remote/sse_cases.json) 7 条已写好，但**没有 runner**。
- **缺口**：
  - SSE blocking client：`tests/harness/event_stream.rs`（**新建文件**，独立 `reqwest::blocking`、per-read timeout、1 MiB 行上限、EOF 与 timeout 区分）。
  - 初次连接 replay 全量 `session_updated` snapshot。
  - submit → `status_changed: thinking` → tool_call → `status_changed: idle` 的事件序列与 `seq` 单调性。
  - attach/detach 触发 `controller_changed`。
  - 双订阅者均收到 submit 引发的事件（broadcast fan-out）。
  - 断开重连后 initial replay 含最新 transcript（不丢）。
  - streaming fixture 下 transcript 单调增长，无丢字。
  - **降级 P2**：lagged stream 跨进程稳定再现困难，仅做 in-process broadcast 单测。
- **优先级**：P0。
- **测试位置**：**L2 新增** `tests/harness/event_stream.rs` + `tests/code_ui_remote_sse_matrix.rs`。**L0 新增** SSE 解析器单元（`event:` / `data:` 行 + 空行分块）。
- **AI 落地提示**：`event_stream.rs` 是**硬性阻塞项**，必须在 Wave 1 完成。签名建议：
  ```rust
  pub struct EventStream { /* reqwest::blocking::Response */ }
  impl EventStream {
      pub fn open(client: &reqwest::blocking::Client, url: &str, timeout: Duration) -> Result<Self>;
      pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SseEvent>>;
  }
  pub struct SseEvent { pub event: String, pub data: String }
  ```
  `matrix.rs` 需要新增 `Step::OpenEventStream { name, timeout_ms }`、`Step::WaitEvent { name, event_type, timeout_ms }`。

### 5.6 Local TUI Control 锁 / 审计

- **现状 ✅⚠️**：[docs/automation/local-tui-control.md](../automation/local-tui-control.md) 已规约；`code_ui_scenarios.rs` 覆盖 `CONTROL_INSTANCE_CONFLICT`。
- **缺口**：
  - 多实例 advisory lock + stale PID 接管（spawn 一个 → kill -9 → 第二个能启）。
  - 自定义 `--control-token-file` / `--control-info-file` 的 token 隔离与 audit 同步。
  - token 文件权限模式（0600）跨平台行为（Unix only 断言）。
  - audit log（`AuditSink`）字段裁剪（`client_id` 80 字符上限、控制字符替换）。
- **优先级**：P0（audit redaction）、P1（advisory lock）。
- **测试位置**：**L2 扩展** `code_ui_scenarios.rs` 或新增 `tests/code_control_lock_test.rs`；**L1 新增** audit redaction 单测（构造 `ControlAuditRecord` 注入 secret-like client_id）。

### 5.7 Browser Control

- **现状 ✅**：`code_ui_scenarios.rs` 6 条 browser 场景（attach/submit/detach/oversize/cancel/unknown_interaction/second_browser_conflict）。
- **缺口**：
  - 同 clientId 重连（浏览器 reload）：预期可读错误而非 panic。
  - `--browser-control off` + browser attach 的错误顺序（loopback → `BROWSER_CONTROL_DISABLED`）。
  - 浏览器 token 过期后 submit 的 detach 自动化（grace 处理）。
- **优先级**：P1。
- **测试位置**：**L2 扩展** `code_ui_scenarios.rs` 三条 case。

### 5.8 TUI 渲染快照

- **现状 ❌**：除 `ai_usage_tui_test.rs` 外，TUI 渲染没有快照测试；`libra code` 进入 TUI 后的 ratatui Buffer 未被验证。
- **缺口**：
  - `insta` / `ratatui::backend::TestBackend` 快照：初始空 transcript、收到 assistant delta、approval prompt 弹窗、错误 banner、controller reclaim 状态条。
  - 关键键位：`Ctrl+C` 取消、`q` 退出、approval 选择 yes/no/always、滚动键。
- **优先级**：P1。
- **测试位置**：**L1 新增** `tests/code_tui_render_test.rs` + `tests/snapshots/`，使用 `TestBackend` 渲染 `App` 不同 state。
- **AI 落地提示**：**前置阻塞项**——必须先给 `Cargo.toml` 加 `insta = "1.40"`。`ratatui = "0.30.0"` 已在 deps。若不加 `insta`，可改用 `assert_eq!(buffer.cell(x, y).symbol(), "expected")` 手动断言，但维护成本高。

### 5.9 Tool ACL / context / approval policy

- **现状 ⚠️**：单 tool 行为有 `ai_dag_tool_loop_test.rs` / `ai_semantic_tools_test.rs` 覆盖；Code 路径下的组合矩阵未跑。
- **缺口**：
  - tool registry 按 `--context dev|review|research` 过滤后的可见集合（review 模式不应暴露 ApplyPatch / Shell mutating 路径）。
  - tool ACL × `--approval-policy` 矩阵：`never` / `on-failure` / `on-request` / `untrusted` / `allow-all` 各跑一条 fake fixture，断言 approval interaction 是否产生。
  - `--network-access deny` 下 WebSearch / Shell `curl` 被 gate 拦截。
- **优先级**：P0。
- **测试位置**：**L1 新增** `tests/code_tool_acl_test.rs`，直接调用 `HeadlessCodeRuntime` + 不同 context/policy 组合。

### 5.10 Apply-Patch 与文件生成(fake)

- **现状 ❌**：[tests/data/code_ui_remote/generation_cases.json](../../tests/data/code_ui_remote/generation_cases.json) 3 条 case 已写好，**runner 缺失**。
- **缺口**：
  - fake fixture 返回 `apply_patch` → 临时 repo 出现完整 Rust 文件 → `rustc --test` 通过。
  - SSE 订阅期间触发同一生成请求 → 至少观测到 `executing_tool` 或 `session_updated` 含 patch 结果。
  - 失败分支：fixture 返回非法 patch → final snapshot status=`error`，**临时 repo 不留半写文件**。
- **优先级**：P0。
- **测试位置**：**L2 新增** `tests/code_ui_remote_generation_matrix.rs` + provider_fixtures，详见 §6.4 Wave 3。

### 5.11 Approval / Interaction 端到端

- **现状 ⚠️**：`ai_approval_ttl_test.rs` 覆盖 cache 策略，但 Code UI 的端到端 approval **未覆盖**。
- **缺口**：
  - **P0**：fake fixture 触发 `Shell` tool → 因 `--approval-policy on-request` 进入 `awaiting_interaction` → harness POST `/interactions/{id}` 接受 → tool 执行 → assistant 完成。
  - **P0**：拒绝路径：harness POST `approved=false` → tool 返回拒绝结果 → assistant 看到拒绝。
  - **P0**：`apply_to_future` 缓存：第二次同 tool 自动通过（`accept_all` 触发 `approve_session` cache key，需要两轮使用相同 command + cwd 才能命中）。
  - **P1**：多个 pending interaction 并发的 ID 路由。降级原因：fake provider 每轮只发一个 tool_call，单 turn 并发需要扩展 fixture schema 支持 parallel tool calls，与 §5.13 之外的工作量不相称；P0 三条已经覆盖 ID 寻址正确性（每轮单 pending 也是 ID 路由的最小用例）。
- **优先级**：P0（前三条），P1（并发降级）。
- **测试位置**：**L2 新增** `tests/code_ui_approval_flow_test.rs`，扩展 `code_session.rs` 的 `respond_interaction()` helper。
- **AI 落地提示**：`CodeSession` 已有 `browser_respond_interaction()` 和 `respond_interaction_expect_error()`，但缺少通用的 `respond_interaction(id, approved, selected_option, apply_to_future)`。建议新增：
  ```rust
  pub fn respond_interaction(
      &self,
      interaction_id: &str,
      approved: bool,
      selected_option: Option<&str>,
      apply_to_future: bool,
  ) -> Result<(StatusCode, Value)>;
  ```

### 5.12 Orchestrator 闸门与契约

- **现状 ⚠️**：[src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) 有 `execute_implementation_task_carries_review_rejection_into_agent_output`（最近新增）；[src/internal/ai/orchestrator/gate.rs](../../src/internal/ai/orchestrator/gate.rs) 改动未充分测试。
- **缺口**：
  - 工具循环 `max_turns`（48 / 32）边界：临界 47 轮成功、49 轮被截断且 reason 写入 `agent_output`。
  - Workspace 契约违规：fake fixture 写 workspace 外路径 → 被 gate 拦截，`agent_output` 含 violation。
  - 网络策略冲突：tool 声明 `requires_network=true` 但 `--network-access deny` → 拒绝。
  - FUSE 不可用回退到 copy backend 的契约一致性。
- **优先级**：P0。
- **测试位置**：**L1 扩展** `executor.rs` 已有的 `mod tests`，每条违规一条 fake mock。
- **AI 落地提示**：executor `mod tests` 中已有 `MockModel` 和 `ConditionalModel`。新增 max_turns 边界测试时，可构造一个总是返回 tool_call 的 mock，计数调用次数，断言第 48 轮仍成功、第 49 轮返回截断消息。

### 5.13 Codex 旁路运行时

- **现状 ⚠️**：仅 [tests/code_codex_default_tui_test.rs](../../tests/code_codex_default_tui_test.rs) 静态守卫；运行时无端到端。
- **缺口**：
  - 启动一个本地伪 Codex app-server（mock WebSocket 服务），`libra code --provider codex --codex-port <port>` 连上 → 收 notification → 落到 `.libra/objects/` 与 history index。
  - Codex plan-first（`--plan-mode true`）：在 plan approve 之前拒绝执行。
  - Codex 断开重连。
- **优先级**：P1。
- **测试位置**：**L2 新增** `tests/code_codex_runtime_test.rs` + 简易 mock WS server（用 `tokio-tungstenite` 接受连接 + 回放固定脚本）。
- **AI 落地提示**：`tokio-tungstenite` 已在 `Cargo.toml`。mock WS server 可仿照 `tests/helpers/mock_codex.rs` 的 `MockCodexServer` 模式。注意 Codex 路径使用 JSON-RPC over WebSocket，需按 Codex 协议握手。

### 5.14 MCP 服务双入口一致

- **现状 ✅**：[tests/mcp_integration_test.rs](../../tests/mcp_integration_test.rs)、[tests/e2e_mcp_flow.rs](../../tests/e2e_mcp_flow.rs) 覆盖资源/工具列表与 initialize 握手。
- **缺口**（Code 路径）：
  - `libra code` 启动后 MCP server 的端口号写入 `--control-info-file`（automation 发现）。
  - `--stdio` 模式下 web + tui 不启的互斥。
  - MCP `tools/call` 与 web `/messages` 双入口对同一 thread 的状态一致性（一个写，另一个 SSE 看到）。
- **优先级**：P1。
- **测试位置**：**L1 新增** `tests/code_mcp_dual_entry_test.rs` 跑双入口同步。

### 5.15 Diagnostics 与 Secret Redaction

- **现状 ✅**：[tests/diagnostics_redaction_test.rs](../../tests/diagnostics_redaction_test.rs)、[tests/redaction_contract_test.rs](../../tests/redaction_contract_test.rs) 在 AI 层面覆盖。
- **缺口**：
  - HTTP `/api/code/diagnostics` 返回值经 `SecretRedactor::default_runtime()`。
  - `LIBRA_LOG_FILE` 中 secret-like path 被脱敏（来自 `security_cases.json`）。
  - Audit JSON 在 redaction 后无 token 原文。
- **优先级**：P1。
- **测试位置**：**L2 新增** `code_ui_remote_security_matrix.rs` runner（已有 JSON）。

### 5.16 Session / History 持久化(Code 路径)

- **现状 ✅**：[tests/ai_session_jsonl_test.rs](../../tests/ai_session_jsonl_test.rs)、[tests/ai_storage_flow_test.rs](../../tests/ai_storage_flow_test.rs) 覆盖底层。
- **缺口**（Code 路径）：
  - `--resume <thread_id>` 启动时 transcript 完整恢复，`status=idle`。
  - 中途 kill `libra code`（SIGTERM）→ 重启 `--resume` → 最新 turn 继续。
  - 并发同一 thread 的两个 `libra code` 实例（应被 control-instance lock 拦截，见 5.6）。
- **优先级**：P1。
- **测试位置**：**L2 新增** `tests/code_resume_test.rs`。

### 5.17 并发边界 / 体积限制

- **现状 ⚠️**：`code_ui_scenarios.rs` 覆盖 256 KiB 边界与第二浏览器 conflict。
- **缺口**（`state_cases.json` 已写 8 条）：
  - 两线程并发 attach → 一胜一负（200 / 409）。
  - thinking 中二次 submit → 409 `SESSION_BUSY`。
  - cancel idle → 409 `SESSION_BUSY` 且文档化。
  - 257 KiB / 1 MiB 拒绝且不挂死。
  - streaming 进行中 detach → assistant 状态收敛到 idle 而非死锁。
- **优先级**：P1。
- **测试位置**：**L2 新增** `tests/code_ui_remote_state_matrix.rs` runner（JSON 已就位）。

### 5.18 性能与稳定性 smoke

- **现状 ❌**。
- **缺口**：
  - 100k 行 transcript 下 `/session` 序列化耗时上限（如 < 200 ms）。
  - SSE 长连接 1 小时不漏 event（缩比版：5 分钟 + 1000 events）。
  - 10 并发 `/threads` 查询不死锁。
- **优先级**：P2。
- **测试位置**：**L2 新增** `tests/code_ui_perf_smoke_test.rs`，`#[ignore]` + `LIBRA_RUN_PERF=1` 时跑。

### 5.19 真实模型生成

- **现状 ❌**：[tests/data/code_ui_remote/model_generation_cases.json](../../tests/data/code_ui_remote/model_generation_cases.json) 2 条 P0 case 已写，runner 缺；`.env.test` 路由缺。
- **缺口**：见 §6.4 Wave 4 详细规约。
- **优先级**：P0（条件：CI nightly + `LIBRA_RUN_LIVE=1`）。
- **测试位置**：**L3 新增** `tests/code_ui_remote_model_generation_matrix.rs` + `harness::matrix` 扩 `ProviderRef::ModelFromEnvFile` 分支。

### 5.20 错误码契约同步

- **现状 ⚠️**：错误码散落在 `code_ui.rs` 的 `CodeUiApiError` 与文档中。
- **缺口**：单一 source-of-truth + 测试断言。
- **优先级**：P1。
- **测试位置**：**L0 新增** `code_ui::error::tests` 列出全部 ErrorCode → status 的映射，并在 [docs/automation/local-tui-control.md](../automation/local-tui-control.md) 加注脚同步。
- **AI 落地提示**：`CodeUiApiError` 是 `pub struct` 含 `status: u16, code: String, message: String`。建议新增一个 `code_ui_error_codes()` 函数返回 `Vec<(&'static str, StatusCode)>`，然后在 `mod tests` 中遍历断言每个 code 与 status 的对应关系。新增 error code 时开发者必须同步更新该列表，否则测试编译失败。

---

## L2 远端矩阵

本节是 §5 中 5.4 / 5.5 / 5.10 / 5.15 / 5.17 / 5.19 的合并实施细节。原“L2 远端测试落地”方案完整保留在此。

### 6.1 可行性判断

**可直接落地：**

- controller attach 的 missing/invalid control token、invalid kind、conflict、detach、stale token、same client renewal。
- SSE initial replay、status_changed、session_updated、controller_changed、双订阅者、断线后重连读取最新 snapshot。
- 通过 `/api/code/messages` 调用 Code 服务，输入完整代码生成请求。确定性回归由 fake provider 驱动 `apply_patch`；模型能力回归默认使用仓库根目录 `.env.test` 中的 DeepSeek `deepseek-v4-flash` 配置，并开启 thinking/high reasoning。
- 并发 attach 一胜一负、busy submit、256 KiB 边界、1 MiB drain 不挂死、cancel idle 返回文档化错误。
- diagnostics redaction、`/threads` query validation/clamp、route 级 non-loopback 拒绝顺序。

**需要小改造后落地：**

- lease expiry L2：默认 TTL 是 120s，必须加 test-only TTL override（**已落地**于 [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 的 `test_lease_duration_override()`）。
- `--control observe` L2：当前 `CodeSession::spawn` 固定传 `--control write`，需要给 `CodeSessionOptions` 加控制模式（**已存在** `with_control_observe()`）。
- SSE blocking client：当前 harness 没有 `/events` reader，需要新增 `event_stream.rs`（**唯一硬性阻塞项**）。
- 模型能力测试：当前 `CodeSession::spawn` 固定 `--provider fake --fake-fixture ...`；需要新增 provider 配置，支持默认从 `.env.test` 启动 DeepSeek `deepseek-v4-flash`。

**建议推迟或降级：**

- “lagged stream 不死”跨进程测试容易依赖 socket backpressure 和 broadcast polling 时序，放到 P2 或以 in-process 单元测试覆盖解析器。
- “cancel during executing tool phase”若要稳定命中 `executing_tool`，需要 fake fixture 支持一次性/序列化响应，或选择稳定长耗时 tool。当前先用数据文件记录候选，实现时若不稳定则降级到 L1。

### 6.2 数据驱动设计

新增 [tests/harness/matrix.rs](../../tests/harness/matrix.rs)（**已就位**），从 `tests/data/code_ui_remote/*.json` 读取 case。结构：

```rust
#[derive(Deserialize)]
pub struct RemoteCase {
    pub name: String,
    pub priority: Priority,
    pub provider: Option<ProviderRef>,
    pub fixture: Option<FixtureRef>,
    pub options: CaseOptions,
    pub steps: Vec<Step>,
}
```

关键点：

- `fixture.path` 支持 repo-root 相对路径，例如 `tests/fixtures/code_ui/basic_chat.json` 或 `tests/data/code_ui_remote/provider_fixtures/streaming_chat.json`；仅 `provider.mode == "fake"` 时使用。
- `provider.mode == "model_from_env_file"` 时默认读取仓库根目录 `.env.test`，并从 `LIBRA_CODE_TEST_PROVIDER` / `LIBRA_CODE_TEST_MODEL` 解析 CLI provider/model。真实模型矩阵默认固定为 `deepseek` / `deepseek-v4-flash`，启动时额外传 `--deepseek-thinking enabled --deepseek-reasoning-effort high`，再把同一个 `.env.test` 传给 `libra code --env-file` 作为 provider 凭证来源。
- 每个 JSON case 仍映射到一个 `#[test] #[serial]` 函数，函数里按 name 查 case 并执行，保证 cargo 输出有明确失败定位。
- `matrix::run(case)` 失败时统一拼接 `scenario '<name>' step '<step>' failed` 和 `CodeSession::debug_context()`。
- 测试数据只描述动作和断言，不编码 Rust 闭包。复杂谓词通过枚举名实现，例如 `controller_kind_eq`、`transcript_contains`、`event_seen`。

### 6.3 必要代码改动

#### Harness

修改 [tests/harness/code_session.rs](../../tests/harness/code_session.rs)：

- `CodeSessionOptions` 新增：
  - `provider: CodeSessionProvider`，默认 `Fake { fixture }`。
  - `with_model_from_env_test()`：默认读取 repo-root `.env.test`，解析 `LIBRA_CODE_TEST_PROVIDER` / `LIBRA_CODE_TEST_MODEL`，spawn 时传 `--env-file <repo>/.env.test --provider <provider> --model <model> --deepseek-thinking enabled --deepseek-reasoning-effort high`。
  - `control_write: bool`，默认 `true`；`with_control_observe()` 让 spawn 不传 `--control write`（**已存在**）。
  - `lease_duration_ms: Option<u64>`；spawn 时设置 `LIBRA_CODE_LEASE_DURATION_MS`（**已存在**）。
  - `extra_env: Vec<(String, String)>`；用于 diagnostics redaction / audit log 场景，并且要在 harness 默认 env 之后应用，保证 case 能覆盖 `LIBRA_LOG_FILE`。
- `.env.test` 处理规则：
  - 不读取、不打印 secret 值到 `debug_context()`。
  - 缺少 `.env.test`、`LIBRA_CODE_TEST_PROVIDER` 或 `LIBRA_CODE_TEST_MODEL` 时，模型矩阵应 fail fast，错误说明需要创建/补齐 `.env.test`。
  - 默认要求 `LIBRA_CODE_TEST_PROVIDER=deepseek` 且 `LIBRA_CODE_TEST_MODEL=deepseek-v4-flash`；若不是 deepseek，runner 应失败并提示该矩阵固定验证 DeepSeek thinking/high reasoning 模式。
  - `.env.test` 中还应包含 DeepSeek 凭证，例如 `DEEPSEEK_API_KEY` 和可选 DeepSeek base URL。
  - 模型矩阵默认 `--context dev --approval-policy never`，避免 classifier 额外消耗模型调用，并保证 workspace 内 `apply_patch` 不需要人工确认。
- 新增通用 HTTP helper（以下方法当前**不存在**，需新增）：
  - `attach_automation_expect(...) -> Result<(StatusCode, Value)>`
  - `attach_kind_expect(kind, client_id, control_token_mode) -> Result<(StatusCode, Value)>`
  - `detach_with_token(client_id, controller_token) -> Result<(StatusCode, Value)>`
  - `submit_with_token(text, controller_token) -> Result<(StatusCode, Value)>`
  - `cancel_expect() -> Result<(StatusCode, Value)>`
  - `get_threads(limit, offset) -> Result<(StatusCode, Value)>`
  - `diagnostics_raw_text() -> Result<String>`
  - `libra_log_text() -> Result<String>`
  - `read_repo_file(path) -> Result<String>`
  - `run_repo_command(args, timeout) -> Result<Output>`
  - `open_event_stream(timeout) -> Result<EventStream>`
  - `respond_interaction(id, response) -> Result<(StatusCode, Value)>`

新增 `tests/harness/event_stream.rs`：

- 使用独立 `reqwest::blocking::Client`。不要复用全局 5s total timeout，而是设置 per-read timeout。
- 手工解析 SSE block，只识别 `event:` 和 `data:`。
- 单行上限 1 MiB，`next()` 超时返回 `Ok(None)`，EOF 返回明确错误。
- 建议对外暴露：
  ```rust
  pub struct SseEvent { pub event: String, pub data: String }
  pub struct EventStream { /* private */ }
  impl EventStream {
      pub fn open(client: &Client, url: &str, bearer_token: Option<&str>) -> Result<Self>;
      pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SseEvent>>;
  }
  ```

新增 / 扩展 [tests/harness/matrix.rs](../../tests/harness/matrix.rs)：

- 读取 `tests/data/code_ui_remote/*.json`。
- 分发 step。
- 把 `TokenSource::{current, stale, forged, none}` 和上一轮 attach/detach 的 token 状态保存在 runner context 中（**已实现**）。
- 新增 `Step::OpenEventStream` / `Step::WaitEvent` / `Step::RespondInteraction`。
- 新增 `ProviderRef::ModelFromEnvFile`。
- 新增 assertion 谓词：`event_seen`、`transcript_contains`、`file_exists`、`repo_command_exit_0`。

#### Runtime 短 TTL（已落地）

[src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 已新增 `CodeUiRuntimeOptions { browser_write_enabled, automation_write_enabled, initial_controller, lease_duration: Option<chrono::Duration> }`、`CodeUiRuntimeHandle::build_with_options(adapter, options) -> Arc<Self>`，保留 `build()` / `build_with_control()` 委托；`lease_duration == None` 时继续用 `DEFAULT_BROWSER_CONTROLLER_LEASE_SECS`。

[src/command/code.rs](../../src/command/code.rs) 已在 `cfg(feature = "test-provider")` 下解析 `LIBRA_CODE_LEASE_DURATION_MS`，仅接受正整数毫秒，非法值让启动失败；非 `test-provider` 构建不读该 env var。

#### Inline loopback tests

在 [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 的现有 `mod tests` 追加 route 级测试即可：

- `GET /api/code/session` 带 `ConnectInfo(192.0.2.10:4000)` 返回 403 / `LOOPBACK_REQUIRED`。
- `POST /api/code/messages` 同样先返回 `LOOPBACK_REQUIRED`，证明 loopback 校验先于 body/controller token 校验。

不需要修改 `build_router` 可见性。参考已有 `loopback_api_request_rejects_remote_clients` 模式，但需要构造完整 `Request` 并过 router。

### 6.4 L2 远端 Wave 0–5

#### Wave 0：基础设施

1. 新增 `tests/data/code_ui_remote/` 数据目录（**已存在**）。
2. 新增 `event_stream.rs` 和 `matrix.rs`（**matrix.rs 已就位，event_stream.rs 待补**）。
3. 扩展 `CodeSessionOptions` 和通用 HTTP helper。
4. 增加 test-only lease TTL override（**已落地**）。

验收：

```bash
cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1
cargo test --lib code_ui
```

#### Wave 1：Controller Lease

新增 / 扩展 [tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs)（**已存在，需 macro 扩到 9/9**），读取 [tests/data/code_ui_remote/lease_cases.json](../../tests/data/code_ui_remote/lease_cases.json)。

首批 P0 case：

- automation attach 成功并返回 `controllerToken` / 未来 `leaseExpiresAt`。
- automation attach 缺 control token 返回 `MISSING_CONTROL_TOKEN`。
- automation attach 错 control token 返回 `INVALID_CONTROL_TOKEN`。
- invalid kind 返回 `INVALID_CONTROLLER_KIND`。
- 同 client 续约延长 expiry。
- 不同 client attach 返回 `CONTROLLER_CONFLICT`。
- detach 后 controller 回到 `tui` 或 `none`。
- wrong/stale token 不能 detach/submit。
- 短 TTL 过期后旧 token 失效，新 client 可 attach。

**AI 落地提示**：该文件当前有 4 条 `lease_case!` 调用，只需在文件末尾追加剩余 5 条。JSON 中已有完整 step 定义，无需改 JSON。

#### Wave 2：SSE

新增 `tests/code_ui_remote_sse_matrix.rs`，读取 [tests/data/code_ui_remote/sse_cases.json](../../tests/data/code_ui_remote/sse_cases.json)。

首批 P0/P1 case：

- initial connect replay `session_updated` 全量 snapshot。
- submit 后看到 `status_changed` 且 status 为 `thinking`。
- assistant 完成后看到 `session_updated`，最终 status 为 `idle`。
- attach/detach 触发 `controller_changed`。
- 两个订阅者都收到 submit 产生的状态变化。
- 断开 SSE 后 submit，重连 initial replay 包含最新 transcript。
- streaming fixture 的 transcript 内容单调增长。

**AI 落地提示**：`event_stream.rs` 必须先完成。`matrix.rs` 需新增 `Step::OpenEventStream { name, timeout_ms }` 和 `Step::WaitEvent { name, event, timeout_ms, assertions }`。`CodeSession` 需新增 `open_event_stream(&self) -> Result<EventStream>`，URL 为 `self.url("/events")`。

#### Wave 3：Code 服务生成代码并测试

新增 `tests/code_ui_remote_generation_matrix.rs`，读取 [tests/data/code_ui_remote/generation_cases.json](../../tests/data/code_ui_remote/generation_cases.json)。

这组 case 必须覆盖完整闭环，而不是只断言 transcript：

1. 启动真实 `libra code` PTY session。
2. automation attach 后调用 `POST /api/code/messages`，输入完整的代码生成请求，例如 `/chat generate-code-greeting`。
3. fake provider 返回 `apply_patch` tool call，写入一个完整、可独立测试的源文件。
4. 等待 snapshot 回到 `idle`，确认 transcript 含最终助手文本。
5. harness 从临时 repo 读取生成文件，断言关键源码片段存在。
6. harness 在临时 repo 内运行验证命令，例如 `rustc --test generated_greeting.rs -o generated_greeting_test && ./generated_greeting_test`，断言 exit 0。

首批 P0/P1 case：

- automation 通过 Code 服务生成 `generated_greeting.rs`，文件包含函数和单测，`rustc --test` 通过。
- SSE 订阅期间触发同一生成请求，至少观察到 `executing_tool` 或 `session_updated` 中的 tool-call/patch 结果，最终 replay 含生成完成文本。
- 失败分支：provider 生成非法 patch 时，Code 服务不挂死，最终 snapshot 为 `error` 或 transcript 含可诊断错误，临时 repo 不出现半写文件。

注意：

- 请求文本使用 `/chat ...`，避免 plain message 被计划工作流接管。
- 该矩阵验证的是 Code 服务和 tool loop 的集成，不是 Rust 编译器本身。生成代码应保持小而确定，不要依赖 Cargo 项目结构或外部网络。
- fake fixture 需要一轮 tool call 后返回最终文本。当前 fake provider 会在 tool result 后把最新 user text 视为空字符串，因此 fixture 可用 `{"match":{"equals":""},"type":"text"}` 作为 follow-up 响应。

#### Wave 4：真实模型生成能力

新增 `tests/code_ui_remote_model_generation_matrix.rs`，读取 [tests/data/code_ui_remote/model_generation_cases.json](../../tests/data/code_ui_remote/model_generation_cases.json)。

该矩阵不是 fake fixture 回归，而是验证真实模型是否能通过 Code 服务完成代码生成：

1. runner 默认使用仓库根目录 `.env.test`。
2. `.env.test` 必须至少提供：

```bash
LIBRA_CODE_TEST_PROVIDER=deepseek
LIBRA_CODE_TEST_MODEL=deepseek-v4-flash
DEEPSEEK_API_KEY=...
```

该矩阵固定使用 DeepSeek，不接受其它 provider。其它 provider 可另建可选模型矩阵，避免默认验收语义漂移。

3. harness 启动命令形态：

```bash
libra code --env-file .env.test --provider "$LIBRA_CODE_TEST_PROVIDER" \
  --model "$LIBRA_CODE_TEST_MODEL" --context dev --approval-policy never \
  --deepseek-thinking enabled --deepseek-reasoning-effort high \
  --control write --port 0 --mcp-port 0
```

4. automation 通过 `/api/code/messages` 输入完整代码生成任务。
5. 断言模型实际使用 `apply_patch` 或等价写入路径产出文件。
6. 运行本地验证命令，例如 `rustc --test generated_model_greeting.rs -o generated_model_greeting_test && ./generated_model_greeting_test`。
7. 对较完整的 Cargo 项目生成任务，还要运行 `cargo fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all`，并额外检查 CLI 输出、空 `$HOME/.linked/config`、Rust 2024 edition、无嵌套 VCS 元数据和 `.libraignore` 中的 `target/`。

首批 P0 case：

- `model_generation_code_service_creates_tested_rust_file`：使用真实模型生成 `generated_model_greeting.rs`，要求文件包含 `pub fn greeting` 和单测，并通过 `rustc --test`。
- `model_generation_linked_cargo_cli_project_passes_quality_gates`：使用英文测试说明驱动真实模型生成 `linked` Cargo CLI 项目。该项目必须通过 `cargo new linked --vcs none` 初始化，使用 `clap` 实现 `code` / `cloud` / `backup` 三个子命令，从 `$HOME/.linked/config` 读取空配置，使用 Rust 2024，在 `.libraignore` 中忽略 Rust 编译目录，并通过 fmt、clippy、test 和命令输出验收。

验收边界：

- 该测试允许比 fake 矩阵更慢，单文件生成 case timeout 建议 120s，完整 Cargo 项目生成 case timeout 建议 180s。
- 失败输出只打印 provider/model、case/step、snapshot/log tail；不得打印 `.env.test` 原文或任何 `*_API_KEY`。
- 如果 provider 需要网络但运行环境无网络，测试应失败并显示 provider bootstrap/HTTP 错误，不要静默跳过。

#### Wave 5：State 和 Security

新增：

- `tests/code_ui_remote_state_matrix.rs`，读取 [tests/data/code_ui_remote/state_cases.json](../../tests/data/code_ui_remote/state_cases.json)。
- `tests/code_ui_remote_security_matrix.rs`，读取 [tests/data/code_ui_remote/security_cases.json](../../tests/data/code_ui_remote/security_cases.json)。

首批 P1 case：

- 两线程并发 attach，一个 200，一个 409。
- thinking 中二次 submit 返回 409 / `SESSION_BUSY`。
- cancel idle 返回 409 / `SESSION_BUSY` 并写入文档。
- 256 KiB body 被接受，257 KiB/1 MiB 被 413 拒绝且不挂死。
- diagnostics 不包含 control/controller token，`LIBRA_LOG_FILE` 中 secret-like path 被脱敏。
- `--control observe` 下 automation attach 返回 403 / `CONTROL_DISABLED`。
- `/threads?limit=abc` 返回 400 / `INVALID_QUERY_PARAM`；大 limit clamp 到 200。
- control audit log 不包含 secret-like client id 原文。

---

## 覆盖矩阵

| # | 功能面 | L0 | L1 | L2 | L3 | 现状 | 优先级 |
|---|---|---|---|---|---|---|---|
| 5.1 | CLI 解析 / 模式分发 | – | new | – | – | ⚠️ | P0 |
| 5.2 | Provider boot + flag passthrough | – | new | – | – | ⚠️ | P1 |
| 5.3 | HTTP 读路由 + loopback gate | new | new | matrix | – | ⚠️ | P0 |
| 5.4 | HTTP 写路由 + lease 状态机 | – | exist | matrix(扩) | – | ⚠️ | P0 |
| 5.5 | SSE | new | – | new matrix | – | ❌ | P0 |
| 5.6 | Local TUI Control 锁/审计 | new | – | exist+扩 | – | ⚠️ | P0/P1 |
| 5.7 | Browser Control | – | – | exist+扩 | – | ✅⚠️ | P1 |
| 5.8 | TUI 渲染快照 | – | new | – | – | ❌ | P1 |
| 5.9 | Tool ACL / context / policy | – | new | – | – | ⚠️ | P0 |
| 5.10 | Apply-Patch 生成(fake) | – | – | new matrix | – | ❌ | P0 |
| 5.11 | Approval / Interaction E2E | – | – | new | – | ❌ | P0 |
| 5.12 | Orchestrator gate / max_turns | – | exist+扩 | – | – | ⚠️ | P0 |
| 5.13 | Codex 旁路运行时 | – | – | new(mock WS) | – | ⚠️ | P1 |
| 5.14 | MCP 双入口一致 | – | new | exist | – | ⚠️ | P1 |
| 5.15 | Diagnostics redaction | – | exist | new matrix | – | ✅⚠️ | P1 |
| 5.16 | Session resume / kill | – | – | new | – | ❌ | P1 |
| 5.17 | 并发 / size limits | – | – | new matrix | – | ⚠️ | P1 |
| 5.18 | 性能 smoke | – | – | new(ignore) | – | ❌ | P2 |
| 5.19 | 真实模型生成 | – | – | – | new matrix | ❌ | P0(nightly) |
| 5.20 | 错误码契约 | new | – | – | – | ⚠️ | P1 |

---

## 关键文件路径

### 待新增 — 基础设施

- `tests/harness/event_stream.rs`（**新建，P0 阻塞项**）
- 扩展 [tests/harness/code_session.rs](../../tests/harness/code_session.rs)：`with_model_from_env_test()`、`respond_interaction()`、`open_event_stream()`、`get_threads()`、`diagnostics_raw_text()`、`libra_log_text()`、`read_repo_file()`、`run_repo_command()`（**扩展现有文件**）
- 扩展 [tests/harness/matrix.rs](../../tests/harness/matrix.rs)：`ProviderRef::ModelFromEnvFile`、`Step::OpenEventStream` / `WaitEvent`、`Step::RespondInteraction`（**扩展现有文件**）

### 待新增 — 矩阵 runner（数据已就位）

- `tests/code_ui_remote_sse_matrix.rs`（读 `sse_cases.json`）
- `tests/code_ui_remote_generation_matrix.rs`（读 `generation_cases.json`）
- `tests/code_ui_remote_model_generation_matrix.rs`（读 `model_generation_cases.json`）
- `tests/code_ui_remote_state_matrix.rs`（读 `state_cases.json`）
- `tests/code_ui_remote_security_matrix.rs`（读 `security_cases.json`）

### 待新增 — 纵深面

- `tests/code_cli_dispatch_test.rs`
- `tests/code_provider_boot_test.rs`
- `tests/code_provider_flag_passthrough_test.rs`
- `tests/code_tool_acl_test.rs`
- `tests/code_ui_approval_flow_test.rs`
- `tests/code_codex_runtime_test.rs`
- `tests/code_mcp_dual_entry_test.rs`
- `tests/code_resume_test.rs`
- `tests/code_tui_render_test.rs` + `tests/snapshots/`（需先加 `insta` 依赖）
- `tests/code_ui_perf_smoke_test.rs`（`#[ignore]`）
- `tests/code_control_lock_test.rs`

### 待扩展 — 现有

- 扩展 [tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs) macro 覆盖到 9/9 case（**只需加 5 行**）。
- 扩展 [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) `mod tests`：route-level loopback 拒绝顺序（**参考已有 `code_write_body_limit_returns_json_error` 模式**）。
- 扩展 [src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) `mod tests`：max_turns / contract violation 矩阵（**参考已有 `MockModel`/`ConditionalModel` 模式**）。
- 扩展 [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) `mod tests`：interaction state machine、redaction 单测、错误码契约 L0（**参考已有 `RecordingCodeUiAdapter` 模式**）。
- 同步 [docs/automation/local-tui-control.md](../automation/local-tui-control.md) 错误码 / 取消行为注脚。

### 复用现有 fixture / data

- [tests/fixtures/code_ui/basic_chat.json](../../tests/fixtures/code_ui/basic_chat.json)、[tests/fixtures/code_ui/delayed_chat.json](../../tests/fixtures/code_ui/delayed_chat.json)
- [tests/data/code_ui_remote/](../../tests/data/code_ui_remote/) 下全部 JSON
- [tests/data/code_ui_remote/provider_fixtures/code_generation_apply_patch.json](../../tests/data/code_ui_remote/provider_fixtures/code_generation_apply_patch.json)、[code_generation_invalid_patch.json](../../tests/data/code_ui_remote/provider_fixtures/code_generation_invalid_patch.json)、[streaming_chat.json](../../tests/data/code_ui_remote/provider_fixtures/streaming_chat.json)、[tool_call_chat.json](../../tests/data/code_ui_remote/provider_fixtures/tool_call_chat.json)（按 [tests/data/code_ui_remote/README.md](../../tests/data/code_ui_remote/README.md) 声明）

---

## 验证流程

按顺序执行：

```bash
# 格式 / 静态检查（CI 强制）
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings

# L0 + L1
cargo test --lib
cargo test --lib code_ui
cargo test --features test-provider --test code_cli_dispatch_test
cargo test --features test-provider --test code_provider_boot_test
cargo test --features test-provider --test code_provider_flag_passthrough_test
cargo test --features test-provider --test code_tool_acl_test
cargo test --features test-provider --test code_tui_render_test
cargo test --features test-provider --test code_mcp_dual_entry_test

# L2 PTY/HTTP（串行）
cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_lease_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_sse_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_generation_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_state_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_security_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_approval_flow_test -- --test-threads=1
cargo test --features test-provider --test code_resume_test -- --test-threads=1
cargo test --features test-provider --test code_codex_runtime_test -- --test-threads=1
cargo test --features test-provider --test code_control_lock_test -- --test-threads=1

# L3（nightly / LIBRA_RUN_LIVE=1）
LIBRA_RUN_LIVE=1 cargo test --features test-provider \
  --test code_ui_remote_model_generation_matrix -- --test-threads=1

# 性能 smoke（可选）
LIBRA_RUN_PERF=1 cargo test --features test-provider \
  --test code_ui_perf_smoke_test -- --ignored --test-threads=1

# 全量
cargo test --all --all-features
```

失败诊断验收（所有 L2 矩阵共用）：

- 人为改错一个 case 的 expected code。
- 运行对应 matrix test。
- 输出必须包含 case name、step name、`target/code-ui-scenarios/<case>/`、redacted control info、snapshot、`pty.log` tail、`libra.log` tail。
- 不得泄露 `.env.test` 原文或任何 `*_API_KEY`。

---

## 落地顺序（12 PR Wave roadmap）

每个 PR 保持 [tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs) 现有 smoke 全绿；不要在同一 PR 中迁移旧 smoke 进矩阵。

| PR | 主题 | 要点 | 可行性 | 阻塞项 |
|---|---|---|---|---|
| 1 | 基础设施 | 扩 `code_session.rs` + `matrix.rs`，**新增 `event_stream.rs`**，只让 1 条 SSE case 跑通做收敛 | 🟡 | `event_stream.rs` 必须先完成 |
| 2 | CLI / route P0 + 错误码契约 L0 | `code_cli_dispatch_test.rs` + `web/mod.rs` inline loopback test + `code_ui::error::tests` 单 source-of-truth | 🟢 | 无 |
| 3 | Lease 全量 | lease matrix 9/9 + `--control observe` + audit redaction L0 | 🟢 | 无（`with_control_observe` 已存在） |
| 4 | SSE 全量 | `code_ui_remote_sse_matrix.rs` + 7 条 case | 🟡 | 依赖 PR 1 的 `event_stream.rs` |
| 5 | 生成 fake | `code_ui_remote_generation_matrix.rs` + provider_fixtures + apply_patch + 失败分支 | 🟢 | 无 |
| 6 | Approval flow | `code_ui_approval_flow_test.rs` + `respond_interaction()` helper + accept / reject / `apply_to_future` 三条 P0 | 🟡 | 需新增 `respond_interaction` helper（并发 pending 已按 §5.11 P1 拆出，单 turn 单 pending 已覆盖 ID 路由最小集） |
| 7 | State / Security 矩阵 | 复用已就位 JSON；state busy/body/parallel/streaming + security diagnostics/threads/audit | 🟢 | 无 |
| 8 | Orchestrator gate / max_turns / Tool ACL | `executor.rs` mod tests 扩 + `code_tool_acl_test.rs` 矩阵 | 🟢 | 无 |
| 9 | Codex runtime + MCP 双入口 + resume | mock WS server + `code_codex_runtime_test.rs` + `code_mcp_dual_entry_test.rs` + `code_resume_test.rs` | 🟡 | 需手写 Codex JSON-RPC WS mock |
| 10 | TUI 快照 + provider boot/flag passthrough | `insta` 接入 + `httpmock` provider boot + flag passthrough | 🔴 | **必须先加 `insta` + `httpmock` 到 Cargo.toml** |
| 11 | Model generation L3 | `.env.test` 路由 + `code_ui_remote_model_generation_matrix.rs` + nightly CI 工作流 | 🟡 | 需 CI 配置 `DEEPSEEK_API_KEY` + `.env.test` |
| 12 | 性能 smoke + 错误码文档同步 | `code_ui_perf_smoke_test.rs`（`#[ignore]`）+ [docs/automation/local-tui-control.md](../automation/local-tui-control.md) 错误码注脚同步 | 🟢 | 无 |

每条 Wave 在 CI 上独立可门：失败仅阻塞当前 PR，不污染主干。

**建议的并行策略**：
- PR 1–3 可串行快速落地（基础设施 → CLI/错误码 → Lease）。
- PR 4–8 在 PR 1 合并后可并行启动（它们只依赖 `event_stream.rs` 和已有 harness）。
- PR 9–12 可独立并行，但 PR 10 必须等 Cargo.toml 审批通过。

---

## 决策与折中

- **不在同一矩阵里混真实/伪 provider**：model_generation 单独矩阵，文件名带 `model_` 前缀，避免误跑。
- **lagged SSE / cancel-during-tool 降级 P2**：跨进程稳定性差，改为 in-process 单测覆盖解析器/状态机即可。
- **TUI 渲染用 `TestBackend` + 快照**：不要起真实 PTY 验 ratatui buffer——成本高、抖动大；快照足够锁回归。
- **Codex 旁路用 mock WebSocket**：不在 CI 拉真 Codex 二进制；行为契约由本仓库定义即可。
- **不为 `--web` / `--stdio` 互斥单独写 L2**：L1 dispatch test 直接打 `Args::parse_from(...)` 验错误已足够。
- **真实模型矩阵固定 DeepSeek**：其它 provider 可另建可选矩阵，避免默认验收语义漂移。
- **lease TTL override 仅 `test-provider` 下生效**：生产构建不读 `LIBRA_CODE_LEASE_DURATION_MS`，默认 120s 行为不变。
- **优先使用已有测试模式**：新增测试应复用 `tests/command/mod.rs` 的隔离习惯、`tests/harness/` 的 PTY 模式、`src/*/mod tests` 的 mock adapter 模式，不要发明新范式。

---

## 落地完成判定

执行 PR 12 完成后：

1. `cargo test --features test-provider` 在主干 CI 全绿，包含 lease/SSE/state/security/generation/approval/resume/codex/MCP-dual/control-lock 全部矩阵。
2. nightly CI 跑 `code_ui_remote_model_generation_matrix` 至少连续 5 天通过率 ≥ 90%。
3. 故意改坏任一 P0 case 的 expected，对应矩阵失败输出含 case/step/snapshot/log tail，**不含 secret**。
4. [docs/improvement/test.md](test.md)（本文件）与 [docs/automation/local-tui-control.md](../automation/local-tui-control.md) 错误码列表与代码 `CodeUiApiError` 字符串字面量一致——由错误码契约 L0 测试（PR 2 + Wave 12 `code_ui_error_code_listing_matches_authoritative_doc`）保证。
5. 覆盖矩阵第 7 节中 P0 行全部从 ⚠️/❌ 转为 ✅；P1 行至少 70% 转为 ✅。

## 当前 Wave 状态（2026-05-10 同步）

Wave 1–9 + Wave 12 部分已完成；Wave 10 / 11 / 12 部分均按 Codex pass-1 review 拆为独立后续 PR：

| Wave | 状态 | 备注 |
|---|---|---|
| 1 | ✅ closed | 基础设施 + event_stream.rs + 1 条 SSE 回归 |
| 2 | ✅ closed | CLI dispatch + route loopback + 错误码契约 L0 |
| 3 | ✅ closed | Lease matrix 9/9 + observe-mode + audit redaction L0 |
| 4 | ✅ closed | SSE matrix 7/7 |
| 5 | ✅ closed | Generation matrix 3/3（apply_patch fake fixture） |
| 6 | ✅ closed | Approval matrix 3/3（accept / reject / `apply_to_future`），并发 pending 已按 §5.11 P1 拆出 |
| 7 | ✅ closed | State 7/7 + Security 6/6 + diagnostics SecretRedactor wire-up |
| 8 | ✅ closed | Tool ACL 6/6（含 MCP bridge 前缀防退化） |
| 9 | ⚠️ partial | `--resume` CLI 表面 3 条（unknown UUID、unknown 非 UUID 字符串、happy-path 跨进程恢复 transcript）；§5.16 closure criterion ✅；§5.13 Codex runtime mock + §5.14 MCP dual entry 仍未交付（roadmap-sized）；§5.16 SIGTERM-mid-turn 仍未交付 |
| 10 | ⚠️ partial | §5.2 provider boot 6/7 reachable providers 已落地（DeepSeek+flags、OpenAI、Anthropic、Kimi+flags、Ollama+flag、Zhipu）经 `tests/helpers/mock_provider_server.rs`（无新增 dev-dep，复用 `axum`）；Gemini 因 `with_base_url` 未暴露需要 runtime 改动后跟进；Codex 见 §5.13；§5.8 TUI 快照仍 deferred（需 `insta` 或手写 buffer 断言） |
| 11 | ✅ closed | `tests/harness/matrix.rs` 已加 `ProviderSpec::ModelFromEnvFile` + DeepSeek thinking/high-reasoning 自动注入；`tests/code_ui_remote_model_generation_matrix.rs` 在 `LIBRA_RUN_LIVE=1` 下真正调用矩阵；`.github/workflows/model-generation-nightly.yml` 提供每日 cron + `workflow_dispatch`，需 maintainer 配置 `DEEPSEEK_API_KEY` secret 后才会运行；regression L0 测试（`build_session_options_for_*_provider_*`）锁定 DeepSeek 旗标注入逻辑 |
| 12 | ✅ closed | 错误码 doc/code sync L0 测试 + perf smoke 3 条（10 并发 `/threads`、100k transcript snapshot 序列化 < 500ms 可由 `LIBRA_PERF_CEILING_MS` 调整、SSE broadcast 1 000 events monotonic seq L1）`#[ignore]` + `LIBRA_RUN_PERF=1`；唯一 deferred 项是 1-小时真网 SSE soak（独立 nightly job） |

落地完成判定的全部门只有在 Wave 9 §5.13/§5.14、Wave 10 §5.8 + Gemini boot smoke 都补齐之后才算 PASS（Wave 9 §5.16 happy-path、Wave 10 §5.2 主体 6/7 provider 已闭合）。Wave 11 已具备完整工作流（harness wiring + 每日 nightly + L0 regression）；剩余条件（5 天连续 ≥ 90% 通过率）依赖 maintainer 配置 `DEEPSEEK_API_KEY` 并等待 5 个 nightly run。Wave 12 perf smoke 现含 1k events SSE broadcast monotonic 验证；1-小时真网 soak 需独立 nightly job。当前仓库状态对应"基础矩阵已落地 + Wave 11/12 已 wire + Wave 9 / 10 部分 deferred"。
