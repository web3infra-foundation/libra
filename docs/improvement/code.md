# Code 命令改进详细计划

```
User Input / --resume <thread_id>
         │
         ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  PHASE 0  Input Preprocessing                                                        ║
║  ─────────────────────────────────────────────────────────────────────────────────── ║
║  Intent 提取 · 风险评估                                                              ║
║  Thread bootstrap (new / resume)  ·  Scheduler init  ·  ContextSnapshot (条件写入)  ║
║  → writes: Intent[S]                                                                 ║
╚══════════════════════════════════════════════════════════════════════════════════════╝
         │
         ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  PHASE 1  Planning & Review                                                          ║
║  ─────────────────────────────────────────────────────────────────────────────────── ║
║                                                                                      ║
║   ┌─────────────────────────────────┐    ┌─────────────────────────────────────┐    ║
║   │  Codex  (collaborationMode)     │    │  Generic Provider (CompletionModel) │    ║
║   │  turn/start → plan text         │    │  plan-first prompt → plan text      │    ║
║   └────────────────┬────────────────┘    └──────────────────┬──────────────────┘    ║
║                    └──────────────────┬──────────────────────┘                      ║
║                                       ▼                                              ║
║                    ┌──────────────────────────────────────────┐                     ║
║                    │    IntentSpec Markdown Review UI          │                     ║
║                    │    show_intentspec_review()  [共享函数]   │                     ║
║                    │   ┌──────────┬──────────┬─────────────┐  │                     ║
║                    │   │ Execute  │  Modify  │   Cancel    │  │                     ║
║                    │   └────┬─────┴────┬─────┴──────┬──────┘  │                     ║
║                    └════════╪══════════╪════════════╪══════════┘                     ║
║                             │          │            └──→ Decision(Cancelled)[E]      ║
║                             │   handle_modify_request()  [共享函数]                  ║
║                             │   new Intent[S] · Plan[S] · Task[S]                   ║
║                             │   Scheduler.selected_plan_id ← new Plan               ║
║                             │   (回到 Review 循环)                                   ║
║  → writes: Plan[S]  Task[S] │                                                        ║
╚═════════════════════════════╪════════════════════════════════════════════════════════╝
                              │ Execute
                              ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  PHASE 2  Execution   [控制权归 Libra Scheduler]                                    ║
║  ─────────────────────────────────────────────────────────────────────────────────── ║
║                                                                                      ║
║  Libra Scheduler                                                                     ║
║  │  next_ready_task()                                                                ║
║  │  load prerequisite context (ContextFrame / PatchSet)                              ║
║  │                                                                                   ║
║  │          ┌──────────────────────────┐   ┌──────────────────────────────────┐     ║
║  │          │   CodexTaskExecutor      │   │   CompletionTaskExecutor<M>      │     ║
║  │          │   impl TaskExecutor      │   │   impl TaskExecutor              │     ║
║  │          │  ──────────────────────  │   │  ──────────────────────────────  │     ║
║  │          │  WS → Codex app-server  │   │  CompletionModel API             │     ║
║  │          │  collaborationMode:plan  │   │  tool_loop                       │     ║
║  │          │  approvalPolicy=never    │   │  retry / replan                  │     ║
║  │          └────────────┬─────────────┘   └──────────────────┬───────────────┘     ║
║  │                       └────────────────────────────────────┘                     ║
║  │                                  │ TaskExecutionResult                            ║
║  │                                  │ (PatchSet diff · tool calls · usage)          ║
║  │                                  ▼                                                ║
║  │          ┌─────────────────────────────────────────────────────────────┐         ║
║  │          │   WorkflowRuntime  [所有 provider 共享的 formal write 层]   │         ║
║  │          │  ─────────────────────────────────────────────────────────  │         ║
║  │          │  write_run · write_patchset · write_provenance              │         ║
║  │          │  append_task_event · run_event · plan_step_event            │         ║
║  │          │  append_tool_invocation · evidence · context_frame          │         ║
║  │          │  append_run_usage                                            │         ║
║  │          └─────────────────────────────────────────────────────────────┘         ║
║  │  mark_task_complete()                                                             ║
║  └─ advance to next ready task                                                       ║
║                                                                                      ║
║  → writes: Run[S]  PatchSet[S]  Provenance[S]                                       ║
║            TaskEvent[E]  RunEvent[E]  PlanStepEvent[E]  ToolInvocation[E]           ║
║            Evidence[E]  ContextFrame[E]  RunUsage[E]                                 ║
╚══════════════════════════════════════════════════════════════════════════════════════╝
                              │ all tasks done
                              ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  PHASE 3  System-level Validation & Audit                                            ║
║  ─────────────────────────────────────────────────────────────────────────────────── ║
║  E2E tests · Performance · Compatibility  →  Evidence[E]                            ║
║  SAST · SCA · Compliance                  →  Evidence[E]                            ║
║                                                                                      ║
║  fail (auto-fixable) ──→ Phase 2  (new Run + replan · reset active_run_id)          ║
║  fail (audit)        ──→ Phase 2  (new Plan revision · reset current_plan_heads)    ║
║  pass                ──→ terminal TaskEvent[E] / RunEvent[E]                        ║
║                          optional ContextSnapshot[S]                                 ║
╚══════════════════════════════════════════════════════════════════════════════════════╝
                              │
                              ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  PHASE 4  Decision & Release                                                         ║
║  ─────────────────────────────────────────────────────────────────────────────────── ║
║  risk = Phase0.risk_level + Σ Evidence(Fail)×weight + diff_scope_score              ║
║                                                                                      ║
║  low risk  ──→ Decision(AutoMerge, chosen_patchset_id)                              ║
║                Thread.current_intent_id 推进 · Scheduler → idle                     ║
║                                                                                      ║
║  high risk ──→ Human Review UI (change summary · audit chain · impact)              ║
║                approve        → Decision(HumanApprove) · advance projections        ║
║                reject         → Decision(HumanReject)  · Scheduler → Phase 1        ║
║                request-changes→ new Intent revision    · Scheduler → Phase 1 Modify ║
║                                                                                      ║
║  → writes: Decision[E]  optional IntentEvent[E](terminal)                           ║
╚══════════════════════════════════════════════════════════════════════════════════════╝
                              │
                              ▼
╔══════════════════════════════════════════════════════════════════════════════════════╗
║  git-internal  (Libra object store)                                                  ║
║  Snapshot[S]  Intent · Plan · Task · Run · PatchSet · ContextSnapshot · Provenance  ║
║  Event[E]     TaskEvent · RunEvent · PlanStepEvent · RunUsage · ToolInvocation       ║
║               Evidence · Decision · ContextFrame · IntentEvent                       ║
╠══════════════════════════════════════════════════════════════════════════════════════╣
║  Libra Projection  (SQLite · rebuildable from Snapshot + Event)                      ║
║  Thread · Scheduler · QueryIndex · live_context_window                               ║
╚══════════════════════════════════════════════════════════════════════════════════════╝
```

## Context

本计划把 `libra code` 作为一个完整命令来收敛，目标有五项：

1. 删除 `claudecode` managed runtime 支持，使 `code` 命令不再维护第二套 provider-specific session 模型。
2. 统一 `thread_id` 语义，使 Libra `thread_id` 成为唯一的用户可见恢复标识，同时对齐 Scheduler Projection 字段。
3. 将 Codex 的执行控制权交还 Libra Scheduler，通过共享 `WorkflowRuntime` 层统一所有 formal object 写入。
4. 为通用 provider 补全可实施的 Phase 0-4 闭环，使其与 Codex 路径使用相同的 review loop、revision chain、formal write 和 projection 更新逻辑。
5. 统一 Web UI 集成，所有 provider 通过同一 `CodeUiProviderAdapter` 层接入，消除通用路径的 placeholder。

本计划采用"先定义上位契约，再说明当前基线，最后给出按顺序执行的实施步骤"的结构。阅读顺序和执行顺序保持一致，避免实现者在多个章节之间来回跳读。

### Hard Constraints

