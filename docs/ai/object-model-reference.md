# AI 对象模型 · 整合参考

本文档是当前 Libra Agent 设计的整合参考。

它对 `docs/ai/object-model.md` 与 `docs/ai/workflow.md` 中描述的对象模型进行了汇总。
如有任何歧义，以这两份设计文档为准（source of truth）。

## 核心边界

```text
git-internal: immutable facts
Libra: current state / scheduling state / index projections
```

系统被划分为三层：

- `git-internal` 快照（Snapshot）对象回答："在该 revision 上定义了什么？"
- `git-internal` 事件（Event）对象回答："之后发生了什么？"
- Libra 投影（Projection）回答："当前的运行视图是什么？"

可变的运行时协调不得通过改写快照（Snapshot）对象来实现。

## 分层模型

```text
+--------------------------------------------------------------------------------------+
|                                      Libra [L]                                       |
|--------------------------------------------------------------------------------------|
| Thread / Scheduler / UI / Query Index                                                |
|                                                                                      |
|  current_intent_id                                                                   |
|  latest_intent_id                                                                    |
|  selected_plan_ids[]                                                                 |
|  current_plan_heads[]                                                                |
|  active_task_id / active_run_id                                                      |
|  live_context_window                                                                 |
|  reverse indexes: intent->plans, task->runs, run->events, run->patchsets, ...       |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                               git-internal : Event [E]                               |
|--------------------------------------------------------------------------------------|
|  IntentEvent / TaskEvent / RunEvent / PlanStepEvent / RunUsage                       |
|  ToolInvocation / Evidence / Decision / ContextFrame                                 |
|                                                                                      |
|  Rule: append-only execution facts and audit records                                 |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                              git-internal : Snapshot [S]                             |
|--------------------------------------------------------------------------------------|
|  Intent / Plan / Task / Run / PatchSet / ContextSnapshot / Provenance                |
|                                                                                      |
|  Rule: immutable definitions and revisioned structure                                |
+--------------------------------------------------------------------------------------+
```

## 归属规则

### `git-internal` 中的 Snapshot 对象

- `Intent`
- `Plan`
- `Task`
- `Run`
- `PatchSet`
- `ContextSnapshot`
- `Provenance`

### `git-internal` 中的 Event 对象

- `IntentEvent`
- `TaskEvent`
- `RunEvent`
- `PlanStepEvent`
- `RunUsage`
- `ToolInvocation`
- `Evidence`
- `Decision`
- `ContextFrame`

### Libra 中的投影与运行时状态

- `Thread`
- `Scheduler`
- 面向 UI 的当前视图
- 查询索引与反向索引
- live context window
- ready queue / parallel groups / checkpoints / retry routing

## 主关系图

```text
Snapshot layer
==============

Intent[S] --parents------------------------> Intent[S]
Intent[S] --analysis_context_frames-------> ContextFrame[E]
Plan[S]   --intent_id----------------------> Intent[S]
Plan[S]   --parents------------------------> Plan[S]
Plan[S]   --context_frames-----------------> ContextFrame[E]
Task[S]   --intent_id?---------------------> Intent[S]
Task[S]   --parent_task_id?----------------> Task[S]
Task[S]   --origin_step_id?---------------> Plan[S].step_id
Task[S]   --dependencies-------------------> Task[S]
Run[S]    --task_id------------------------> Task[S]
Run[S]    --plan_id?-----------------------> Plan[S]
Run[S]    --context_snapshot_id?-----------> ContextSnapshot[S]
PatchSet[S]   --run_id---------------------> Run[S]
Provenance[S] --run_id---------------------> Run[S]

Event layer
===========

IntentEvent[E]   --intent_id---------------> Intent[S]
IntentEvent[E]   --next_intent_id?---------> Intent[S]
TaskEvent[E]     --task_id-----------------> Task[S]
RunEvent[E]      --run_id------------------> Run[S]
RunUsage[E]      --run_id------------------> Run[S]
PlanStepEvent[E] --plan_id-----------------> Plan[S]
PlanStepEvent[E] --step_id-----------------> Plan[S].step_id
PlanStepEvent[E] --run_id?-----------------> Run[S]
ToolInvocation[E] --run_id-----------------> Run[S]
Evidence[E]       --run_id-----------------> Run[S]
Evidence[E]       --patchset_id?----------> PatchSet[S]
Decision[E]       --run_id-----------------> Run[S]
Decision[E]       --chosen_patchset_id?---> PatchSet[S]
ContextFrame[E]   --intent_id?-------------> Intent[S]
ContextFrame[E]   --run_id?----------------> Run[S]
ContextFrame[E]   --plan_id?---------------> Plan[S]
ContextFrame[E]   --step_id?---------------> Plan[S].step_id

Libra layer
===========

Thread[L] --------current_intent_id-------> Intent[S]
Thread[L] --------latest_intent_id--------> Intent[S]
Thread[L] --------intents[].intent_id-----> Intent[S]
Thread[L] --------intents[].is_head-------> marks current branch heads

Scheduler[L] -----selected_plan_ids[]-----> Plan[S]
Scheduler[L] -----current_plan_heads------> Plan[S]
Scheduler[L] -----active_task_id----------> Task[S]
Scheduler[L] -----active_run_id-----------> Run[S]
Scheduler[L] -----live_context_window-----> ContextFrame[E]
```

