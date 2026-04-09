# Code 命令改进详细计划

## Rename Notice

本文件是原 `docs/improvement/codex-plan-mode.md` 的后继文件，也是 `libra code` 最终批次改进计划的唯一主文档。旧文件名只保留为跳转说明，后续讨论、实现和验收都以本文件为准。

## Context

本计划不再只讨论 Codex 的 plan mode，而是把 `libra code` 作为一个完整命令来收敛。目标有三项：第一，把 Codex 运行时收敛到 Libra 自身的工作流、对象模型和版本控制语义中；第二，统一 `Thread ID` 语义，使 Libra `thread_id` 成为唯一的用户可见恢复标识；第三，直接移除当前 `claudecode` managed runtime 支持，使 `code` 命令不再维护第二套 provider-specific session 模型。

本计划采用“先定义上位契约，再说明当前基线，最后给出按顺序执行的实施步骤”的结构。阅读顺序和执行顺序保持一致，避免实现者在多个章节之间来回跳读。

### Hard Constraints

1. Libra 是版本管理核心，Codex 是接入到 Libra 工作流中的托管 provider/runtime。
2. provider 的 VCS 操作必须通过 Libra 能力完成，禁止直接使用 `git`、`jj` 或其它版本管理工具。
3. Codex 路径中的 `approvalPolicy` 以目标态强制为 `never`。
4. 所有阶段划分、对象写入和运行时投影，都必须遵循 `docs/agent/agent-workflow.md` 和 `docs/agent/ai-object-model-reference.md`。
5. 合成出的 `IntentSpec` 必须以易读的 Markdown review 形式展示给开发者确认，不能以原始 JSON 或难以审阅的结构化对象替代审查界面。
6. Libra `thread_id` 是 `code` 命令唯一的用户可见恢复 ID；provider-specific ID 只允许作为内部映射字段存在。
7. `claudecode` 不是过渡期保留组件；第一步即直接删除其全部运行时代码、CLI 标志、文档说明和测试，不保留 feature flag、deprecated path、兼容别名或中间运行态。
8. “不保留中间过渡状态”只适用于 `claudecode` runtime 和其对外接口；本地 `thread_id` 数据迁移允许一次性兼容读取旧字段，但不能反向保留旧的用户可见契约。

## Recommended Reading Order

建议按下面顺序阅读本计划：

1. `Workflow Contract`：先建立上位模型和 Phase 0-4 的边界。
2. `Current Baseline`：理解当前代码已经做到什么、还缺什么。
3. `Delivery Order`：理解这批改造为什么必须按 Step 1 -> Step 2 -> Step 3 执行。
4. `Step 1 / Step 2 / Step 3`：按执行顺序阅读实施内容、影响模块、完成定义和用户影响。
5. `Appendix A / Appendix B`：再看当前 prompt assembly、ContextFrame、provider mapping 等实现细节。
6. `Validation`：最后看验收标准和测试矩阵。

## Workflow Contract

Codex 集成必须服从 `docs/agent/agent-workflow.md` 定义的 Phase 0-4 工作流，而不是自成一套 provider 层状态机。系统真相必须落在 Libra 定义的 Snapshot / Event / Projection 三层边界内。

### Phase-to-Layer Mapping

| Phase | 目标 | Snapshot 写入 | Event 写入 | Libra runtime / projection |
|---|---|---|---|---|
| Phase 0 | 输入与上下文引导 | `Intent`，必要时 `ContextSnapshot` | none | Thread 初始化、live context bootstrap |
| Phase 1 | 计划构建与审查 | `Plan`、`Task` | none | 当前 plan head、ready queue、审查 UI |
| Phase 2 | 执行与过程事实 | `Run`、`PatchSet`、`Provenance` | `TaskEvent`、`RunEvent`、`PlanStepEvent`、`ToolInvocation`、`Evidence`、`ContextFrame`、`RunUsage` | active run、live context window、staging 状态 |
| Phase 3 | 验证与审计 | 可选 `ContextSnapshot` | `Evidence`、`Decision`、terminal `TaskEvent` / `RunEvent` / `IntentEvent` | 审计视图、release candidate 视图 |
| Phase 4 | 决策与释放 | none | `Decision`、可选 terminal `IntentEvent` | 线程与调度投影推进 |