1. Libra 是版本管理核心，Codex 是接入到 Libra 工作流中的托管 provider/runtime。
2. provider 的 VCS 操作必须通过 Libra 能力完成，禁止直接使用 `git`、`jj` 或其它版本管理工具。
3. Codex 路径中的 `approvalPolicy` 以目标态强制为 `never`。
4. 所有阶段划分、对象写入和运行时投影，都必须遵循 `docs/agent/agent-workflow.md` 和 `docs/agent/ai-object-model-reference.md`。
5. 合成出的 `IntentSpec` 必须以易读的 Markdown review 形式展示给开发者确认，不能以原始 JSON 或难以审阅的结构化对象替代审查界面。
6. Libra `thread_id` 是 `code` 命令唯一的用户可见恢复 ID；provider-specific ID 只允许作为内部映射字段存在。
7. `claudecode` 不是过渡期保留组件；第一步即直接删除其全部运行时代码、CLI 标志、文档说明和测试，不保留 feature flag、deprecated path、兼容别名或中间运行态。
8. "不保留中间过渡状态"只适用于 `claudecode` runtime 和其对外接口；本地 `thread_id` 数据迁移允许一次性兼容读取旧字段，但不能反向保留旧的用户可见契约。
9. Libra Scheduler 拥有 Phase 2 的控制权：Codex 不能自主跑完整个计划；Libra 按 Task 驱动所有 provider（包括 Codex），而不是 provider 告诉 Libra 执行到了哪一步。
10. 持久化统一：所有 provider 路径只允许通过共享 `WorkflowRuntime` 层写入 formal objects，禁止再写 provider-specific shadow snapshot/event 家族。
11. Projection 字段直接对应：代码中的 `pending_plan_revision` 必须重构为 `Scheduler.selected_plan_id` / `current_plan_heads` 直接映射，不允许在 `code` 命令专有状态中另造独立变量。
12. 通用方案对等原则：凡是 Codex 路径具备的 formal 能力（review loop、revision chain、formal writes、projection 更新），通用 provider 路径必须具备相同能力，不允许降级为 placeholder 或 stub。
13. Query Index 可重建契约：`Thread`、`Scheduler`、Query Index 的丢失不能阻塞读访问；必须能从 Snapshot + Event 完整重建，重建入口必须有显式触发条件和降级读路径定义。

## Recommended Reading Order

建议按下面顺序阅读本计划：

1. `Workflow Contract`：先建立上位模型、Phase 0-4 的边界，重点理解 Phase 3/4 状态机和控制权归属。
2. `Query Index And Rebuild/Read Contract`：理解投影层的可重建承诺。
3. `Current Baseline`：理解当前代码已经做到什么、哪些是核心缺陷（而非中性事实陈述）。
4. `Delivery Order`：理解这批改造为什么必须按 Step 1 → Step 5 执行。
5. `Step 1 / Step 2 / Step 3 / Step 4 / Step 5`：按执行顺序阅读实施内容、影响模块、完成定义和用户影响。
6. `Shared Function Boundary`：理解哪些函数必须共享，哪些仅 provider 适配层可实现。
7. `Module Design`：理解 `WorkflowRuntime` 共享层和 TUI 解耦方案。
8. `Appendix A / Appendix B`：再看当前 prompt assembly、ContextFrame、provider mapping 等实现细节。
9. `Validation`：最后看验收标准和完整测试策略。

---

## Workflow Contract

Codex 集成和通用 provider 集成都必须服从 `docs/agent/agent-workflow.md` 定义的 Phase 0-4 工作流，而不是各自维护一套 provider 层状态机。系统真相必须落在 Libra 定义的 Snapshot / Event / Projection 三层边界内。

### Phase-to-Layer Mapping

| Phase | 目标 | Snapshot 写入 | Event 写入 | Libra runtime / projection |
|---|---|---|---|---|
| Phase 0 | 输入与上下文引导 | `Intent`，必要时 `ContextSnapshot` | none | Thread 初始化、Scheduler 初始化、live context bootstrap |
| Phase 1 | 计划构建与审查 | `Plan`、`Task` | none | selected_plan_id、current_plan_heads、ready queue、审查 UI |
| Phase 2 | 执行与过程事实 | `Run`、`PatchSet`、`Provenance` | `TaskEvent`、`RunEvent`、`PlanStepEvent`、`ToolInvocation`、`Evidence`、`ContextFrame`、`RunUsage` | active_run_id、live_context_window、staging 状态 |
| Phase 3 | 验证与审计 | 可选 `ContextSnapshot` | `Evidence`、`Decision`、terminal `TaskEvent` / `RunEvent` / `IntentEvent` | 审计视图、release candidate 视图 |
| Phase 4 | 决策与释放 | none | `Decision`、可选 terminal `IntentEvent` | Thread / Scheduler 投影推进 |

### Phase 2 控制权归属

`agent-workflow.md` 明确：Phase 2 由 **Libra Scheduler** 读取 ready Task 并调度执行，mutable runtime state 归 Libra 所有。

原计划描述"Execute 走 Codex runtime 直连，不经过 generic Orchestrator\<M\>"是**当前基线的核心缺陷**，不是应延续的设计。

目标态（适用于所有 provider，包括 Codex 和通用 completion）：

```text
Libra Scheduler
  → 弹出当前 ready Task
  → 为该 Task 组装 prerequisite context（从 ContextFrame / PatchSet 加载）
  → 将 Task prompt + context 通过 TaskExecutor trait 发给 provider（Codex / generic）
  → 等待 provider 完成该 Task 返回结果
  → Libra（WorkflowRuntime）写入 Run / PatchSet / Events
  → Scheduler 推进到下一个 ready Task
  → 所有 retry / replan 由 Libra 决策，不由 provider 自主发起
```

### Phase 3 状态机

Phase 3 是"系统级验证 + 审计链收口"，必须定义明确的状态流：

```text
[Phase 3 Entry]
  │
  ├─ 全局验证：E2E tests / performance benchmarks / compatibility checks
  │    → 通过：产出 Evidence(status=Pass)
  │    → 失败：产出 Evidence(status=Fail)
  │         → 若可自动修复：回退到 Phase 2（新 Run + replan，更新 active_run_id、current_plan_heads）
  │         → 若需人工介入：挂起到 Phase 4 human review
  │
  ├─ 安全审计：SAST / SCA / compliance checks
  │    → 通过：产出 Evidence(status=Pass)
  │    → 失败：产出 Evidence(status=Fail, severity=High)
  │         → 一律回退到 Phase 2（写新 Plan revision + 新 Run）
  │         → Libra Scheduler 更新 current_plan_heads，active_run_id 重置为 None
  │
  ├─ 审计链收口：
  │    → 写 terminal TaskEvent / RunEvent
  │    → 按条件写 final ContextSnapshot
  │    → 写 terminal IntentEvent（如 Phase 3 是 terminal state）
  │
  └─ 进入 Phase 4
```

Phase 3 → Phase 2 回退时的投影更新：

```text
active_run_id = None（当前 Run 已终止）
selected_plan_id = 新 Plan revision ID
current_plan_heads = [新 Plan revision ID]
Scheduler.ready_queue 重新填充新 Plan 的 Task
```

### Phase 4 状态机

```text
[Phase 4 Entry]
  │
  ├─ 风险聚合：
  │    风险分 = Phase 0 risk_level 基础分
  │           + Σ Evidence(status=Fail) × severity_weight
  │           + diff_scope_score（文件数、行数）
  │
  ├─ 分支：
  │    低风险：
  │      → 写 Decision(kind=AutoMerge, chosen_patchset_id=<最终 PatchSet>)
  │      → Thread.current_intent_id = 已完成的 Intent
  │      → Scheduler.active_run_id = None，selected_plan_id = None
  │      → 写 terminal IntentEvent（可选）
  │
  │    高风险：
  │      → Libra UI 展示：change summary、audit chain、Evidence、impact analysis
  │      → 等待人工 approve / reject / request-changes
  │      → approve → Decision(kind=HumanApprove, chosen_patchset_id=...)，推进投影
  │      → reject  → Decision(kind=HumanReject)，Scheduler 回到 Phase 1
  │      → request-changes → 进入新 Intent revision loop（回到 Phase 1 Modify 流程）
  │
  └─ [Done]
```

### Phase 1 Review Contract

1. `IntentSpec` 必须以易读的 Markdown review 展示，而不是原始 JSON。
2. Markdown review 至少要突出 summary / problem statement、objectives、in scope / out of scope、acceptance criteria、risk / rationale。
3. 开发者必须在看到 Markdown review 之后，才能进入 `Execute / Modify / Cancel`。
4. `Modify` 不表示原地编辑当前对象，而表示创建新的 `Intent` revision（`Intent.parents` 指向前版），同时创建新 `Plan`（`Plan.parents` 指向前版），Scheduler 更新 `selected_plan_id` 到新 Plan。
5. `pending_plan_revision`、当前选中的 plan head 都属于 `Scheduler.selected_plan_id` / `current_plan_heads`，不允许在 `code` 命令状态中独立维护。
6. `Execute` 后，Scheduler 弹出 ready Task，进入 Phase 2 执行循环。
7. **通用 provider 与 Codex provider 必须使用相同的 Phase 1 review loop 实现。**

### Snapshot / Event / Projection Placement

根据 `docs/agent/ai-object-model-reference.md`：

- Snapshot 只存"定义了什么"：`Intent`、`Plan`、`Task`、`Run`、`PatchSet`、`ContextSnapshot`、`Provenance`。
- Event 只存"后来发生了什么"：`IntentEvent`、`TaskEvent`、`RunEvent`、`PlanStepEvent`、`RunUsage`、`ToolInvocation`、`Evidence`、`Decision`、`ContextFrame`。
- Libra projection 只存当前运行视图：`Thread`、`Scheduler`、live context window、query index、UI-facing current view。

实现约束：不能把 provider 运行态事实塞回 Snapshot，也不能把线程/调度视图固化成历史对象，更不能让 provider-specific history 继续成为 `code` 命令运行时的主真相源。

### ContextSnapshot 写入条件