## Libra 运行时术语

### Thread

`Thread` 是覆盖一组相关 `Intent` DAG 的会话级投影（Projection）根。

它拥有当前的会话视图，而非不可变（immutable）历史。

当前设计字段：

| Field | Type | Meaning |
|---|---|---|
| `thread_id` | `Uuid` | Libra 侧主键 |
| `title` | `Option<String>` | 人类可读标题 |
| `owner` | `ActorRef` | 会话创建者 |
| `participants` | `Vec<ThreadParticipant>` | 携带 thread-local 元数据的人类与 agent 参与者 |
| `current_intent_id` | `Option<Uuid>` | 当前被 UI / Scheduler 聚焦的 Intent |
| `latest_intent_id` | `Option<Uuid>` | 最近链接的 Intent；resume 回退项 |
| `intents` | `Vec<ThreadIntentRef>` | 带 `ordinal`、`is_head`、`linked_at`、`link_reason` 的有序成员关系 |
| `metadata` | `Option<serde_json::Value>` | UI 与路由提示 |
| `archived` | `bool` | 已关闭 thread 标记 |

说明：

- `participants` 不只是 `Vec<ActorRef>`；它携带 thread-local
  的角色与加入时间元数据。
- `head_intent_ids` 由 `ThreadIntentRef.is_head` 表示，而非以一个重复的
  独立数组表示。
- `current_intent_id` 是当前焦点。
- `latest_intent_id` 是最近链接的 revision，也是在未设置当前焦点时的默认
  resume 回退项。

### Scheduler

`Scheduler` 是运行时调度投影。

它回答：现在应该运行什么、什么处于活动状态，以及当前选定的执行 /
测试计划对（pair）是哪一组。

当前设计字段：

| Field | Type | Meaning |
|---|---|---|
| `selected_plan_ids` | `Vec<Uuid>` | 以稳定顺序排列的当前规范 plan heads：`[execution_plan_id, test_plan_id]` |
| `current_plan_heads` | `Vec<Uuid>` | 活动 plan leaves |
| `active_task_id` | `Option<Uuid>` | 当前被 Scheduler / UI 强调的 Task |
| `active_run_id` | `Option<Uuid>` | 实时 run 尝试 |
| `live_context_window` | `Vec<Uuid>` | 当前可见的 `ContextFrame` id |

说明：

- `selected_plan_ids` 是一个逻辑上固定的对（pair），而非开放式列表。
  Scheduler 必须按该顺序恰好维护一个 `execution` plan id 与一个 `test`
  plan id。
- Phase 2 使用保守的阶段屏障（stage barrier）：先运行 `execution_dag`，
  仅当执行工作完成后，才将活动阶段切换为 `test`。
- 当前 Code-mode 持久化会显式写入该对：非门禁（gate）工作映射到 execution
  plan，门禁（gate）工作映射到 test plan，并且 Scheduler 重建会保持该对
  处于 `[execution, test]` 顺序。

Scheduler 还可派生或缓存：

- ready queue
- `active_dag_stage`（`execution` 或 `test`）
- 当前阶段 DAG 进度
- parallel groups
- checkpoints
- retry routing
- staging / integration 状态
- 重规划（replanning）决策

### Query Index

`Query Index` 是一个可重建的反规范化查找层，用于快速读取。

典型索引：

- `intent -> plans`
- `intent -> context_frames`
- `task -> runs`
- `run -> events`
- `run -> patchsets`

索引不是历史真相（historical truth），且必须能安全地重建。

## 对象说明

### Intent

用户请求及已分析规范的不可变（immutable）快照（Snapshot）。

- 保留 `parents`、`prompt`、`spec`、`analysis_context_frames`
- 不保留可变的生命周期、选定 plan 指针，或最终执行结果
- 生命周期归属于 `IntentEvent`

### Plan

策略与步骤结构的不可变（immutable）快照（Snapshot）。

- 保留 `intent`、`parents`、`steps`、`context_frames`
- `PlanStep.step_id` 是跨 plan revision 的稳定逻辑步骤标识
- 运行时步骤进度归属于 `PlanStepEvent`
- 面向 provider 的草稿输出不是 `Plan`；仅在本地 planner 接受后，它才被
  规范化为不可变的 `Plan.steps`
- `PlanStep` 保留在 `Plan` 内部；它不是顶层快照（Snapshot）对象

`git-internal` 中没有可变的 `ExecutionPlan` 对象。

### Task

稳定的工作单元定义。

