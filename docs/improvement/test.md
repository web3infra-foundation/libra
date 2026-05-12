# Libra Code 完整测试范围规划（AI 可落地版）

## 概述

`libra code` 是仓库目前最复杂的子命令：一条入口同时拉起 TUI、HTTP/SSE Web 服务、MCP 服务、AI Agent 工具循环、Codex 旁路、Local TUI Automation Control。当前测试侧已经从最初的 smoke 覆盖扩展到 L2 数据驱动矩阵、L3 live 模型 gate 与多条纵深面专项测试：

- L0 单元/契约：headless / projection / wire / codex 默认 TUI 守卫，以及 tool loop / session jsonl / redaction / wire 序列化等通过 Code 路径间接受益的测试。
- L2 PTY+HTTP smoke：[tests/code_ui_scenarios.rs](../../tests/code_ui_scenarios.rs) 已覆盖 attach/detach/cancel/conflict/oversize 等 13 条场景。
- L2 数据驱动：[tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs) 已覆盖 10 条 lease case；SSE/state/security/generation/approval/model_generation 均已有 Rust runner。
- L3 真实模型：[tests/code_ui_remote_model_generation_matrix.rs](../../tests/code_ui_remote_model_generation_matrix.rs) 已接入 `libra code` 服务路径，默认由 `.env.test` + `LIBRA_RUN_LIVE=1` gate 驱动 DeepSeek live case；nightly 工作流仍依赖 maintainer 配置 `DEEPSEEK_API_KEY` 后收集连续稳定性。

本文目标：给出整个 `libra code` 功能的端到端测试范围地图，并把历史 Wave 规划与当前实现状态对齐。第 5 节按“功能面 × 测试分层 × 现状/缺口/优先级”组织 20 项纵深面；第 6 节内嵌 L2 远端矩阵方案；第 7–10 节给出统一覆盖矩阵、文件清单、验证命令和 12 PR Wave roadmap。当前剩余项集中在 Wave 9 Codex plan approve / reconnect 与 Wave 12 长时 SSE soak；不要再把已落地的 harness/matrix 文件列为前置阻塞。

---

## 前置依赖与基线核查

### 2.1 已就位的基础设施（无需改动即可使用）

| 组件 | 状态 | 路径/说明 |
|---|---|---|
| 数据驱动矩阵骨架 | ✅ 就位 | `tests/harness/matrix.rs`：已定义 `CaseFile`、`Case`、`Step`（Attach/Detach/Submit/Sleep/WaitSnapshot/OpenEventStream/WaitEvent/RespondInteraction/ReadRepoFile/RunRepoCommand 等）、`AuthMode`、`TokenSource`、`TokenSlot`、断言求值器 |
| PTY Session Harness | ✅ 就位 | `tests/harness/code_session.rs`：`CodeSession::spawn()`、`snapshot()`、`matrix_attach/detach/submit()`、`open_event_stream()`、`respond_interaction()`、`read_repo_file()`、`run_repo_command()`、`with_control_observe()`、`with_lease_duration_ms()` |
| Lease TTL override | ✅ 就位 | `src/internal/ai/web/code_ui.rs` 有 `test_lease_duration_override()`；`src/command/code.rs` 在 `cfg(feature = "test-provider")` 下解析 `LIBRA_CODE_LEASE_DURATION_MS` |
| 全部 JSON 数据文件 | ✅ 就位 | `tests/data/code_ui_remote/` 下 6 个矩阵 JSON + `provider_fixtures/` 下 4 个 fake fixture |
| 现有 L2 smoke | ✅ 就位 | `tests/code_ui_scenarios.rs` 13 条 case（10 条 `#[cfg(feature = "test-provider")]` + 3 条 browser） |
| 现有 L2 lease | ✅ 就位 | `tests/code_ui_remote_lease_matrix.rs` 已覆盖 10 条 case（含 observe-mode automation attach 拒绝） |
| 现有 L2 远端矩阵 | ✅ 就位 | SSE 7/7、Generation 3/3、Approval 7/7、State 7/7、Security 6/6、Model generation 2/2 live-gated runner |
| Inline 网络层测试 | ✅ 就位 | `src/internal/ai/web/mod.rs` `mod tests` 已覆盖 loopback、control auth、body limit、audit、route-level loopback gate |
| Executor 单元测试 | ⚠️ 部分 | `src/internal/ai/orchestrator/executor.rs` `mod tests` 已有 mock model 和基础 tool loop 测试 |
| WS 库 | ✅ 就位 | `tokio-tungstenite = "0.29.0"` 已在 `Cargo.toml` `[dependencies]`，可用于 Codex mock WS |

### 2.2 依赖裁决与剩余前置条件

| 依赖 / 条件 | 影响 Wave | 当前裁决 | 备注 |
|---|---|---|---|
| `insta` | PR 10（TUI 快照） | 未采用 | 当前 TUI smoke 采用 inline buffer 断言；更复杂快照仍可作为 quick-follow 引入 |
| `httpmock` | PR 10（Provider boot） | 未采用 | Provider boot 复用 `axum` mock server，无需新增 dev-dependency |
| `event_stream.rs` | PR 1 / PR 4（SSE） | ✅ 已落地 | `tests/harness/event_stream.rs` 已提供 blocking SSE reader，并由 SSE matrix 使用 |
| `.env.test` + `DEEPSEEK_API_KEY` | PR 11（Model generation L3） | 运行条件 | live 矩阵 runner 已落地；nightly 稳定性指标依赖 maintainer 配置 secret 并等待 5 个 run |

> **AI 编码落地原则**：不要重复实现已落地的前置文件；后续变更应聚焦当前 Wave 状态表中仍标记 partial/deferred 的具体缺口。

### 2.3 可行性分级