### Snapshot / Event / Projection Placement

根据 `docs/agent/ai-object-model-reference.md`：

- Snapshot 只存“定义了什么”：`Intent`、`Plan`、`Task`、`Run`、`PatchSet`、`ContextSnapshot`、`Provenance`。
- Event 只存“后来发生了什么”：`IntentEvent`、`TaskEvent`、`RunEvent`、`PlanStepEvent`、`RunUsage`、`ToolInvocation`、`Evidence`、`Decision`、`ContextFrame`。
- Libra projection 只存当前运行视图：`Thread`、`Scheduler`、live context window、query index、UI-facing current view。

实现约束是：不能把 provider 运行态事实塞回 Snapshot，也不能把线程/调度视图固化成历史对象，更不能让 provider-specific history 继续成为 `code` 命令运行时的主真相源。

### Phase 1 Review Contract

Phase 1 不只是“生成一个计划”，而是要形成一个可审查、可修订、可执行的 review loop。

1. `IntentSpec` 必须以易读的 Markdown review 展示，而不是原始 JSON。
2. Markdown review 至少要突出 summary / problem statement、objectives、in scope / out of scope、acceptance criteria、risk / rationale。
3. 开发者必须在看到 Markdown review 之后，才能进入 `Execute / Modify / Cancel`。
4. `Modify` 不表示原地编辑当前对象，而表示围绕当前 spec 基线创建新的 `Intent` revision。
5. 新的 `Intent` revision 必须通过 `Intent.parents` 关联前一版。
6. 如果计划结构发生变化，则新的 revision 还应生成新的 `Plan`，必要时生成新的 `Task` 快照。
7. `pending_plan_revision`、`revision_number`、当前选中的 plan head 都属于 Libra runtime / projection，不属于 Snapshot 或 Event。
8. 当开发者选择 `Execute` 或 `Cancel` 时，revision loop 结束；终态进入 `Decision`，必要时补 terminal `IntentEvent`。

## Current Baseline

当前仓库已经具备一条可用但尚未完全闭环的 Codex plan-first 路径。它解决了“先审查再执行”的主入口，但还没有形成完全符合 Libra workflow contract 的执行闭环。

### 已成立的事实

| 能力 | 现状 |
|---|---|
| Native plan collaboration mode | `turn/start` 已携带 `collaborationMode: {"mode":"plan"}` |
| 强制 Plan 审查入口 | TUI 可在 Codex 响应后进入 `Execute / Modify / Cancel` 审查流 |
| Early intercept | `response_text >= 100` 或已有 plan summary 时可提前合成 IntentSpec |
| Timeout fallback | 30 秒无有效响应时，可从 prompt 合成 IntentSpec |
| IntentSpec 合成 | Codex 响应可转为以易读 Markdown 展示的 IntentSpec review，进入现有审查工作流 |
| Plan 修订 | 支持基于现有 IntentSpec 发起多轮 plan revision；当前待修订链由 Libra runtime 状态维护 |
| 局部 MCP 跟踪 | 当前已写入 prompt 提交和 post-plan 选择对应的 `ContextFrame` |

### 已落地的主路径

```text
用户 Prompt
  -> Codex native plan mode
  -> PostPlanChoice 或 early intercept
  -> IntentSpec review
  -> TUI: [Execute] [Modify] [Cancel]
```

其中，`Modify` 走 generic planner 路径，继续使用现有 `submit_intent_draft` 工作流；`Execute` 走 Codex runtime 直连执行，不经过 generic `Orchestrator<M>`。

### 当前基线的主要差距

1. `McpExecutionTracker` 目前仍主要表现为“已有接口与局部接线”，不是完整执行追踪实现。
2. Codex 直连执行路径下，`Run / PlanStepEvent / RunUsage / Evidence / Decision` 的最小写入集合还未完全收敛为稳定流程。
3. `code` 命令当前仍同时存在 `SessionState.id`、Code UI `session_id`、Codex `threadId`、`ai_thread.thread_id` 等多套 thread/session 标识。
4. `code` 命令当前仍保留 `claudecode` provider 和其专属 CLI / resume 标志，导致对外接口同时维护两套 managed runtime 语义。
5. Codex 运行时当前仍保留 provider-specific history / view rebuild 逻辑，没有完全回到 formal Snapshot / Event / Projection 作为唯一真相源。