- 保留不可变的溯源（provenance）链接：
  `intent`、`parent`、`origin_step_id`、`dependencies`
- 运行时状态、retries 与活动 run 归属于事件（Event）或 Libra
  投影（Projection）
- `Task.origin_step_id` 指向生成该工作单元快照的、已持久化的
  `PlanStep.step_id`

### Run

不可变的执行尝试封套（envelope）。

- 保留 `task`、可选 `plan`、`commit`、可选 `snapshot`、
  `environment`
- 状态转换与失败详情归属于 `RunEvent`
- usage 与成本归属于 `RunUsage`

### PatchSet

不可变的候选 diff 快照（Snapshot）。

- 保留 `run`、`sequence`、`commit`、`format`、`artifact`、`touched`、
  `rationale`
- 验收（acceptance）、拒绝与最终选择归属于 `Decision` 或
  Libra 投影（Projection）

### Provenance

针对单次 run 的不可变模型 / provider / 执行参数记录。

- 保留 provider、model 与执行参数
- 用量核算由 `RunUsage` 单独跟踪

### ContextSnapshot

可选的稳定环境基线。

- 当系统需要冻结的起始或结束上下文时使用
- 并非每个 phase 都需要
- 不应被当作可变的运行时上下文容器使用
- CLI 与 MCP 读取方以 `context_snapshot` 作为公开类型名；
  `git-internal` 写入的历史遗留数据可能仍存储在内部的 `snapshot`
  目录名下。

### ContextFrame

不可变的增量上下文事实。

- 取代旧的可变 `ContextPipeline` 运行时概念
- 可附加到 intent 分析、planning、execution 或 step 级别的
  上下文
- Phase 0 / Phase 1 中的只读 provider 分析也应发出
  `ContextFrame`
- Libra 仅保留当前的 `live_context_window`

## 工作流映射

| Phase | Libra runtime / projection | Snapshot writes (`git-internal`) | Event writes (`git-internal`) |
|---|---|---|---|
| Phase 0 | Thread 引导、当前 intent revision、IntentSpec 评审、live context 引导 | `Intent`、可选 `ContextSnapshot` | `ToolInvocation`、`ContextFrame`、可选终态 `Decision` / `IntentEvent` |
| Phase 1 | 选定 plan 集合 heads、当前 plan heads、plan 评审、ready queue 预览 | `Plan`、`Task` | `ToolInvocation`、`ContextFrame`、可选终态 `Decision` / `IntentEvent` |
| Phase 2 | live context window、retry / replan 循环、staging area | `Run`、`PatchSet`、`Provenance` | `TaskEvent`、`RunEvent`、`PlanStepEvent`、`ToolInvocation`、`Evidence`、`ContextFrame`、`RunUsage` |
| Phase 3 | 审计索引、发布候选视图 | 可选最终 `ContextSnapshot` | `Evidence`、`Decision`、终态 `TaskEvent` / `RunEvent` / `IntentEvent` |
| Phase 4 | 评审 UI、当前 thread 指针 | 无 | `Decision`、可选终态 `IntentEvent` |

Phase 0 的 `IntentSpec` 评审仅暴露三个正式动作：
`Confirm`、`Modify` 与 `Cancel`。

对象模型中没有单独的 `Regenerate` 工作流状态。
如果 UI 提供了"重试" / regenerate 的交互入口，Libra 应将其建模为
`Modify` 并复用同一条 revision 路径，而不引入一个独立的
`Snapshot` / `Event` / `Projection` 转换。

Phase 3 是针对发布候选的固定 validator 流水线。它
不会物化或执行由 planner 定义的 DAG。

## 重建与读取契约

投影（Projection）丢失不得阻断读取访问。

规则：

1. `Thread`、`Scheduler` 与 `Query Index` 可从
   不可变的快照（Snapshot）与事件（Event）重建。
2. 缺失的投影行意味着"投影缺失或陈旧"，而不
   一定意味着"该逻辑对象不存在"。
3. 读取路径应优先使用 Libra 投影（Projection），随后在投影数据缺失时
   回退到重建或历史遍历。

示例：

- `Thread` 可从 `Intent`、`Intent.parents` 与
  `IntentEvent.next_intent_id` 的边重建。
- `Scheduler` 可从 `Plan`、`Task`、`Run`、
  `PlanStepEvent` 及相关执行事件重建。
- 查询索引可通过扫描快照（Snapshot）与事件（Event）
  历史重新生成。

## 已弃用或移除的概念

当前设计有意移除了若干较旧的模式：

- 没有可变的 `ContextPipeline`；改用 `live_context_window +
  ContextFrame`
- `Intent`、`Task` 或 `Run` 上没有可变的对象内（in-object）生命周期字段
- `git-internal` 中没有可变的 `ExecutionPlan` 对象
- 不会向 `PatchSet` 回写 "accepted" 字段

## 总结规则

```text
1. Snapshot stores "what it is"
2. Event stores "what happened"
3. Libra stores "what is current"
```
