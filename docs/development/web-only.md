# `libra code` Web-only 迁移计划

> Status: draft (code/doc anchors calibrated against the working tree on 2026-05-31)
> Last updated: 2026-06-01
> Scope: `libra code` no longer launches or depends on the Code TUI. The browser Code UI becomes the only supported interactive surface, including thread version-graph inspection.
> Companion docs: current Web-runtime status in [`docs/improvement/web.md`](../improvement/web.md); control surface in [`docs/commands/code-control.md`](../commands/code-control.md).
> 所有 `path:line` 锚点以 2026-05-31 工作树为准；执行前用 `rg <symbol>` 复核符号仍存在即可（行号会随改动漂移）。完整锚点见文末「附录 A」。

## 目标

把 `libra code` 收敛为 Web-only 入口：

- `libra code` 默认启动 Web Code UI，不再进入 ratatui/crossterm TUI。
- `libra code --web` / `--web-only` 不设计兼容期；在默认 Web 切换同一阶段从 `CodeArgs`、help、docs 和 tests 中删除，旧脚本需要改为裸 `libra code`。
- 浏览器在 loopback 上默认具备写控制能力；非 loopback 访问仍只能看到 remote access notice，不能读写 session/API。
- 移除独立 `libra graph` 终端命令；thread/version graph 作为 Web Code UI 的内置视图展示。
- `src/internal/tui/app.rs` 中承载的 agent 行为必须迁移到 provider-neutral runtime，不能因为移除 TUI 而丢失计划确认、approval、goal、usage、skills、hooks、multi-agent、resume 等现有能力。

## 非目标

- 不开放公网 Web 写控制。Code UI v1 仍以 loopback 为安全边界。
- 不把 browser UI 做成多用户协作产品；同一 session 仍只有一个 active controller lease。
- 不在 Web terminal 暴露任意 shell prompt；命令执行必须继续通过 tool runtime、sandbox 和 approval。
- 不在终端里保留第二套 graph UI。`src/internal/tui/*`、ratatui/crossterm 依赖和共享 terminal helper 是否删除，以 Code TUI 与 `libra graph` 都迁完后的真实调用者为准。

## 方案评估结论（2026-05-31 校准）

本节是对原迁移方案的评估，并记录本次修订对正文做的事实性修正，使每张任务卡都能被 Claude Code 直接落地。

整体结论：**阶段切分与依赖顺序合理且安全**——先补 MCP stdio 替代入口（W1），再切默认 Web（W2/W3），抽 session bootstrap 与 plan/goal 行为（W5/W6/W7），移除 TUI bridge/reclaim（W8），迁 Web graph 并删除 `libra graph`（W8G），最后才删 TUI startup（W9）。"禁止提前执行的依赖" 一节正确地拦住了"未迁移即删除"导致的能力丢失。这套顺序可以照搬执行。

原始稿存在 4 类会直接误导执行 Agent 的偏差，已在本次修订中校准（均经 `rg`/`ls` 复核）：