## Delivery Order

本批次按下面顺序推进，不采用“先保留双栈、后慢慢下线”的中间状态：

1. `Step 1: Direct Claude Code Removal`
2. `Step 2: Thread ID Unification`
3. `Step 3: Codex Formal Workflow Closure`

这个顺序不是文档风格选择，而是实现约束：

- 如果不先删除 `claudecode`，`thread_id` 语义就会继续被 `provider_session` 模型污染。
- 如果不先统一 `thread_id`，Codex formal object model 的落地就会继续挂在多套 thread/session 标识上。
- 如果不先清理 runtime identity 和 provider 分叉，Step 3 很容易把 shadow history 再做成一层新的兼容胶水。

## Step 1: Direct Claude Code Removal

### Goal

第一步直接从 `libra code` 主路径删除 `claudecode`，使 `code` 命令的 managed runtime 只剩 Codex，不再保留第二套 provider-specific session / resume 模型。

### Affected Modules

| 模块 / 文件 | 调整内容 |
|---|---|
| `src/internal/ai/claudecode/` | 整体删除 |
| `src/command/code.rs` | 删除 `CodeProvider::Claudecode`、dispatch、参数校验、帮助文本、capability 分支 |
| `src/internal/tui/app.rs` | 删除 Claude managed runtime 分支 |
| `src/internal/tui/app_event.rs` | 删除带有 Claude managed 语义的命名残留 |
| `src/internal/ai/web/code_ui.rs` | 移除 `provider_session_resume` 这类 Claude-specific capability 语义 |
| `docs/commands/code.md` | 删除 `claudecode` provider、相关 flags、恢复说明 |
| `tests/command/code_claudecode_test.rs` | 删除整组测试 |
| `tests/data/ai/claude_managed_*` | 删除夹具 |

### Public Contract Changes

1. `--provider=claudecode` 被直接移除。
2. `--resume-session`、`--session-id`、`--resume-at`、`--fork-session` 被直接移除。
3. `code` 命令不再承诺任何 Claude provider session 恢复语义。
4. `provider_session_resume` 不再作为 Code UI capability 对外存在。

### User Migration And Release Impact

1. 这是一个明确的 breaking change，必须写入 release notes。
2. 旧的 `--provider=claudecode` 调用方式不再可用；用户应改用 `--provider=codex` 或通用 completion provider。
3. 旧的 Claude managed sessions 不再通过 `libra code` 恢复。
4. 历史磁盘数据可以物理保留，但 `libra code` 主路径不再读取、恢复、解释或展示这些 artifacts。
5. README、命令文档、帮助文本都必须同步删除 Claude managed runtime 示例，避免用户继续依赖已删路径。

### Non-goals

1. 不保留 deprecated flag。
2. 不保留 feature flag。
3. 不保留只读 `claudecode` runtime。
4. 不保留兼容 shim、隐藏桥接入口或离线恢复入口。

### Definition Of Done

1. 仓库中不再编译或引用 `src/internal/ai/claudecode/`。
2. `src/command/code.rs` 中不再存在 `CodeProvider::Claudecode`、相关 flags、相关帮助文本和相关 capability 分支。
3. `docs/commands/code.md`、README、改进计划都不再把 `claudecode` 作为支持项。
4. `tests/command/code_claudecode_test.rs` 和 `tests/data/ai/claude_managed_*` 已删除。
5. `code` 命令对外接口不再出现通用 `provider session` 恢复语义。

## Step 2: Thread ID Unification

### Goal

把 Libra `thread_id` 收敛为 `code` 命令唯一的用户可见恢复 ID，并让 CLI、Web、MCP、Code UI、projection rebuild 共享同一个 canonical identity。

### Canonical Rule

