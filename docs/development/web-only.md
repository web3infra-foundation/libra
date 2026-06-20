# `libra code` Web-only 迁移计划

> Status: draft
> Last updated: 2026-05-31
> Scope: `libra code` no longer launches or depends on the Code TUI. The browser Code UI becomes the only supported interactive surface.

## 目标

把 `libra code` 收敛为 Web-only 入口：

- `libra code` 默认启动 Web Code UI，不再进入 ratatui/crossterm TUI。
- `libra code --web` / `--web-only` 在兼容期内保留为 no-op alias，并在帮助文案里标记为兼容旧脚本；最终可移除。
- 浏览器在 loopback 上默认具备写控制能力；非 loopback 访问仍只能看到 remote access notice，不能读写 session/API。
- `libra graph` 这类独立 TUI 命令不属于本计划；它们可以继续使用共享 terminal helper。
- `src/internal/tui/app.rs` 中承载的 agent 行为必须迁移到 provider-neutral runtime，不能因为移除 TUI 而丢失计划确认、approval、goal、usage、skills、hooks、multi-agent、resume 等现有能力。

## 非目标

- 不开放公网 Web 写控制。Code UI v1 仍以 loopback 为安全边界。
- 不把 browser UI 做成多用户协作产品；同一 session 仍只有一个 active controller lease。
- 不在 Web terminal 暴露任意 shell prompt；命令执行必须继续通过 tool runtime、sandbox 和 approval。
- 不删除所有 `src/internal/tui/*` 文件，除非确认没有被 `libra graph` 或共享渲染测试使用。

## 当前基线

`src/command/code.rs` 现在有三条 mode 分支：

- `execute_tui(args)`：默认路径。它初始化 `tui_init()`，创建 `App`，并启动背景 Web/MCP server。
- `execute_web_only(&args)`：`--web-only` / `--web` 路径。它启动 Web + MCP；Codex 使用 managed app-server runtime，非 Codex 使用 `HeadlessCodeRuntime`。
- `execute_stdio(&args)`：MCP stdio transport。

Web 侧已有可复用基础：

- `src/internal/ai/web/mod.rs` 服务静态 Web app 和 `/api/code/*`。
- `src/internal/ai/web/code_ui.rs` 是 Code UI wire contract 和 controller lease 的源头。
- `src/internal/ai/web/headless.rs` 已支持非 Codex provider 的 browser submit、streaming、approval/user-input、cancel、plan/patchset projection、session persistence。
- `docs/development/commands/_general.md` 记录了 Web UI、browser-control、headless runtime 的现状。

主要缺口在 TUI-owned 行为：

- generic provider 的 IntentSpec / Plan 两阶段确认和自动 repair loop 仍深耦合在 `src/internal/tui/app.rs`。
- slash commands、`/goal`、`/usage`、`/skill`、`/plan continue`、local reclaim 等交互现在由 TUI App 处理。
- `TuiCodeUiAdapter` / `TuiControlCommand` 是 HTTP write 到 TUI App 的桥；Web-only 后这层应消失或只保留为迁移期测试辅助。
- 多个测试目标和文档把 "TUI + background Web" 当作默认契约。

## TUI-owned behavior inventory

这张表是后续执行任务的风险清单。凡是 `must-migrate` 项，在默认入口切到 Web 并删除 TUI path 前必须有 Web/session runtime 等价实现和测试。