下列情形写入 `ContextSnapshot`；不满足条件时不写：
- Phase 0 首次 run，且 workspace 存在未提交变更。
- Phase 3 验证通过后确认为 release candidate。
- 人工请求冻结当前上下文。

---

## Query Index And Rebuild/Read Contract

对应 `docs/agent/ai-object-model-reference.md` 第 213-330 行要求。

### 可重建承诺

| 投影对象 | 可重建来源 | 触发条件 |
|---|---|---|
| `ThreadProjection` | `Intent` + `Intent.parents` + `IntentEvent.next_intent_id` | Thread 行缺失或 version 不一致时 |
| `SchedulerState` | `Plan` + `Task` + `Run` + `PlanStepEvent` + `RunEvent` | Scheduler 行缺失或 active_run_id 指向已终止 Run 时 |
| Query Index | 扫描全量 Snapshot + Event | 索引行缺失或查询结果与 Snapshot 不一致时 |

### 读路径降级规则

```text
读 Thread：
  1. 优先 Libra projection（DB ThreadProjection 行）
  2. 若缺失：触发 rebuild，异步填充，同步返回重建结果
  3. 若重建失败：返回从 Intent history 直接构造的轻量视图，标记为"projection stale"

读 Scheduler：
  1. 优先 Libra projection（DB SchedulerState 行）
  2. 若缺失：从 Plan + Task + Run 事件流重建，同步返回
  3. 不允许因 Scheduler 缺失而阻塞 Phase 2 执行

读 Query Index（intent->plans 等）：
  1. 优先内存 / DB index
  2. 若缺失：全量扫描 Snapshot + Event 生成
  3. Index 扫描不影响主路径正确性，只影响读性能
```

### 重建策略

- Thread / Scheduler rebuild 仅触发一次，结果落库；不在每次请求时重建。
- rebuild 期间读取允许返回"stale"标记，不允许返回错误（保证可用性）。
- rebuild 失败时记录 `Evidence(kind=ProjectionRebuildFail)`，人工介入。

---

## Current Baseline

### 已成立的事实

| 能力 | 现状 |
|---|---|
| Native plan collaboration mode | Codex `turn/start` 已携带 `collaborationMode: {"mode":"plan"}` |
| 强制 Plan 审查入口 | Codex TUI 可在 Codex 响应后进入 `Execute / Modify / Cancel` 审查流 |
| Early intercept | `response_text >= 100` 或已有 plan summary 时可提前合成 IntentSpec |
| Timeout fallback | 30 秒无有效响应时，可从 prompt 合成 IntentSpec |
| IntentSpec 合成 | Codex 响应可转为以易读 Markdown 展示的 IntentSpec review，进入现有审查工作流 |
| Plan 修订 | 支持基于现有 IntentSpec 发起多轮 plan revision；当前待修订链由 Libra runtime 状态维护 |
| 局部 MCP 跟踪 | 当前已写入 prompt 提交和 post-plan 选择对应的 `ContextFrame` |
| ThreadProjection / SchedulerState | Projection 类型已完整定义（`projection/thread.rs`、`projection/scheduler.rs`） |
| Orchestrator\<M\> | 通用路径已有完整 Phase 0-4 骨架 |

### 已落地的主路径

```text
用户 Prompt
  -> Codex native plan mode
  -> PostPlanChoice 或 early intercept
  -> IntentSpec review
  -> TUI: [Execute] [Modify] [Cancel]
```

其中，`Modify` 走 generic planner 路径，继续使用现有 `submit_intent_draft` 工作流；`Execute` 走 Codex runtime 直连执行，不经过 generic `Orchestrator<M>`。

### 当前基线的核心缺陷

| 缺陷 | 严重性 | 说明 |
|---|---|---|
| **通用 provider 方案无可实施闭环** | 高 | 改进计划聚焦 Codex，通用 Orchestrator\<M\> 的 review loop、revision chain 均为 stub |
| **Phase 3/4 缺少状态机机制定义** | 高 | 有写入表格但无状态流、回退条件、投影推进细节 |
| **Query Index / Rebuild 合同未落地** | 高 | 对象模型要求明确，但无显式触发条件、降级读路径定义 |
| **Execute 走 Codex 直连，不走 Scheduler** | 高 | 控制权在 Codex，Libra 退化为 Logger，违反 Workflow Contract |
| **两套执行引擎共存** | 高 | `Orchestrator<M>` 与 Codex TUI turn loop 完全分离，持久化分叉 |
| **McpExecutionTracker 旁路写入** | 高 | TUI 层监听 Codex 事件并写 formal objects，UI 与核心模型严重耦合 |
| **共享函数边界未定义** | 中 | plan review、revision chain、formal write、projection update 哪些必须共享未说明 |
| `claudecode` provider 仍存在 | 中 | `code.rs` 仍有 Claudecode provider 和相关分支 |
| 多套 thread/session 标识并存 | 中 | `SessionState.id`、Code UI `session_id`、`provider_session_resume`、Codex `threadId` |
| provider-specific history 仍为主真相源 | 中 | `codex/history.rs` 仍驱动 thread summary / rebuild |
| `pending_plan_revision` 游离于 Scheduler 投影外 | 中 | 未映射到 `Scheduler.selected_plan_id` / `current_plan_heads` |
| Web UI 通用路径仅有 placeholder | 中 | `--web-only` 下非 Codex provider 无真实 Code UI 支持 |

---

## Delivery Order

本批次按下面顺序推进，不采用"先保留双栈、后慢慢下线"的中间状态：

1. `Step 1: Direct Claude Code Removal`
2. `Step 2: Thread ID Unification + Projection Alignment`
3. `Step 3: Codex Formal Workflow Closure + Inversion of Control`
4. `Step 4: Generic Provider Closed Loop`
5. `Step 5: Web UI Unification`

执行约束：

- Step 1 必须先于 Step 2，否则 `thread_id` 语义会继续被 `provider_session` 模型污染。
- Step 2 必须先于 Step 3，否则 Codex formal object model 的落地会继续挂在多套 thread/session 标识上。
- Step 3 必须先于 Step 4，否则共享 `WorkflowRuntime` 没有 Codex 路径的验证基础。
- Step 4 与 Step 5 可并行，但 Step 5 依赖 Step 4 的 `CodeUiProviderAdapter` 定义。
- `WorkflowRuntime` 骨架建议在 Step 2 完成后、Step 3 开始前先建立，Step 3/4 在其基础上填充实现。

---

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
| `src/internal/ai/web/code_ui.rs` | 移除 `provider_session_resume` capability、`session_id` 等 Claude-specific 字段 |
| `docs/commands/code.md` | 删除 `claudecode` provider、相关 flags、恢复说明 |
| `tests/command/code_claudecode_test.rs` | 删除整组测试（如存在） |
| `tests/data/ai/claude_managed_*` | 删除夹具（如存在） |

### Public Contract Changes

1. `--provider=claudecode` 被直接移除。
2. `--resume-session`、`--session-id`、`--resume-at`、`--fork-session` 被直接移除。
3. `code` 命令不再承诺任何 Claude provider session 恢复语义。
4. `provider_session_resume` 不再作为 Code UI capability 对外存在。

### User Migration And Release Impact

1. 这是一个明确的 breaking change，必须写入 release notes。
2. 旧的 `--provider=claudecode` 调用方式不再可用；用户应改用 `--provider=codex` 或通用 completion provider。
3. 历史磁盘数据可以物理保留，但 `libra code` 主路径不再读取、恢复、解释或展示这些 artifacts。

### Non-goals

不保留 deprecated flag、feature flag、只读 `claudecode` runtime、兼容 shim 或隐藏桥接入口。

### Definition Of Done

1. 仓库中不再编译或引用 `src/internal/ai/claudecode/`。
2. `src/command/code.rs` 中不再存在 `CodeProvider::Claudecode`、相关 flags、相关帮助文本和相关 capability 分支。
3. `docs/commands/code.md`、README、改进计划都不再把 `claudecode` 作为支持项。
4. `code` 命令对外接口不再出现通用 `provider session` 恢复语义。

---

## Step 2: Thread ID Unification + Projection Alignment

### Goal

把 Libra `thread_id` 收敛为 `code` 命令唯一的用户可见恢复 ID，同时将 TUI 专有状态（`pending_plan_revision` 等）对齐到 Scheduler Projection 字段，让 CLI、Web、MCP、Code UI、projection rebuild 共享同一个 canonical identity。

### Canonical Rule

1. `ThreadProjection.thread_id` / `ai_thread.thread_id` 是 Libra formal thread 的主键。
2. `code` 命令的本地保存/恢复模型必须与这个 `thread_id` 对齐，不能再独立维护另一套不同语义的 `SessionState.id`。
3. Codex 的 `threadId` 不是 Libra thread identity；它只作为 `provider_thread_id` 写入 metadata 和对外诊断字段。
4. 非 Codex provider 不再引入 provider-native session/thread 概念；恢复统一从 Libra `thread_id` 出发。
5. **`pending_plan_revision` 必须重构为对 `Scheduler.selected_plan_id` / `current_plan_heads` 的直接操作，不允许在 `code` 命令专有状态中独立维护。**
6. projection rebuild 在存在既有 Libra `thread_id` 时必须复用该值，而不是再从 intent/task roots 派生一套新的 thread identity。