1. `ThreadProjection.thread_id` / `ai_thread.thread_id` 是 Libra formal thread 的主键。
2. `code` 命令的本地保存/恢复模型必须与这个 `thread_id` 对齐，不能再独立维护另一套不同语义的 `SessionState.id`。
3. Codex 的 `threadId` 不是 Libra thread identity；它只作为 `provider_thread_id` 写入 metadata 和对外诊断字段。
4. 非 Codex provider 不再引入 provider-native session/thread 概念；恢复统一从 Libra `thread_id` 出发。
5. projection rebuild 在存在既有 Libra `thread_id` 时必须复用该值，而不是再从 intent/task roots 派生一套新的 thread identity。

### Field And Name Changes

| 当前类型 / 字段 | 当前问题 | 目标调整 |
|---|---|---|
| `SessionState.id` | 本地会话文件 ID，语义与 Libra formal thread 不一致 | 收敛为 canonical `thread_id`，或作为 `thread_id` 的 serde alias 仅用于本地数据迁移 |
| `CodeUiSessionSnapshot.session_id` | UI 快照仍使用 session 术语 | 重命名为 `thread_id`，对外 JSON 使用 `threadId` |
| `ThreadProjection.thread_id` | 语义正确，但当前未统一到保存/恢复链路 | 保持不变，成为 `code` 命令 thread identity 的 source of truth |
| `ai_thread.thread_id` | 与设计一致，但未统一到 CLI / MCP 返回字段 | 保持不变，作为 SQLite projection 主键 |
| `AgentEvent::ManagedResponseComplete.provider_session_id` | 对 Codex 路径命名不准确 | 改为 `provider_thread_id` |
| `CodeUiCapabilities.provider_session_resume` | 带有 Claude managed 语义 | 删除该 capability |
| `session.metadata["provider_thread_id"]` | 目前只在目标态中出现 | 保留为 Codex-only runtime 映射字段 |

### Affected Modules

| 模块 / 文件 | 调整内容 |
|---|---|
| `src/internal/ai/session/state.rs` | 让本地会话持久化对齐 canonical `thread_id` |
| `src/internal/ai/session/store.rs` | 读写 keyed by canonical `thread_id` |
| `src/command/code.rs` | `--resume [THREAD_ID]` 只接受 Libra canonical `thread_id` |
| `src/internal/ai/web/code_ui.rs` | snapshot 字段从 `session_id` 收口到 `thread_id` |
| `src/internal/ai/web/mod.rs` | `/threads` 和 live thread summary 返回 canonical `threadId` |
| `src/internal/ai/mcp/resource.rs` | `list_saved_threads_impl()` 返回 canonical `threadId`，并可选返回 `providerThreadId` |
| `src/internal/ai/projection/rebuild.rs` | rebuild 必须优先复用 canonical `thread_id` |
| `src/internal/tui/app_event.rs` | Codex 路径下改用 `provider_thread_id` 命名 |
| `src/internal/ai/codex/mod.rs` / `src/internal/ai/codex/tui.rs` | provider `threadId` 只作为内部映射字段，不再污染 UI / session 术语 |

### Data Migration Policy

这一节是“本地 thread 数据格式迁移”，不是“保留 `claudecode` runtime 兼容层”。两者必须明确区分。

1. 允许在本地 session 序列化和 Code UI snapshot 中一次性兼容读取 legacy `id` / `session_id` 字段。
2. 允许为已存在的 saved sessions 做一次 backfill：解析或创建 canonical Libra `thread_id`，并把旧值保存在 `legacy_session_id` metadata 中用于本地兼容回查。
3. 不允许继续把 `session_id` 作为用户可见的主术语。
4. 兼容读取只服务于本地持久化数据升级，不服务于对外 CLI / Web / MCP 契约。

### Definition Of Done

1. `--resume [THREAD_ID]` 是唯一的用户可见恢复入口。
2. `list_saved_threads_impl()`、Web `/threads`、Code UI snapshot 都以 canonical `threadId` 为主字段。
3. `provider_thread_id` 只作为可选内部映射字段存在。
4. projection rebuild 在存在 canonical `thread_id` 时不再自行派生新的线程标识。
5. 用户可见文档和接口中不再暴露泛化的 `session_id` 术语。

