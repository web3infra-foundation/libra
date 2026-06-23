# AI 对象模型设计（权威来源）

本文档描述了在快照（Snapshot）/ 事件（Event）/ Libra 拆分之后，`git-internal` 中的 AI 对象模型。

## 设计原则

`git-internal` 存储不可变的历史事实。

- **Snapshot objects** 回答：“在这一修订版本存储了什么？”
- **Event objects** 回答：“之后发生了什么？”
- **Libra projections** 回答：“系统当前的视图是什么？”

高频的运行时状态不得通过在 `git-internal` 中改写父对象的方式累积。

## 三层 ASCII 图示

```text
+--------------------------------------------------------------------------------------+
|                                      Libra [L]                                       |
|--------------------------------------------------------------------------------------|
| Thread / Scheduler / UI / Query Index                                                |
|                                                                                      |
|  current_intent_id                                                                   |
|  selected_plan_id                                                                    |
|  current_plan_heads[]                                                                |
|  active_task_id / active_run_id                                                      |
|  task_latest_run_id                                                                  |
|  run_latest_patchset_id                                                              |
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
|  Rule: every event is append-only; no parent object is rewritten to append history   |
+--------------------------------------------+-----------------------------------------+
                                             |
                                             v
+--------------------------------------------------------------------------------------+
|                              git-internal : Snapshot [S]                             |
|--------------------------------------------------------------------------------------|
|  Intent / Plan / Task / Run / PatchSet / ContextSnapshot / Provenance                |
|                                                                                      |
|  Rule: a snapshot only answers "what it is at this revision"                        |
+--------------------------------------------------------------------------------------+
```

## Libra 层术语

Libra 层不属于 `git-internal` 的对象存储。它持有由不可变快照与事件重建而来的当前运行视图。

### Thread

针对相关 `Intent` 快照的会话级投影（Projection）。

- 将某个进行中的讨论或任务流的 `Intent` DAG 聚合在一起
- 存储当前的恢复目标、分支头（branch heads）以及线程局部（thread-local）元数据
- 始终可以从不可变历史加上 Libra 侧的投影记录重建

### Scheduler

将不可变历史转化为可执行工作的运行时调度器（Scheduler）。

- 选定活跃的 `Plan` 头并计算当前就绪的工作
- 跟踪活跃的 `Task` / `Run`、重试路由以及重新规划（replanning）决策
- 在不改写快照对象的前提下管理实时执行顺序

### UI

面向用户、覆盖在当前系统视图之上的呈现层。

- 展示活跃的 thread、所选的 plan、task / run 进度以及审计证据（evidence）
- 从 Libra 投影与不可变历史中读取数据
- 不定义历史真相；它只渲染当前视图

### Query Index

可重建的查找结构与反规范化（denormalized）访问结构，用于快速查询。

- 例如：`intent -> plans`、`intent -> analysis_context_frames`、
  `task -> runs`、`run -> events`、`run -> patchsets`
- 针对检索、过滤与仪表盘查询进行优化
- 不属于不可变对象图，必要时可以重新计算

## 主要对象关系

```text
Snapshot layer
==============

Intent[S] --parents------------------------> Intent[S]
Intent[S] --analysis_context_frames-------> ContextFrame[E]
Plan[S]   --intent_id----------------------> Intent[S]
Plan[S]   --context_frames-----------------> ContextFrame[E]
Plan[S]   --parents------------------------> Plan[S]
Task[S]   --intent_id?---------------------> Intent[S]
Task[S]   --parent_task_id?----------------> Task[S]
Task[S]   --origin_step_id?---------------> Plan[S].step_id
Run[S]    --task_id------------------------> Task[S]
Run[S]    --plan_id?-----------------------> Plan[S]
Run[S]    --context_snapshot_id?-----------> ContextSnapshot[S]
PatchSet[S]   --run_id---------------------> Run[S]
Provenance[S] --run_id---------------------> Run[S]

Event layer
===========

IntentEvent[E]   --intent_id---------------> Intent[S]
IntentEvent[E]   --next_intent_id?---------> Intent[S]
ContextFrame[E]  --intent_id?--------------> Intent[S]
TaskEvent[E]     --task_id-----------------> Task[S]
RunEvent[E]      --run_id------------------> Run[S]
RunUsage[E]      --run_id------------------> Run[S]
PlanStepEvent[E] --plan_id-----------------> Plan[S]
PlanStepEvent[E] --step_id-----------------> Plan[S].step_id
PlanStepEvent[E] --run_id------------------> Run[S]
ToolInvocation[E] --run_id-----------------> Run[S]
Evidence[E]       --run_id-----------------> Run[S]
Evidence[E]       --patchset_id?----------> PatchSet[S]
Decision[E]       --run_id-----------------> Run[S]
Decision[E]       --chosen_patchset_id?---> PatchSet[S]
ContextFrame[E]   --run_id? / plan_id? / step_id? --> Run[S] / Plan[S] / Plan[S].step_id

Libra layer
===========

Thread[L] --------current_intent_id-------> Intent[S]
Thread[L] --------latest_intent_id--------> Intent[S]
Thread[L] --------intents[].intent_id-----> Intent[S]
Thread[L] --------intents[].is_head-------> marks current branch heads

Scheduler[L] -----selected_plan_id--------> Plan[S]
Scheduler[L] -----current_plan_heads------> Plan[S]
Scheduler[L] -----active_task_id----------> Task[S]
Scheduler[L] -----active_run_id-----------> Run[S]
Scheduler[L] -----live_context_window-----> ContextFrame[E]

QueryIndex[L] ----task_latest_run_id------> Run[S]
QueryIndex[L] ----run_latest_patchset_id--> PatchSet[S]
QueryIndex[L] ----reverse indexes---------> all [S] / [E]
```