| 行为 | 当前代码锚点 | 分类 | 目标位置 | 迁移验收 |
|------|--------------|------|----------|----------|
| 默认 `libra code` dispatch | `src/command/code.rs::execute`, `execute_tui`, `execute_web_only`, `validate_mode_args` | must-migrate | `src/command/code.rs::execute_web` | 默认 `libra code` 不调用 `execute_tui`; `--web`/`--web-only` 为 no-op alias |
| MCP stdio transport | `src/command/code.rs::execute_stdio`, `init_mcp_server` | web-replace | 新的 `libra mcp --stdio` 或等价非 `code` 命令 | `libra code --stdio` 有迁移错误; 新命令覆盖 MCP stdio e2e |
| IntentSpec draft/review | `start_plan_workflow*`, `begin_plan_workflow`, `handle_intent_review_choice`, `build_plan_prompt`, `phase0_plan_tool_loop_config`, `persist_phase0_intent_for_review` | must-migrate | provider-neutral `CodeSessionRuntime` / `plan_workflow` | Web submit 后先出现 `intent_review_choice`; 未确认前不执行 mutating tools |
| Execution plan draft/review | `begin_plan_revision_flow`, `begin_execution_plan_revision_flow`, `handle_post_plan_choice`, `build_execution_plan_prompt`, `phase1_plan_tool_loop_config`, `provider_plan_draft_from_args` | must-migrate | provider-neutral `CodeSessionRuntime` / `plan_workflow` | Web 能 confirm/modify/cancel plan; `submit_plan_draft` 仍是 terminal tool |
| Orchestrator execution and repair loop | `format_orchestrator_result`, `record_orchestrator_thread_metadata`, `automatic_plan_repair_*`, `execution_requires_plan_repair`, pending revision fields | must-migrate | session runtime + Code UI interactions | failed execution feeds repair prompt; threshold 后 Web 显示 continue/modify/cancel |
| Sandbox/tool approval | `handle_exec_approval_request`, `submit_exec_approval_decision`, `reject_pending_exec_approval`, `cancel_pending_exec_approval` | must-migrate | `HeadlessCodeRuntime` approval channel already exists; extend to plan workflow | approval/sandbox approval in Web uses `CodeUiInteractionRequest`; cancel resolves pending approval |
| `request_user_input` | `pending_user_input`, `cancel_pending_user_input`, `RequestUserInputHandler`, TUI bottom pane answer flow | must-migrate | `HeadlessCodeRuntime` pending user-input map already exists; reuse for all workflows | structured questions round-trip through `/api/code/interactions/{id}` |
| Goal lifecycle | `src/internal/tui/goal_session.rs`, `goal_command.rs`, `goal_session_*_from_control`, `replay_goal_session_from_session_root`, `run_goal_supervised_tool_loop` integration | must-migrate | UI-neutral goal session module under `src/internal/ai/goal/` or `src/internal/ai/code_session/` | Web goal start/status/cancel and resume replay work without TUI |
| Usage display and cancellation accounting | `usage_snapshot`, `format_usage_badge`, `format_usage_detail_panel`, `record_usage_failure`, `/usage` handling | must-migrate | session runtime usage service + Web transcript/snapshot projection | Web shows session usage or transcript info; cancel records failure usage row |
| Skills and slash command effects | `load_skills`, `SkillDispatcher`, `handle_builtin_command`, `/skill`, `/plan`, `/intent`, `/task`, `/goal` | must-migrate | command service behind Web API / structured interactions | skill activation preserves allowed-tools; `/plan` and `/intent` equivalents exist in Web |
| Hooks, agents config, SourcePool, sub-agents | `run_tui_with_model_inner` setup: `HookRunner::load`, `load_commands`, `load_profiles`, `AgentsConfig::load_or_default`, `SourcePool::with_persistence`, `build_subagent_runtime_for_session` | must-migrate | shared `build_code_session_services` helper | Web mode honors hooks, profiles, agents.toml, source logging, gated task dispatch |
| Browser/automation bridge | `TuiCodeUiAdapter`, `TuiControlCommand`, `handle_tui_control_command`, `CodeUiInitialController::LocalTui` | delete-with-tui | direct Web/session runtime adapter | HTTP submit/respond/cancel no longer enters TUI App; controller never returns `tui` |
| Local reclaim | `/control reclaim`, `reclaim_local_controller`, `reclaim_local_tui_controller`, `is_control_reclaim_command_input` | web-replace | lease expiry/detach/conflict handling | reclaim tests rewritten; no `/control reclaim` docs for `libra code` |
| PTY test harness | `tests/harness/*`, `tests/harness_self_test.rs`, `tests/code_ui_scenarios.rs`, `portable-pty`, `write_tui_line` | web-replace | Web process + HTTP/SSE harness | cross-process tests run without TTY |
| Graph terminal UI | `src/command/graph.rs`, `src/internal/tui/terminal.rs` | graph/shared | keep or move shared terminal helper | `libra graph` remains TUI and compiles after Code TUI deletion |