## Step 3: Codex Formal Workflow Closure

### Goal

让 Codex `Execute` 路径完整落回 Libra 的 formal Snapshot / Event / Projection 模型，并让 formal objects + ThreadProjection 成为唯一的运行时真相源。

### Required Formal Writes By Phase

| Phase | 必须写入 |
|---|---|
| Phase 1 | `Intent`、`Plan`、`Task` |
| Phase 2 | `Run`、`PatchSet`、`Provenance`、`TaskEvent`、`RunEvent`、`PlanStepEvent`、`ToolInvocation`、`Evidence`、`ContextFrame`、`RunUsage` |
| Phase 3 | `Evidence`、`Decision`、terminal `TaskEvent` / `RunEvent` / `IntentEvent`，必要时 final `ContextSnapshot` |
| Phase 4 | 用户或系统的最终 `Decision`，以及线程/调度投影推进 |

### Tool Boundary And MCP Exposure

#### Core Rule

MCP 功能不是通过 prompt 暴露给 Codex 的。工具是否存在，取决于 MCP registration + connection；工具如何被使用，才由 prompt 来约束。

#### Libra-first Tool Policy

1. AI 开发工具可以桥接给 provider 使用，但结果必须回写 Libra 对象模型。
2. Libra VCS 工具是 provider 唯一允许的 VCS 入口。
3. `shell` 可以保留，但只用于构建、测试、读环境和非 VCS 命令。
4. `shell` 必须拦截 `git`、`jj` 和其他 VCS 命令，并提示改用 Libra 工具。

#### AI Development Tools

| 工具 | 说明 |
|---|---|
| `read_file` | 读文件，支持分页 |
| `list_dir` | 列目录，支持深度 |
| `grep_files` | 正则搜索 |
| `shell` | 非 VCS shell 命令；VCS 命令必须被拦截 |
| `apply_patch` | 应用补丁 |
| `update_plan` | 更新计划状态 |
| `submit_intent_draft` | 提交 IntentDraft |

`request_user_input` 继续由 TUI 交互通道处理，不作为 provider 直连桥接工具。

#### Libra VCS Tools

| 类别 | 工具 |
|---|---|
| 读操作 | `libra_status` / `libra_log` / `libra_diff` / `libra_show` / `libra_blame` / `libra_branch_list` / `libra_grep` / `libra_shortlog` / `libra_describe` / `libra_reflog` / `libra_cat_file` / `libra_show_ref` |
| 写操作 | `libra_add` / `libra_commit` / `libra_push` / `libra_pull` / `libra_fetch` / `libra_merge` / `libra_rebase` / `libra_reset` / `libra_restore` / `libra_revert` / `libra_cherry_pick` / `libra_switch` / `libra_branch_create` / `libra_branch_delete` / `libra_tag` / `libra_stash` / `libra_clean` / `libra_mv` / `libra_remove` |
| 仓库管理 | `libra_init` / `libra_clone` / `libra_remote` / `libra_config` / `libra_worktree` |

#### Workflow Object Tools

| 类别 | 工具 | 所属层级 | 用途 |
|---|---|---|---|
| Snapshot | `create_intent` / `create_task` / `create_run` / `create_plan` / `create_patchset` / `create_context_snapshot` / `create_provenance` | Snapshot | 定义与结构化快照写入 |
| Event | `create_tool_invocation` / `create_evidence` / `create_decision` / `create_context_frame` / `create_plan_step_event` / `create_run_usage` | Event | 执行事实与审计记录 |
| Query / List | `list_intents` / `list_threads` / `list_tasks` / `list_runs` / `list_plans` / `list_patchsets` / `list_evidences` / `list_tool_invocations` / `list_provenances` / `list_decisions` / `list_context_frames` / `list_plan_step_events` / `list_run_usages` | Projection / Query | 当前视图与历史读取 |

### Provider-specific History Target State

这一节必须明确决策，避免后续实现摇摆。