### Field And Name Changes

| 当前类型 / 字段 | 当前问题 | 目标调整 |
|---|---|---|
| `SessionState.id` | 本地会话文件 ID，语义与 Libra formal thread 不一致 | 收敛为 canonical `thread_id`；旧值存 `legacy_session_id`（仅用于数据迁移） |
| `CodeUiSessionSnapshot.session_id` | UI 快照仍使用 session 术语 | 重命名为 `thread_id`，对外 JSON 使用 `threadId` |
| `ThreadProjection.thread_id` | 语义正确，但当前未统一到保存/恢复链路 | 保持不变，成为 `code` 命令 thread identity 的 source of truth |
| `ai_thread.thread_id` | 与设计一致，但未统一到 CLI / MCP 返回字段 | 保持不变，作为 SQLite projection 主键 |
| `AgentEvent::ManagedResponseComplete.provider_session_id` | 对 Codex 路径命名不准确 | 改为 `provider_thread_id` |
| `CodeUiCapabilities.provider_session_resume` | 带有 Claude managed 语义 | 删除该 capability |
| `session.metadata["provider_thread_id"]` | 目前只在目标态中出现 | 保留为 Codex-only runtime 映射字段 |
| **`pending_plan_revision`（TUI 状态）** | 游离于 Scheduler 投影外 | 重构为 `Scheduler.selected_plan_id` / `current_plan_heads` 直接映射 |

### Phase 0 Bootstrap 流程

```text
TUI 启动
  → 检查 --resume <thread_id>
  → 有：ThreadProjection::find_by_id() + SchedulerState 恢复，重建 live_context_window
  → 无：ThreadProjection::create()，初始化空 SchedulerState
  → 写入 Intent snapshot（Phase 0 写入点）
  → 仅满足条件时写 ContextSnapshot（见 ContextSnapshot 写入条件）
```

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
| `src/internal/ai/codex/mod.rs` | provider `threadId` 只作为内部映射字段，不再污染 UI / session 术语 |

### Data Migration Policy

1. 允许在本地 session 序列化和 Code UI snapshot 中一次性兼容读取 legacy `id` / `session_id` 字段。
2. 允许为已存在的 saved sessions 做一次 backfill：解析或创建 canonical Libra `thread_id`，并把旧值保存在 `legacy_session_id` metadata 中用于本地兼容回查。
3. 不允许继续把 `session_id` 作为用户可见的主术语。
4. 兼容读取只服务于本地持久化数据升级，不服务于对外 CLI / Web / MCP 契约。

### Definition Of Done

1. `--resume [THREAD_ID]` 是唯一的用户可见恢复入口。
2. `list_saved_threads_impl()`、Web `/threads`、Code UI snapshot 都以 canonical `threadId` 为主字段。
3. `provider_thread_id` 只作为可选内部映射字段存在。
4. `pending_plan_revision` 从代码库中消失，替换为对 `Scheduler.selected_plan_id` / `current_plan_heads` 的直接操作。
5. projection rebuild 在存在 canonical `thread_id` 时不再自行派生新的线程标识。
6. 用户可见文档和接口中不再暴露泛化的 `session_id` 术语。

---

## Step 3: Codex Formal Workflow Closure + Inversion of Control

### Goal

将 Codex 的执行控制权交还 Libra Scheduler，Codex 退化为实现 `TaskExecutor` trait 的 provider adapter；所有 formal object 写入通过共享 `WorkflowRuntime` 层完成；formal objects + ThreadProjection 成为唯一的运行时真相源。

不能仅仅是"补齐数据写入"，而是必须将控制权反转：Libra 的 Scheduler 喂 Task 给 Codex，而不是 Codex 告诉 Libra 执行到了哪一步。

### WorkflowRuntime 共享层

新建 `src/internal/ai/workflow_runtime/mod.rs`，提供所有 provider 路径共享的 formal object 写入 API：

```rust
/// 所有 provider 路径共享的 formal object 写入抽象
/// 通用 Orchestrator<M> executor 和 Codex adapter 都只调用这层
pub struct WorkflowRuntime {
    mcp: Arc<LibraMcpServer>,
    thread_id: ThreadId,
}

impl WorkflowRuntime {
    // Phase 0
    pub async fn write_intent(&self, spec: &IntentSpec) -> Result<Uuid>;
    pub async fn write_context_snapshot_if_needed(&self, ...) -> Result<Option<Uuid>>;
    pub async fn init_thread(&self, ...) -> Result<ThreadProjection>;
    pub async fn resume_thread(&self, thread_id: &ThreadId) -> Result<(ThreadProjection, SchedulerState)>;

    // Phase 1
    pub async fn write_plan(&self, intent_id: Uuid, spec: &ExecutionPlanSpec) -> Result<Uuid>;
    pub async fn write_task(&self, plan_id: Uuid, spec: &TaskSpec) -> Result<Uuid>;
    pub async fn update_scheduler_plan_head(&self, plan_id: Uuid) -> Result<()>;

    // Phase 2
    pub async fn write_run(&self, task_id: Uuid, ...) -> Result<Uuid>;
    pub async fn write_patchset(&self, run_id: Uuid, seq: u32, ...) -> Result<Uuid>;
    pub async fn write_provenance(&self, run_id: Uuid, ...) -> Result<Uuid>;
    pub async fn append_task_event(&self, ...) -> Result<()>;
    pub async fn append_run_event(&self, ...) -> Result<()>;
    pub async fn append_plan_step_event(&self, plan_id: Uuid, step_id: Uuid, ...) -> Result<()>;
    pub async fn append_tool_invocation(&self, run_id: Uuid, ...) -> Result<()>;
    pub async fn append_evidence(&self, run_id: Uuid, patchset_id: Option<Uuid>, ...) -> Result<()>;
    pub async fn append_context_frame(&self, kind: ContextFrameKind, ...) -> Result<()>;
    pub async fn append_run_usage(&self, run_id: Uuid, ...) -> Result<()>;

    // Phase 3 / 4
    pub async fn write_decision(&self, run_id: Uuid, chosen_patchset_id: Option<Uuid>, ...) -> Result<()>;
    pub async fn write_terminal_intent_event(&self, intent_id: Uuid, ...) -> Result<()>;
    pub async fn advance_thread_scheduler(&self, ...) -> Result<()>;
}
```

### TaskExecutor Trait（控制权反转）

```rust
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Libra Scheduler 调用：执行单个 Task，返回执行结果
    /// provider 不负责写 formal objects；写入由 Scheduler 通过 WorkflowRuntime 完成
    async fn execute_task(
        &self,
        task: &Task,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult>;
}

pub struct CodexTaskExecutor { ... }                    // impl TaskExecutor
pub struct CompletionTaskExecutor<M: CompletionModel> { ... } // impl TaskExecutor
```

### McpExecutionTracker 退出

`McpExecutionTracker` 被移除。TUI 只订阅 `broadcast::Receiver<WorkflowEvent>`，不调用任何 formal write API。formal writes 只由 `WorkflowRuntime` 在 Scheduler 层完成。

### Provider-specific History 退出

1. `src/internal/ai/codex/history.rs` 在 Step 3 完成后不再在 `libra code` 主路径中调用。
2. `codex/history.rs` 保留文件，顶部标记 `#[deprecated]`，供独立离线诊断工具使用。
3. `HISTORY_APPEND_LOCK` 全局 mutex 在调用方消失后删除。
4. `codex/view.rs` 的 `HistoryReader::rebuild_view()` 保留，仅供诊断，不在主路径。

### PlanStep.step_id 稳定性

在 Task 创建时生成 stable UUID 作为 `step_id`；replan 时新 Plan revision 的 steps 复用旧 Task 的 `step_id`（通过 `Task.origin_step_id` 追踪）；只有全新增加的步骤才生成新的 `step_id`。`PlanStepEvent.step_id` 指向 `Task.origin_step_id`，确保跨 revision 事件可对齐。

### Tool Boundary And MCP Exposure

MCP 功能通过 registration + connection 暴露，不通过 prompt 传递。

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

#### shell VCS 拦截实现

在 `src/internal/ai/tools/handlers/shell.rs` 的执行前 preflight 位置：

```rust
const VCS_BLOCKED_CMDS: &[&str] = &["git", "jj", "hg", "svn"];

fn check_vcs_interception(cmd: &str) -> Option<ToolError> {
    let bin_name = Path::new(cmd.split_whitespace().next().unwrap_or(""))
        .file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if VCS_BLOCKED_CMDS.contains(&bin_name) {
        Some(ToolError::Blocked {
            message: format!(
                "`{}` is not available in this context. \
                 Use the corresponding `libra_*` tool instead.",
                bin_name
            )
        })
    } else {
        None
    }
}
```

#### Libra VCS Tools