## 设计决策

### CLI 契约

最终用户契约：

```bash
libra code                         # starts Web Code UI
libra code --provider ollama ...    # starts Web Code UI with selected provider
libra code --port 4400 --host 127.0.0.1
libra code --resume <thread_id>
```

兼容期：

- `--web` / `--web-only` 解析成功但不改变行为；帮助文案说明 Web 已是唯一模式。
- 删除 "TUI mode" 概念，重命名内部 `web_only` 字段为 `web` 或移除字段。
- `--browser-control` 默认改为 Web-primary：loopback host 上默认 `loopback`，非 loopback host 上默认 `off` 并展示 remote notice；显式 `--browser-control loopback` 仍必须要求 loopback host。
- `--control` 从 "Local TUI automation control" 改为 "Local Code UI automation control"。保留 token/lock/info 文件安全模型，但文档和错误信息不再提 TUI reclaim。

`--stdio` 需要单独决策，因为它不是 Web 模式：

- 严格 Web-only 版本：`libra code --stdio` 改为 usage error，并在同一 PR 提供替代入口，例如新的 `libra mcp --stdio` 或 `libra agent mcp --stdio`。
- 兼容版本：短期保留 `--stdio`，但文档明确它不是 interactive Code UI 模式；后续用 deprecation warning 迁出。

如果目标是 "code 命令只支持 Web 模式"，推荐采用严格版本，并把 MCP stdio 迁移到独立命令，避免 `code` 继续有非 Web mode。

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

## 实施阶段

### Phase 0: 行为盘点和冻结

- 列出 `src/internal/tui/app.rs` 中属于 agent/session 行为的函数，按 "必须迁移"、"Web 不需要"、"graph/shared terminal 可保留" 分类。
- 对必须迁移项补源代码级 guard 或单测，先固定现有行为：plan workflow、approval、request_user_input、goal、usage、skills、hooks、multi-agent、resume、cancel、session persistence。
- 标记 `docs/commands/code.md`、README、`docs/development/commands/_general.md` 中所有 TUI-default 文案；旧 local TUI automation 独立文档已移除，不能再作为事实源。

Exit criteria:

- 有一张迁移清单，每个 TUI-owned 行为都有目标模块和测试目标。
- `rg "TUI|tui|--web-only|--stdio" docs/commands/code.md README.md docs/development/commands/_general.md` 的待改位置已确认。

### Phase 1: CLI 默认改为 Web

- 在 `src/command/code.rs` 中把 `execute()` 改为默认调用 Web path。
- 将 `execute_web_only()` 重命名为 `execute_web()`，更新注释、错误上下文和 banner。
- `CODE_EXAMPLES` 改为 Web-first 示例；`libra code` 说明为 "Start the Web Code UI"。
- `validate_mode_args()` 删除 "TUI-specific flags rejected in web-only mode" 的旧规则。provider/model/context/resume/env-file/approval/network/goal 等 flags 都应在 Web mode 可用。
- `BrowserControlMode` 默认改为 Web-primary，覆盖 loopback 默认可写、非 loopback fail-closed 的矩阵。
- `ControlMode` 文案从 local TUI automation 改为 local Code UI automation。

Exit criteria:

- `CodeArgs::try_parse_from(["libra"])` 后走 Web runtime。
- `CodeArgs::try_parse_from(["libra", "--web"])` 和 `["libra", "--web-only"]` 与默认行为等价。
- `src/command/code.rs` 默认路径不调用 `execute_tui`。

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
- `CodeUiControllerKind::Tui` 在 `libra code` session 中不再出现；wire enum 可保留一段时间以兼容旧客户端和 `libra graph` 无关测试。
- 移除 `/control reclaim` 语义；controller 冲突由 browser/automation lease 过期、detach、cancel 解决。
- `code-control` 文档和实现从 "drive a local TUI" 改为 "drive a local Code UI session"。
- `control.json` 字段保留，但描述从 TUI session 改为 Code UI session。

Exit criteria:

- `GET /api/code/session` 在默认 `libra code` 中返回 `controller.kind in {none,browser,automation,cli}`，不会返回 `tui`。
- automation submit/respond/cancel 不经过 `TuiControlCommand`。
- 旧的 TUI reclaim tests 被删除或改写为 Web lease conflict/expiry tests。

### Phase 4: 删除 `libra code` TUI path

- 删除 `execute_tui`、`TuiLaunchConfig`、`run_tui_with_model*`、`build_tui_code_ui_runtime` 等只服务 `libra code` TUI 的代码。
- 从 `src/command/code.rs` imports 中移除 `crate::internal::tui::*`。
- 如果 `src/internal/tui/app.rs`、`bottom_pane.rs`、`chatwidget.rs` 等只服务 Code TUI，删除；如果 `graph` 或测试仍需 terminal helper，先把 `terminal.rs` 迁到共享模块，例如 `src/internal/terminal.rs`。
- 保留或迁移 `diff` / `markdown_render` 等可复用渲染逻辑，前提是仍有真实调用者。
- 删除 Codex "managed-tui" mode；Codex provider 只走 Web managed runtime。

Exit criteria:

- `rg "execute_tui|run_tui_with_model|TuiCodeUiAdapter|TuiControlCommand|tui_init\\(\\)" src/command/code.rs` 无结果。
- `cargo check` 不再通过 `libra code` 引用 ratatui/crossterm TUI runtime。
- `libra graph` 仍可编译并保留自己的 TUI 行为。

### Phase 5: 测试迁移

更新或删除以下测试目标：

- 删除/改写 `tests/harness_self_test.rs`：不再需要 PTY 启动 `libra code`。
- 改写 `tests/code_ui_scenarios.rs`：从 PTY harness 改为 Web process + HTTP/SSE/browser-control harness。
- 删除 `tests/code_codex_default_tui_test.rs`，新增 `code_web_default_test` 源码级 guard，确保 `libra code --provider codex` 不走 TUI。
- 更新 `tests/code_cli_dispatch_test.rs`：默认 Web、`--web` no-op、`--web-only` no-op/deprecated、`--stdio` 按最终决策迁移或拒绝。
- 更新 remote matrix JSON：移除 `controller_kind_tui_or_none`、`lease_detach_releases_to_local_tui` 等 TUI expectation。
- 保留并强化 `tests/ai_code_ui_headless_test.rs`、`tests/ai_code_ui_wire_test.rs`、`tests/code_ui_remote_*_matrix.rs`。
- 同步 `Cargo.toml` 和 `tests/INDEX.md`，删除或重命名 test target 行。

新增覆盖：

- `libra code --port 0 --provider fake --browser-control loopback` 能启动 Web session，HTTP submit 后 transcript/SSE 更新。
- non-Codex provider Web mode 使用 `ProviderFactory`、env-file、vault fallback、approval/network policy。
- Codex Web mode 启动 managed app-server，browser submit/respond/cancel 正常。
- plan workflow 在 Web mode 中完成 intent review、plan review、execute、repair。
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

### Phase 6: 文档和兼容性

- 更新 `docs/commands/code.md`：
  - synopsis 改为 Web-only
  - 删除 TUI mode、TUI output、TUI troubleshooting
  - `--web` / `--web-only` 标为 compatibility alias
  - `--stdio` 按最终决策迁移/拒绝
  - browser-control 默认矩阵改为 Web-primary