1. `src/internal/ai/codex/history.rs` 和 provider-specific `*_snapshot` / event families 不能继续作为 `libra code` 运行时的主真相源。
2. `libra code` 主路径必须以 formal MCP/git-internal Snapshot/Event 和 Libra `ThreadProjection` / `Scheduler` projection 为唯一数据源。
3. 如果未来仍需要 provider-specific 历史读取能力，它只能存在于独立的离线诊断或迁移工具中，不能继续挂在 `libra code` runtime path 上。
4. 本计划不保留“双写 formal objects + shadow history 再慢慢迁”的长期中间状态；Step 3 的目标是把运行时读取和写入都切回 formal 模型。

### Additional Object-model Requirements

1. `PlanStep.step_id` 必须是跨 plan revisions 稳定的逻辑标识，不能继续使用 `plan_id + ordinal` 这种会在 replan 时变更 identity 的生成方式。
2. `ContextFrame` 只存摘要型增量事实，不作为原始 provider 输出容器。
3. final `ContextSnapshot` 只能在确实需要冻结稳定上下文时写入，不能把每轮 turn completion 都写成默认 snapshot。

### Affected Modules

| 模块 / 文件 | 调整内容 |
|---|---|
| `src/internal/ai/codex/mod.rs` | turn lifecycle、plan updates、run usage、decision、formal object writes、MCP connection 注入 |
| `src/internal/ai/codex/tui.rs` | 扩展 `McpExecutionTracker`，补齐 formal Event / Snapshot 写入 |
| `src/internal/tui/app.rs` | review loop、execute path、ContextFrame 接线、post-plan choice 对齐 formal objects |
| `src/internal/ai/codex/history.rs` | 退出 `libra code` 主路径；不再承担 thread view / summary 的运行时真相源职责 |
| `src/internal/ai/mcp/resource.rs` | 补齐 workflow object tools，统一 list/query 路径 |
| `src/internal/ai/tools/handlers/*` | 复用现有 handler/schema 暴露 AI dev tools 和 Libra VCS tools |
| `src/internal/ai/web/*` | Web thread list、live thread summary、Code UI snapshot 全部从 canonical thread + formal objects 出发 |

### Definition Of Done

1. Codex `Execute` 路径稳定写出 formal Phase 1/2/3/4 所需的最小对象集合。
2. `McpExecutionTracker` 不再只是局部 `ContextFrame` 接线，而是形成完整 formal write path。
3. provider-specific history 不再驱动 thread summaries、thread rebuild 或 live runtime reads。
4. `PlanStep.step_id` 在 plan revisions 间保持稳定。
5. prompt assembly 只承担阶段引导，不承担 MCP 暴露或对象持久化职责。
6. `shell` 的 VCS 绕行路径被拦截，provider 的 VCS 意图全部通过 Libra 工具完成。

## Appendix A: Current Prompt Assembly

本节记录当前实现细节，便于实现 Step 3 时理解现状。它不是上位契约，真正的上位契约以前文 `Workflow Contract` 和三个 Step 为准。

### Entry Point

Codex TUI 运行时在 `run_tui_turn_with_revision()` 中调用 `runtime_handle.adapter().submit_message(prompt)`。实际的 prompt assembly 发生在 `CodexCodeUiAdapter::submit_message()` 与 `submit_thread_message()` 中。

最终发给 Codex app-server 的 `turn/start` 请求，协议层真实使用的是 `threadId`：

```json
{
  "input": [{ "type": "text", "text": "<request_text>" }],
  "threadId": "...",
  "approvalPolicy": "...",
  "collaborationMode": { "mode": "plan", ... }
}
```

其中，`request_text` 是 prompt assembly 的产物；`threadId` 是 Codex app-server 在 `thread/start` 后返回的 provider 线程标识，后续所有 `turn/start` 都复用它；当前实现没有在这条 RPC 链路中并行使用独立的 Codex `sessionId`。

### Current Branches