| 类别 | 工具 |
|---|---|
| 读操作 | `libra_status` / `libra_log` / `libra_diff` / `libra_show` / `libra_blame` / `libra_branch_list` / `libra_grep` / `libra_shortlog` / `libra_describe` / `libra_reflog` / `libra_cat_file` / `libra_show_ref` |
| 写操作 | `libra_add` / `libra_commit` / `libra_push` / `libra_pull` / `libra_fetch` / `libra_merge` / `libra_rebase` / `libra_reset` / `libra_restore` / `libra_revert` / `libra_cherry_pick` / `libra_switch` / `libra_branch_create` / `libra_branch_delete` / `libra_tag` / `libra_stash` / `libra_clean` / `libra_mv` / `libra_remove` |
| 仓库管理 | `libra_init` / `libra_clone` / `libra_remote` / `libra_config` / `libra_worktree` |

#### Workflow Object Tools

| 类别 | 工具 | 所属层级 |
|---|---|---|
| Snapshot | `create_intent` / `create_task` / `create_run` / `create_plan` / `create_patchset` / `create_context_snapshot` / `create_provenance` | Snapshot |
| Event | `create_tool_invocation` / `create_evidence` / `create_decision` / `create_context_frame` / `create_plan_step_event` / `create_run_usage` | Event |
| Query / List | `list_intents` / `list_threads` / `list_tasks` / `list_runs` / `list_plans` / `list_patchsets` / `list_evidences` / `list_tool_invocations` / `list_provenances` / `list_decisions` / `list_context_frames` / `list_plan_step_events` / `list_run_usages` | Projection / Query |

### Affected Modules

| 模块 / 文件 | 调整内容 |
|---|---|
| `src/internal/ai/workflow_runtime/mod.rs` | **新建**：WorkflowRuntime 共享写入层 |
| `src/internal/ai/codex/mod.rs` | 重构为 `CodexTaskExecutor`（TaskExecutor trait 实现者）；删除 HISTORY_APPEND_LOCK |
| `src/internal/ai/codex/history.rs` | 标记 deprecated，退出主路径 |
| `src/internal/ai/orchestrator/executor.rs` | 接入 WorkflowRuntime，使用 CompletionTaskExecutor |
| `src/internal/tui/app.rs` | 删除 McpExecutionTracker 写入分支；改为纯渲染 + broadcast 订阅 |
| `src/internal/ai/mcp/resource.rs` | 补齐 workflow object tools；统一 list/query 路径 |
| `src/internal/ai/tools/handlers/shell.rs` | 增加 VCS 拦截逻辑 |
| `src/internal/ai/web/*` | Web thread list、live thread summary 全部从 canonical thread + formal objects 出发 |
| `src/internal/ai/workflow_objects.rs` | 扩展：增加 Run/PatchSet/Evidence/Decision builder 函数 |

### Definition Of Done

1. `TaskExecutor` trait 存在，`CodexTaskExecutor` 和通用 `CompletionTaskExecutor` 都实现它。
2. Scheduler 主循环通过 `TaskExecutor` 驱动所有 provider，不存在任何 provider-specific 执行分支。
3. 所有 formal object 写入只通过 `WorkflowRuntime` 完成，TUI 不调用任何 `create_*` / `append_*` API。
4. `pending_plan_revision` 从代码库中消失。
5. `PlanStep.step_id` 在 plan revisions 间保持稳定。
6. `shell` 的 VCS 绕行路径被拦截。
7. provider-specific history 不再驱动主路径的 thread summary / rebuild / runtime reads。

---

## Step 4: Generic Provider Closed Loop

### Goal

为通用 provider（Anthropic、Gemini、OpenAI、DeepSeek、Zhipu、Ollama）补全可实施的 Phase 0-4 闭环，使其与 Codex 路径使用相同的 review loop、revision chain、formal write 和 projection 更新逻辑，不允许降级为 placeholder 或 stub。

### 当前差距

原 `run_tui_with_model()` 和 `Orchestrator<M>` 已有 Phase 0-4 骨架，但：
1. Plan review（IntentSpec Markdown 展示 + `Execute / Modify / Cancel`）缺失
2. Phase 1 plan-first 模式缺失
3. revision chain（Modify 产生新 Intent revision）未实现
4. Phase 3 Security Audit 和回退逻辑未实现
5. Phase 4 风险聚合和决策逻辑未实现

### 统一 IntentSpec Review UI

现有 Codex 路径的 Markdown review UI 抽取为独立函数，通用路径复用：

```rust
/// 展示 IntentSpec 审查视图，等待用户选择
/// 所有 provider 共用此函数
pub async fn show_intentspec_review(
    spec: &IntentSpec,
    tui_tx: &mpsc::Sender<AppEvent>,
) -> Result<ReviewChoice>;

pub enum ReviewChoice {
    Execute,
    Modify(String),  // 修订请求文本
    Cancel,
}
```

### 统一 Revision Chain

所有 provider 共用同一 revision chain 实现：

```rust
pub async fn handle_modify_request(
    runtime: &WorkflowRuntime,
    current_intent_id: Uuid,
    current_plan_id: Uuid,
    modify_request: &str,
    executor: &dyn TaskExecutor,
) -> Result<(Uuid, Uuid)>  // (new_intent_id, new_plan_id)
{
    // 1. 写新 Intent revision（Intent.parents = [current_intent_id]）
    // 2. 生成新 Plan（Plan.parents = [current_plan_id]）
    // 3. 写新 Task snapshots（复用稳定 step_id）
    // 4. 更新 Scheduler: selected_plan_id = new_plan_id, current_plan_heads = [new_plan_id]
}
```

### 通用 Provider 统一入口

```rust
pub async fn run_unified_provider_session<M: CompletionModel>(
    model: M,
    runtime: Arc<WorkflowRuntime>,
    config: TuiLaunchConfig,
) -> Result<()> {
    // Phase 0: 初始化 Thread + Scheduler，写 Intent
    // Phase 1: 生成计划 → show_intentspec_review()（统一）
    //   → Execute: 进入 TaskExecutor loop（Phase 2）
    //   → Modify:  handle_modify_request() → 更新 Scheduler → 重新进入 Phase 1
    //   → Cancel:  write_decision(kind=Cancelled) + write_terminal_intent_event()
    // Phase 2: Scheduler 通过 CompletionTaskExecutor 按 Task 执行
    // Phase 3: 全局验证 + 安全审计 → Evidence → 按状态机回退或前进
    // Phase 4: 风险聚合 → Decision → 投影推进
}
```

### Affected Modules

| 模块 / 文件 | 调整内容 |
|---|---|
| `src/command/code.rs` | `run_tui_with_model()` 替换为 `run_unified_provider_session()` |
| `src/internal/ai/orchestrator/mod.rs` | 接入 WorkflowRuntime；补全 Phase 3/4 状态机 |
| `src/internal/tui/app.rs` | 接入统一 IntentSpec review UI |
| `src/internal/ai/intentspec/` | `show_intentspec_review()` 统一实现 |
| `src/internal/ai/workflow_runtime/revision.rs` | `handle_modify_request()` 统一实现 |
| `src/internal/ai/orchestrator/decider.rs` | `aggregate_risk_score()` 共享实现 |

### Definition Of Done

1. 通用 provider 与 Codex provider 使用相同的 `show_intentspec_review()` 函数。
2. 通用 provider 的 `Modify` 产生新 Intent revision，链接 `Intent.parents`。
3. 通用 provider 的 Phase 3 Security Audit 路径存在并可按状态机回退到 Phase 2。
4. 通用 provider 的 Phase 4 风险聚合和 Decision 写入逻辑与 Codex 路径行为一致。
5. 两条路径都通过 DAG 同构测试。

---

## Step 5: Web UI Unification

### Goal

所有 provider 通过同一 `CodeUiProviderAdapter` 层接入 Web UI，消除通用路径的 placeholder。

### 实施内容

```rust
pub trait CodeUiProviderAdapter: Send + Sync {
    async fn submit_message(&self, text: &str, thread_id: &ThreadId) -> Result<()>;
    fn current_snapshot(&self) -> CodeUiSessionSnapshot;
    fn capabilities(&self) -> CodeUiCapabilities;
}

pub struct CodexCodeUiAdapter { ... }
pub struct GenericCodeUiAdapter<M: CompletionModel> { ... }
```

统一 Web thread list、live thread summary：

```rust
async fn build_web_code_ui_runtime(
    args: &CodeArgs,
    working_dir: &Path,
    adapter: Arc<dyn CodeUiProviderAdapter>,
) -> Arc<CodeUiRuntimeHandle>;
```

### Definition Of Done

1. `--web-only` 下非 Codex provider 无 placeholder；所有 provider 通过 `CodeUiProviderAdapter` 统一接入 Web UI。
2. Web thread list 和 live thread summary 均以 canonical `threadId` 为主字段。

---

## src/internal/ai 模块重构建议

基于当前实际模块树（148 个 Rust 文件）和五个 Step 的改造目标，按操作类型列出所有需要变动的模块。

### 删除