- 更新 README 的 "Libra Code Modes"，只保留 Web interactive mode 和外部 MCP 替代入口。
- 删除旧 local TUI automation 独立文档；local Code UI automation control 的用户可见事实源收敛到 `docs/commands/code.md`，并同步 docs consistency script。
- 更新 `docs/development/commands/_general.md`，把 "Headless web-only follow-up" 转为默认运行时要求。
- 更新 `COMPATIBILITY.md`、`docs/error-codes.md`、`docs/commands/README.md` 和 release notes。
- 搜索并消除误导文案：

```bash
rg -n "TUI|tui|web-only|--web-only|Local TUI|managed-tui" README.md docs src tests web
```

Exit criteria:

- `docs/commands/code.md` 不再声称 `libra code` 支持 TUI。
- README 首段不再说 `libra code` starts an interactive TUI。
- CI 文档检查不再依赖 local TUI automation wording。

## 可直接派发的任务卡

下面的任务卡按 PR 粒度拆分。每张卡都可以直接交给 Codex 或 Claude Code 执行；执行者应只做该卡范围内的改动，除非测试暴露了同一行为链上的必要修复。

推荐执行顺序：

1. W0 校准清单。
2. W1 先提供 MCP stdio 替代入口。
3. W2 切默认 Web，W3 再从 `code` 拒绝 stdio。
4. W4 调整 Web browser-control 默认。
5. W5 抽共享 session bootstrap。
6. W6 迁移 plan workflow，W7 迁移 goal/usage/skill/task 控制。这两张可并行，但合并前要共同跑 Web headless/remote matrix。
7. W8 移除 TUI bridge/reclaim，W9 删除 `libra code` TUI startup。
8. W10 替换 PTY harness。
9. W11 文档/help 收口。
10. W12 最终审计。

禁止提前执行的依赖：

- 未完成 W1 时不要执行 W3，否则 MCP stdio 用户没有替代入口。
- 未完成 W5/W6/W7 时不要执行 W8/W9，否则会丢 plan workflow、goal、usage、skills、task dispatch 等能力。
- 未确认 `libra graph` 编译路径前不要删除 `src/internal/tui/terminal.rs` 或 ratatui 依赖。
- 未完成 W10 前不要删除所有 harness fixture；要先保证 Web harness 覆盖同等跨进程行为。

### W0: 校准迁移清单

目标：验证并扩充上方 `TUI-owned behavior inventory`，避免后续删除时丢能力。

可交给 Agent 的 prompt：

```text
阅读 docs/agent/web-only.md 和当前代码，校准 `libra code` Web-only 迁移清单。
只修改 docs/agent/web-only.md。
更新 `TUI-owned behavior inventory` 小节，补齐 src/internal/tui/app.rs 中必须迁移到 Web/session runtime 的遗漏行为、相关函数或搜索关键词、目标模块和覆盖测试。
不要改 Rust 代码。
```

涉及文件：

- `docs/agent/web-only.md`
- 只读参考：`src/internal/tui/app.rs`
- 只读参考：`src/internal/tui/slash_command.rs`
- 只读参考：`src/internal/tui/goal_session.rs`
- 只读参考：`src/command/code.rs`
- 只读参考：`src/internal/ai/web/headless.rs`

具体步骤：

1. 用 `rg -n "phase|intent|plan|repair|goal|usage|skill|task|request_user_input|approval|cancel|session" src/internal/tui/app.rs src/internal/tui` 找行为入口。
2. 将行为分为 `must-migrate`、`web-replace`、`delete-with-tui`、`graph/shared` 四类。
3. 给每个 `must-migrate` 项写目标模块，优先使用 `src/internal/ai/web/headless.rs` 或一个新建的 provider-neutral runtime 模块。
4. 给每项写最小验收测试，例如 `ai_code_ui_headless_test`、`code_ui_remote_approval_matrix`、新增 source guard。

验收：