| 分支 | 判定条件 | 发送内容 |
|---|---|---|
| 计划修订 | 当前线程存在 `pending_plan_revision` | 使用 `codex_revise_plan_prompt(plan_text, user_request)`，且 `plan_first = false` |
| 默认提交 | 不存在 `pending_plan_revision` | 先保留用户输入 `text`，再在 `submit_thread_message()` 中按 `plan_first = true` 包装 |
| 执行已批准计划（Code UI 交互） | `respond_interaction()` 收到 `selected_option == "execute"` | 使用 `codex_execute_approved_plan_prompt(plan.text)`，且 `plan_first = false` |

### Current Prompt Templates

1. 首轮 / 普通消息：`codex_plan_first_prompt(request)`。
2. 计划修订：`codex_revise_plan_prompt(plan_text, request)`。
3. 执行已批准计划：`codex_execute_approved_plan_prompt(plan_text)`。

其中首轮模板中的 “3-7 numbered objectives” 是 prompt steering，不是协议级硬约束；它的目标是帮助模型稳定输出“像计划”的 Markdown 审查文本。

### First Turn And IntentSpec

按当前实现，首轮不会把 `IntentSpec` 发送给 Codex。首轮路径是：用户原始输入 -> `codex_plan_first_prompt(request)` -> `turn/start` -> Codex 返回 plan text / response text -> Libra 本地 `resolve_intentspec(...)` / `prepare_review_from_existing_spec(...)` -> Markdown review。

当前约束应明确为：

1. 首轮不向 Codex 发送 `IntentSpec`。
2. 不把原始 `IntentSpec JSON` 当作首轮 prompt 的一部分注入给 Codex。
3. `IntentSpec` 是 Libra 本地对象，在 Codex 返回后生成并进入 Markdown review。
4. 对后续非首轮场景，如果需要把已确认的计划上下文发回 Codex，优先级应是：已批准的 Markdown plan -> 经过人工确认的 spec 摘要 -> 最后才考虑结构化对象内容。

### Plan Revision Prompt Semantics

`codex_revise_plan_prompt(plan_text, request)` 只承担 prompt 层的行为引导，不等同于对当前 `IntentSpec` 做原地修改。它表达的是“基于当前已审查计划进行 revision”，而不是“把 provider 当作 Intent 对象的可变写入端”。

### Execute Prompt Consolidation

当前实现里存在两个执行阶段 prompt 入口：

1. Code UI 交互确认执行：直接使用 `codex_execute_approved_plan_prompt(plan.text)`。
2. TUI `start_codex_execution()`：先构造本地执行提示，再走标准 `submit_message()` 路径。

Step 3 的目标之一是统一“执行态 prompt assembly”的单一入口。

## Appendix B: ContextFrame And Provider Mapping

### ContextFrame Contract

`ContextFrame` 是 Event，不是原始日志容器。它的目标是维护 live context window，而不是复制 provider 的所有原始输出。

#### commandExecution

1. `commandExecution` 继续作为 `ToolInvocation` 记录执行事实。
2. 同时产出摘要型 `ContextFrame`，包括命令、exit code、cwd、是否产生文件变更。
3. `ContextFrame` 是 live context window 的增量事实，不替代 `ToolInvocation`。

#### agentMessage

1. `agentMessage` 只产出 commentary / intent-analysis 风格的摘要型 `ContextFrame`。
2. 不写入完整原文副本，避免把 provider 输出无界灌入上下文窗口。

#### contextCompaction

1. `contextCompaction` 也应纳入 `ContextFrame` 体系。
2. 它是上下文窗口维护事实的一部分，应作为 Event 记录压缩/收敛行为。

### Provider Thread Mapping

Codex 的 `threadId` 是 provider 运行态标识；Libra 自身仍维护自己的 `thread_id`。两者不能混为一谈。

当前实现中的命名问题是：

1. Codex 协议层真实传输的是 `threadId`。
2. Libra Code UI / TUI 某些通用字段当前命名为 `session_id` 或 `provider_session_id`，但在 Codex 路径下实际承载的是 provider `threadId`。

因此，文档语义必须以 `provider_thread_id` 为准，而不是继续暗示 Codex 同时存在一个独立的 `provider_session_id` 概念。

目标态语义是：

```rust
session.metadata.insert("provider_thread_id", codex_thread_id);
```

该字段只用于恢复、查询和诊断，不替代 Libra 自身的 `thread_id`。