| 路径 | 原因 |
|---|---|
| `src/internal/ai/claudecode/` | Step 1：完整删除 claudecode managed runtime，含 `mod.rs`、`audit_objects.rs`、`common.rs`、`extraction.rs`、`managed_artifacts.rs`、`managed_inputs.rs`、`managed_run.rs`、`plan_checkpoint.rs`、`project_settings.rs`、`provider_session.rs`、`snapshot_family.rs`、`helper.cjs`、`helper.py`、`prompts/` |

### 新建

| 路径 | 职责 |
|---|---|
| `src/internal/ai/workflow_runtime/mod.rs` | `WorkflowRuntime` struct；所有 provider 共享的 formal object write API |
| `src/internal/ai/workflow_runtime/phase0.rs` | `write_intent`、`write_context_snapshot_if_needed`、`init_thread`、`resume_thread` |
| `src/internal/ai/workflow_runtime/phase1.rs` | `write_plan`、`write_task`、`update_scheduler_plan_head` |
| `src/internal/ai/workflow_runtime/phase2.rs` | `write_run`、`write_patchset`、`write_provenance`、`append_*` event 系列 |
| `src/internal/ai/workflow_runtime/phase34.rs` | `write_decision`、`write_terminal_intent_event`、`advance_thread_scheduler` |
| `src/internal/ai/workflow_runtime/revision.rs` | `handle_modify_request()`（所有 provider 共享的 Intent revision chain） |
| `tests/helpers/mock_codex.rs` | `MockCodexServer`：测试用 WebSocket mock，替代真实 Codex app-server |
| `tests/command/code_thread_id_test.rs` | Step 2 集成测试 |
| `tests/ai_codex_formal_flow_test.rs` | Step 3 formal writes 集成测试 |
| `tests/ai_generic_formal_flow_test.rs` | Step 4 通用 provider formal writes 集成测试 |
| `tests/ai_isomorphism_test.rs` | DAG 同构 + Rebuild 恢复 E2E 测试 |

### 重构（结构性变化）

**`src/internal/ai/codex/mod.rs`**
- 移除：自主执行主循环、`HISTORY_APPEND_LOCK` 全局 mutex、`McpExecutionTracker` 旁路写入
- 改为：`CodexTaskExecutor`（实现 `TaskExecutor` trait）；只负责 WebSocket 协议、`turn/start` 发送与响应接收
- `provider_thread_id` 从 Codex `threadId` 提取后只写入 `session.metadata`，不暴露到 UI/session 术语

**`src/internal/ai/codex/history.rs`**
- 移除：从 `libra code` 主路径的调用
- 保留：文件本身，顶部标注 `#[deprecated]`，仅供独立离线诊断工具使用
- 同步删除：所有主路径中对 `HistoryRecorder::snapshot()` / `HistoryRecorder::event()` 的调用

**`src/internal/ai/codex/view.rs`**
- 与 `history.rs` 同策略：标注 `#[deprecated]`，`HistoryReader::rebuild_view()` 保留供诊断

**`src/internal/ai/orchestrator/executor.rs`**
- 移除：自有 formal object 写入逻辑
- 改为：`CompletionTaskExecutor<M: CompletionModel>`（实现 `TaskExecutor` trait）
- 接入 `WorkflowRuntime` 完成所有写入

**`src/internal/ai/orchestrator/decider.rs`**
- 新增：`aggregate_risk_score(risk_level, evidences, diff_scope)` 共享函数
- 新增：Phase 4 风险分支逻辑（low risk auto-merge / high risk human review）

**`src/internal/ai/orchestrator/planner.rs`**（或 `verifier.rs`）
- 新增：Phase 3 状态机（全局验证 → 安全审计 → 回退 Phase 2 / 前进 Phase 4）
- 新增：Phase 3 → Phase 2 回退时的 Scheduler 投影更新序列

**`src/internal/ai/session/state.rs`**
- `SessionState.id` 字段重命名为 `thread_id`（类型不变，serde alias 保留兼容旧格式）
- 新增 `legacy_session_id: Option<String>` 字段用于一次性 backfill

**`src/internal/ai/session/store.rs`**
- 所有 keying 改为 canonical `thread_id`；读取旧格式时执行 backfill

**`src/internal/ai/web/code_ui.rs`**
- 新增：`CodeUiProviderAdapter` trait（`submit_message` / `current_snapshot` / `capabilities`）
- 新增：`GenericCodeUiAdapter<M>` 实现，替换现有通用路径 placeholder
- 现有：`CodexCodeUiAdapter` 迁移为实现 `CodeUiProviderAdapter`
- 字段：`CodeUiSessionSnapshot.session_id` → `thread_id`；移除 `provider_session_resume` capability

**`src/internal/tui/app.rs`**
- 移除：`McpExecutionTracker` 写入分支，claudecode managed runtime 分支
- 改为：纯渲染 + 订阅 `broadcast::Receiver<WorkflowEvent>`
- 接入：`show_intentspec_review()` 统一 IntentSpec 审查 UI

### 扩展（新增函数或文件，不改变现有结构）

| 路径 | 新增内容 |
|---|---|
| `src/internal/ai/intentspec/` | 新增 `review.rs`：`show_intentspec_review()` 与 `ReviewChoice` 枚举（两条路径共用） |
| `src/internal/ai/workflow_objects.rs` | 新增 `build_git_run`、`build_git_patchset`、`build_git_evidence`、`build_git_decision` builder 函数 |
| `src/internal/ai/tools/handlers/shell.rs` | 新增 `check_vcs_interception()` 与 `VCS_BLOCKED_CMDS` 常量；在 preflight 阶段拦截 `git`/`jj` 等命令 |
| `src/internal/ai/projection/rebuild.rs` | 新增显式触发条件、降级读路径函数；rebuild 失败时写 `Evidence(kind=ProjectionRebuildFail)` |
| `src/internal/ai/mcp/resource.rs` | 补齐 Workflow Object Tools（`create_intent`/`create_run`/`list_*` 等完整工具集） |
| `src/internal/ai/projection/scheduler.rs` | 已完整，无需修改 |
| `src/internal/ai/projection/thread.rs` | 已完整，无需修改 |
| `src/command/code.rs` | 删除 `CodeProvider::Claudecode` 分支；`run_tui_with_model()` 替换为 `run_unified_provider_session()` |

### 保持不变

下列模块在本批次改造中无需修改：

`agent/`、`commands/`、`completion/`、`hooks/`、`prompt/`、`providers/`、`sandbox/`、`tools/registry.rs`、`tools/spec.rs`、`tools/apply_patch/`、`node_adapter.rs`、`history.rs`（顶层，非 codex 内）

### 目标态模块树（重构后）

```
src/internal/ai/
├── mod.rs
├── client.rs
├── history.rs
├── intent.rs
├── node_adapter.rs
├── util.rs
├── workflow_objects.rs          ← 扩展：增加 Run/PatchSet/Evidence/Decision builders
├── workspace_snapshot.rs
│
├── workflow_runtime/            ← 新建：所有 provider 共享的 formal write 层
│   ├── mod.rs                   #   WorkflowRuntime struct
│   ├── phase0.rs                #   Thread init/resume · Intent · ContextSnapshot
│   ├── phase1.rs                #   Plan · Task · Scheduler update
│   ├── phase2.rs                #   Run · PatchSet · Events
│   ├── phase34.rs               #   Evidence · Decision · terminal events
│   └── revision.rs              #   handle_modify_request() [共享]
│
├── agent/                       ← 不变
├── commands/                    ← 不变
├── completion/                  ← 不变
├── hooks/                       ← 不变
├── prompt/                      ← 不变
├── providers/                   ← 不变
├── sandbox/                     ← 不变
│
├── codex/                       ← 重构
│   ├── mod.rs                   #   CodexTaskExecutor (impl TaskExecutor)
│   ├── protocol.rs              #   WebSocket 协议类型（不变）
│   ├── schema_v2.rs             #   （不变）
│   ├── schema_v2_generated.rs   #   （不变）
│   ├── types.rs                 #   （不变）
│   ├── model.rs                 #   （不变）
│   ├── history.rs               #   [deprecated] 仅供离线诊断
│   └── view.rs                  #   [deprecated] 仅供离线诊断
│
├── intentspec/                  ← 扩展
│   ├── mod.rs
│   ├── review.rs                #   新增：show_intentspec_review() [共享]
│   ├── canonical.rs
│   ├── draft.rs
│   ├── persistence.rs
│   ├── profiles.rs
│   ├── repair.rs
│   ├── resolver.rs
│   ├── scope.rs
│   ├── summary.rs
│   ├── types.rs
│   └── validator.rs
│
├── mcp/                         ← 扩展（补齐 Workflow Object Tools）
│   ├── mod.rs
│   ├── resource.rs
│   ├── server.rs
│   └── tests.rs
│
├── orchestrator/                ← 重构 executor + decider + planner/verifier
│   ├── mod.rs
│   ├── acl.rs
│   ├── checkpoint_policy.rs
│   ├── decider.rs               #   新增 aggregate_risk_score() · Phase 4 决策逻辑
│   ├── executor.rs              #   CompletionTaskExecutor (impl TaskExecutor)
│   ├── gate.rs
│   ├── persistence.rs
│   ├── planner.rs               #   Phase 3 状态机
│   ├── policy.rs
│   ├── replan.rs
│   ├── run_state.rs
│   ├── types.rs
│   ├── verifier.rs
│   └── workspace.rs
│
├── projection/                  ← 扩展 rebuild.rs
│   ├── mod.rs
│   ├── index.rs
│   ├── rebuild.rs               #   新增触发条件 · 降级读路径
│   ├── scheduler.rs             #   已完整，不变
│   └── thread.rs                #   已完整，不变
│
├── session/                     ← 重构 state.rs · store.rs
│   ├── mod.rs
│   ├── state.rs                 #   SessionState.id → thread_id
│   └── store.rs                 #   keying by canonical thread_id
│
├── tools/                       ← 扩展 shell.rs
│   ├── mod.rs
│   ├── context.rs
│   ├── error.rs
│   ├── registry.rs
│   ├── spec.rs
│   ├── utils.rs
│   ├── apply_patch/
│   └── handlers/
│       ├── mod.rs
│       ├── apply_patch.rs
│       ├── grep_files.rs
│       ├── list_dir.rs
│       ├── mcp_bridge.rs
│       ├── plan.rs
│       ├── read_file.rs
│       ├── request_user_input.rs
│       ├── shell.rs             #   新增 check_vcs_interception()
│       └── submit_intent_draft.rs
│
└── web/                         ← 重构：CodeUiProviderAdapter trait
    ├── mod.rs
    └── code_ui.rs               #   CodeUiProviderAdapter · GenericCodeUiAdapter<M>
```