## 放置规则

### `git-internal` 中的快照（Snapshot）对象

- `Intent`
- `Plan`
- `Task`
- `Run`
- `PatchSet`
- `ContextSnapshot`
- `Provenance`

### `git-internal` 中的事件（Event）对象

- `IntentEvent`
- `TaskEvent`
- `RunEvent`
- `PlanStepEvent`
- `RunUsage`
- `ToolInvocation`
- `Evidence`
- `Decision`
- `ContextFrame`

`IntentEvent.next_intent_id` 是一条用于表示“在当前 Intent 完成之后接下来应该处理哪个 Intent”的推荐边（recommendation edge）。它并不取代 `Intent.parents`，后者仍然是语义上的修订血缘（revision lineage）。

### Libra 中的运行时 / 投影（Projection）状态

- 当前选定的 plan 头
- 活跃的 task / 活跃的 run
- thread 头 / 最新的 intent
- 实时上下文窗口（live context window）
- 反向索引与查询加速结构

## 对象说明

### Intent

用户请求以及可选的已分析 spec 的快照。

- 保留：`parents`、`prompt`、`spec`、`analysis_context_frames`
- 不在快照中保留：可变的状态日志、所选 plan 指针、最终 commit 指针
- 生命周期属于 `IntentEvent`
- `analysis_context_frames` 冻结了用于推导该 `IntentSpec` 修订版本的上下文

### Plan

策略与步骤结构的快照。

- 保留：`intent`、`parents`、`context_frames`、`steps`
- `context_frames` 是从 `IntentSpec` 推导出该 plan 时所用的规划期上下文，而非 prompt 分析上下文
- `PlanStep.step_id` 是跨 Plan 修订版本保持稳定的逻辑步骤标识
- 执行期的步骤状态属于 `PlanStepEvent`

### Task

稳定的工作定义。

- 保留：title、description、goal、约束（constraints）、验收标准（acceptance criteria）、requester
- 保留规范的溯源（provenance）链接：`intent`、`parent`、`origin_step_id`、`dependencies`
- 运行时进度属于 `TaskEvent`

### Run

执行尝试的封装体。

- 保留：`task`、`plan`、`commit`、`snapshot`、`environment`
- 阶段变更、失败详情以及指标属于 `RunEvent`
- 用量 / 成本属于 `RunUsage`

### PatchSet

候选 diff 快照。

- 保留：`run`、`sequence`、`commit`、`format`、`artifact`、`touched`、`rationale`
- 验收（acceptance）/ 拒绝属于 `Decision` 或 Libra 投影

### Provenance

某一次 run 的不可变模型 / provider 配置。

- 保留：provider / model / parameters / temperature / max_tokens
- 用量属于 `RunUsage`

### ContextFrame

不可变的增量上下文记录。

- 替代了旧的可变 `ContextPipeline` 运行时容器
- 被 `Intent.analysis_context_frames`、
  `Plan.context_frames` 以及 `PlanStepEvent.consumed_frames` /
  `produced_frames` 引用
- `intent_id` 可以将某个 frame 直接挂接到 intent 分析阶段

## 总结规则

```text
1. Snapshot stores "what it is"
2. Event stores "what happened"
3. Libra stores "what is current"
```