| 分级 | 含义 | 包含项 |
|---|---|---|
| 🟢 可直接编码 | 文件/类型已存在，只需扩写或新增同结构文件 | TUI complex-state buffer 断言、executor 继续扩写 |
| 🟡 需新增中等复杂度组件 | 需要补齐 Codex plan approve / reconnect 编排，但已有明确 helper 可参考 | Codex plan approve gate、reconnect |
| 🔴 需外部运行条件 | 依赖 CI secret、长时间 soak 或 nightly 稳定性窗口 | Model generation 5-day pass-rate gate、1 小时真网 SSE soak |

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
| Loopback inline test | [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 的 `mod tests` 已能直接使用 private `code_router()` / `code_write_router()` | 不需要把 `build_router` 提升为 `pub(crate)`；route-level 403 已由 Wave 2 覆盖 |
| SSE lagged | `/events` 使用 `BroadcastStream` 后对 lagged error 静默丢弃，不会发 `event: lagged` | P0 不做强行制造 lag 的跨进程测试，只测重连和 initial replay |
| Lease TTL override | [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 已新增 `CodeUiRuntimeOptions` 与 `test_lease_duration_override()`，在 `cfg(feature = "test-provider")` 下读 `LIBRA_CODE_LEASE_DURATION_MS` | 已就位，矩阵直接消费即可 |
| Tool-loop max_turns | [src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) 已把 Implementation 24→48、Analysis 24→32 | 默认值与 48-turn 边界测试已新增 |

### 本次扩写新增覆盖面

| 维度 | 当前 | 扩写后 |
|---|---|---|
| CLI 模式 / 互斥 / 解析 | 隐式触发 | L1 dispatch 测试覆盖每条模式与互斥错误码 |
| Provider boot / flag passthrough | 7/7 reachable providers 已覆盖；missing-key / `--api-base` quick-follow 已由 `build_helper_missing_api_key_errors_name_canonical_env_vars` 与 `build_helper_honors_cli_api_base_for_deepseek` 锁定 | 无本地 quick-follow |
| HTTP 读路由 + loopback gate 顺序 | 已覆盖 | inline route test + security 矩阵 |
| HTTP 写路由 lease 矩阵 | 10/10 | 9 条原 lease case + observe-mode automation attach 拒绝 |
| SSE | 7/7 | event_stream harness + SSE matrix |
| Local TUI Control 锁 / 审计 | instance conflict + stale-PID takeover + custom-path security matrix + audit redaction L0 | 无本地 quick-follow |
| TUI 渲染 | TestBackend / inline buffer smoke 已覆盖复杂状态 | 空 transcript、assistant delta、滚动窗口、approval prompt、retry error 与退出键契约均已覆盖；当前未引入 `insta` |
| Tool ACL × context × policy | 6/6 | 含 MCP bridge 前缀防退化 |
| Apply-Patch 文件生成(fake) | 3/3 | generation_cases.json 全量 + 失败分支 |
| Approval / Interaction E2E | 7/7 | accept/reject/apply_to_future + `never` / `on-failure` / `untrusted` / `allow-all`；并发 pending 仍按 P1 拆出 |
| Orchestrator gate 边界 | review rejection + max_turns 边界 + workspace/FUSE/network contract tests | 无本地 quick-follow |
| Codex 旁路运行时 | 静态守卫 | mock WS app-server + plan-first 拦截 |
| MCP 双入口一致 | done | control.json mcpUrl + --stdio mutex + dual-reachability smoke + web `/messages` → SSE/MCP observe + MCP `create_task` → web SSE observe |
| Session resume / kill 重启 | 已覆盖 | happy-path resume + SIGTERM-mid-turn 恢复最近提交消息 |
| 性能 smoke | 3 条 ignored smoke | 10 并发 `/threads`、100k transcript、1k event SSE；1 小时真网 soak 仍 deferred |
| 真实模型生成 | runner 已落地 | DeepSeek `deepseek-v4-flash` live gate 已接入；5-day pass-rate 等待 secret/nightly |
| 错误码契约 | 已覆盖 | L0 single-source mapping + 文档同步测试 |

---

## 纵深功能面 → 测试策略

每节体例：**现状（✅/⚠️/❌）→ 缺口 → 优先级 → 测试位置**。

### 5.1 CLI 解析与模式分发

- **现状 ✅**：`tests/code_cli_dispatch_test.rs` 已专项覆盖 `--web` / `--web-only` / `--stdio` 互斥、`--mcp-port 0`、`--port 0`、`--env-file`、`--repo`、`--cwd`、`--resume` 与 `--browser-control loopback` + `--stdio` 冲突；`src/command/code.rs` 单测覆盖 `--plan-mode` 默认/显式值矩阵。
- **缺口**：无（5.1 已关闭）。
- **优先级**：已完成。
- **测试位置**：**L1 已覆盖** `tests/code_cli_dispatch_test.rs`；`--plan-mode` 由 `src/command/code.rs` `effective_plan_mode_*` 单测覆盖。
- **AI 落地提示**：`src/command/code.rs` 的 `CodeArgs` 已用 `clap` derive。测试只需 `CodeArgs::try_parse_from(["code", "--web"])` 等组合，断言 `Ok` / `Err` 和具体错误消息。不要 spawn 子进程。

### 5.2 Provider 配置与启动

- **现状 ✅⚠️**：fake provider 完全覆盖；DeepSeek/OpenAI/Anthropic/Gemini/Kimi/Zhipu/Ollama provider boot 7/7 已经在 Code 路径下通过 `tests/helpers/mock_provider_server.rs` 覆盖。Codex 仍按 §5.13 独立处理。
- **缺口**：
  - 已完成：每个 reachable provider 至少一条 boot smoke；DeepSeek/Kimi/Ollama 相关 thinking/reasoning/stream/compact-tools 旗标透传。
  - 已完成：缺 API Key 时的可读错误覆盖（`build_helper_missing_api_key_errors_name_canonical_env_vars`）。
  - 已完成：`--api-base` 覆盖默认 base URL 的显式回归（`build_helper_honors_cli_api_base_for_deepseek`）。
- **优先级**：P1（Codex 除外，见 5.13）。
- **测试位置**：**L1 已新增** `tests/code_provider_boot_test.rs`，用 `tests/helpers/mock_provider_server.rs` 捕获 outgoing request body。
- **AI 落地提示**：不要再引入 `httpmock` 作为前置依赖；当前实现已选择 `axum` mock server。

### 5.3 HTTP 读路由 + loopback gate

- **现状 ✅**：`/session`、`/diagnostics`、`/threads` 与 loopback gate 已由 inline route test 和 security matrix 覆盖；`/events` 已由 SSE matrix 覆盖。覆盖项包括 `/session` 完整 schema（`controller`、`status`、`activeInteractionId`、`patchsets`）、`/diagnostics` secret redaction、`/threads?limit/offset` 边界（`limit=abc` → 400 `INVALID_QUERY_PARAM`，大 limit clamp 到 200，空集 `[]`）、以及任意读路由 non-loopback 客户端先返回 403 `LOOPBACK_REQUIRED`。
- **缺口**：无（5.3 已关闭）。
- **优先级**：已完成。
- **测试位置**：**L1 已新增** [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) `mod tests` route-level loopback checks；**L2 已新增** `code_ui_remote_security_matrix.rs`。
- **AI 落地提示**：新增读/写路由时继续先断言 loopback gate，再断言 body/token 语义，避免错误码顺序退化。

### 5.4 HTTP 写路由 + Controller Lease 状态机

- **现状 ✅**：[tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs) 已接入 10 条 case：9 条原 lease P0 + `--control observe` 下 automation attach 的 `CONTROL_DISABLED` 回归；`/messages` 大 body / busy / 422 仍由 `code_ui_scenarios.rs` 与 state matrix 覆盖；approval interaction P0 已由 approval matrix 覆盖。
- **缺口**：
  - 已完成：lease_cases.json 全量接入 `lease_case!()` 宏。
  - 已完成：短 TTL 真过期重新 attach（`LIBRA_CODE_LEASE_DURATION_MS`）。
  - 已完成：`--control observe` 下 automation attach → 403 `CONTROL_DISABLED`。
  - 已完成：同 client 续约 vs 不同 client conflict 的 token 失效顺序。
  - 已完成：approval 接受/拒绝/`apply_to_future` 三条 P0，以及 `never` / `on-failure` / `untrusted` / `allow-all` policy 行为覆盖。
  - 仍待 P1：多个 pending interaction 并发的 ID 路由。
- **优先级**：P0（功能已覆盖；长时/lagged soak 降级为 P2）。
- **测试位置**：**L2 已扩展** lease matrix 10/10；approval P0 由 `tests/code_ui_remote_approval_matrix.rs` 覆盖。
- **AI 落地提示**：新增 lease case 时仍保持“一条 JSON case + 一条 `lease_case!()`”的 cargo 输出定位。

### 5.5 SSE 事件流

- **现状 ✅**：`tests/harness/event_stream.rs` 与 [tests/code_ui_remote_sse_matrix.rs](../../tests/code_ui_remote_sse_matrix.rs) 已落地，7 条 `sse_cases.json` case 全部有 Rust runner。
- **缺口**：
  - 已完成：blocking SSE client、initial replay、status/session/controller events、双订阅者、断线重连 replay、streaming fixture 单调增长。
  - 仍待 P2：lagged stream 跨进程稳定再现困难，仅做 in-process broadcast 单测或长时 soak。
- **优先级**：P0。
- **测试位置**：**L2 已新增** `tests/harness/event_stream.rs` + `tests/code_ui_remote_sse_matrix.rs`；perf smoke 覆盖 1k event monotonic seq。
- **AI 落地提示**：后续扩展继续复用现有 API：
  ```rust
  pub struct EventStream { /* reqwest::blocking::Response */ }
  impl EventStream {
      pub fn open(client: &reqwest::blocking::Client, url: &str, timeout: Duration) -> Result<Self>;
      pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SseEvent>>;
  }
  pub struct SseEvent { pub event: String, pub data: String }
  ```
  `matrix.rs` 已有 `Step::OpenEventStream { name, timeout_ms }`、`Step::WaitEvent { name, event_type, timeout_ms }`，新增 SSE case 只需扩 JSON 与对应宏。

### 5.6 Local TUI Control 锁 / 审计

- **现状 ✅**：[docs/automation/local-tui-control.md](../automation/local-tui-control.md) 已规约；`code_ui_scenarios.rs` 覆盖 `CONTROL_INSTANCE_CONFLICT` 与 `default_control_paths_restart_after_stale_pid_takeover`（spawn 一个 → SIGKILL → 第二个能用同一默认 control path 启动并替换 token）；`harness_self_test::code_session_starts_tui_and_cleans_control_files` 在默认 harness custom path 模式下覆盖 `--control-token-file` / `--control-info-file` 文件创建、diagnostics 不泄露 token marker、shutdown 清理；`security_cases.json` 在 custom path 模式下覆盖 control/controller token 不进 diagnostics、secret-like `LIBRA_LOG_FILE` redaction、control audit log 不泄露 secret-like `clientId`；`code_control_files` 单测已覆盖默认 token 0600、宽权限拒绝、symlink 拒绝、stale `control.json` 不阻塞 lock、custom token/info path 独立 lock；`web::tests::sanitized_audit_client_id_*` 已覆盖 audit `client_id` 80 字符上限、控制字符替换、空值 fallback、marker redaction、按 char 截断。
- **缺口**：无（5.6 已关闭）。
- **优先级**：已完成。
- **测试位置**：**L2 已覆盖** `default_control_paths_reject_second_live_instance`、`default_control_paths_restart_after_stale_pid_takeover`、`harness_self_test::code_session_starts_tui_and_cleans_control_files`、`code_ui_remote_security_matrix`；**L1/L0 已覆盖** `code_control_files::*` 与 `web::tests::sanitized_audit_client_id_*`。

### 5.7 Browser Control

- **现状 ✅**：`code_ui_scenarios.rs` 9 条 browser 场景，覆盖 attach/submit/detach、同 `clientId` reload 续租、`--browser-control off` attach 的 `BROWSER_CONTROL_DISABLED` 顺序、过期 token 写入后释放 snapshot、oversize、cancel、unknown interaction、TUI reclaim、second-browser conflict。
- **缺口**：无（5.7 已关闭）。
- **优先级**：已完成。
- **测试位置**：**L2 已覆盖** `browser_controller_attach_submit_detach_roundtrip`、`browser_same_client_reconnect_renews_existing_lease`、`browser_attach_rejected_when_control_disabled`、`browser_expired_controller_token_is_rejected_and_releases_snapshot`、`browser_oversized_message_returns_payload_too_large`、`browser_cancel_turn_aborts_in_flight_turn_without_automation_token`、`browser_unknown_interaction_id_is_rejected_without_state_change`、`local_tui_reclaim_invalidates_browser_lease`、`second_browser_attach_with_different_client_returns_conflict`。

### 5.8 TUI 渲染快照

- **现状 ✅**：`libra code` TUI 已落地复杂状态的 inline render 覆盖；当前裁决仍是不引入 `insta` 快照。
- **缺口**：无（5.8 已关闭）。单键 `q` 不作为聊天输入框退出键，避免与普通文本输入冲突；当前退出契约是 `Ctrl+C` 和 `/quit`。
- **优先级**：已完成。
- **测试位置**：**L1 已覆盖** `src/internal/tui/chatwidget.rs` 的 `ratatui::backend::TestBackend` 空 transcript / assistant delta、inline buffer 滚动窗口；`src/internal/tui/bottom_pane.rs` 的 approval prompt / retry error 状态行；`src/internal/tui/app.rs` 的滚动键解析、`/quit`、approval yes/no/always 映射；**L2 已覆盖** `/control reclaim` 控制权回收状态。

### 5.9 Tool ACL / context / approval policy

- **现状 ✅**：tool registry 已按 `--context dev|review|research` / intent 过滤覆盖；approval policy 已在 Code UI fake fixture 矩阵中覆盖 `never` / `on-failure` / `on-request` / `untrusted` / `allow-all`；network deny 由 orchestrator policy 与 `web_search` runtime 单测覆盖。
- **缺口**：无（5.9 已关闭）。新增 tool 或 policy enum 时必须同步扩展 ACL / approval / network-deny 断言。
- **优先级**：已完成。
- **测试位置**：**L1 已覆盖** `tests/code_tool_acl_test.rs`；**L2 已覆盖** `tests/code_ui_remote_approval_matrix.rs`；network deny 由 `src/internal/ai/orchestrator/policy.rs` 的 `test_shell_network_policy_denies_curl_like_commands` / `test_web_search_honors_network_policy` 与 `src/internal/ai/tools/handlers/web_search.rs::web_search_requires_network_enabled_runtime` 覆盖。

### 5.10 Apply-Patch 与文件生成(fake)

- **现状 ✅**：[tests/code_ui_remote_generation_matrix.rs](../../tests/code_ui_remote_generation_matrix.rs) 已覆盖 [tests/data/code_ui_remote/generation_cases.json](../../tests/data/code_ui_remote/generation_cases.json) 的 3 条 case。
- **缺口**：无（5.10 已关闭）。已覆盖 fake fixture 返回 `apply_patch` → 临时 repo 出现完整 Rust 文件 → `rustc --test` 通过；SSE 订阅期间触发同一生成请求并观测 tool/patch 结果；非法 patch 失败分支 final snapshot status=`error` 且临时 repo 不留半写文件。
- **优先级**：已完成。
- **测试位置**：**L2 已新增** `tests/code_ui_remote_generation_matrix.rs` + provider_fixtures，详见 §6.4 Wave 3。

### 5.11 Approval / Interaction 端到端

- **现状 ✅⚠️**：`ai_approval_ttl_test.rs` 覆盖 cache 策略；Code UI 的端到端 approval P0 与 approval-policy 矩阵已由 [tests/code_ui_remote_approval_matrix.rs](../../tests/code_ui_remote_approval_matrix.rs) 覆盖。
- **缺口**：
  - 已完成：fake fixture 触发 `Shell` tool → 因 `--approval-policy on-request` 进入 `awaiting_interaction` → harness POST `/interactions/{id}` 接受 → tool 执行 → assistant 完成。
  - 已完成：`never` 无 interaction 且拒绝 needs-human shell；`on-failure` / `untrusted` 会产生 approval interaction；`allow-all` 不产生 prompt 且执行同一 shell fixture。
  - 已完成：拒绝路径：harness POST `approved=false` → tool 返回拒绝结果 → assistant 看到拒绝。
  - 已完成：`apply_to_future` 缓存：第二次同 tool 自动通过。
  - **P1**：多个 pending interaction 并发的 ID 路由。降级原因：fake provider 每轮只发一个 tool_call，单 turn 并发需要扩展 fixture schema 支持 parallel tool calls，与 §5.13 之外的工作量不相称；P0 三条已经覆盖 ID 寻址正确性（每轮单 pending 也是 ID 路由的最小用例）。
- **优先级**：P0（前三条），P1（并发降级）。
- **测试位置**：**L2 已新增** `tests/code_ui_remote_approval_matrix.rs`，并扩展 `code_session.rs` 的 `respond_interaction()` helper。
- **AI 落地提示**：`CodeSession` 已有通用的 `respond_interaction(id, approved, selected_option, apply_to_future)`；新增 case 应直接复用：
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

- **现状 ✅**：[src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) 已覆盖 review rejection 写入 `agent_output`、Implementation/Analysis 默认 `max_turns`、Implementation 默认 48-turn 边界（第 48 次模型调用返回 final text 成功；需要第 49 次时失败且 reason 写入 `agent_output`）、workspace contract violation、FUSE infrastructure failure 不进入普通 retry loop；网络 deny 由 [src/internal/ai/orchestrator/policy.rs](../../src/internal/ai/orchestrator/policy.rs) 与 `web_search` handler 单测覆盖。
- **缺口**：无（5.12 已关闭）。后续修改 executor/gate 仍必须同步扩展对应 contract test。
- **优先级**：已完成。
- **测试位置**：**L1/L0 已覆盖** `src/internal/ai/orchestrator/executor.rs`、`src/internal/ai/orchestrator/policy.rs`、`src/internal/ai/tools/handlers/web_search.rs`。
- **AI 落地提示**：executor `mod tests` 中已有 `MockModel`、`ConditionalModel`、`ImplementationTurnLimitModel`。新增 executor/gate contract 时继续用小 fake model 直接打 `execute_task`，不要把纯契约测试上升到 L2。

### 5.13 Codex 旁路运行时

- **现状 ✅⚠️**：[tests/code_codex_default_tui_test.rs](../../tests/code_codex_default_tui_test.rs) 提供静态守卫；[tests/code_codex_runtime_test.rs](../../tests/code_codex_runtime_test.rs) 已有 `--codex-port 0` validation smoke、`MockCodexWsServer` helper、handshake round-trip smoke、真实 `libra code --provider codex --codex-port <mock>` binary boot smoke，以及 `thread/started` notification 持久化 smoke。binary smoke 已验证运行时向 mock app-server 发出 `initialize` / `thread/start`，转发选定 model，并在 Codex 默认 plan mode 下带上 `developerInstructions` / `baseInstructions` payload；notification smoke 已验证 mock `thread/started` 写入 `.libra/objects/` 并通过 MCP history index 可读。
- **缺口**：
  - Codex plan-first（`--plan-mode true`）：在 plan approve 之前拒绝执行（当前只锁定 plan-mode payload 注入）。
  - Codex 断开重连。
- **优先级**：P1。
- **测试位置**：**L2 已新增并需扩展** `tests/code_codex_runtime_test.rs` + 简易 mock WS server（用 `tokio-tungstenite` 接受连接 + 回放固定脚本）。
- **AI 落地提示**：`tokio-tungstenite` 已在 `Cargo.toml`。继续在现有 `MockCodexWsServer` 上补 plan approve gate / reconnect，不要另起并行 helper。

### 5.14 MCP 服务双入口一致

- **现状 ✅**：[tests/mcp_integration_test.rs](../../tests/mcp_integration_test.rs)、[tests/e2e_mcp_flow.rs](../../tests/e2e_mcp_flow.rs) 覆盖资源/工具列表与 initialize 握手；[tests/code_mcp_dual_entry_test.rs](../../tests/code_mcp_dual_entry_test.rs) 已覆盖 control.json mcpUrl、`--stdio` mutex、同进程 web/MCP dual-reachability smoke，web `/messages` 写入后由 web SSE 与 MCP `list_tasks` 同时观察，以及 MCP `tools/call create_task` 写入后由 web SSE 观察的反向一致性路径。
- **缺口**（Code 路径）：
  - 已完成：`libra code` 启动后 MCP server 的端口号写入 `--control-info-file`（automation 发现）。
  - 已完成：`--stdio` 模式下 web + tui 不启的互斥。
  - 已完成：同一进程内 web `/messages` 提交后，web SSE 观察 transcript 更新，MCP `tools/call list_tasks` 观察对应 TUI turn-tracking Task。
  - 已完成：MCP-originated `tools/call create_task` 写入广播到 web SSE transcript。
- **缺口**：无（5.14 已关闭）。新增外部 MCP 写入工具时，应同步考虑是否需要映射到 Code UI read model/SSE。
- **优先级**：已完成。
- **测试位置**：**L1 已扩展** `tests/code_mcp_dual_entry_test.rs`。

### 5.15 Diagnostics 与 Secret Redaction

- **现状 ✅**：[tests/diagnostics_redaction_test.rs](../../tests/diagnostics_redaction_test.rs)、[tests/redaction_contract_test.rs](../../tests/redaction_contract_test.rs) 在 AI 层面覆盖。
- **缺口**：
  - 已完成：HTTP `/api/code/diagnostics` 返回值经 `SecretRedactor::default_runtime()`。
  - 已完成：`LIBRA_LOG_FILE` 中 secret-like path 被脱敏（来自 `security_cases.json`）。
  - 已完成：Audit JSON 在 redaction 后无 token 原文。
- **优先级**：P1。
- **测试位置**：**L2 已新增** `code_ui_remote_security_matrix.rs` runner。

### 5.16 Session / History 持久化(Code 路径)

- **现状 ✅**：[tests/ai_session_jsonl_test.rs](../../tests/ai_session_jsonl_test.rs)、[tests/ai_storage_flow_test.rs](../../tests/ai_storage_flow_test.rs) 覆盖底层；[tests/code_resume_test.rs](../../tests/code_resume_test.rs) 覆盖 Code 路径的 unknown id、unknown non-UUID、happy-path resume，以及 SIGTERM-mid-turn 后 `--resume` 恢复最近提交的用户消息。
- **覆盖范围**：
  - 已完成：`--resume <thread_id>` 启动时 transcript 完整恢复，`status=idle`。
  - 已完成：中途 SIGTERM `libra code`（delayed fake provider 保持 `thinking`）→ 重启 `--resume` → 最近提交的用户消息仍在 transcript。
  - 并发同一 thread 的两个 `libra code` 实例由 5.6 control-instance lock 覆盖。
- **缺口**：无（5.16 已关闭）。
- **优先级**：已完成。
- **测试位置**：**L2 已覆盖** `tests/code_resume_test.rs`。

### 5.17 并发边界 / 体积限制

- **现状 ✅**：`code_ui_scenarios.rs` 覆盖 256 KiB 边界与第二浏览器 conflict；[tests/code_ui_remote_state_matrix.rs](../../tests/code_ui_remote_state_matrix.rs) 已覆盖 7 条 state case。
- **缺口**（`state_cases.json` 已写 8 条）：
  - 已完成：两线程并发 attach → 一胜一负（200 / 409）。
  - 已完成：thinking 中二次 submit → 409 `SESSION_BUSY`。
  - 已完成：cancel idle → 409 `SESSION_BUSY` 且文档化。
  - 已完成：257 KiB / 1 MiB 拒绝且不挂死。
  - Deferred：streaming 进行中 detach → assistant 状态收敛到 idle 而非死锁。
- **优先级**：P1。
- **测试位置**：**L2 已新增** `tests/code_ui_remote_state_matrix.rs` runner。

### 5.18 性能与稳定性 smoke

- **现状 ✅⚠️**：`tests/code_ui_perf_smoke_test.rs` 已有 3 条 `#[ignore]` smoke；长时真网 soak 仍 deferred。
- **缺口**：
  - 已完成：100k 行 transcript 下 `/session` 序列化耗时上限（默认 < 500 ms，可由 `LIBRA_PERF_CEILING_MS` 调整）。
  - 已完成：1 000 events SSE broadcast monotonic seq。
  - 已完成：10 并发 `/threads` 查询不死锁。
  - 仍待：1 小时真网 SSE soak，需独立 nightly job。
- **优先级**：P2。
- **测试位置**：**L2 已新增** `tests/code_ui_perf_smoke_test.rs`，`#[ignore]` + `LIBRA_RUN_PERF=1` 时跑。

### 5.19 真实模型生成

- **现状 ✅⚠️**：[tests/code_ui_remote_model_generation_matrix.rs](../../tests/code_ui_remote_model_generation_matrix.rs) 已接入 [tests/data/code_ui_remote/model_generation_cases.json](../../tests/data/code_ui_remote/model_generation_cases.json) 的 2 条 P0 case；`.env.test` 路由与 DeepSeek thinking/high-reasoning 自动注入已落地。
- **缺口**：见 §6.4 Wave 4 详细规约。剩余是运行条件与稳定性指标，不是实现文件缺口。
- **优先级**：P0（条件：CI nightly + `LIBRA_RUN_LIVE=1`）。
- **测试位置**：**L3 已新增** `tests/code_ui_remote_model_generation_matrix.rs` + `harness::matrix` 的 `ProviderSpec::ModelFromEnvFile` 分支。

### 5.20 错误码契约同步

- **现状 ✅**：错误码已有 source-of-truth + 测试断言，并同步 [docs/automation/local-tui-control.md](../automation/local-tui-control.md)。
- **缺口**：新增 error code 时必须继续更新测试断言和文档注脚。
- **优先级**：P1。
- **测试位置**：**L0 已新增** `code_ui::error::tests` 列出全部 ErrorCode → status 的映射，并与 [docs/automation/local-tui-control.md](../automation/local-tui-control.md) 同步。
- **AI 落地提示**：新增 error code 时开发者必须同步更新该列表，否则测试编译失败。

---

## L2 远端矩阵

本节是 §5 中 5.4 / 5.5 / 5.10 / 5.15 / 5.17 / 5.19 的合并实施细节。历史“L2 远端测试落地”方案已基本实现；本节保留当前设计约束和剩余 deferred 项。

### 6.1 可行性判断

**已落地：**

- controller attach 的 missing/invalid control token、invalid kind、conflict、detach、stale token、same client renewal。
- SSE initial replay、status_changed、session_updated、controller_changed、双订阅者、断线后重连读取最新 snapshot。
- 通过 `/api/code/messages` 调用 Code 服务，输入完整代码生成请求。确定性回归由 fake provider 驱动 `apply_patch`；模型能力回归默认使用仓库根目录 `.env.test` 中的 DeepSeek `deepseek-v4-flash` 配置，并开启 thinking/high reasoning。
- 并发 attach 一胜一负、busy submit、256 KiB 边界、1 MiB drain 不挂死、cancel idle 返回文档化错误。
- diagnostics redaction、`/threads` query validation/clamp、route 级 non-loopback 拒绝顺序。

**已经完成的小改造：**

- lease expiry L2：默认 TTL 是 120s，必须加 test-only TTL override（**已落地**于 [src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 的 `test_lease_duration_override()`）。
- `--control observe` L2：`CodeSessionOptions::with_control_observe()` 已存在并被 security/lease case 使用。
- SSE blocking client：`tests/harness/event_stream.rs` 已存在并被 SSE matrix 使用。
- 模型能力测试：`ProviderSpec::ModelFromEnvFile` 已支持默认从 `.env.test` 启动 DeepSeek `deepseek-v4-flash`。

**建议推迟或降级：**

- “lagged stream 不死”跨进程测试容易依赖 socket backpressure 和 broadcast polling 时序，放到 P2 或以 in-process 单元测试覆盖解析器。
- “cancel during executing tool phase”若要稳定命中 `executing_tool`，需要 fake fixture 支持一次性/序列化响应，或选择稳定长耗时 tool。当前仍保留为 deferred 候选。

### 6.2 数据驱动设计

[tests/harness/matrix.rs](../../tests/harness/matrix.rs) 已就位，从 `tests/data/code_ui_remote/*.json` 读取 case。结构：

```rust
#[derive(Deserialize)]
pub struct RemoteCase {
    pub name: String,
    pub priority: Priority,
    pub provider: Option<ProviderSpec>,
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

### 6.3 已落地的代码改动

#### Harness

[tests/harness/code_session.rs](../../tests/harness/code_session.rs) 已完成以下扩展：

- `CodeSessionOptions` 已新增：
  - `provider_override: Option<String>` / `model_override: Option<String>`，默认仍走 fake fixture。
  - `with_live_provider(provider, model)` + `with_env_file(path)`：默认读取 repo-root `.env.test`，解析 `LIBRA_CODE_TEST_PROVIDER` / `LIBRA_CODE_TEST_MODEL`，spawn 时传 `--env-file <repo>/.env.test --provider <provider> --model <model> --deepseek-thinking enabled --deepseek-reasoning-effort high`。
  - `control_write: bool`，默认 `true`；`with_control_observe()` 让 spawn 不传 `--control write`（**已存在**）。
  - `lease_duration_ms: Option<u64>`；spawn 时设置 `LIBRA_CODE_LEASE_DURATION_MS`（**已存在**）。
  - `extra_env: Vec<(String, String)>`；用于 diagnostics redaction / audit log 场景，并且要在 harness 默认 env 之后应用，保证 case 能覆盖 `LIBRA_LOG_FILE`。
- `.env.test` 处理规则：
  - 不读取、不打印 secret 值到 `debug_context()`。
  - 缺少 `.env.test`、`LIBRA_CODE_TEST_PROVIDER` 或 `LIBRA_CODE_TEST_MODEL` 时，模型矩阵应 fail fast，错误说明需要创建/补齐 `.env.test`。
  - 默认要求 `LIBRA_CODE_TEST_PROVIDER=deepseek` 且 `LIBRA_CODE_TEST_MODEL=deepseek-v4-flash`；若不是 deepseek，runner 应失败并提示该矩阵固定验证 DeepSeek thinking/high reasoning 模式。
  - `.env.test` 中还应包含 DeepSeek 凭证，例如 `DEEPSEEK_API_KEY` 和可选 DeepSeek base URL。
  - 模型矩阵默认 `--context dev --approval-policy never`，避免 classifier 额外消耗模型调用，并保证 workspace 内 `apply_patch` 不需要人工确认。
- 通用 HTTP helper 已新增，后续矩阵应复用这些方法：
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

`tests/harness/event_stream.rs` 已新增：

- 使用独立 `reqwest::blocking::Client`。不要复用全局 5s total timeout，而是设置 per-read timeout。
- 手工解析 SSE block，只识别 `event:` 和 `data:`。
- 单行上限 1 MiB，`next()` 超时返回 `Ok(None)`，EOF 返回明确错误。
- 当前对外暴露：
  ```rust
  pub struct SseEvent { pub event: String, pub data: String }
  pub struct EventStream { /* private */ }
  impl EventStream {
      pub fn open(client: &Client, url: &str, bearer_token: Option<&str>) -> Result<Self>;
      pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SseEvent>>;
  }
  ```

[tests/harness/matrix.rs](../../tests/harness/matrix.rs) 已扩展：

- 读取 `tests/data/code_ui_remote/*.json`。
- 分发 step。
- 把 `TokenSource::{current, stale, forged, none}` 和上一轮 attach/detach 的 token 状态保存在 runner context 中（**已实现**）。
- 新增 `Step::OpenEventStream` / `Step::WaitEvent` / `Step::RespondInteraction`。
- 新增 `ProviderSpec::ModelFromEnvFile`。
- 新增 assertion 谓词：`event_seen`、`transcript_contains`、`file_exists`、`repo_command_exit_0`。

#### Runtime 短 TTL（已落地）

[src/internal/ai/web/code_ui.rs](../../src/internal/ai/web/code_ui.rs) 已新增 `CodeUiRuntimeOptions { browser_write_enabled, automation_write_enabled, initial_controller, lease_duration: Option<chrono::Duration> }`、`CodeUiRuntimeHandle::build_with_options(adapter, options) -> Arc<Self>`，保留 `build()` / `build_with_control()` 委托；`lease_duration == None` 时继续用 `DEFAULT_BROWSER_CONTROLLER_LEASE_SECS`。

[src/command/code.rs](../../src/command/code.rs) 已在 `cfg(feature = "test-provider")` 下解析 `LIBRA_CODE_LEASE_DURATION_MS`，仅接受正整数毫秒，非法值让启动失败；非 `test-provider` 构建不读该 env var。

#### Inline loopback tests

在 [src/internal/ai/web/mod.rs](../../src/internal/ai/web/mod.rs) 的现有 `mod tests` 已追加 route 级测试：

- `GET /api/code/session` 带 `ConnectInfo(192.0.2.10:4000)` 返回 403 / `LOOPBACK_REQUIRED`。
- `POST /api/code/messages` 同样先返回 `LOOPBACK_REQUIRED`，证明 loopback 校验先于 body/controller token 校验。

不需要修改 `build_router` 可见性。后续新增路由继续参考已有 `loopback_api_request_rejects_remote_clients` 模式，构造完整 `Request` 并过 router。

### 6.4 L2 远端 Wave 0–5

#### Wave 0：基础设施

1. `tests/data/code_ui_remote/` 数据目录已存在。
2. `event_stream.rs` 和 `matrix.rs` 已就位。
3. `CodeSessionOptions` 和通用 HTTP helper 已扩展。
4. test-only lease TTL override 已落地。

验收：

```bash
cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1
cargo test --lib code_ui
```

#### Wave 1：Controller Lease

[tests/code_ui_remote_lease_matrix.rs](../../tests/code_ui_remote_lease_matrix.rs) 已读取 [tests/data/code_ui_remote/lease_cases.json](../../tests/data/code_ui_remote/lease_cases.json)，并扩展到 10 条 `lease_case!()`。

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

**AI 落地提示**：该文件保持“一条 JSON case + 一条 `lease_case!()`”模式；新增 case 时不要重新写自定义 runner。

#### Wave 2：SSE

`tests/code_ui_remote_sse_matrix.rs` 已新增，读取 [tests/data/code_ui_remote/sse_cases.json](../../tests/data/code_ui_remote/sse_cases.json)。

首批 P0/P1 case：

- initial connect replay `session_updated` 全量 snapshot。
- submit 后看到 `status_changed` 且 status 为 `thinking`。
- assistant 完成后看到 `session_updated`，最终 status 为 `idle`。
- attach/detach 触发 `controller_changed`。
- 两个订阅者都收到 submit 产生的状态变化。
- 断开 SSE 后 submit，重连 initial replay 包含最新 transcript。
- streaming fixture 的 transcript 内容单调增长。

**AI 落地提示**：`event_stream.rs`、`Step::OpenEventStream`、`Step::WaitEvent` 与 `CodeSession::open_event_stream()` 均已存在；新增 SSE case 只需补 JSON 和对应 `sse_case!()`。

#### Wave 3：Code 服务生成代码并测试

`tests/code_ui_remote_generation_matrix.rs` 已新增，读取 [tests/data/code_ui_remote/generation_cases.json](../../tests/data/code_ui_remote/generation_cases.json)。

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

`tests/code_ui_remote_model_generation_matrix.rs` 已新增，读取 [tests/data/code_ui_remote/model_generation_cases.json](../../tests/data/code_ui_remote/model_generation_cases.json)。

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

已新增：

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

| # | 功能面 | L0 | L1 | L2 | L3 | 当前状态 | 剩余风险 |
|---|---|---|---|---|---|---|---|
| 5.1 | CLI 解析 / 模式分发 | – | done | – | – | ✅ | 持续随 CLI option 增量维护 |
| 5.2 | Provider boot + flag passthrough | – | done | – | – | ✅ | missing-key / `--api-base` quick-follow closed |
| 5.3 | HTTP 读路由 + loopback gate | done | done | done | – | ✅ | 新路由需维持 loopback-first 错误顺序 |
| 5.4 | HTTP 写路由 + lease 状态机 | – | done | done | – | ✅ | pending interaction 并发仍 P1 |
| 5.5 | SSE | done | – | done | – | ✅⚠️ | lagged / 1 小时 soak deferred |
| 5.6 | Local TUI Control 锁/审计 | done | done | done | – | ✅ | 无 |
| 5.7 | Browser Control | – | – | done | – | ✅ | 无 |
| 5.8 | TUI 渲染快照 | – | done | done | – | ✅ | 无 |
| 5.9 | Tool ACL / context / policy | – | done | – | – | ✅ | 持续随 tool registry 增量维护 |
| 5.10 | Apply-Patch 生成(fake) | – | – | done | – | ✅ | 持续随 fixture schema 增量维护 |
| 5.11 | Approval / Interaction E2E | – | – | done | – | ✅⚠️ | 并发 pending 仍 P1 |
| 5.12 | Orchestrator gate / max_turns | done | done | – | – | ✅ | 持续随 executor/gate contract 增量维护 |
| 5.13 | Codex 旁路运行时 | – | – | partial | – | ⚠️ | plan approve gate / reconnect |
| 5.14 | MCP 双入口一致 | – | done | smoke + both directions observe | – | ✅ | 无 |
| 5.15 | Diagnostics redaction | done | exist | done | – | ✅ | 持续随 diagnostics 字段增量维护 |
| 5.16 | Session resume / kill | – | – | done | – | ✅ | 并发同 thread 由 5.6 control-instance lock 覆盖 |
| 5.17 | 并发 / size limits | – | – | done | – | ✅⚠️ | streaming detach during tool phase deferred |
| 5.18 | 性能 smoke | – | – | done(ignore) | – | ✅⚠️ | 1 小时真网 SSE soak |
| 5.19 | 真实模型生成 | – | – | – | done(gated) | ✅⚠️ | secret + 5-day nightly pass-rate |
| 5.20 | 错误码契约 | done | – | – | – | ✅ | 新 error code 需同步文档和测试 |

---

## 关键文件路径

### 已就位 — 基础设施

- `tests/harness/event_stream.rs`
- [tests/harness/code_session.rs](../../tests/harness/code_session.rs)：`with_live_provider()`、`with_env_file()`、`respond_interaction()`、`open_event_stream()`、`get_threads()`、`diagnostics_raw_text()`、`libra_log_text()`、`read_repo_file()`、`run_repo_command()`
- [tests/harness/matrix.rs](../../tests/harness/matrix.rs)：`ProviderSpec::ModelFromEnvFile`、`Step::OpenEventStream` / `WaitEvent`、`Step::RespondInteraction`

### 已就位 — 矩阵 runner

- `tests/code_ui_remote_sse_matrix.rs`（读 `sse_cases.json`）
- `tests/code_ui_remote_generation_matrix.rs`（读 `generation_cases.json`）
- `tests/code_ui_remote_model_generation_matrix.rs`（读 `model_generation_cases.json`）
- `tests/code_ui_remote_state_matrix.rs`（读 `state_cases.json`）
- `tests/code_ui_remote_security_matrix.rs`（读 `security_cases.json`）
- `tests/code_ui_remote_approval_matrix.rs`（读 approval cases）
- `tests/code_ui_remote_lease_matrix.rs`（读 `lease_cases.json`）

### 已就位 / 已部分就位 — 纵深面

- `tests/code_cli_dispatch_test.rs`
- `tests/code_provider_boot_test.rs`
- `tests/code_tool_acl_test.rs`
- `tests/code_codex_runtime_test.rs`
- `tests/code_mcp_dual_entry_test.rs`
- `tests/code_resume_test.rs`
- `tests/code_ui_perf_smoke_test.rs`（`#[ignore]`）
- TUI render smoke 当前为 inline buffer 断言；未采用 `tests/code_tui_render_test.rs` + `insta` 快照路线。
- Approval flow 当前由 `tests/code_ui_remote_approval_matrix.rs` 覆盖，不再单独维护额外的 approval-flow 测试文件。

### 仍待扩展 — 现有

- 扩展 [src/internal/ai/orchestrator/executor.rs](../../src/internal/ai/orchestrator/executor.rs) `mod tests`：max_turns / contract violation / FUSE infrastructure failure 已覆盖；后续 executor/gate contract 继续参考 `MockModel` / `ConditionalModel` / `ImplementationTurnLimitModel` 模式。
- 扩展 [tests/code_codex_runtime_test.rs](../../tests/code_codex_runtime_test.rs)：plan approve gate、reconnect。
- [tests/code_mcp_dual_entry_test.rs](../../tests/code_mcp_dual_entry_test.rs)：MCP-originated `tools/call create_task` 写入与 web SSE 观察、web `/messages` → SSE/MCP observe 两个方向均已覆盖。
- [tests/code_resume_test.rs](../../tests/code_resume_test.rs)：unknown id、happy-path resume、SIGTERM-mid-turn 恢复最近提交消息均已覆盖。
- 扩展 TUI render quick-follow：复杂状态 inline buffer 断言。

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
cargo test --features test-provider --test code_tool_acl_test
cargo test --features test-provider --test code_mcp_dual_entry_test

# L2 PTY/HTTP（串行）
cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_lease_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_sse_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_generation_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_state_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_security_matrix -- --test-threads=1
cargo test --features test-provider --test code_ui_remote_approval_matrix -- --test-threads=1
cargo test --features test-provider --test code_resume_test -- --test-threads=1
cargo test --features test-provider --test code_codex_runtime_test -- --test-threads=1

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

| PR | 主题 | 当前状态 | 剩余项 | 阻塞项 |
|---|---|---|---|---|
| 1 | 基础设施 | ✅ closed | 无 | 无 |
| 2 | CLI / route P0 + 错误码契约 L0 | ✅ closed | 无 | 无 |
| 3 | Lease 全量 | ✅ closed | 无 | 无 |
| 4 | SSE 全量 | ✅ closed | 长时/lagged soak deferred | 无 |
| 5 | 生成 fake | ✅ closed | 无 | 无 |
| 6 | Approval flow | ✅ closed | 并发 pending P1 | 无 |
| 7 | State / Security 矩阵 | ✅ closed | streaming detach candidate | 无 |
| 8 | Orchestrator gate / max_turns / Tool ACL | ✅ closed | 无 | 无 |
| 9 | Codex runtime + MCP 双入口 + resume | ⚠️ partial | Codex plan approve gate / reconnect | 无新增依赖 |
| 10 | TUI render + provider boot/flag passthrough | ✅ closed | provider boot 7/7、missing-key、`--api-base`、TUI 复杂状态 render 均已闭合 | 不再要求 `insta` / `httpmock` |
| 11 | Model generation L3 | ✅ runner closed | 5-day pass-rate 需 secret/nightly 数据 | `DEEPSEEK_API_KEY` |
| 12 | 性能 smoke + 错误码文档同步 | ✅ mostly closed | 1 小时真网 SSE soak | nightly job |

每条 Wave 在 CI 上独立可门：失败仅阻塞当前 PR，不污染主干。

**建议的并行策略**：
- PR 1–8 与 PR 11/12 的 runner/doc-code sync 已不再是前置瓶颈。
- 后续并行工作应只围绕 PR 9 的明确 partial 项，以及 PR 12 的独立 nightly soak。

---

## 决策与折中

- **不在同一矩阵里混真实/伪 provider**：model_generation 单独矩阵，文件名带 `model_` 前缀，避免误跑。
- **lagged SSE / cancel-during-tool 降级 P2**：跨进程稳定性差，改为 in-process 单测覆盖解析器/状态机即可。
- **TUI 渲染用 `TestBackend` / inline buffer 断言**：不要起真实 PTY 验 ratatui buffer——成本高、抖动大；当前不引入 `insta`，用稳定文本/符号断言锁回归。
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

Wave 1–8、Wave 10、Wave 11 与 Wave 12 部分已完成；Wave 9 / Wave 12 剩余项均按 Codex pass-1 review 拆为独立后续 PR：

| Wave | 状态 | 备注 |
|---|---|---|
| 1 | ✅ closed | 基础设施 + event_stream.rs + 1 条 SSE 回归 |
| 2 | ✅ closed | CLI dispatch + route loopback + 错误码契约 L0 |
| 3 | ✅ closed | Lease matrix 9/9 + observe-mode + audit redaction L0 |
| 4 | ✅ closed | SSE matrix 7/7 |
| 5 | ✅ closed | Generation matrix 3/3（apply_patch fake fixture） |
| 6 | ✅ closed | Approval matrix 7/7（accept / reject / `apply_to_future` + `never` / `on-failure` / `untrusted` / `allow-all`），并发 pending 已按 §5.11 P1 拆出 |
| 7 | ✅ closed | State 7/7 + Security 6/6 + diagnostics SecretRedactor wire-up |
| 8 | ✅ closed | Tool ACL 6/6（含 MCP bridge 前缀防退化）+ executor max_turns / workspace contract / FUSE failure contract |
| 9 | ⚠️ partial | `--resume` CLI 表面 4 条（unknown UUID、unknown 非 UUID 字符串、happy-path 跨进程恢复 transcript、SIGTERM-mid-turn 恢复最近提交消息）；§5.16 closure criterion ✅；§5.13 partial — `--codex-port 0` validation smoke + `MockCodexWsServer` helper（tokio-tungstenite，accept WS handshake、回 initialize/thread/start JSON-RPC envelope）+ 端到端 round-trip smoke ✅ + binary 级 boot smoke（`libra code --provider codex` 真实链入 mock，锁定 `initialize` / `thread/start` 和默认 plan-mode payload）✅ + `thread/started` notification persistence ✅；§5.14 item 1/2/3-smoke ✅（control.json mcpUrl + --stdio mutex + 同进程 web/MCP dual-reachability via `Mcp-Session-Id` 头）；§5.14 web `/messages` → SSE/MCP observe ✅；§5.14 MCP `tools/call create_task` → web SSE observe ✅；§5.13 plan approve gate / reconnect 仍未交付（roadmap-sized） |
| 10 | ✅ closed | §5.2 provider boot 7/7 reachable providers 已落地（DeepSeek+flags、OpenAI、Anthropic、Kimi+flags、Ollama+flag、Zhipu、Gemini）经 `tests/helpers/mock_provider_server.rs`（无新增 dev-dep，复用 `axum`）+ 新增 `GeminiClient::with_base_url` test-only 构造器；§5.2 missing-key / `--api-base` quick-follow 已闭合；Codex 见 §5.13；§5.8 TUI 复杂状态已由 TestBackend / inline buffer 覆盖 |
| 11 | ✅ closed | `tests/harness/matrix.rs` 已加 `ProviderSpec::ModelFromEnvFile` + DeepSeek thinking/high-reasoning 自动注入；`tests/code_ui_remote_model_generation_matrix.rs` 在 `LIBRA_RUN_LIVE=1` 下真正调用矩阵；`.github/workflows/model-generation-nightly.yml` 提供每日 cron + `workflow_dispatch`，需 maintainer 配置 `DEEPSEEK_API_KEY` secret 后才会运行；regression L0 测试（`build_session_options_for_*_provider_*`）锁定 DeepSeek 旗标注入逻辑 |
| 12 | ✅ closed | 错误码 doc/code sync L0 测试 + perf smoke 3 条（10 并发 `/threads`、100k transcript snapshot 序列化 < 500ms 可由 `LIBRA_PERF_CEILING_MS` 调整、SSE broadcast 1 000 events monotonic seq L1）`#[ignore]` + `LIBRA_RUN_PERF=1`；唯一 deferred 项是 1-小时真网 SSE soak（独立 nightly job） |

落地完成判定的全部门：剩余阻塞项均为明确的 roadmap-sized 多 PR 工作 —— Wave 9 §5.13 plan approve gate / reconnect（在现有 `MockCodexWsServer` 上继续扩展）。已闭合：Wave 9 §5.13 partial（`--codex-port 0` validation + `MockCodexWsServer` 协议 helper + handshake round-trip smoke + binary boot smoke + 默认 plan-mode payload + `thread/started` notification persistence）、§5.14 item 1/2/3-smoke + web `/messages` → SSE/MCP observe + MCP `create_task` → web SSE observe、§5.16 happy-path + SIGTERM-mid-turn、Wave 10 §5.2 全量 7/7 provider + missing-key / `--api-base` quick-follow、Wave 10 §5.8 TUI render 复杂状态。Wave 11 已具备完整工作流（harness wiring + 每日 nightly + L0 regression）；剩余条件（5 天连续 ≥ 90% 通过率）依赖 maintainer 配置 `DEEPSEEK_API_KEY` 并等待 5 个 nightly run。Wave 12 perf smoke 现含 1k events SSE broadcast monotonic 验证；1-小时真网 SSE soak 需独立 nightly job。当前仓库状态对应"基础矩阵已落地 + Wave 10/11/12 已 wire + Wave 9 Codex 部分 deferred"。