### 关键依赖关系图

```
libra code (TUI / Web / MCP)
  │
  ├─ TUI (tui/app.rs)
  │    └─ 只订阅 broadcast::Receiver<WorkflowEvent>（纯渲染，不写 formal objects）
  │
  └─ Libra Scheduler
       ├─ CodexTaskExecutor (codex/mod.rs)          ← impl TaskExecutor
       └─ CompletionTaskExecutor (orchestrator/executor.rs) ← impl TaskExecutor
            │
            ▼  共同调用
       WorkflowRuntime (workflow_runtime/mod.rs)     ← 唯一 formal write 入口
            │
            ▼
       LibraMcpServer → git-internal Snapshot / Event
            │
            ▼
       Libra Projection (SQLite)
       ThreadProjection · SchedulerState · QueryIndex
       rebuildable from Snapshot + Event
```

## Shared Function Boundary

以下函数**必须共享**（Codex 和通用 provider 不允许各自实现）：

| 功能 | 共享函数 | 位置 |
|---|---|---|
| IntentSpec Markdown review 展示 | `show_intentspec_review()` | `src/internal/ai/intentspec/` |
| Intent revision chain | `handle_modify_request()` | `src/internal/ai/workflow_runtime/revision.rs` |
| Scheduler 投影更新 | `WorkflowRuntime::update_scheduler_plan_head()` 等 | `src/internal/ai/workflow_runtime/` |
| 全部 formal object 写入 | `WorkflowRuntime::write_*/append_*` | `src/internal/ai/workflow_runtime/` |
| Phase 4 风险聚合 | `aggregate_risk_score()` | `src/internal/ai/orchestrator/decider.rs` |
| Phase 4 Decision 写入 | `WorkflowRuntime::write_decision()` | `src/internal/ai/workflow_runtime/` |
| Thread / Scheduler 重建 | `ThreadProjection::rebuild_from_history()` | `src/internal/ai/projection/rebuild.rs` |
| VCS shell 拦截 | `check_vcs_interception()` | `src/internal/ai/tools/handlers/shell.rs` |

以下功能**仅 provider 适配层可实现**（不需要共享）：

| 功能 | 说明 |
|---|---|
| WebSocket 协议与 Codex app-server 通信 | `CodexTaskExecutor` 内部 |
| Completion API 请求与响应解析 | `CompletionTaskExecutor<M>` 内部 |
| `provider_thread_id` 映射（仅 Codex） | `session.metadata["provider_thread_id"]` |
| Codex `collaborationMode` 发送 | `CodexTaskExecutor` 内部 |

---

## Appendix A: Current Prompt Assembly

本节记录当前实现细节，便于实现 Step 3/4 时理解现状。它不是上位契约，真正的上位契约以前文 `Workflow Contract` 和各 Step 为准。

### Entry Point

Codex TUI 运行时在 `run_tui_turn_with_revision()` 中调用 `runtime_handle.adapter().submit_message(prompt)`。实际的 prompt assembly 发生在 `CodexCodeUiAdapter::submit_message()` 与 `submit_thread_message()` 中。

最终发给 Codex app-server 的 `turn/start` 请求：

```json
{
  "input": [{ "type": "text", "text": "<request_text>" }],
  "threadId": "...",
  "approvalPolicy": "...",
  "collaborationMode": { "mode": "plan", ... }
}
```

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

Step 3 / Step 4 目标：统一"执行态 prompt assembly"为单一入口，由 Scheduler 组装 `execute_task_prompt(task, context)` 发给 provider，不再有两个入口。

### First Turn And IntentSpec

按当前实现，首轮不会把 `IntentSpec` 发送给 Codex。首轮路径是：用户原始输入 -> `codex_plan_first_prompt(request)` -> `turn/start` -> Codex 返回 plan text / response text -> Libra 本地 `resolve_intentspec(...)` -> Markdown review。

约束：
1. 首轮不向 Codex 发送 `IntentSpec`。
2. 不把原始 `IntentSpec JSON` 当作首轮 prompt 的一部分注入给 Codex。
3. `IntentSpec` 是 Libra 本地对象，在 Codex 返回后生成并进入 Markdown review。

---

## Appendix B: ContextFrame And Provider Mapping

### ContextFrame Contract

`ContextFrame` 是 Event，不是原始日志容器。它的目标是维护 live context window，而不是复制 provider 的所有原始输出。

#### ContextFrame 类型 Schema

```rust
pub enum ContextFrameKind {
    /// tool/shell 执行事实（摘要，不含完整输出）
    CommandExecution {
        tool_name: String,
        exit_code: Option<i32>,
        cwd: String,
        produced_file_changes: bool,
    },
    /// agent 思考/意图摘要（不含完整 assistant 输出）
    AgentMessage {
        summary: String,
        phase: String,  // "planning" / "execution" / "validation"
    },
    /// 上下文压缩事件
    ContextCompaction {
        frames_compacted: u32,
        tokens_before: u64,
        tokens_after: u64,
    },
}
```

#### commandExecution

1. `commandExecution` 继续作为 `ToolInvocation` 记录执行事实。
2. 同时产出摘要型 `ContextFrame`，包括命令、exit code、cwd、是否产生文件变更。
3. `ContextFrame` 是 live context window 的增量事实，不替代 `ToolInvocation`。

#### agentMessage

1. `agentMessage` 只产出 commentary / intent-analysis 风格的摘要型 `ContextFrame`。
2. 不写入完整原文副本，避免把 provider 输出无界灌入上下文窗口。

#### contextCompaction

`contextCompaction` 也应纳入 `ContextFrame` 体系，作为 Event 记录压缩/收敛行为。

### Provider Thread Mapping

目标态语义：
```rust
session.metadata.insert("provider_thread_id", codex_thread_id);
```

该字段只用于恢复、查询和诊断，不替代 Libra 自身的 `thread_id`。

### TUI Display Goal

TUI 的目标是"线程标识可追踪"：默认在可见位置展示线程标识；窄终端允许截断；宽终端或详情视图可以展示完整值；不破坏现有 cwd / branch badge 的显示。

---

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
7. 通用 provider plan-first 模式说明。

---

## Validation

验证按 Step 1 → Step 5 的顺序组织。测试策略从"验收条目"升级为分层测试矩阵。

### 测试分层

**Layer 1：单元测试（Unit）**

| 测试内容 | 位置 |
|---|---|
| formal object builder 函数（Run、PatchSet、Evidence、Decision） | `workflow_runtime/` tests |
| `check_vcs_interception()` VCS 拦截 | `tools/handlers/shell.rs` tests |
| `aggregate_risk_score()` 风险聚合 | `orchestrator/decider.rs` tests |
| `PlanStep.step_id` 稳定性（replan 复用 step_id） | `workflow_objects.rs` tests |
| legacy `session_id` → `thread_id` backfill | `session/state.rs` tests |
| Thread / Scheduler rebuild 从 Snapshot + Event 重建 | `projection/rebuild.rs` tests |

**Layer 2：集成测试（Integration）**

| 测试文件 | 覆盖内容 |
|---|---|
| `tests/command/code_test.rs`（扩展） | Step 1: claudecode 拒绝、flags 拒绝 |
| `tests/command/code_thread_id_test.rs`（新建） | Step 2: thread_id 统一、legacy backfill、provider_thread_id 仅内部 |
| `tests/ai_codex_formal_flow_test.rs`（新建） | Step 3: Phase 1/2/3/4 formal writes，使用 MockCodexServer |
| `tests/ai_generic_formal_flow_test.rs`（新建） | Step 4: 通用 provider Phase 1/2/3/4，使用 mock CompletionModel |
| `tests/ai_storage_flow_test.rs`（扩展） | WorkflowRuntime 集成，formal write 顺序和最小对象集 |
| `tests/intent_flow_test.rs`（扩展） | Phase 0 Thread/Scheduler bootstrap，ContextSnapshot 条件写入 |