### TUI Display Goal

TUI 的目标是“线程标识可追踪”，而不是硬性要求任何布局中都完整显示 UUID。下一阶段的 UI 要求是：默认在可见位置展示线程标识；窄终端允许截断；宽终端或详情视图可以展示完整值；不破坏现有 cwd / branch badge 的显示。

## Documentation Follow-ups

本计划文档调整后，后续实现落地时还需要同步更新：

1. `docs/commands/code.md`
2. `docs/improvement/README.md` 的状态描述
3. README 和帮助文本中的 provider / flags / 恢复说明

尤其需要同步的是：

1. Codex provider 的 `approvalPolicy` 说明。
2. native plan mode 语义。
3. Libra VCS 工具与 `shell` 约束。
4. MCP 能力暴露方式。
5. `thread_id` 统一后的 CLI / Web / MCP 对外字段。
6. `claudecode` 移除后的 provider / flag / 帮助文本。

## Validation

验证按 Step 1 -> Step 2 -> Step 3 的顺序组织，每一步都有独立的完成定义，同时保留跨步骤的最终一致性检查。

### Step 1 Validation: Claude Code Removal

1. provider 列表中不再包含 `claudecode`。
2. `code` 命令不再接受 `--resume-session`、`--session-id`、`--resume-at`、`--fork-session`。
3. 仓库中不再编译或引用 `src/internal/ai/claudecode/`。
4. `tests/command/code_claudecode_test.rs` 与 `tests/data/ai/claude_managed_*` 已删除。
5. README、命令文档和帮助文本不再把 `claudecode` 作为支持中的 provider。

### Step 2 Validation: Thread ID Unification

1. `--resume <THREAD_ID>` 以 Libra canonical `thread_id` 作为唯一恢复入口。
2. `thread_id` 成为 CLI、Web、MCP 共享的唯一用户可见恢复 ID。
3. `provider_thread_id` 只作为可选内部映射字段存在。
4. `list_saved_threads_impl()` 返回 canonical `threadId`，并可选返回 `providerThreadId`。
5. 新会话、resume、projection rebuild 下的 `thread_id` 语义一致。
6. 用户可见接口和文档中不再出现泛化 `session_id` 作为主术语。

### Step 3 Validation: Codex Formal Workflow Closure

1. Codex turn 进入 native `plan` collaboration mode。
2. `PostPlanChoice` 路径能够进入 IntentSpec review。
3. early intercept 路径能够在响应未完全结束前进入 review。
4. timeout fallback 路径能够从 prompt 合成 review。
5. `Modify` 能回到 generic planner，并形成新的 `Intent` revision chain，而不是覆盖旧 `Intent`。
6. 修订后的每一版都会重新进入 `Execute / Modify / Cancel` 审查。
7. `Execute` 能进入 Codex 直连执行路径，并稳定写出 formal Phase 1/2/3/4 所需对象。
8. 合成出的 `IntentSpec` 以易读 Markdown review 显示，而不是原始 JSON。
9. `PlanStepEvent`、`RunUsage`、`Evidence`、`Decision`、terminal 事件都按设计落到 formal object model。
10. provider-specific history 不再驱动主路径的 thread summary / rebuild / runtime reads。

### Cross-cutting Validation

1. 文档对 Phase 0-4 的描述与 `docs/agent/agent-workflow.md` 一致。
2. 文档对 Snapshot / Event / Projection 的分层与 `docs/agent/ai-object-model-reference.md` 一致。
3. 文档把“已实现”和“待补齐”清晰分开，没有把未来能力写成现状事实。
4. 文档明确区分 revision chain 的 Snapshot 语义和 `pending_plan_revision` / `revision_number` 的 Libra runtime 语义。
5. 文档明确区分 canonical `thread_id` 与可选 `provider_thread_id` 的边界。
6. provider 可见 Libra VCS 工具，`shell` 中的 `git` / `jj` / 其他 VCS 命令会被拦截并提示改用 Libra 工具。
7. thread 标识在 UI 中可见且可追踪，窄终端截断策略不破坏 cwd / branch 显示，宽终端或详情视图可展示完整值。