- `TUI-owned behavior inventory` 表与当前 `rg` 结果一致，没有明显遗漏的 TUI-owned runtime 行为。
- 表中至少包含 plan workflow、approval/user input、goal、usage、skills、hooks、multi-agent、resume、cancel、controller reclaim。
- 本卡不产生 Rust 编译变更。

### W1: 提供 MCP stdio 替代入口

目标：在拒绝 `libra code --stdio` 前，先提供非 `code` 的 MCP stdio 入口，避免现有 MCP 用户无迁移路径。

可交给 Agent 的 prompt：

```text
为 Libra 增加独立 MCP stdio 入口，目标是后续让 `libra code` 只保留 Web interactive mode。
新增或选择一个非 `code` 子命令，例如 `libra mcp --stdio`；复用当前 src/command/code.rs 的 execute_stdio/init_mcp_server 逻辑。
保持 `libra code --stdio` 暂时不变，只新增替代入口和测试/文档。
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
5. 文档说明 `libra code --stdio` 将迁移到 `libra mcp --stdio`。

验收：

- `cargo test --test command_test mcp` 或新增对应 target 通过。
- `libra mcp --help` 显示 stdio 用法。
- `libra code --stdio` 仍保持旧行为，给下一卡迁移。

### W2: 将 `libra code` 默认入口改为 Web

目标：`libra code` 不加 flag 时启动当前 `execute_web_only` 路径；`--web` / `--web-only` 仅作为兼容 alias。

可交给 Agent 的 prompt：

```text
把 `libra code` 默认入口切到 Web runtime。保留 `--web`/`--web-only` 作为兼容 no-op alias。
不要删除 TUI 代码，只让默认 dispatch 不再调用 execute_tui。
同步更新 src/command/code.rs 内部单测、tests/code_cli_dispatch_test.rs、tests/command/code_test.rs 中与默认 TUI 和 --web-only 互斥相关的断言。
```

涉及文件：

- `src/command/code.rs`
- `tests/code_cli_dispatch_test.rs`
- `tests/command/code_test.rs`
- 可能涉及：`tests/code_mcp_dual_entry_test.rs`

具体步骤：

1. 将 `execute_web_only` 重命名为 `execute_web`，或先保留函数名但让默认分支调用它。
2. `execute(args, output)` 改为：`--stdio` 仍走旧 stdio 或 W3 的拒绝；其他都走 Web。
3. 修改 `CodeArgs.web_only` 注释：从 "Run the web server only" 改为 compatibility alias。
4. 删除 `validate_mode_args()` 中 `if args.web_only { reject_non_tui_flags(...) }` 的行为；Web 是默认后 provider/model/context/resume/env-file/approval/network 都应合法。
5. 更新 `rejects_tui_flags_in_web_mode`、`accepts_default_tui_mode` 等单测名称和断言。
6. `CODE_EXAMPLES` 第一行改为 Web session 示例。

验收：

- `CodeArgs::try_parse_from(["libra"])` 的单测说明默认 Web。
- `validate_mode_args()` 接受 `--provider ollama --model llama3 --web`。
- `validate_mode_args()` 接受 `--env-file .env.test --web`。
- `rg "Launch the default TUI session|accepts_default_tui_mode|rejects_tui_flags_in_web_mode" src/command/code.rs tests/command/code_test.rs tests/code_cli_dispatch_test.rs` 无结果。

### W3: 从 `libra code` 拒绝 stdio 模式

目标：让 `code` 命令只剩 Web interactive mode。此卡依赖 W1。

可交给 Agent 的 prompt：

```text
在已有 `libra mcp --stdio` 替代入口的前提下，让 `libra code --stdio` 返回 command_usage 错误。
错误必须明确提示使用新的 MCP stdio 命令。
删除 `--stdio` 和 `--mcp-stdio` 在 `CodeArgs` 的支持，或保留隐藏 deprecated flag 但统一报错。
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