**Layer 3：E2E 测试**

| 测试文件 | 覆盖内容 |
|---|---|
| `tests/ai_isomorphism_test.rs`（新建） | DAG 同构：同一需求分别用 Codex 和通用 provider 执行，断言对象图在深度、引用关系和 Phase 推进上同构 |
| `tests/ai_rebuild_recovery_test.rs`（新建） | Rebuild 恢复：中断后仅凭 thread_id 重建 SchedulerState 并继续执行 |

**Layer 4：失败注入测试（Failure Injection）**

| 测试场景 | 验证内容 |
|---|---|
| Codex WebSocket 断连后重连 | Scheduler 保持 active_run_id，重连后继续执行 |
| Tool 执行失败（非致命） | retry 计数递增，RunEvent 追加，重试后继续 |
| Phase 3 Security Audit 失败 | 正确回退到 Phase 2，投影更新，新 Plan revision 写入 |
| Projection 丢失（DB 清空） | 从 Snapshot + Event 重建 Thread / Scheduler，不阻塞读 |
| Timeout（Codex 无响应 30s） | 从 prompt 合成 IntentSpec，进入 review 流 |

### Mock 基础设施

```rust
// tests/helpers/mock_codex.rs（新建）
pub struct MockCodexServer { addr: SocketAddr, handle: JoinHandle<()> }
impl MockCodexServer {
    pub async fn start(script: Vec<MockCodexTurn>) -> Self;
    pub fn ws_url(&self) -> String;
}
pub struct MockCodexTurn {
    pub plan_text: Option<String>,
    pub patch_diff: Option<String>,
    pub tool_calls: Vec<MockToolCall>,
}
```

### Formal Object Write 覆盖矩阵

| Phase | 对象 | Codex 路径 | 通用路径 | 有测试 |
|---|---|---|---|---|
| Phase 0 | Intent | 需新增 | 部分 | 部分 |
| Phase 0 | ContextSnapshot（条件） | 需新增 | 需新增 | 需新增 |
| Phase 1 | Plan | 需新增 | ✓ | 部分 |
| Phase 1 | Task | 需新增 | ✓ | 部分 |
| Phase 2 | Run | 需新增 | 需新增 | 需新增 |
| Phase 2 | PatchSet | 需新增 | 需新增 | 需新增 |
| Phase 2 | Provenance | 需新增 | 需新增 | 需新增 |
| Phase 2 | TaskEvent | 需新增 | 需新增 | 需新增 |
| Phase 2 | RunEvent | 需新增 | 需新增 | 需新增 |
| Phase 2 | PlanStepEvent | 需新增 | 需新增 | 需新增 |
| Phase 2 | ToolInvocation | 需新增 | 需新增 | 需新增 |
| Phase 2 | Evidence | 需新增 | 需新增 | 需新增 |
| Phase 2 | ContextFrame | 部分 | 需新增 | 需新增 |
| Phase 2 | RunUsage | 需新增 | 需新增 | 需新增 |
| Phase 3 | Evidence（验证） | 需新增 | 需新增 | 需新增 |
| Phase 3 | Decision | 需新增 | 需新增 | 需新增 |
| Phase 4 | Decision（final）+ chosen_patchset_id | 需新增 | 需新增 | 需新增 |
| Phase 4 | IntentEvent（terminal） | 需新增 | 需新增 | 需新增 |

### Step 1 Validation: Claude Code Removal

1. provider 列表中不再包含 `claudecode`。
2. `code` 命令不再接受 `--resume-session`、`--session-id`、`--resume-at`、`--fork-session`。
3. 仓库中不再编译或引用 `src/internal/ai/claudecode/`。
4. `tests/command/code_claudecode_test.rs` 与 `tests/data/ai/claude_managed_*` 已删除。
5. README、命令文档和帮助文本不再把 `claudecode` 作为支持中的 provider。

### Step 2 Validation: Thread ID Unification + Projection Alignment

1. `--resume <THREAD_ID>` 以 Libra canonical `thread_id` 作为唯一恢复入口。
2. `thread_id` 成为 CLI、Web、MCP 共享的唯一用户可见恢复 ID。
3. `provider_thread_id` 只作为可选内部映射字段存在。
4. `pending_plan_revision` 从代码库中消失，替换为 `Scheduler.selected_plan_id` / `current_plan_heads` 操作。
5. 新会话、resume、projection rebuild 下的 `thread_id` 语义一致。
6. 用户可见接口和文档中不再出现泛化 `session_id` 作为主术语。

### Step 3 Validation: Codex Formal Workflow Closure + Inversion of Control

1. `TaskExecutor` trait 存在，Codex 和通用路径都通过它调度。
2. Scheduler 主循环控制 Phase 2 推进，Codex 不再自主控制整个计划执行。
3. TUI 不调用任何 formal object 写入 API；`McpExecutionTracker` 从代码库中消失。
4. `WorkflowRuntime` 存在，Orchestrator 和 Codex adapter 都调用它。
5. provider-specific history 不再驱动主路径的 thread summary / rebuild / runtime reads。
6. `shell` 的 VCS 绕行路径被拦截并提示改用 Libra 工具。
7. `PlanStep.step_id` 在 plan revisions 间保持稳定。

### Step 4 Validation: Generic Provider Closed Loop

1. 通用 provider 与 Codex provider 使用相同的 `show_intentspec_review()` 函数。
2. 通用 provider 的 `Modify` 产生新 Intent revision，链接 `Intent.parents`。
3. 通用 provider 的 Phase 3 Security Audit 路径存在并可回退到 Phase 2。
4. 通用 provider 的 Phase 4 风险聚合和 Decision 写入逻辑与 Codex 路径行为一致。
5. DAG 同构测试通过：两条路径产出的对象图结构、引用关系和 Phase 推进同构。
6. Rebuild 恢复测试通过：中断后仅凭 `thread_id` 重建 SchedulerState 并继续执行。

### Step 5 Validation: Web UI Unification

1. `--web-only` 下通用 provider 无 placeholder。
2. 所有 provider 通过 `CodeUiProviderAdapter` 统一接入 Web UI。
3. Web thread list 和 live thread summary 均以 canonical `threadId` 为主字段。

### Cross-cutting Validation

1. 文档对 Phase 0-4 的描述与 `docs/agent/agent-workflow.md` 一致。
2. 文档对 Snapshot / Event / Projection 的分层与 `docs/agent/ai-object-model-reference.md` 一致。
3. 文档明确区分 revision chain 的 Snapshot 语义和 `Scheduler.selected_plan_id` / `current_plan_heads` 的 Libra runtime 语义。
4. 文档明确区分 canonical `thread_id` 与可选 `provider_thread_id` 的边界。
5. provider 可见 Libra VCS 工具，`shell` 中的 `git` / `jj` / 其他 VCS 命令会被拦截并提示改用 Libra 工具。
6. thread 标识在 UI 中可见且可追踪，窄终端截断策略不破坏 cwd / branch 显示，宽终端或详情视图可展示完整值。
7. Formal Object Write 覆盖矩阵中所有"需新增"项都有对应测试。
8. 失败注入测试全部通过。

---

## Implementation Timeline

| 阶段 | 内容 | 说明 |
|---|---|---|
| Phase A（基础清理） | Step 1（claudecode 删除）+ Step 2（thread_id 统一）+ WorkflowRuntime 骨架 | Step 3/4 的前置依赖 |
| Phase B（Codex 收敛） | Step 3（TaskExecutor + 控制权反转 + formal writes + history 退出） | 建立可验证的 formal write path |
| Phase C（通用方案增强） | Step 4（通用 provider 闭环）+ Step 5（Web UI 统一） | 可并行，Step 5 依赖 Step 4 的 Adapter 定义 |
| Phase D（测试与验证） | 新增测试覆盖 + 同构性测试 + 失败注入测试 + 文档同步 | 贯穿各阶段，最终全覆盖 |

## Risk Assessment

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| Thread ID migration 导致数据丢失 | 低 | 高 | 备份 + 渐进式迁移 + backfill 测试 |
| Codex 控制权反转导致功能退化 | 中 | 高 | 完整测试覆盖 + MockCodexServer |
| 通用 Plan 模式复杂度超预期 | 中 | 中 | 先 Codex 稳定（Step 3）后再实施 Step 4 |
| WorkflowRuntime API 设计不稳定 | 中 | 中 | Phase A 先建立骨架，Step 3/4 验证后再稳定 API |
| Web UI 统一兼容性问题 | 低 | 中 | Step 5 最后实施，不阻塞核心路径 |
| Formal writes 增多导致性能下降 | 低 | 中 | 基准测试 + 批量写入优化 |