1. **文档自引用路径错误（阻断级）**：W0 / W12 让 Agent "阅读并只修改 `docs/agent/web-only.md`"，但该文件在仓库中**根本不存在**——本计划文件实际是 `docs/development/web-only.md`。已把所有自引用统一指向本文件；照原稿执行的 Agent 会因找不到文件而失败。
2. **`local-tui-control.md` 清理状态误判（阻断级）**：原稿说 `docs/automation/local-tui-control.md` 已删除且脚本已完全 repoint，但当前工作树中该文件仍存在，并且 `src/internal/ai/web/code_ui.rs` 的错误码同步测试、`tests/INDEX.md`、`tests/compat/README.md`、`tests/compat/help_flag_descriptions.rs`、`tests/code_cli_dispatch_test.rs`、`docs/improvement/web.md`、`docs/improvement/test.md` 等仍引用它。the (now-removed) docs-consistency script——其 endpoint/header 检查现已迁入 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`，断言 `docs/commands/code-control.md` 覆盖 `/api/code/*` 路由——历史上曾 repoint 到 `docs/commands/code-control.md`，并仍钉 `.github/workflows/base.yml` 的 "Run TUI automation scenarios" 和 `tests/code_codex_default_tui_test.rs`。已把 **W13** 改成未完成的显式 cleanup 卡：先迁移错误码 doc source-of-truth 和引用，再删除旧文档，不能把它当作已执行事实。
3. **CLI flag 结构**：当前 `--web` 是 `web_only` 字段（`code.rs:462`）的 clap alias（`code.rs:461`，`conflicts_with = "stdio"`）；`--mcp-stdio` 是 `stdio` 字段（`code.rs:600`）的 alias（`code.rs:599`，`conflicts_with = "web_only"`）。它们不是独立字段，改写要落到这两个字段上。本修订明确不设计 `--web`/`--web-only` 兼容期：W2 在默认 Web 切换时删除 `web_only` 字段、`web` alias、相关 conflicts 和所有 `args.web_only` 分支，而不是继续解析旧 flags。`execute()`（`code.rs:696-705`）当前按 `stdio → web_only → else(tui)` 分发；`execute_stdio` 走 `resolve_code_working_dir(args)` + `init_mcp_server(&working_dir)`，`CodeArgs` 已有 `cwd`(:474)/`repo`(:478) 供 W1 复用。
4. **失效的代码锚点**：`record_usage_failure` 在代码库中不存在；usage 真实锚点是 `App.usage_snapshot`（`app.rs:535`）+ `src/internal/ai/usage/format.rs::format_usage_badge`(:45)/`format_usage_detail_panel`(:13)。`SkillDispatcher`、`run_goal_supervised_tool_loop` 并不在 `src/internal/tui`，而在 `src/internal/ai/skills/dispatcher.rs:15`、`src/internal/ai/goal/driver.rs:111`。inventory 表与附录 A 已给真实路径行号。

保持不变（原稿正确）：跨进程 harness 确实依赖 `portable-pty`（`Cargo.toml:156`，`tests/harness/code_session.rs` 用 `native_pty_system()`）配合 `write_tui_line` 注入 TUI 按键；W10 把这套 PTY+按键驱动换成 Web process + HTTP/SSE 是正确方向。

## 当前基线

`src/command/code.rs` 现在有三条 mode 分支：

- `execute_tui(args)`：默认路径。它初始化 `tui_init()`，创建 `App`，并启动背景 Web/MCP server。
- `execute_web_only(&args)`：当前 `--web-only` / `--web` 路径。它启动 Web + MCP；Codex 使用 managed app-server runtime，非 Codex 使用 `HeadlessCodeRuntime`。W2 会把这条实现改名/收敛为默认 `execute_web`，并删除旧 flags。
- `execute_stdio(&args)`：MCP stdio transport。

Web 侧已有可复用基础：

- `src/internal/ai/web/mod.rs` 服务静态 Web app 和 `/api/code/*`。
- `src/internal/ai/web/code_ui.rs` 是 Code UI wire contract 和 controller lease 的源头。
- `src/internal/ai/web/headless.rs` 已支持非 Codex provider 的 browser submit、streaming、approval/user-input、cancel、plan/patchset projection、session persistence。
- `docs/improvement/web.md` 记录了 Web UI、browser-control、headless runtime 的现状。

主要缺口在 TUI-owned 行为和独立 graph 入口：

- generic provider 的 IntentSpec / Plan 两阶段确认和自动 repair loop 仍深耦合在 `src/internal/tui/app.rs`。
- slash commands、`/goal`、`/usage`、`/skill`、`/plan continue`、local reclaim 等交互现在由 TUI App 处理。
- `TuiCodeUiAdapter` / `TuiControlCommand` 是 HTTP write 到 TUI App 的桥；Web-only 后这层应消失或只保留为迁移期测试辅助。
- `src/command/graph.rs` 是独立 ratatui/crossterm TUI 命令，但其 thread projection loader 和 object detail 逻辑需要迁入 Web graph service。
- 多个测试目标和文档把 "TUI + background Web" 和独立 `libra graph` 当作默认契约。

## TUI-owned behavior inventory

这张表是后续执行任务的风险清单。凡是 `must-migrate` 项，在默认入口切到 Web 并删除 TUI path 前必须有 Web/session runtime 等价实现和测试。

| 行为 | 当前代码锚点 | 分类 | 目标位置 | 迁移验收 |
|------|--------------|------|----------|----------|
| 默认 `libra code` dispatch | `src/command/code.rs::execute`, `execute_tui`, `execute_web_only`, `validate_mode_args` | must-migrate | `src/command/code.rs::execute_web` | 默认 `libra code` 不调用 `execute_tui`; `--web`/`--web-only` 从 `CodeArgs`/help/docs/tests 删除，旧 flags 解析失败 |
| MCP stdio transport | `src/command/code.rs::execute_stdio`, `init_mcp_server` | web-replace | 新的 `libra mcp --stdio` 或等价非 `code` 命令 | `libra code --stdio` 有迁移错误; 新命令覆盖 MCP stdio e2e |
| IntentSpec draft/review | `start_plan_workflow*`, `begin_plan_workflow`, `handle_intent_review_choice`, `build_plan_prompt`, `phase0_plan_tool_loop_config`, `persist_phase0_intent_for_review` | must-migrate | provider-neutral `CodeSessionRuntime` / `plan_workflow` | Web submit 后先出现 `intent_review_choice`; 未确认前不执行 mutating tools |
| Execution plan draft/review | `begin_plan_revision_flow`, `begin_execution_plan_revision_flow`, `handle_post_plan_choice`, `build_execution_plan_prompt`, `phase1_plan_tool_loop_config`, `provider_plan_draft_from_args` | must-migrate | provider-neutral `CodeSessionRuntime` / `plan_workflow` | Web 能 confirm/modify/cancel plan; `submit_plan_draft` 仍是 terminal tool |
| Orchestrator execution and repair loop | `format_orchestrator_result`(app.rs:8639)、`record_orchestrator_thread_metadata`(app.rs:231)、`automatic_plan_repair_*`(app.rs:11142+，如 `automatic_plan_repair_request_from_report`:11149)、`execution_requires_plan_repair`(app.rs:10911)、pending revision fields | must-migrate | session runtime + Code UI interactions | failed execution feeds repair prompt; threshold 后 Web 显示 continue/modify/cancel |
| Sandbox/tool approval | `handle_exec_approval_request`, `submit_exec_approval_decision`, `reject_pending_exec_approval`, `cancel_pending_exec_approval` | must-migrate | `HeadlessCodeRuntime` approval channel already exists; extend to plan workflow | approval/sandbox approval in Web uses `CodeUiInteractionRequest`; cancel resolves pending approval |
| `request_user_input` | `pending_user_input`, `cancel_pending_user_input`, `RequestUserInputHandler`, TUI bottom pane answer flow | must-migrate | `HeadlessCodeRuntime` pending user-input map already exists; reuse for all workflows | structured questions round-trip through `/api/code/interactions/{id}` |
| Goal lifecycle | `src/internal/tui/goal_session.rs`、`goal_command.rs`、`goal_session_*_from_control`(app.rs:1111/1146/1157/1201)、`replay_goal_session_from_session_root`(app.rs:11840)；driver `run_goal_supervised_tool_loop`(`src/internal/ai/goal/driver.rs:111`) | must-migrate | UI-neutral goal session module under `src/internal/ai/goal/` or `src/internal/ai/code_session/` | Web goal start/status/cancel and resume replay work without TUI |
| Usage display and cancellation accounting | `App.usage_snapshot`(app.rs:535)、`src/internal/ai/usage/format.rs::format_usage_badge`(:45)/`format_usage_detail_panel`(:13)、`/usage` in `handle_builtin_command`(app.rs:4887)。注：原稿的 `record_usage_failure` 不存在；cancel 的 usage 记账走 usage recorder/context，不是独立函数 | must-migrate | session runtime usage service + Web transcript/snapshot projection | Web shows session usage or transcript info; cancel 经 usage recorder 记一条 failure usage |
| Skills and slash command effects | `load_skills`(`src/internal/ai/skills/loader.rs:13`)、`SkillDispatcher`(`src/internal/ai/skills/dispatcher.rs:15`)、`handle_builtin_command`(app.rs:4887)、`/skill`、`/plan`、`/intent`、`/task`、`/goal` | must-migrate | command service behind Web API / structured interactions | skill activation preserves allowed-tools; `/plan` and `/intent` equivalents exist in Web |
| Hooks, agents config, SourcePool, sub-agents | `run_tui_with_model_inner` setup: `HookRunner::load`, `load_commands`, `load_profiles`, `AgentsConfig::load_or_default`, `SourcePool::with_persistence`, `build_subagent_runtime_for_session` | must-migrate | shared `build_code_session_services` helper | Web mode honors hooks, profiles, agents.toml, source logging, gated task dispatch |
| Browser/automation bridge | `TuiCodeUiAdapter`, `TuiControlCommand`, `TuiControlError` downcast in `CodeUiApiError::unsupported_from_error`, `handle_tui_control_command`, `CodeUiInitialController::LocalTui` | delete-with-tui | direct Web/session runtime adapter | HTTP submit/respond/cancel no longer enters TUI App; `src/internal/ai/web/*` no longer imports/downcasts TUI control errors; controller never returns `tui` |
| Local reclaim | `/control reclaim`, `reclaim_local_controller`, `reclaim_local_tui_controller`, `is_control_reclaim_command_input` | web-replace | lease expiry/detach/conflict handling | reclaim tests rewritten; no `/control reclaim` docs for `libra code` |
| PTY test harness | `tests/harness/*`, `tests/harness_self_test.rs`, `tests/code_ui_scenarios.rs`, `portable-pty`, `write_tui_line` | web-replace | Web process + HTTP/SSE harness | cross-process tests run without TTY |
| Thread version graph | `src/command/graph.rs::load_thread_graph`, `ThreadGraph::from_projection`, `run_graph_tui`, `render_graph` | web-replace | UI-neutral graph service + Web Code UI graph view | `libra graph` CLI/TUI 删除；Web 可显示同一 thread 的 DAG/list/detail，支持当前/历史 thread |

## 设计决策

### CLI 契约

最终用户契约：

```bash
libra code                         # starts Web Code UI
libra code --provider ollama ...    # starts Web Code UI with selected provider
libra code --port 4400 --host 127.0.0.1
libra code --resume <thread_id>
```

无兼容期删除决策：

- `--web` / `--web-only` 不保留旧入口，不出现在 help、synopsis、examples 或 docs。W2 同时删除 `CodeArgs.web_only` 字段、`alias = "web"`、`conflicts_with = "stdio"` 相关逻辑和所有 `args.web_only` 分支；旧脚本必须改用裸 `libra code`。
- 旧 `--web` / `--web-only` 调用应 fail fast：首选从 clap 参数结构中移除，使其成为 unknown argument；若团队选择保留自定义 usage error，也必须立即失败，不能启动 Web runtime，也不能在 help 中列出。
- 删除 "TUI mode" 和 "web-only mode" 概念。Web 是 `libra code` 的唯一 interactive mode，`execute_web_only` 应改名或收敛为 UI-neutral 的 `execute_web`。
- `--browser-control`（解析为 `BrowserControlMode`，`code.rs:250`，变体 `Off`/`Loopback`）默认改为 Web-primary：loopback host 上默认 `Loopback`，非 loopback host 上默认 `Off` 并展示 remote notice；显式 `--browser-control loopback` 仍必须要求 loopback host。解析入口是 `default_browser_control_mode`(`code.rs:1839`)。
- `--control`（`ControlMode`，`code.rs:235`，变体 `Observe`/`Write`）的帮助文案从 "Local TUI automation control" 改为 "Local Code UI automation control"。保留 token/lock/info 文件安全模型，但文档和错误信息不再提 TUI reclaim。

`--stdio` 也不是 Web interactive mode，不在 `libra code` 中保留兼容期。注意 `--stdio` 与 `--mcp-stdio` 在 `CodeArgs` 中是**同一个字段** `stdio`（`code.rs:600`，alias `mcp-stdio` 在 `code.rs:599`），处理这两者只需改 `stdio` 字段：

- W1 先提供替代入口，例如新的 `libra mcp --stdio` 或 `libra agent mcp --stdio`。
- W3 随后从 `CodeArgs`、help、docs 和 tests 中删除 `--stdio` / `--mcp-stdio`；若为了更友好的错误保留解析，也只能立即返回 usage error，提示使用新 MCP 命令，不能作为隐藏旧模式继续运行。

目标是让 `code` 命令只支持 Web interactive mode，把 MCP stdio 迁移到独立命令，避免 `code` 继续有非 Web mode。

### Runtime 架构

把 `libra code` 拆成两层：

- `CodeSessionRuntime`：provider-neutral session driver，负责消息、plan workflow、approval、tools、usage、goal、skills、hooks、multi-agent、session persistence。
- `CodeUiRuntimeHandle`：Web/API-facing adapter，负责 snapshot、SSE、controller lease、HTTP submit/respond/cancel。

Web-only 后，`src/command/code.rs` 不应引用：

- `App`, `AppConfig`, `ExitReason`
- `Tui`, `tui_init`, `tui_restore`
- `TuiCodeUiAdapter`
- `TuiControlCommand`
- `run_tui_with_model*`

`HeadlessCodeRuntime` 可以作为 `CodeSessionRuntime` 的起点，但必须补齐 TUI App 里的行为后才能删除默认 TUI。

### Web 版本图展示设计

`libra graph` 的价值不是终端交互本身，而是把 AI thread 的投影关系可视化：intent → plan → task → run → event/patchset。Web-only 后这成为 Code UI 的内置视图，而不是独立命令。

数据层：

- 从 `src/command/graph.rs` 提取不依赖 ratatui/crossterm 的 loader 和 DTO builder，例如 `src/internal/ai/graph/service.rs`。
- 复用 `ProjectionResolver::load_or_rebuild_thread_bundle`、`ProjectionRebuilder`、`HistoryManager`、`load_projection_index_rows`、`load_graph_object_details` 的语义，输出稳定 JSON DTO。
- DTO 至少包含：`thread_id`、`title`、`freshness`、`thread_version`、`scheduler_version`、`updated_at`、`selected_plan_id`、`active_task_id`、`active_run_id`、`nodes[]`、`edges[]`、`details`。
- `nodes[]` 使用稳定 `id` + `kind`（`intent`/`plan`/`task`/`run`/`event`/`patchset`）+ `label` + `status`（`queued`/`running`/`blocked`/`succeeded`/`failed`/`neutral`）+ `is_current` / `is_active` / `is_selected` 等 UI-neutral 标记。
- 大对象 detail 采用截断和懒加载：graph 初始响应只带摘要；选中节点时再通过 detail endpoint 或已有 snapshot detail 字段加载完整受限详情，继续沿用当前 `MAX_OBJECT_DETAIL_*` 的安全上限。

API / SSE：

- 新增 Web API：`GET /api/code/graph?thread_id=<uuid>`；未传 `thread_id` 时返回当前 session thread 的 graph，未建立 thread 时返回 empty state 而非 500。
- 如 detail 独立加载，则新增 `GET /api/code/graph/nodes/{kind}/{id}`，要求 `kind` 白名单校验和同 repo/session 权限边界。
- 当 `ProjectionResolver` / session runtime 观察到 plan、task、run、patchset 或 thread projection 变化时，SSE 发送 `graphUpdated` 或在现有 session snapshot 中递增 `graph_version`，前端据此重新拉取 graph。
- 所有 graph API 只读；不新增 graph 写控制，也不绕过 browser/automation controller lease。

Web UI：

- 在 Code UI 内增加 `Graph` tab/pane，入口放在 thread header、workflow/sidebar 或 transcript toolbar；支持当前 thread，也支持用 URL/query 打开历史 `thread_id`。
- 初版布局复刻当前 TUI 信息架构但适配浏览器：左侧 DAG/树，中央 children/list，右侧 detail panel；窄屏降级为 `Graph list → Detail` 的纵向布局。
- 不能做 canvas-only UI：节点和边可用 SVG/HTML 渲染，但必须提供键盘可达的 tree/list、ARIA label、状态文本、搜索/过滤和空/错误/加载态。
- 状态表达不能只靠颜色：沿用 glyph + label（OK/RUN/WAIT/BLOCK/FAIL）并映射到设计系统 token。
- 大 graph 使用虚拟列表、按层级折叠、节点 detail 懒加载；默认聚焦 `active_run_id`、`active_task_id` 或 `selected_plan_id`，支持搜索 object id / task title / status。

删除 CLI：

- Web graph 可用并有测试后，删除 `libra graph` 子命令、`src/command/graph.rs` 的 TUI renderer、`docs/commands/graph.md` 和 command/help/compat 注册。
- 若 `src/command/graph.rs` 中的 loader 被迁出后已无调用者，删除该文件；若只剩共享 DTO/service，放在 `src/internal/ai/graph/`，不得保留 `src/command/graph.rs` 作为隐藏入口。

## 实施阶段

### Phase 0: 行为盘点和冻结

- 列出 `src/internal/tui/app.rs` 中属于 agent/session 行为的函数，按 "必须迁移"、"Web 不需要"、"delete-with-tui" 分类；不要为 `libra graph` 预留 terminal/TUI 分类。
- 对必须迁移项补源代码级 guard 或单测，先固定现有行为：plan workflow、approval、request_user_input、goal、usage、skills、hooks、multi-agent、resume、cancel、session persistence。
- 标记 `docs/commands/code.md`、README、`docs/commands/code-control.md`、`docs/improvement/web.md`、`docs/improvement/test.md`、`docs/improvement/agent.md`、`tests/INDEX.md`、`tests/compat/README.md`、`tests/compat/help_flag_descriptions.rs`、`tests/code_cli_dispatch_test.rs`、`src/internal/ai/web/code_ui.rs` 中所有 TUI-default / `local-tui-control.md` 文案；旧 `docs/automation/local-tui-control.md` 仍存在，必须在 W13 完成迁移后再删。

Exit criteria:

- 有一张迁移清单，每个 TUI-owned 行为都有目标模块和测试目标。
- `rg "TUI|tui|--web|--web-only|--stdio" docs/commands/code.md docs/commands/code-control.md README.md docs/improvement/web.md` 的待改位置已确认（docs-consistency 检查现由 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix` 承担，不再有 shell 脚本可扫）。

### Phase 1: CLI 默认改为 Web

- 在 `src/command/code.rs` 中把 `execute()` 改为默认调用 Web path。
- 将 `execute_web_only()` 重命名为 `execute_web()`，更新注释、错误上下文和 banner。
- 删除 `CodeArgs.web_only`、`--web-only`、`--web` alias 和所有 `args.web_only` dispatch/validation 分支；旧 flags 不进入 Web runtime。
- `CODE_EXAMPLES` 改为 Web-first 示例；`libra code` 说明为 "Start the Web Code UI"。
- `validate_mode_args()` 删除 "TUI-specific flags rejected in web-only mode" 的旧规则。provider/model/context/resume/env-file/approval/network/goal 等 flags 都应在 Web mode 可用。
- `BrowserControlMode` 默认改为 Web-primary，覆盖 loopback 默认可写、非 loopback fail-closed 的矩阵。
- `ControlMode` 文案从 local TUI automation 改为 local Code UI automation。

Exit criteria:

- `CodeArgs::try_parse_from(["libra"])` 后走 Web runtime。
- `CodeArgs::try_parse_from(["libra", "--web"])` 和 `["libra", "--web-only"]` 失败，或返回立即 usage error；两者都不能启动 Web runtime。
- `src/command/code.rs` 默认路径不调用 `execute_tui`。
- `rg "web_only|--web-only|alias = \"web\"" src/command/code.rs tests/command tests/code_cli_dispatch_test.rs` 无结果。

### Phase 2: 抽出 TUI-owned agent 行为

- 从 `src/internal/tui/app.rs` 抽出 provider-neutral plan workflow：
  - IntentSpec draft / confirm
  - Plan draft / confirm
  - execute / modify / cancel / network policy selection
  - failed execution repair loop 和 `/plan continue` 等价能力
- 将 slash command 中影响 runtime 的部分改为命令服务，而不是 TUI key handler 私有逻辑：
  - goal start/status/cancel
  - usage summary
  - skill activation
  - task dispatch
- 把 hooks、skills、agent profiles、SourcePool、usage recorder、sub-agent runtime 的 session bootstrap 移到 Web runtime 构造路径。
- 为 Web 交互补 UI/API 表达：
  - plan review choice
  - intent review choice
  - network allow/deny
  - continue repair / cancel repair
  - goal status and cancellation

Exit criteria:

- `HeadlessCodeRuntime` 或新 `CodeSessionRuntime` 能跑完整 generic provider plan workflow。
- Web snapshot 能表达 TUI 过去展示的 pending plan/intent/approval 交互。
- `src/internal/tui/app.rs` 不再是任何 `libra code` 行为的唯一实现位置。

### Phase 3: Web automation/control 收口

- 删除 `TuiCodeUiAdapter` 对 `libra code` 的依赖；HTTP submit/respond/cancel 直接进入 Web/session runtime。
- `CodeUiControllerKind::Tui` 在 `libra code` session 中不再出现；wire enum 若还存在，只能服务旧 wire 反序列化测试，不能因为 `libra graph` 继续保留。
- 移除 `/control reclaim` 语义；controller 冲突由 browser/automation lease 过期、detach、cancel 解决。
- `code-control` 文档和实现从 "drive a local TUI" 改为 "drive a local Code UI session"。
- `control.json` 字段保留，但描述从 TUI session 改为 Code UI session。

Exit criteria:

- `GET /api/code/session` 在默认 `libra code` 中返回 `controller.kind in {none,browser,automation,cli}`，不会返回 `tui`。
- automation submit/respond/cancel 不经过 `TuiControlCommand`。
- 旧的 TUI reclaim tests 被删除或改写为 Web lease conflict/expiry tests。

### Phase 4: Web graph 和 TUI path 删除

- 先把 `src/command/graph.rs` 中的 thread graph loader、projection rows、object detail、status mapping 抽到 UI-neutral graph service；删除 ratatui/crossterm renderer。
- 在 Web API 中提供只读 graph endpoint，并在 Code UI 中增加 Graph 视图，覆盖 DAG/tree、children/list、detail、状态、搜索/过滤、空/错误/加载态。
- 删除 `libra graph` 子命令注册、`GraphArgs`、`GRAPH_EXAMPLES`、`docs/commands/graph.md` 和相关 compat/help/docs tests；README 不再列出 `libra graph`。
- 删除 `execute_tui`、`TuiLaunchConfig`、`run_tui_with_model*`、`build_tui_code_ui_runtime` 等只服务 `libra code` TUI 的代码。
- 从 `src/command/code.rs` imports 中移除 `crate::internal::tui::*`。
- 如果 `src/internal/tui/app.rs`、`bottom_pane.rs`、`chatwidget.rs`、`terminal.rs` 等只服务已删除的 Code TUI / graph TUI，删除；保留或迁移 `diff` / `markdown_render` 等可复用渲染逻辑，前提是仍有真实调用者。
- 删除 Codex "managed-tui" mode；Codex provider 只走 Web managed runtime。

Exit criteria:

- `rg "execute_tui|run_tui_with_model|TuiCodeUiAdapter|TuiControlCommand|tui_init\\(\\)" src/command/code.rs` 无结果。
- `rg -n "libra graph|GraphArgs|GRAPH_EXAMPLES|run_graph_tui|render_graph|src/command/graph.rs" src docs tests README.md` 无用户可见旧命令或 TUI renderer 引用（迁移计划除外）。
- Web graph API 和前端 Graph 视图有 Rust/API/React 覆盖，并能展示当前 thread 的 graph。
- `cargo check` 不再通过 `libra code` 或 `libra graph` 引用 ratatui/crossterm TUI runtime。

### Phase 5: 测试迁移

更新或删除以下测试目标：

- 删除/改写 `tests/harness_self_test.rs`：不再需要 PTY 启动 `libra code`。
- 改写 `tests/code_ui_scenarios.rs`：从 PTY harness 改为 Web process + HTTP/SSE/browser-control harness。
- 删除 `tests/code_codex_default_tui_test.rs`，新增 `code_web_default_test` 源码级 guard，确保 `libra code --provider codex` 不走 TUI。
- 更新 `tests/code_cli_dispatch_test.rs`：默认 Web、`--web` / `--web-only` 解析失败或 usage error、`--stdio` 迁移或拒绝。
- 更新 remote matrix JSON：移除 `controller_kind_tui_or_none`、`lease_detach_releases_to_local_tui` 等 TUI expectation。
- 保留并强化 `tests/ai_code_ui_headless_test.rs`、`tests/ai_code_ui_wire_test.rs`、`tests/code_ui_remote_*_matrix.rs`。
- 新增 Web graph 覆盖：graph DTO builder 单测、`GET /api/code/graph` API 测试、Web Graph 组件测试、SSE graph refresh 或 `graph_version` 更新测试。
- 同步 `Cargo.toml` 和 `tests/INDEX.md`，删除或重命名 test target 行。

新增覆盖：

- `libra code --port 0 --provider fake --browser-control loopback` 能启动 Web session，HTTP submit 后 transcript/SSE 更新。
- non-Codex provider Web mode 使用 `ProviderFactory`、env-file、vault fallback、approval/network policy。
- Codex Web mode 启动 managed app-server，browser submit/respond/cancel 正常。
- plan workflow 在 Web mode 中完成 intent review、plan review、execute、repair。
- Web Graph view 能展示当前 thread 的 intent/plan/task/run/patchset graph，并能打开历史 `thread_id`。
- no-terminal 环境下默认 `libra code` 不访问 stdin raw mode 或 alternate screen。

推荐验收命令：

```bash
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test ai_code_ui_wire_test \
  --test ai_code_ui_headless_test \
  --test code_ui_remote_lease_matrix \
  --test code_ui_remote_sse_matrix \
  --test code_ui_remote_state_matrix \
  --test code_ui_remote_security_matrix \
  --test code_ui_remote_generation_matrix \
  --test code_ui_remote_approval_matrix \
  -- --test-threads=1
pnpm --dir web lint
pnpm --dir web build
```

Ubuntu sandbox 条件门禁：

这不是每个 Web-only PR 的默认全量门禁。只有满足下列任一条件时，才必须在当前 Ubuntu 系统额外跑 sandbox smoke：

- 改动触达 `src/internal/ai/sandbox/*`、`src/command/sandbox.rs`、`template/seccomp-default.json`、`.libra/sandbox.toml` 解析或 `docs/sandbox-seccomp.md`。
- Web runtime 改动影响 `shell`、`apply_patch`、network policy、approval/sandbox approval、writable roots、session runtime context 传递。
- 从 TUI path 迁移到 Web/headless path 的代码原本依赖 `default_tui_runtime_context`、`handle_exec_approval_request` 或同等 sandbox/approval glue。
- 最终删除 TUI startup 前，需要证明 no-TTY Web mode 仍保留 sandbox evidence、approval 和 network-deny 行为。

Ubuntu smoke 命令：

```bash
cargo test --test command_test sandbox_status
cargo test --lib sandbox
cargo test --test ai_code_ui_headless_test
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test code_ui_remote_approval_matrix \
  -- --test-threads=1
libra sandbox status
libra sandbox --json status
libra sandbox --machine status
LIBRA_USE_LINUX_SANDBOX_BWRAP=1 libra sandbox status
LIBRA_LINUX_SANDBOX_EXE=/tmp/libra-never-exists \
  LIBRA_SANDBOX_ENFORCEMENT=required \
  libra sandbox status
```

Seccomp 扩展检查只在修改 seccomp/bwrap/helper/policy 时要求；普通 Web-only UI 改造不需要每次跑：

```bash
seccompiler-bin \
  --target-arch "$(uname -m)" \
  --input-file template/seccomp-default.json \
  --output-file ~/.libra/seccomp.bpf
LIBRA_SECCOMP_POLICY=~/.libra/seccomp.bpf libra sandbox status
```

验收重点：

- `libra sandbox status` 在 Ubuntu 上能清晰报告 `platform=linux`、`sandbox_type`、`effective_enforcement`、`bwrap_available` 和 warnings。
- 缺少 helper/bwrap 或 `LIBRA_SANDBOX_ENFORCEMENT=required` 时，不能静默降级执行需要 sandbox 的流程。
- Web/headless approval 仍通过 `CodeUiInteractionKind::SandboxApproval` 表达，cancel 能释放 pending approval。
- `--network-access deny` 或等价 config 在 Web mode 中继续阻止 network tool，而不是因为 TUI 删除而绕过 runtime policy。

### Phase 6: 文档和公开契约

- 更新 `docs/commands/code.md`：
  - synopsis 改为 Web-only
  - 删除 TUI mode、TUI output、TUI troubleshooting
  - 从 options/help/examples 中删除 `--web` / `--web-only`，release notes 明确旧脚本改用裸 `libra code`
  - 删除 `--stdio` / `--mcp-stdio`，或只保留立即失败且指向新 MCP 命令的 usage error
  - browser-control 默认矩阵改为 Web-primary
- 更新 README 的 "Libra Code Modes"，只保留 Web interactive mode 和外部 MCP 替代入口；版本图入口描述为 Web Code UI 内置 Graph view。
- 将 `docs/commands/code-control.md` 从 "drive a local TUI" 改写为 local Code UI automation control；旧 `docs/automation/local-tui-control.md` 在 W13 迁完错误码测试和引用后删除。
- 更新 `docs/improvement/web.md`，把 "Headless web-only follow-up" 转为默认运行时要求。
- 删除或重定向 `docs/commands/graph.md`；更新 `COMPATIBILITY.md`、`docs/error-codes.md`、`docs/commands/README.md` 和 release notes，明确 `libra graph` 已由 Web Graph view 取代。
- 搜索并消除误导文案：

```bash
rg -n "TUI|tui|web-only|--web|--web-only|Local TUI|managed-tui|libra graph" README.md docs/commands docs/improvement docs/automation src tests web
```

Exit criteria:

- `docs/commands/code.md` 不再声称 `libra code` 支持 TUI。
- README 首段不再说 `libra code` starts an interactive TUI。
- CI 文档检查不再依赖 local TUI automation wording。
- `docs/commands/graph.md` 不再作为可见命令文档存在；README 不再推荐 `libra graph`。

## 可直接派发的任务卡

下面的任务卡按 PR 粒度拆分。每张卡都可以直接交给 Codex 或 Claude Code 执行；执行者应只做该卡范围内的改动，除非测试暴露了同一行为链上的必要修复。

推荐执行顺序：

1. W0 校准清单。
2. W1 先提供 MCP stdio 替代入口。
3. W2 切默认 Web 并同步删除 `--web` / `--web-only`，W3 再从 `code` 拒绝 stdio。
4. W4 调整 Web browser-control 默认。
5. W5 抽共享 session bootstrap。
6. W6 迁移 plan workflow，W7 迁移 goal/usage/skill/task 控制。这两张可并行，但合并前要共同跑 Web headless/remote matrix。
7. W8 移除 TUI bridge/reclaim。
8. W8G 迁移 graph 到 Web 并删除 `libra graph` 命令。
9. W9 删除 `libra code` TUI startup。
10. W10 替换 PTY harness。
11. W11 文档/help 收口。
12. W12 最终审计。
13. W13：舍弃 `docs/automation/local-tui-control.md` 并清理 remaining code/test/docs references（当前尚未完成；不能假设旧文档已删除）。

禁止提前执行的依赖：

- 未完成 W1 时不要执行 W3，否则 MCP stdio 用户没有替代入口。
- 未完成 W5/W6/W7 时不要执行 W8/W9，否则会丢 plan workflow、goal、usage、skills、task dispatch 等能力。
- 未完成 W8G 前不要删除 `src/command/graph.rs` 里的 loader 逻辑；要先迁到 UI-neutral graph service 并接入 Web API/UI。
- 未完成 W8G/W9 前不要移除 ratatui/crossterm 依赖；要先确认 Code TUI 与 graph TUI 都没有真实调用者。
- 未完成 W10 前不要删除所有 harness fixture；要先保证 Web harness 覆盖同等跨进程行为。
- 不要把 `--web` / `--web-only` 留作旧脚本兼容入口；它们必须随 W2 同步删除或立即拒绝。

### W0: 校准迁移清单

目标：验证并扩充上方 `TUI-owned behavior inventory`，避免后续删除时丢能力。

可交给 Agent 的 prompt：

```text
阅读 docs/development/web-only.md 和当前代码，校准 `libra code` Web-only 迁移清单。
只修改 docs/development/web-only.md。
更新 `TUI-owned behavior inventory` 小节，补齐 src/internal/tui/app.rs 中必须迁移到 Web/session runtime 的遗漏行为、相关函数或搜索关键词、目标模块和覆盖测试。
不要改 Rust 代码。
```

涉及文件：

- `docs/development/web-only.md`（本文件即迁移计划；原稿误写的 `docs/agent/web-only.md` 不存在）
- 只读参考：`src/internal/tui/app.rs`
- 只读参考：`src/internal/tui/slash_command.rs`
- 只读参考：`src/internal/tui/goal_session.rs`
- 只读参考：`src/command/code.rs`
- 只读参考：`src/internal/ai/web/headless.rs`

具体步骤：

1. 用 `rg -n "phase|intent|plan|repair|goal|usage|skill|task|request_user_input|approval|cancel|session" src/internal/tui/app.rs src/internal/tui` 找行为入口。
2. 将行为分为 `must-migrate`、`web-replace`、`delete-with-tui` 三类；thread graph 属于 `web-replace`，不能作为 `graph/shared` 保留。
3. 给每个 `must-migrate` 项写目标模块，优先使用 `src/internal/ai/web/headless.rs` 或一个新建的 provider-neutral runtime 模块。
4. 给每项写最小验收测试，例如 `ai_code_ui_headless_test`、`code_ui_remote_approval_matrix`、新增 source guard。

验收：

- `TUI-owned behavior inventory` 表与当前 `rg` 结果一致，没有明显遗漏的 TUI-owned runtime 行为。
- 表中至少包含 plan workflow、approval/user input、goal、usage、skills、hooks、multi-agent、resume、cancel、controller reclaim、thread graph。
- 本卡不产生 Rust 编译变更。

### W1: 提供 MCP stdio 替代入口

目标：在移除 `libra code --stdio` 前，先提供非 `code` 的 MCP stdio 入口，避免最终状态里 MCP 用户无替代路径；W1 与 W3 应作为同一发布/同一 PR stack 的连续改动，不设计公开兼容期。

可交给 Agent 的 prompt：

```text
为 Libra 增加独立 MCP stdio 入口，目标是后续让 `libra code` 只保留 Web interactive mode。
新增或选择一个非 `code` 子命令，例如 `libra mcp --stdio`；复用当前 src/command/code.rs 的 execute_stdio/init_mcp_server 逻辑。
本卡只新增替代入口和测试/文档；若作为单独前置提交，`libra code --stdio` 的旧行为只允许作为未发布或同栈过渡，最终合并/发布前必须由 W3 移除或立即拒绝。
```

涉及文件：

- `src/cli.rs`
- `src/command/mod.rs`
- 新增：`src/command/mcp.rs` 或团队确认的等价路径
- `src/command/code.rs`（只抽公共 helper，不改默认行为）
- `docs/commands/README.md`
- 新增或更新：`docs/commands/mcp.md`
- `tests/command/*` 或新增 integration test
- `tests/INDEX.md`（如新增 test target）

具体步骤：

1. 从 `execute_stdio(args: &CodeArgs)` 抽出可复用 helper，例如 `run_mcp_stdio(working_dir: &Path) -> CliResult<()>`。
2. 新增 `McpArgs`，至少支持 `--cwd` / `--repo` 或沿用 `code` 的工作目录解析语义。
3. 在 `src/cli.rs::Commands` 增加 `Mcp`，并在 `command_preflight` 中设置 repo preflight。
4. 给 `libra mcp --stdio` 加 help test 或 clap parse test。
5. 文档说明 MCP stdio 的最终入口是 `libra mcp --stdio`；不要把 `libra code --stdio` 描述为可长期使用的兼容入口。

验收：

- `cargo test --test command_test mcp` 或新增对应 target 通过。
- `libra mcp --help` 显示 stdio 用法。
- 若 W1 独立执行，`libra code --stdio` 只作为同栈过渡；最终合并/发布前 W3 必须让旧入口移除或 fail fast。

### W2: 将 `libra code` 默认入口改为 Web 并删除旧 Web flags

目标：`libra code` 不加 flag 时启动当前 Web runtime；`--web` / `--web-only` 不设兼容期，随默认切换同步删除或立即拒绝。

可交给 Agent 的 prompt：

```text
把 `libra code` 默认入口切到 Web runtime。不要保留 `--web`/`--web-only` 旧入口；同步删除 `CodeArgs.web_only`、`web` alias、相关 conflicts 和 `args.web_only` 分支，或让旧 flags 立即返回 usage error。
不要删除 TUI 代码，只让默认 dispatch 不再调用 execute_tui。
同步更新 src/command/code.rs 内部单测、tests/code_cli_dispatch_test.rs、tests/command/code_test.rs 中与默认 TUI、旧 `--web-only` / `--web` flags 相关的断言。
```

涉及文件：

- `src/command/code.rs`
- `tests/code_cli_dispatch_test.rs`
- `tests/command/code_test.rs`
- 可能涉及：`tests/code_mcp_dual_entry_test.rs`

具体步骤：

1. 将 `execute_web_only` 重命名为 `execute_web`，或先保留函数名但让默认分支调用它。
2. `execute(args, output)` 改为：`--stdio` 仍走旧 stdio 或 W3 的拒绝；其他都走 Web。
3. 删除 `CodeArgs.web_only` 字段、`--web-only` long flag、`alias = "web"`、`conflicts_with = "stdio"` 以及所有 `args.web_only` 引用；若选择自定义 usage error，也不得在 help 中展示旧 flags。
4. 删除 `validate_mode_args()` 中 `if args.web_only { reject_non_tui_flags(...) }` 的行为；Web 是默认后 provider/model/context/resume/env-file/approval/network 都应合法。
5. 更新 `rejects_tui_flags_in_web_mode`、`accepts_default_tui_mode` 等单测名称和断言，新增旧 `--web` / `--web-only` 失败断言。
6. `CODE_EXAMPLES` 第一行改为 Web session 示例，且不包含 `--web` / `--web-only`。

验收：

- `CodeArgs::try_parse_from(["libra"])` 的单测说明默认 Web。
- `CodeArgs::try_parse_from(["libra", "--web"])` 和 `["libra", "--web-only"]` 失败，或旧 flags 触发立即 usage error；两者都不进入 `execute_web`。
- `validate_mode_args()` 接受 `--provider ollama --model llama3`。
- `validate_mode_args()` 接受 `--env-file .env.test`。
- `rg "web_only|--web-only|alias = \"web\"|Launch the default TUI session|accepts_default_tui_mode|rejects_tui_flags_in_web_mode" src/command/code.rs tests/command/code_test.rs tests/code_cli_dispatch_test.rs` 无结果。

### W3: 从 `libra code` 拒绝 stdio 模式

目标：让 `code` 命令只剩 Web interactive mode。此卡依赖 W1。

可交给 Agent 的 prompt：

```text
在已有 `libra mcp --stdio` 替代入口的前提下，让 `libra code --stdio` 不再作为 code mode 运行。
错误必须明确提示使用新的 MCP stdio 命令。
删除 `--stdio` 和 `--mcp-stdio` 在 `CodeArgs` 的支持；若为了可读错误保留解析，也只能立即 command_usage，不能作为隐藏旧模式运行。
同步更新测试和 docs/commands/code.md。
```

涉及文件：

- `src/command/code.rs`
- `src/cli.rs`（如果 help/about 要同步）
- `docs/commands/code.md`
- `README.md`
- `tests/code_cli_dispatch_test.rs`
- `tests/code_mcp_dual_entry_test.rs`
- `tests/e2e_mcp_flow.rs`
- `docs/commands/mcp.md`

具体步骤：

1. 移除 `CodeArgs.stdio` / `alias = "mcp-stdio"`；如需自定义迁移提示，只保留最小解析入口并立即返回 usage error，不作为隐藏旧模式运行。
2. 错误文案：`libra code is Web-only; use libra mcp --stdio for MCP stdio transport`。
3. 更新 `web_only_and_stdio_are_mutually_exclusive` 一类测试为 `code_stdio_is_rejected_with_migration_hint`。
4. 将 `tests/e2e_mcp_flow.rs` 中的旧 `libra code --web-only` / `--stdio` 用法迁到裸 `libra code` 和新 MCP 命令，或拆成 Web/MCP 两个测试。
5. 文档删除 "code supports MCP/stdio mode"。

验收：

- `libra code --stdio` 非零退出并包含新命令提示。
- 新 MCP stdio integration test 仍覆盖 MCP server。
- `rg -n "libra code --stdio|--mcp-stdio|MCP/stdio mode|libra code --web|libra code --web-only" docs/commands/code.md README.md tests` 只剩迁移说明或无结果。

### W4: Web browser-control 默认改为可写

目标：Web 是唯一交互面后，loopback browser 默认应能提交消息；非 loopback 继续 fail closed。

可交给 Agent 的 prompt：

```text
调整 `libra code` Web-only 设计下的 browser-control 默认值。
默认 `libra code` 在 loopback host 上启用 BrowserControlMode::Loopback；host 非 loopback 时默认 Off 并继续 remote notice。
显式 `--browser-control loopback` 在非 loopback host 上仍报错。
更新 src/command/code.rs browser_control_resolution_matrix_pins_mode_provider_and_host_contract。
```

涉及文件：

- `src/command/code.rs`
- `src/internal/ai/web/code_ui.rs`（如 controller default tests 需要调整）
- `tests/data/code_ui_remote/lease_cases.json`
- `tests/data/code_ui_remote/sse_cases.json`
- `tests/harness/matrix.rs`
- `docs/commands/code.md`

具体步骤：

1. 修改 `default_browser_control_mode(args)`：非 stdio 的 Web runtime 在 loopback host 上默认为 `Loopback`。
2. 保留显式 `Off` 覆盖默认值。
3. 更新矩阵用例名称，删除 TUI/non-Codex web-only 的旧区分。
4. 更新远程 fixture 中 `controller.kind` 的期望，从 `tui/none` 迁移到 `none/browser` 或具体 Web controller 状态。

验收：

- `browser_control_resolution_matrix_pins_mode_provider_and_host_contract` 覆盖：
  - default `libra code --host 127.0.0.1` -> loopback
  - default `libra code --host 0.0.0.0` -> off
  - explicit off on loopback -> off
  - explicit loopback on non-loopback -> error
- Web submit scenario 不再需要用户显式传 `--browser-control loopback`。

### W5: 抽出 Web session bootstrap parity

目标：Web runtime 构造路径具备原 TUI session bootstrap 的配置能力，但还不迁移完整 plan workflow。

可交给 Agent 的 prompt：

```text
把 `run_tui_with_model_inner` 中与 terminal rendering 无关的 session bootstrap 能力迁到 Web/headless runtime。
目标是 Web mode 支持 hooks、commands、skills、agent profiles、agents.toml、SourcePool、usage recorder、approval config、runtime context。
不要删除 TUI path；先让 Web path 和 TUI path 共享 helper。
```

涉及文件：

- `src/command/code.rs`
- `src/internal/ai/web/headless.rs`
- 可能新增：`src/internal/ai/web/session_runtime.rs` 或 `src/internal/ai/session_runtime.rs`
- `tests/ai_code_ui_headless_test.rs`
- `tests/code_tool_acl_test.rs`
- `tests/ai_usage_stats_test.rs`
- `tests/ai_usage_tui_test.rs`（后续可能改名）

具体步骤：

1. 从 `run_tui_with_model_inner` 提取无 UI 副作用的 helper：
   - hook loading
   - `load_commands`
   - `load_skills`
   - `load_profiles` / `AgentProfileRouter`
   - `AgentsConfig::load_or_default`
   - SourcePool persistence
   - usage recorder/context
2. 给 helper 命名为 UI-neutral，例如 `build_code_session_services(...)`。
3. 在 `build_headless_web_code_ui_runtime` 中使用同一 helper。
4. 把 `default_tui_runtime_context` 重命名或包装为 `default_code_runtime_context`；旧名字可暂时保留调用新函数。
5. 保持 `task` tool 仍按 feature/config gate，不要无条件开放。

验收：

- Web/headless tests 能证明 `--env-file`、`--network-access`、`--approval-policy`、`--approval-ttl` 在 Web mode 生效。
- `build_headless_tool_registry_omits_task_tool_in_flag_off_default` 仍通过。
- `rg "default_tui_runtime_context" src/internal/ai/web src/command/code.rs` 只剩兼容 wrapper 或无 Web 调用。
- 本卡触达 runtime context，合并前按 Phase 5 的 Ubuntu sandbox 条件门禁至少跑 `command_test sandbox_status`、`ai_code_ui_headless_test`、`code_ui_remote_approval_matrix` 和 `libra sandbox status` smoke。

### W6: 迁移 plan workflow 到 provider-neutral runtime

目标：Web runtime 支持原 generic provider TUI 的 IntentSpec/Plan 确认和 repair loop。

可交给 Agent 的 prompt：

```text
将 generic provider 的 IntentSpec/Plan 两阶段工作流从 src/internal/tui/app.rs 抽到 provider-neutral runtime。
Web/headless runtime 必须通过 CodeUiInteractionRequest 暴露 intent review、plan review、network allow/deny、modify/cancel、repair continue 等交互。
本卡只做 runtime 和 Rust tests；前端若缺控件，可以先用现有 interaction panel 的 generic option 渲染。
```

涉及文件：

- `src/internal/tui/app.rs`（只抽逻辑，不做大删除）
- `src/internal/ai/web/headless.rs`
- `src/internal/ai/web/code_ui.rs`
- 可能新增：`src/internal/ai/code_session/plan_workflow.rs`
- `tests/ai_code_ui_headless_test.rs`
- `tests/code_ui_remote_approval_matrix.rs`
- `tests/data/code_ui_remote/approval_cases.json`
- `tests/code_ui_remote_generation_matrix.rs`

具体步骤：

1. 定位 TUI App 中 Phase 0/1/2 相关 helper，搜索 `intent_review_choice`、`post_plan_choice`、`submit_intent_draft`、`submit_plan_draft`、`phase0`、`phase1`、`repair`。
2. 创建不依赖 ratatui 的 workflow state machine。
3. 将 pending choices 映射为 `CodeUiInteractionKind::IntentReviewChoice` / `PostPlanChoice` 或现有 enum。
4. `HeadlessCodeRuntime::submit_message` 不再直接跑单 turn，而是先进入 workflow driver。
5. 保证 mutating tools 只在 confirmed plan 后执行。
6. 把 network allow/deny 写入 runtime context 或 per-turn policy。

验收：

- 新增 headless test：用户提交普通开发请求后，snapshot 出现 pending intent review，不执行 `apply_patch`。
- 响应 intent confirm 后出现 pending plan review。
- 响应 execute plan 后才允许 mutating tools。
- repair threshold 达到后 Web runtime 进入 pending interaction，而不是静默继续或失败。
- 本卡若实现或改动 network allow/deny、sandbox approval 或 mutating tool gating，合并前触发 Phase 5 的 Ubuntu sandbox 条件门禁。

### W7: Web 化 goal、usage、skill、task 控制

目标：把 TUI slash commands 中的 runtime 功能变成 Web/API 可调用能力。

可交给 Agent 的 prompt：

```text
把 TUI slash commands 中影响 Code runtime 状态的功能迁到 Web/session runtime。
实现 goal start/status/cancel、usage summary、skill activation、task dispatch 的 Web API 或 CodeUiCommandAdapter 方法。
不要保留对 TuiControlCommand 的依赖。
```

涉及文件：

- `src/internal/tui/slash_command.rs`（只读参考）
- `src/internal/tui/goal_session.rs`（迁移或抽共享）
- `src/internal/ai/web/code_ui.rs`
- `src/internal/ai/web/mod.rs`
- `src/internal/ai/web/headless.rs`
- `web/src/lib/code-ui/*`（如需要按钮/API client）
- `tests/ai_goal_state_test.rs`
- `tests/ai_goal_flag_off_regression_test.rs`
- `tests/code_ui_remote_state_matrix.rs`

具体步骤：

1. 将 `goal_session` 从 `src/internal/tui` 迁到 UI-neutral 模块，例如 `src/internal/ai/goal/session.rs`。
2. `CodeUiCommandAdapter` 已有 `goal_start/goal_status/goal_cancel/task_dispatch` 方法时，改为由 Web runtime 实现。
3. HTTP endpoints 如已存在则改 adapter；不存在则新增 `/api/code/goal/*` 或复用 `/api/code/messages` structured command，选择一种并记录在 docs。
4. usage summary 以 transcript info note 或 snapshot field 表达。
5. skill activation 复用 `SkillDispatcher`，但必须保持 allowed-tools 限制。

验收：

- Web runtime 可通过 HTTP 或 Code UI command start/cancel goal。
- `task` dispatch 在 flag off 时仍返回 actionable unsupported。
- `rg "goal_session" src/internal/tui src/internal/ai` 显示源实现已不在 TUI 私有模块。

### W8: 移除 TUI bridge 和 reclaim 语义

目标：Web write 不再通过 TUI App 转发；controller model 不再出现 local TUI owner。

可交给 Agent 的 prompt：

```text
在 Web runtime 已能处理 submit/respond/cancel/goal/task 后，移除 `libra code` 对 TuiCodeUiAdapter/TuiControlCommand 的依赖。
删除 Web controller 的 local TUI reclaim 语义，保留 browser/automation lease、token、audit、body limit。
Graph TUI 不再作为保留项；W8G 会迁出 loader 并删除 `libra graph`。
```

涉及文件：

- `src/command/code.rs`
- `src/internal/tui/code_ui_adapter.rs`
- `src/internal/tui/control.rs`
- `src/internal/tui/app.rs`
- `src/internal/ai/web/code_ui.rs`
- `src/internal/ai/web/mod.rs`
- `tests/code_ui_scenarios.rs`
- `tests/harness/scenario.rs`
- `tests/data/code_ui_remote/lease_cases.json`
- `tests/harness/matrix.rs`

具体步骤：

1. 删除 `code_control_tx/code_control_rx` 从 `libra code` startup 的创建和注入。
2. `CodeUiInitialController::LocalTui` 不再由 `libra code` 使用；保留 enum 兼容时加注释。
3. 移除 `/control reclaim` 对 `libra code` 的文档和测试。
4. 移除 `src/internal/ai/web/code_ui.rs::CodeUiApiError::unsupported_from_error` 对 `crate::internal::tui::control::TuiControlError` 的 downcast；若仍需要结构化 control 错误码，先定义 UI-neutral error type。
5. audit policy 名称从 `local-tui-control/v1` 迁到 `local-code-ui-control/v1`，或保留旧名称一版并文档说明兼容。
6. 更新 matrix assertion：删除 `controller_kind_tui_or_none`。

验收：

- `rg "TuiCodeUiAdapter|TuiControlCommand|LocalTui|reclaim_local_tui_controller" src/command/code.rs src/internal/ai/web src/internal/tui tests` 中不再有 `libra code` runtime 依赖。
- `rg "TuiControlError|crate::internal::tui" src/internal/ai/web` 无结果，Web wire/runtime 层不再依赖 TUI control 模块。
- lease detach 后 controller 回到 `none` 或 browser-expected 状态，不回到 `tui`。
- control audit/redaction tests 仍通过。

### W8G: Web graph view 并删除 `libra graph`

目标：把现有 `libra graph` TUI 的 thread graph 能力迁到 Web Code UI，然后删除独立 graph 命令。

可交给 Agent 的 prompt：

```text
把 `libra graph` 的数据加载和展示迁移到 Web Code UI。
先从 src/command/graph.rs 提取 UI-neutral graph service/DTO，新增只读 /api/code/graph endpoint 和 Web Graph view。
Web Graph view 覆盖 DAG/tree、children/list、detail、状态、搜索/过滤、空/错误/加载态；不能做 canvas-only UI，必须键盘可达。
Graph Web 覆盖完成后，删除 `libra graph` 子命令、docs/commands/graph.md、GraphArgs/GRAPH_EXAMPLES 和 ratatui graph renderer。
```

涉及文件：

- `src/command/graph.rs`（提取 loader 后删除或只作为临时迁移来源）
- `src/command/mod.rs` / CLI command registry（删除 `graph` 子命令）
- 可能新增：`src/internal/ai/graph/service.rs`
- `src/internal/ai/web/mod.rs`
- `src/internal/ai/web/code_ui.rs`
- `web/src/components/workspace/*`（新增 Graph view/tab/panel）
- `docs/commands/graph.md`（删除或从可见命令索引移除）
- `README.md`、`docs/commands/README.md`、`COMPATIBILITY.md`
- `tests/compat/*`、`tests/INDEX.md`

具体步骤：

1. 从 `src/command/graph.rs` 提取 `load_thread_graph`、`load_bundle_for_graph`、projection index rows、object detail、`ThreadGraph::from_projection` 等数据逻辑到 UI-neutral graph service。
2. 定义 JSON DTO：thread metadata、nodes、edges、summary/detail、selected/active/current 标记和状态枚举；不要暴露 ratatui `Line`/`Color`/glyph-only 数据结构。
3. 新增 `GET /api/code/graph?thread_id=<uuid>`；未传 `thread_id` 时使用当前 session thread；无 thread 返回空状态。
4. 如 detail 单独加载，新增 `GET /api/code/graph/nodes/{kind}/{id}`，限制 kind 白名单、同 repo/session 权限和 detail 截断上限。
5. 在 Web UI 增加 Graph 视图：入口来自 thread header/sidebar/workflow toolbar；三栏布局（DAG/tree、children/list、detail）+ 窄屏纵向布局；支持键盘导航、搜索/过滤、状态 legend、空/错误/加载态。
6. 通过 SSE `graphUpdated` 或 snapshot `graph_version` 在 plan/task/run/patchset 变化后刷新 graph。
7. 删除 `libra graph` 命令注册、`GraphArgs`、`GRAPH_EXAMPLES`、`run_graph_tui`、`render_graph`、`docs/commands/graph.md` 和 help/compat/docs 索引引用。
8. 删除 `run_tui_with_model_inner` 退出时的 `Inspect this thread graph with: libra graph {thread_id}` 提示，改为 Web UI 内部入口或本地 URL 提示。

验收：

- `GET /api/code/graph` 能返回当前 thread graph；指定历史 `thread_id` 时返回同 repo 内对应 graph。
- Web Graph view 能展示至少 intent、plan、task、run、patchset 节点及状态，并可打开节点 detail。
- Graph view 可键盘操作；颜色不是唯一状态表达；有 loading、empty、error states。
- `rg -n "libra graph|GraphArgs|GRAPH_EXAMPLES|run_graph_tui|render_graph|Inspect this thread graph" src docs tests README.md` 无结果（本迁移计划除外）。
- `docs/commands/README.md` 和 `COMPATIBILITY.md` 不再列出 `graph` 可见命令。
- Web component/API tests 覆盖 graph DTO、empty state、selected node detail、SSE/snapshot refresh。

### W9: 删除 `libra code` TUI startup 代码

目标：完成代码层 Web-only，`src/command/code.rs` 不再引用 terminal/TUI runtime。

可交给 Agent 的 prompt：

```text
删除 `libra code` 的 TUI startup path。
移除 execute_tui、TuiLaunchConfig、run_tui_with_model*、build_tui_code_ui_runtime 和相关 imports。
`libra graph` 已在 W8G 迁到 Web 并删除；若 src/internal/tui/terminal.rs 只剩 graph/Code TUI 调用者，一并删除。
```

涉及文件：

- `src/command/code.rs`
- `src/internal/tui/mod.rs`
- `src/command/graph.rs`（应已由 W8G 删除或清空为迁移来源；本卡不得保留命令入口）
- `src/internal/mod.rs`
- `Cargo.toml`（确认 ratatui/crossterm 无调用者后移除依赖）
- `tests/code_codex_default_tui_test.rs`
- `tests/harness_self_test.rs`
- `tests/INDEX.md`

具体步骤：

1. 删除 code.rs 中 TUI-only imports。
2. 删除 `execute_tui` 和 `run_tui_with_model*`。
3. 删除或迁移 `build_tui_code_ui_capabilities` / `build_tui_code_ui_runtime`；保留 `build_tui_code_ui_transcript` 时必须重命名为 UI-neutral，例如 `transcript_from_session`.
4. `agent_codex` path 只保留 Web managed runtime。
5. 更新 `code_codex_default_tui_test` 为 `code_web_default_test`，断言不再引用 TUI path。

验收：

- `rg "execute_tui|run_tui_with_model|TuiLaunchConfig|TuiCodeUiAdapter|TuiControlCommand|tui_init\\(\\)|tui_restore\\(" src/command/code.rs` 无结果。
- `cargo check` 通过。
- `rg -n "libra graph|src/command/graph.rs|ratatui|crossterm::event" src docs tests README.md Cargo.toml` 无旧 graph/TUI 依赖（本迁移计划除外）。

### W10: 替换 PTY harness 为 Web harness

目标：测试仍覆盖跨进程行为，但不再启动 TUI/PTY。

可交给 Agent 的 prompt：

```text
把 `libra code` 的跨进程测试 harness 从 PTY/TUI 改为 Web process harness。
当前 harness 用 portable-pty (Cargo.toml:156, tests/harness/code_session.rs 的 native_pty_system()) 启动伪终端，用 write_tui_line 注入 TUI 按键。
新 harness 启动 `libra code --provider fake --port 0 --mcp-port 0`，读取 control.json 或 stdout 中的 bound URL，通过 HTTP/SSE 完成 attach/submit/respond/cancel。
删除 portable-pty 的 PTY 启动、write_tui_line 和 /control reclaim 相关 DSL。
若改 CI step 名（"Run TUI automation scenarios"），同步 .github/workflows/base.yml（docs-consistency 检查现位于 tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix，不再有 shell 脚本需要改）。
```

涉及文件：

- `tests/harness/code_session.rs`
- `tests/harness/scenario.rs`
- `tests/harness_self_test.rs`
- `tests/code_ui_scenarios.rs`
- `tests/data/code_ui_remote/*.json`
- `tests/INDEX.md`
- `.github/workflows/base.yml`

具体步骤：

1. 将 harness 启动方式从 `portable-pty` 改为 `std::process::Command`。
2. 保留 artifact：stdout/stderr、libra.log、control.json、SSE log。
3. 删除 `write_tui_line`，新增 `post_message`、`attach_browser`、`attach_automation`、`respond_interaction`、`cancel_turn`。
4. 改写 reclaim scenario 为 lease conflict/expiry/detach scenario。
5. CI step 名称从 "Run TUI automation scenarios" 改为 "Run Code UI automation scenarios"；此改名只需落到 `.github/workflows/base.yml`（旧的 (now-removed) docs-consistency 脚本曾用 `require_doc "Run TUI automation scenarios" ".github/workflows/base.yml"` 钉这一步名，其检查现已迁入 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`，不再断言 CI step 名）。
6. 改完确认 `Cargo.toml` 是否仍有其它 `portable-pty` 使用方；无则移除依赖。

验收：

- `LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1` 不需要 TTY。
- `rg "portable-pty|write_tui_line|/control reclaim|pty.log|TUI automation" tests .github/workflows/base.yml` 无默认 Code UI harness 依赖。
- `cargo test --test compat_matrix_alignment` 通过（docs-consistency 检查现断言 `docs/commands/code-control.md` 覆盖 `/api/code/*` 路由，已不在 shell 脚本中）。

### W11: 文档和 help 收口

目标：用户文档不再暗示 `libra code` 支持 TUI、stdio mode，或冗余 `--web` / `--web-only` flags。

可交给 Agent 的 prompt：

```text
更新用户文档和 help 文案，使 `libra code` 只描述 Web mode。
删除 `libra graph` 命令文档和 help/compat 索引；版本图改写为 Web Code UI 的 Graph view。
将 local TUI automation 文档改为 local Code UI automation，并同步 docs consistency checks。
```

涉及文件：

- `README.md`
- `docs/commands/code.md`
- `docs/commands/code-control.md`（local TUI → local Code UI 改写；须覆盖 consistency 脚本断言的 13 个 `/api/code/*` endpoint 与 `X-Libra-Control-Token`/`X-Code-Controller-Token` header）
- `docs/commands/graph.md`（删除或从可见命令索引移除）
- `docs/commands/README.md`
- `docs/improvement/web.md`
- `COMPATIBILITY.md`
- `docs/error-codes.md`
- `tests/compat/matrix_alignment.rs`
- `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`（docs-consistency 检查现归这里，已断言 code-control.md 覆盖 `/api/code/*` 路由；本卡只需复核 `cargo test --test compat_matrix_alignment` 通过）

具体步骤：

1. `docs/commands/code.md` synopsis 只保留 Web usage 和 provider flags；options/help/examples 不列 `--web` / `--web-only`。
2. README "Libra Code Modes" 改为 Web interactive + separate MCP command，不再展示 `libra code --web` / `--web-only`。
3. `docs/commands/code-control.md` 说明从 live TUI 改为 live Code UI session；确认它仍覆盖脚本断言的 13 个 `/api/code/*` endpoint 与两个 control header（脚本已把这些 `require_doc` repoint 到此文件）。
4. `docs/automation/local-tui-control.md` 仍是待删除旧文档；W11 只负责把用户可见 control 内容补齐到 `code-control.md`，W13 再迁移测试/索引引用并删除旧文档。
5. 删除 `docs/commands/graph.md` 的 TUI 文案；如保留历史迁移说明，必须明确 `libra graph` 不再是可见命令，入口是 Web Graph view。

验收：

- `rg -n "libra code.*TUI|TUI Mode \\(Default\\)|Local TUI Automation|libra code --stdio|libra code --web|libra code --web-only|libra graph|--web-only.*without the TUI" README.md docs/commands docs/improvement docs/automation tests` 无误导结果（本迁移计划或历史说明除外）。
- `cargo test --test compat_matrix_alignment` 通过。
- `cargo test --test compat_matrix_alignment` 通过（docs-consistency 检查断言 `docs/commands/code-control.md` 仍覆盖所有 `/api/code/*` 路由）。

### W12: 最终删除检查和 release gate

目标：证明最终状态满足 Web-only。

可交给 Agent 的 prompt：

```text
执行 `libra code` Web-only 迁移的最终审计。
不要新增功能；只补遗漏的删除、测试名、文档名和 guard。
用 rg 和测试证明 src/command/code.rs 不再引用 TUI runtime，默认 code 可在无 TTY 环境启动 Web server，版本图只能通过 Web Graph view 访问。
```

涉及文件：

- 按审计结果最小修改
- `docs/development/web-only.md`（更新状态和完成记录）
- `tests/INDEX.md`

必跑检查：

```bash
rg "execute_tui|run_tui_with_model|TuiLaunchConfig|TuiCodeUiAdapter|TuiControlCommand|tui_init\\(\\)|tui_restore\\(" src/command/code.rs
rg -n "TUI Mode \\(Default\\)|Local TUI Automation|libra code --stdio|libra code --web|libra code --web-only|libra graph|GraphArgs|GRAPH_EXAMPLES|Launch the default TUI session" README.md docs/commands docs/improvement docs/automation tests src/command/code.rs
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider \
  --test ai_code_ui_wire_test \
  --test ai_code_ui_headless_test \
  --test code_ui_scenarios \
  --test code_ui_remote_lease_matrix \
  --test code_ui_remote_sse_matrix \
  --test code_ui_remote_state_matrix \
  --test code_ui_remote_security_matrix \
  --test code_ui_remote_generation_matrix \
  --test code_ui_remote_approval_matrix \
  -- --test-threads=1
pnpm --dir web lint
pnpm --dir web build
```

Ubuntu sandbox 条件门禁也要在 W12 做一次判定：如果本系列 PR 曾触达 sandbox/runtime context/approval/network policy，或者删除了原本承担 approval glue 的 TUI 代码，则把 Phase 5 的 Ubuntu smoke 命令纳入最终必跑检查；如果未触达，W12 记录不触发原因即可。

验收：

- 第一条 `rg` 对 `src/command/code.rs` 无结果。
- 第二条 `rg` 只保留本计划/历史迁移说明中的允许项；用户文档不再推荐 `libra code --web` / `--web-only` / `libra graph`。
- Web graph API/UI 测试在必跑或同 PR 专项测试中通过。
- 所有必跑检查通过，或失败项有明确、同 PR 修复。

### W13: 舍弃 `local-tui-control.md` 并清理一致性脚本

目标：放弃 TUI 后，`docs/automation/local-tui-control.md` 不再需要，连同代码、测试、文档、脚本中对它和 "Local TUI Automation Control" 的依赖一起清掉。control 文档统一以 `docs/commands/code-control.md` 为 canonical。

当前状态（2026-06-01 复核）：

- `docs/automation/local-tui-control.md` 仍存在，且内容仍描述 local TUI、`/control reclaim`、`pty.log`、`local-tui-control/v1`。
- docs-consistency 检查（即 the (now-removed) docs-consistency 脚本，其检查现位于 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`）的 endpoint/header 检查已经指向 `docs/commands/code-control.md`，`test-provider`/`code_ui_scenarios` 已指向本文件；历史上该脚本还要求 `.github/workflows/base.yml` 包含 "Run TUI automation scenarios"，并把 `tests/code_codex_default_tui_test.rs` 当作必需 artifact。
- `src/internal/ai/web/code_ui.rs::code_ui_error_code_listing_matches_authoritative_doc` 仍读取 `docs/automation/local-tui-control.md`；删除旧文档前必须把该测试的 doc path、断言文案和注释迁到 `docs/commands/code-control.md` 或新的 canonical 错误码文档。
- 仍有文档/测试索引引用旧文档：`tests/INDEX.md`、`tests/compat/README.md`、`tests/compat/help_flag_descriptions.rs`、`tests/code_cli_dispatch_test.rs`、`docs/improvement/web.md`、`docs/improvement/test.md`、`docs/improvement/agent.md`。

具体步骤：

1. 先确认 `docs/commands/code-control.md` 已包含全部 `/api/code/*` endpoint、两个 control header、错误码表和 `code-control --stdio`/`diagnostics.get` 内容；缺项从旧文档迁入。
2. 修改 `src/internal/ai/web/code_ui.rs::code_ui_error_code_listing_matches_authoritative_doc`，让错误码同步测试读取新的 canonical 文档，并更新失败文案。
3. 更新 `tests/INDEX.md`、`tests/compat/README.md`、`tests/compat/help_flag_descriptions.rs`、`tests/code_cli_dispatch_test.rs` 中对 `docs/automation/local-tui-control.md` 的引用。
4. 更新 `docs/improvement/web.md`、`docs/improvement/test.md`、`docs/improvement/agent.md`：历史段落可保留 TUI 迁移背景，但当前状态和 source-of-truth 指向必须改为 Web/Code UI control。
5. 删除 `docs/automation/local-tui-control.md`；不要留下断链相对链接。
6. 随 W10 改名 CI step 后，把 `.github/workflows/base.yml` 中的 "Run TUI automation scenarios" 同步为 "Run Code UI automation scenarios"（旧的 (now-removed) docs-consistency 脚本曾钉这一步名和错误消息，其检查现已迁入 `tests/compat/matrix_alignment.rs::docs_consistency_covers_code_ui_router_matrix`，不再断言 CI step 名）；随 W9 改名 `tests/code_codex_default_tui_test.rs` 后同步 required_file。

收尾/验收：

- `rg -n "local-tui-control|Local TUI Automation Control|docs/automation/local-tui-control.md" docs src tests` 无结果（除本计划或明确标注的历史迁移说明；`scripts/` 目录已移除，无需再扫）。
- `cargo test --test compat_matrix_alignment` 退出码为 0（docs-consistency 检查现断言 `docs/commands/code-control.md` 覆盖每个 `/api/code/*` 路由）；若缺某个 `/api/code/*` endpoint 或 control header，补进 code-control.md（本就是它的归属），不要复活 `local-tui-control.md`。
- `cargo test --test compat_matrix_alignment docs_consistency_covers_code_ui_router_matrix` 通过。
- `code_ui_error_code_listing_matches_authoritative_doc` 仍覆盖 `code_ui_error_codes()` 与 canonical 文档的同步，但不再读取旧 TUI 文档。

## 风险和缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 直接删除 TUI 导致 plan workflow 丢失 | generic provider 从两阶段计划退化成直接执行 | Phase 2 先抽 provider-neutral workflow，未完成前不删除 TUI path |
| Web 默认可写扩大攻击面 | 本机恶意页面或远程访问尝试控制 session | 写控制继续 loopback-only + controller token；非 loopback only remote notice；保留 body limit 和 audit |
| `--web` / `--web-only` 不设兼容期导致旧脚本失败 | 依赖冗余 Web flag 的脚本/CI 需要同步改命令 | W2 在同一 PR 更新仓库内脚本、测试、docs、help 和 release notes；旧脚本改为裸 `libra code`，不保留旧 flag 解析 |
| `--stdio` 被移除破坏 Claude Desktop/MCP 用户 | 现有集成失败 | W1 先提供替代 MCP 命令；W3 删除或立即拒绝旧 flag，并给明确迁移错误，不保留兼容运行路径 |
| TUI tests 大量删除造成覆盖下降 | 控制面、approval、cancel regressions 更难发现 | 用 Web process + HTTP/SSE harness 替代 PTY harness，不减少矩阵维度 |
| TUI 删除绕过 Linux sandbox/approval glue | Web mode 在 Ubuntu 上可能静默降级、丢失 network deny 或不再发出 sandbox approval | 对触达 runtime context/approval/network policy 的 PR 启用 Ubuntu sandbox 条件门禁，并在 W12 记录触发判定 |
| Web wire 层残留 `TuiControlError` downcast | 删除 `src/internal/tui/control.rs` 后 Web API 错误映射编译失败，或 control 错误退化成 generic unsupported | W8 先引入 UI-neutral control error/response mapping，再要求 `rg "TuiControlError|crate::internal::tui" src/internal/ai/web` 无结果 |
| Web Graph view 未覆盖原 `libra graph` 信息量 | 用户失去 thread DAG/list/detail 的调试能力 | W8G 先迁数据层和 Web UI，再删 CLI；验收要求 DAG/tree、children/list、detail、状态、搜索/过滤和历史 thread |
| Graph 数据层仍耦合 ratatui | 删除 `src/command/graph.rs` 时丢 projection loader 或引入 UI 类型到 Web API | 先抽 UI-neutral `src/internal/ai/graph` DTO/service，禁止 DTO 暴露 ratatui `Line`/`Color` |
| 文档长期混用 TUI/Web 术语 | 用户按旧说明运行失败 | Phase 6 将 docs grep 纳入验收 |
| 放弃 `local-tui-control.md` 后引用清理不完整 | 错误码同步测试、测试索引或历史文档继续钉旧 TUI 文档，导致删除失败或 source-of-truth 分裂 | W13 先迁 `code_ui_error_code_listing_matches_authoritative_doc` 与 tests/docs 引用，再删除旧文档；W11 确保 code-control.md 覆盖全 endpoint/header/error-code 内容 |

## 完成定义

- `libra code` 启动 Web runtime；`libra code --web` 和 `libra code --web-only` 不再被接受，也不出现在 help/docs 中。
- `libra code` 在无 TTY 环境可正常启动 Web server。
- `src/command/code.rs` 不引用 TUI runtime。
- Web UI 覆盖原 TUI 的核心交互：message submit、streaming、intent/plan review、tool approval、request_user_input、cancel、resume、goal、usage、skills、multi-agent dispatch。
- Web UI 内置 Graph view 覆盖 thread version graph：intent/plan/task/run/patchset 节点、状态、children/list、detail、当前/历史 thread。
- `libra graph` 命令、help、docs、CLI 注册和 TUI renderer 均已删除。
- Browser/automation controller lease、audit、redaction、SSE、wire contract 均有测试。
- 文档和 help 文案不再把 TUI 描述为 `libra code` 支持模式，也不再推荐 `libra graph`。

## 附录 A：已校准代码锚点（2026-05-31 工作树）

执行各卡时最常用的符号定位点。行号会漂移，执行前用 `rg <symbol>` 复核；标 ❌ 的是原稿出现过但实际不存在/位置不符的项。

### `src/command/code.rs`

| 符号 | 行 | 说明 |
|------|----|------|
| `CODE_EXAMPLES` | 436 | help 示例常量 |
| `CodeArgs` | 459 | CLI 参数 struct |
| `CodeArgs.web_only` | 462 | 当前字段；`--web` 是其 alias（:461，`conflicts_with="stdio"`）。W2 删除目标，不能保留为旧入口 |
| `CodeArgs.stdio` | 600 | 当前字段；`--mcp-stdio` 是其 alias（:599，`conflicts_with="web_only"`）。`mcp_stdio`/`web` ❌ 不是独立字段；W3 删除或立即拒绝 |
| `CodeArgs.cwd` / `CodeArgs.repo` | 474 / 478 | W1 的 `McpArgs` 工作目录解析可复用 |
| `ControlMode` | 235 | enum：`Observe`/`Write` |
| `BrowserControlMode` | 250 | enum：`Off`/`Loopback` |
| `execute` | 696 | 当前 dispatch：`stdio→execute_stdio` / `web_only→execute_web_only` / else `execute_tui`（696-705）；W2 改为默认 Web 且删除 `web_only` 分支 |
| `execute_web_only` | 759 | 改名或收敛为 `execute_web` 的目标（W2），旧 flags 同步删除 |
| `execute_tui` | 1401 | 默认 TUI 路径（W9 删除） |
| `default_browser_control_mode` | 1839 | W4 修改点 |
| `build_headless_web_code_ui_runtime` | 1968 | Web runtime 构造 |
| `build_headless_tool_registry` | 2070 | task tool gate；测试 `build_headless_tool_registry_omits_task_tool_in_flag_off_default`(5333) |
| `TuiLaunchConfig` | 2510 | W9 删除 |
| `build_tui_code_ui_capabilities` | 2558 | W9 删除/迁移 |
| `build_tui_code_ui_transcript` | 2571 | W9 重命名为 UI-neutral |
| `build_tui_code_ui_runtime` | 2613 | W9 删除 |
| `run_tui_with_model` | 2724 | W9 删除 |
| `run_tui_with_model_inner` | 2753 | W5 抽 bootstrap 的来源 |
| `default_tui_runtime_context` | 3406 | W5 重命名为 `default_code_runtime_context` |
| `init_mcp_server` | 3547 | W1 复用 |
| `build_subagent_runtime_for_session` | 3650 | sub-agent runtime |
| `execute_stdio` | 3927 | W1 抽 `run_mcp_stdio` 的来源；W3 拒绝 |
| `validate_mode_args` | 3967 | W2/W3 修改点 |
| TUI imports | 157-160 | `App, AppConfig, ExitReason, Tui, TuiCodeUiAdapter, control::TuiControlCommand, tui_init, tui_restore`（W9 移除） |
| 测试 `rejects_tui_flags_in_web_mode` | 4334 | W2 改名/改断言 |
| 测试 `accepts_default_tui_mode` | 4350 | W2 改名/改断言 |
| 测试 `browser_control_resolution_matrix_pins_mode_provider_and_host_contract` | 4373 | W4 修改点 |

### `src/command/graph.rs`（W8G 迁移来源，最终删除）

| 符号 | 行 | 说明 |
|------|----|------|
| `GRAPH_EXAMPLES` / `GraphArgs` / `execute_safe` | 116 / 133 / 144 | `libra graph` CLI surface；W8G 删除 |
| `load_thread_graph` | 176 | graph 数据入口；W8G 抽到 UI-neutral service |
| `load_bundle_for_graph` | 201 | projection bundle 加载；W8G 复用到 Web graph service |
| `ThreadGraph` / `ThreadGraph::from_projection` | 380 / 499 | 当前 TUI 数据模型；W8G 拆成 JSON DTO，不能暴露 ratatui 类型 |
| `load_graph_object_details` / `load_graph_object_detail` | 700 / 717 | detail 加载与截断；W8G 复用并保持安全上限 |
| `graph_object_refs` / `push_task_subgraph` | 765 / 845 | 节点引用和 DAG 行构造；W8G 转为 nodes/edges DTO |
| `run_graph_tui` / `GraphTuiApp` / `render_graph` | 1069 / 1151 / 1535 | TUI renderer；W8G 删除，不迁入 Web DTO |
| `render_tree_pane` / `render_list_pane` | 1690 / 1849 | 信息架构参考：DAG/tree + children/list + detail；Web 重新实现为 React 组件 |

### `src/internal/tui/app.rs`（plan workflow / approval / goal）

| 符号 | 行 |
|------|----|
| `record_orchestrator_thread_metadata` | 231 |
| `handle_tui_control_command` | 1063 |
| `goal_session_start/status/cancel/criteria_add_from_control` | 1111/1146/1157/1201 |
| `respond_pending_user_input_from_code_ui` | 1468 |
| `cancel_pending_user_input` | 2160 |
| `handle_exec_approval_request` | 2283 |
| `submit_exec_approval_decision` | 2503 |
| `reject_pending_exec_approval` / `cancel_pending_exec_approval` | 2642 / 2674 |
| `handle_builtin_command` | 4887 |
| `handle_intent_review_choice` | 5884 |
| `begin_execution_plan_revision_flow` | 6038 |
| `handle_post_plan_choice` | 6377 |
| `start_plan_workflow` / `begin_plan_revision_flow` / `begin_plan_workflow` | 6995 / 7026 / 7048 |
| `format_orchestrator_result` | 8639 |
| `build_plan_prompt` / `build_execution_plan_prompt` | 10787 / 10814 |
| `execution_requires_plan_repair` | 10911 |
| `automatic_plan_repair_*`（如 `_request_from_report`） | 11142 / 11149 |
| `persist_phase0_intent_for_review` | 11470 |
| `provider_plan_draft_from_args` | 11574 |
| `phase0_plan_tool_loop_config` / `phase1_plan_tool_loop_config` | 11625 / 11640 |
| `replay_goal_session_from_session_root` | 11840 |
| `App.pending_user_input`（字段，非函数 ❌） / `App.usage_snapshot`（字段） | 511 / 535 |
| `automatic_plan_repair` / `record_usage_failure`（❌ 不存在为独立函数） | — |

### AI 模块（非 tui）

| 符号 | 路径:行 |
|------|---------|
| `format_usage_detail_panel` / `format_usage_badge` | `src/internal/ai/usage/format.rs:13` / `:45` |
| `SkillDispatcher` / `load_skills` | `src/internal/ai/skills/dispatcher.rs:15` / `loader.rs:13` |
| `run_goal_supervised_tool_loop` | `src/internal/ai/goal/driver.rs:111` |
| `HookRunner::load` | `src/internal/ai/hooks/runner.rs:52` |
| `load_commands` | `src/internal/ai/commands/dispatcher.rs:81` |
| `load_profiles` / `AgentsConfig::load_or_default` | `src/internal/ai/agent/profile/router.rs:196` / `config.rs:653` |
| `SourcePool::with_persistence` | `src/internal/ai/sources/mod.rs:407` |
| `RequestUserInputHandler` | `src/internal/ai/tools/handlers/request_user_input.rs:31` |
| `HeadlessCodeRuntime` / `submit_message` / `handle_exec_approval_request` | `src/internal/ai/web/headless.rs:163` / `:290` / `:607` |
| `CodeUiControllerKind`（`None`/`Browser`/`Automation`/`Tui`/`Cli`） | `src/internal/ai/web/code_ui.rs:69` |
| `CodeUiInteractionKind`（`Approval`/`SandboxApproval`/`RequestUserInput`/`IntentReviewChoice`/`PostPlanChoice`） | `src/internal/ai/web/code_ui.rs:129` |
| `CodeUiInteractionRequest` | `src/internal/ai/web/code_ui.rs:158` |
| `CodeUiCommandAdapter`（`task_dispatch`:1179/`goal_start`:1198/`goal_status`:1213/`goal_cancel`:1223） | `src/internal/ai/web/code_ui.rs:705` |
| `CodeUiInitialController`（`Unclaimed`/`Fixed`/`LocalTui`） | `src/internal/ai/web/code_ui.rs:771` |
| `TuiCodeUiAdapter` / `TuiControlCommand` | `src/internal/tui/code_ui_adapter.rs:31` / `src/internal/tui/control.rs:22` |
| audit policy `local-tui-control/v1` | `src/internal/ai/web/mod.rs`（2 处，W8 改名） |

### 测试与数据

| 项 | 位置 |
|----|------|
| `portable-pty` 依赖 / `native_pty_system()` | `Cargo.toml:156` / `tests/harness/code_session.rs` |
| `write_tui_line` | `tests/harness/code_session.rs`、`tests/harness/scenario.rs:48` |
| `controller_kind_tui_or_none` | `tests/data/code_ui_remote/lease_cases.json`、`sse_cases.json` |
| `lease_detach_releases_to_local_tui` | `tests/data/code_ui_remote/lease_cases.json` |
| `test-provider` feature / `LIBRA_ENABLE_TEST_PROVIDER` | `Cargo.toml` / `fake/mod.rs`+`code.rs`（3 处） |
| `Mcp` 子命令、`docs/commands/mcp.md`、`src/command/mcp.rs` | 当前 ❌ 不存在（W1 创建） |
| `docs/commands/graph.md` / `src/command/graph.rs` | 当前存在；W8G 先迁 Web Graph view，再删除可见命令和 TUI renderer |
| `docs/automation/local-tui-control.md` | 当前仍存在，且 `src/internal/ai/web/code_ui.rs` 错误码同步测试仍读取它；W13 迁移 canonical 文档后删除 |