1. 移除或隐藏 `CodeArgs.stdio`，推荐短期保留解析但在 `validate_mode_args` 中返回 usage error。
2. 错误文案：`libra code is Web-only; use libra mcp --stdio for MCP stdio transport`。
3. 更新 `web_only_and_stdio_are_mutually_exclusive` 一类测试为 `code_stdio_is_rejected_with_migration_hint`。
4. 将 `tests/e2e_mcp_flow.rs` 从 `libra code --web-only` / `--stdio` 迁到新 MCP 命令，或拆成 Web/MCP 两个测试。
5. 文档删除 "code supports MCP/stdio mode"。

验收：

- `libra code --stdio` 非零退出并包含新命令提示。
- 新 MCP stdio integration test 仍覆盖 MCP server。
- `rg -n "libra code --stdio|--mcp-stdio|MCP/stdio mode" docs/commands/code.md README.md tests` 只剩迁移说明或无结果。

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
不要删除 graph TUI。
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
4. audit policy 名称从 `local-tui-control/v1` 迁到 `local-code-ui-control/v1`，或保留旧名称一版并文档说明兼容。
5. 更新 matrix assertion：删除 `controller_kind_tui_or_none`。

验收：

- `rg "TuiCodeUiAdapter|TuiControlCommand|LocalTui|reclaim_local_tui_controller" src/command/code.rs src/internal/ai/web src/internal/tui tests` 中不再有 `libra code` runtime 依赖。
- lease detach 后 controller 回到 `none` 或 browser-expected 状态，不回到 `tui`。
- control audit/redaction tests 仍通过。

### W9: 删除 `libra code` TUI startup 代码

目标：完成代码层 Web-only，`src/command/code.rs` 不再引用 terminal/TUI runtime。

可交给 Agent 的 prompt：

```text
删除 `libra code` 的 TUI startup path。
移除 execute_tui、TuiLaunchConfig、run_tui_with_model*、build_tui_code_ui_runtime 和相关 imports。
不要删除 `libra graph` 需要的 terminal/TUI helper；如共享 helper 仍在 src/internal/tui/terminal.rs，先迁出或保留模块给 graph。
```

涉及文件：

- `src/command/code.rs`
- `src/internal/tui/mod.rs`
- `src/command/graph.rs`（如 terminal helper 迁移）
- `src/internal/mod.rs`
- `Cargo.toml`（只有确认 ratatui 不再被 graph 使用时才移除依赖）
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
- `libra graph` tests 仍通过。

### W10: 替换 PTY harness 为 Web harness

目标：测试仍覆盖跨进程行为，但不再启动 TUI/PTY。

可交给 Agent 的 prompt：

```text
把 `libra code` 的跨进程测试 harness 从 PTY/TUI 改为 Web process harness。
新 harness 启动 `libra code --provider fake --port 0 --mcp-port 0`，读取 control.json 或 stdout 中的 bound URL，通过 HTTP/SSE 完成 attach/submit/respond/cancel。
删除 write_tui_line 和 /control reclaim 相关 DSL。
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
5. CI step 名称从 "Run TUI automation scenarios" 改为 "Run Code UI automation scenarios"。

验收：

- `LIBRA_ENABLE_TEST_PROVIDER=1 cargo test --features test-provider --test code_ui_scenarios -- --test-threads=1` 不需要 TTY。
- `rg "portable-pty|write_tui_line|/control reclaim|pty.log|TUI automation" tests .github/workflows/base.yml` 无默认 Code UI harness 依赖。

### W11: 文档和 help 收口

目标：用户文档不再暗示 `libra code` 支持 TUI 或 stdio mode。

可交给 Agent 的 prompt：

```text
更新用户文档和 help 文案，使 `libra code` 只描述 Web mode。
保留 `libra graph` 是 TUI 的说明。
将 local TUI automation 文档改为 local Code UI automation，并同步 docs consistency checks。
```

涉及文件：

- `README.md`
- `docs/commands/code.md`
- `docs/commands/code-control.md`
- `docs/commands/README.md`
- 旧 local TUI automation 独立文档（已删除；事实源改为 `docs/commands/code.md`）
- `docs/development/commands/_general.md`
- `COMPATIBILITY.md`
- `docs/error-codes.md`
- `tests/compat/matrix_alignment.rs`
- `tests/compat/matrix_alignment.rs`

具体步骤：

1. `docs/commands/code.md` synopsis 只保留 Web usage 和 provider flags。
2. README "Libra Code Modes" 改为 Web interactive + separate MCP command。
3. `code-control` 说明从 live TUI 改为 live Code UI session。
4. consistency script endpoint matrix 改新文档路径。
5. 保留 `docs/commands/graph.md` 的 TUI 文案。

验收：

- `rg -n "libra code.*TUI|TUI Mode \\(Default\\)|Local TUI Automation|libra code --stdio|--web-only.*without the TUI" README.md docs tests` 无误导结果。
- `cargo test --test compat_matrix_alignment` 通过。

### W12: 最终删除检查和 release gate

目标：证明最终状态满足 Web-only。

可交给 Agent 的 prompt：

```text
执行 `libra code` Web-only 迁移的最终审计。
不要新增功能；只补遗漏的删除、测试名、文档名和 guard。
用 rg 和测试证明 src/command/code.rs 不再引用 TUI runtime，默认 code 可在无 TTY 环境启动 Web server。
```

涉及文件：

- 按审计结果最小修改
- `docs/agent/web-only.md`（更新状态和完成记录）
- `tests/INDEX.md`

必跑检查：

```bash
rg "execute_tui|run_tui_with_model|TuiLaunchConfig|TuiCodeUiAdapter|TuiControlCommand|tui_init\\(\\)|tui_restore\\(" src/command/code.rs
rg -n "TUI Mode \\(Default\\)|Local TUI Automation|libra code --stdio|Launch the default TUI session" README.md docs tests src/command/code.rs
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
- 第二条 `rg` 只保留 `libra graph` 或历史迁移说明中的允许项。
- 所有必跑检查通过，或失败项有明确、同 PR 修复。

## 风险和缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 直接删除 TUI 导致 plan workflow 丢失 | generic provider 从两阶段计划退化成直接执行 | Phase 2 先抽 provider-neutral workflow，未完成前不删除 TUI path |
| Web 默认可写扩大攻击面 | 本机恶意页面或远程访问尝试控制 session | 写控制继续 loopback-only + controller token；非 loopback only remote notice；保留 body limit 和 audit |
| `--stdio` 被移除破坏 Claude Desktop/MCP 用户 | 现有集成失败 | 同 PR 提供替代 MCP 命令，旧 flag 给明确迁移错误；或先兼容一版再移除 |
| TUI tests 大量删除造成覆盖下降 | 控制面、approval、cancel regressions 更难发现 | 用 Web process + HTTP/SSE harness 替代 PTY harness，不减少矩阵维度 |
| TUI 删除绕过 Linux sandbox/approval glue | Web mode 在 Ubuntu 上可能静默降级、丢失 network deny 或不再发出 sandbox approval | 对触达 runtime context/approval/network policy 的 PR 启用 Ubuntu sandbox 条件门禁，并在 W12 记录触发判定 |
| `src/internal/tui` 被 graph 复用 | 删除过度导致 graph 编译失败 | 先拆共享 terminal helper，再删 Code TUI 专属 App |
| 文档长期混用 TUI/Web 术语 | 用户按旧说明运行失败 | Phase 6 将 docs grep 纳入验收 |

## 完成定义

- `libra code`、`libra code --web`、`libra code --web-only` 都启动同一 Web runtime。
- `libra code` 在无 TTY 环境可正常启动 Web server。
- `src/command/code.rs` 不引用 TUI runtime。
- Web UI 覆盖原 TUI 的核心交互：message submit、streaming、intent/plan review、tool approval、request_user_input、cancel、resume、goal、usage、skills、multi-agent dispatch。
- Browser/automation controller lease、audit、redaction、SSE、wire contract 均有测试。
- 文档和 help 文案不再把 TUI 描述为 `libra code` 支持模式。
