# Agent 设计总览（中文）

本文件是 `docs/agent/agent.md` 与 `docs/agent/agent-workflow.md` 的中文整合说明，
用于帮助开发者快速理解 Libra Agent 的对象模型、运行流程和系统边界。

如果本文件与英文设计文档存在歧义，以原始英文文档为准。

## 1. 设计目标

一轮 Agent 交互在 Libra 中以一个 `Thread` 为组织单位。

可以把 `Thread` 理解为这次交互的会话根：

- 用户和 Agent 围绕同一个目标持续来回交互时，整体都归属于同一个 `Thread`
- 这次交互里产生的多个 `Intent` 修订、计划分支、执行尝试，都会被组织到这个 `Thread` 下
- `Thread` 负责承载“当前这轮交互正在看什么、做到哪里、下一步回到哪里”

`Thread` 不是 `git-internal` 里的不可变历史对象，而是 Libra 中维护的当前会话视图。
也正因为如此，它适合承载“当前态”，而不负责承载“历史真相”。

在别的系统里，和 `Thread` 最接近的概念有时会被叫做 `Session`。
如果从中文语义理解，可以先把它们近似看成一类东西：
都是“一轮持续交互的承载单元”。
本文统一使用 `Thread`，是为了强调它不只是聊天会话，还要承载
与这轮交互相关的 `Intent`、计划分支和执行状态。

围绕这次交互，系统会产生三类本质不同的信息：

- **Snapshot** 是**交互过程中产出的不可变结果** — 某一步得到的结构化结果一旦写入就不再修改
- **Event** 是**交互过程中的变化记录** — 多个 Event 串起来形成变化的历史
- **View** 是**可重建的运行时状态** — 系统现在认为状态是什么

这套设计把这三类信息分别放进三个层次，由 `git-internal` 承载前两层
（Snapshot + Event），由 Libra 承载第三层（View / Runtime）。

这么拆的原因是：

- **不可变性保证审计**：Snapshot 和 Event 一旦写入就不再修改，
  任何历史节点都可以被精确回溯和重放，天然满足审计和合规要求。
- **追加语义避免冲突**：Event 只追加、不回写父对象，
  避免了并发执行时围绕同一份可变状态做覆盖式更新，也避免了对象膨胀。
- **View 可丢失、可重建**：View 是从 Snapshot + Event 推导出来的
  当前态缓存，丢失后可以重建，不需要也不应该成为历史真相的来源。
  这让运行时调度、UI 渲染、索引加速等高频读写路径可以自由演进，
  而不必担心破坏历史完整性。

一句话总结：

```text
git-internal：存不可变事实（Snapshot + Event）
Libra：存当前态 / 调度态 / 索引视图
```

## 2. 三层模型

### 2.1 Snapshot 层

Snapshot 是**交互过程中产出的不可变结果** — 某一步得到的对象结果一旦写入就不再修改。

对象：

- `Intent`
- `Plan`
- `Task`
- `Run`
- `PatchSet`
- `ContextSnapshot`
- `Provenance`

约束：

- Snapshot 保存的是交互过程中某一步产出的结果，不是可反复覆盖的当前状态
- 旧 Snapshot 不被修改，只能通过新版本表达变化
- 父子关系、依赖关系、来源关系都属于不可变结构

### 2.2 Event 层

Event 是**交互过程中的变化记录** — 多个 Event 串起来形成变化的历史。

对象：

- `IntentEvent`
- `TaskEvent`
- `RunEvent`
- `PlanStepEvent`
- `RunUsage`
- `ToolInvocation`
- `Evidence`
- `Decision`
- `ContextFrame`

约束：

- Event 只能追加，不能回写父对象
- 生命周期、状态变化、验证记录、工具调用都落在 Event 层
- 单个 Event 表示一次变化，多个 Event 连接起来才能还原完整变化历史
- Event 是审计和重建当前态的重要依据

### 2.3 Libra View / Runtime 层

View 是**可重建的运行时状态** — 系统现在认为状态是什么。

对象：

- `Thread`
- `Scheduler`

辅助结构：

- `Query Index`

消费/展示层：

- `UI`

相关当前态与索引字段：

- `task_latest_run_id` / `run_latest_patchset_id` 这类快速跳转字段
- live context window
- ready queue / active task / active run
- reverse indexes

约束：

- View 不属于 `git-internal` 对象存储
- View 可以丢失，也必须能够重建
- View 是运行效率和用户体验层，不是历史真相来源

## 3. 关键边界

### 3.1 什么必须放进 `git-internal`

凡是需要长期保留、能形成审计链、能表达不可变历史语义的内容，应进入 `git-internal`：

- 用户请求和分析结果的版本化定义
- 计划结构和任务结构
- 每次执行尝试及其输入环境
- 候选补丁、证据、决策和工具调用记录

### 3.2 什么必须留在 Libra

凡是高频变化、只表达当前运行视图、可由历史事实重建的内容，应留在 Libra：

- 当前选中的 `Intent` / `Plan`
- 当前 branch heads
- ready queue
- active task / active run
- live context window
- 检索加速用的索引和 denormalized 关系

### 3.3 一个常见反模式

错误做法是把运行中的 mutable state 直接塞回 Snapshot，例如：

- 在 `Intent` 上维护当前状态日志
- 在 `Plan` 上维护执行进度
- 在 `PatchSet` 上直接回写“已接受”
- 在 `Run` 上不断覆盖阶段状态

这些都应该拆到 Event 或 Libra View 中。

## 4. 核心对象语义

### 4.1 Intent

`Intent` 是用户请求或分析后规格的不可变快照。

它负责表达：

- 用户目标
- 约束
- 质量要求
- 风险等级相关输入
- `parents` 形成 revision lineage
- `analysis_context_frames` 冻结本次分析所依赖的上下文

它不负责表达：

- mutable 状态推进
- 当前选中的 Plan
- 最终执行结果

### 4.2 Plan

`Plan` 是策略和步骤结构的不可变快照。

它负责表达：

- 和哪个 `Intent` 对应
- 计划版本之间的 `parents`
- planning-time context
- immutable steps 结构

它不负责表达：

- 当前是否执行中
- 某步是否已完成
- 当前选中的 plan head

这些属于 `PlanStepEvent` 和 Libra Scheduler。

### 4.3 Task

`Task` 是稳定的工作单元定义。

它负责表达：

- title / description / constraints / acceptance criteria
- `intent`
- `parent_task_id`
- `origin_step_id`
- `dependencies`

它不负责表达：

- 当前执行状态
- 重试次数
- 当前 run

这些属于 `TaskEvent` 和 Scheduler。

### 4.4 Run

`Run` 是一次执行尝试的不可变包络。

它负责表达：

- 属于哪个 `Task`
- 可选关联哪个 `Plan`
- 基于哪个 commit / snapshot / environment

它不负责表达：

- 执行阶段推进
- 失败细节
- usage / cost

这些分别属于 `RunEvent` 和 `RunUsage`。

### 4.5 PatchSet

`PatchSet` 是某次执行产生的候选补丁快照。

它负责表达：

- 来自哪个 `Run`
- 第几份候选 diff
- 关联 commit / artifact / touched files / rationale

它不负责表达：

- 是否被接受
- 是否最终发布

这些属于 `Decision` 或 Libra View。

### 4.6 Provenance

`Provenance` 是一次 `Run` 的不可变执行来源记录。

它负责表达：

- provider / model / execution parameters
- 一次运行使用了什么模型配置
- 后续审计和供应链证明所需的执行背景

它不负责表达：

- usage / token / cost 的累计结果
- 当前执行阶段

这些分别属于 `RunUsage` 和 `RunEvent`。

### 4.7 ContextSnapshot

`ContextSnapshot` 是可选的稳定上下文基线快照。

它负责表达：

- 某个阶段需要冻结保存的上下文基线
- Run 启动前或 release candidate 阶段的稳定视图

它不负责表达：

- 高频变化的运行时上下文窗口
- 增量上下文追加历史

这些分别属于 Libra 的 live context window 和 `ContextFrame`。

### 4.8 ContextFrame

`ContextFrame` 是增量上下文记录。

它替代旧的 mutable `ContextPipeline` 概念，用来承载：

- intent 分析阶段的上下文
- plan 推导阶段的上下文
- execution 过程产生的新上下文

Libra 维护的是当前 live context window；
`ContextFrame` 本身是不可变事实。

## 5. Libra View 与运行时状态

### 5.1 Thread

`Thread` 是围绕一组相关 `Intent` 形成的会话级根对象。

它关注的是“当前对话视图”，而不是历史真相。

典型字段包括：

- `thread_id`
- `title`
- `owner`
- `participants`
- `current_intent_id`
- `latest_intent_id`
- `intents`
- `metadata`
- `archived`

语义上：

- `current_intent_id` 是当前 UI / Scheduler 聚焦的 intent
- `latest_intent_id` 是最近挂入 thread 的 intent，可作为 resume fallback
- `intents[].is_head` 标记当前 intent DAG 的 branch heads

### 5.2 Scheduler

`Scheduler` 是执行编排器的当前态对象。

它关注的是“当前应该做什么”和“现在跑到哪里了”。

在当前术语约定里，旧文档中的 `Orchestrator / Planner / Scheduler`
运行角色统一记为 `Scheduler`。
因此在 `Phase 1` 中，`Scheduler` 既负责驱动 `Plan` 生成，
也负责把 `Plan + Task` 推导成当前调度视图。

典型字段包括：

- `selected_plan_id`
- `current_plan_heads`
- `active_task_id`
- `active_run_id`
- `live_context_window`

运行时还会进一步衍生：

- ready queue
- parallel groups
- checkpoints
- retry routing
- replan decisions
- staging / integration state

### 5.3 Query Index

为了让 UI、MCP、调度和查询更高效，Libra 还维护可重建索引，例如：

- `intent -> plans`
- `intent -> context_frames`
- `task -> runs`
- `task -> latest_run` / `task_latest_run_id`
- `run -> events`
- `run -> patchsets`
- `run -> latest_patchset` / `run_latest_patchset_id`

这些索引不是历史真相的一部分，只是重建后的读取加速层。

## 6. 五阶段运行流程

### 6.1 Phase 0：输入预处理

对应英文：[agent-workflow.md Phase 0](./agent-workflow.md#phase-0-input-preprocessing)。

系统从用户输入开始：

1. 提取目标、约束、质量要求
2. 形成 `IntentSpec`
3. 写入初始 `Intent` Snapshot
4. 在需要时写入初始 `ContextSnapshot`
5. 初始化 Libra Thread / Scheduler / live context window / reverse indexes

这一阶段的重点是：

- 把请求固化为不可变事实
- 建立最初的运行时视图

如果把 `Intent` 的生成过程展开看，通常会经过下面几个步骤：

- 先把原始用户请求保存成第一个 `Intent`
- 分析过程中追加 `ContextFrame`，保存本次理解请求时使用到的上下文
- 如果分析结果比原始请求更结构化，就创建新的 `Intent` revision，
  并通过 `parents` 把它和前一个 `Intent` 连起来
- 当某个 `Intent` 进入 analyzed / completed / cancelled 等状态时，
  通过 `IntentEvent` 记录变化
- 当要进入 `Phase 1` 时，`Thread` 会从这一组相关 `Intent` 中选出
  一个 `current_intent_id` 作为当前要继续推进的输入；
  `latest_intent_id` 则指向最近挂入的那个 `Intent`

可以把这个过程理解成：

```text
User Query
   |
   v
Intent[S] #1 (raw request)
   |
   +--> ContextFrame[E] #1 (analysis context for #1 -> #2)
   |
   +--> Intent[S] #2 (analyzed revision, parents -> #1)
   |        |
   |        +--> analysis_context_frames -----> ContextFrame[E] #1
   |        |
   |        +--> IntentEvent[E] #1 (analyzed / completed / cancelled for #2)
   |        |
   |        +--> IntentEvent[E] #2 (next_intent_id -> another Intent[S], optional)
   |
   +--> Thread[L].latest_intent_id
   |
   +--> Thread[L].current_intent_id  ----> 选中的 Intent[S] #1 或 #2，进入 Phase 1
```

上图说明：

- `Intent[S] #1` 表示最早保存下来的原始请求版本。
- `ContextFrame[E] #1` 表示从原始请求分析出结构化版本时使用到的上下文记录。
- `Intent[S] #2` 表示分析后的新版本，它不是对 `#1` 的覆盖，而是新的 snapshot。
- `analysis_context_frames -> ContextFrame[E] #1` 表示 `Intent[S] #2` 把分析时使用过的上下文冻结引用下来。
- `IntentEvent[E] #1` 表示围绕 `Intent[S] #2` 发生的生命周期变化，例如 analyzed 或 completed。
- `IntentEvent[E] #2` 表示从当前 `Intent` 指向下一个推荐 `Intent` 的建议边。
- `Thread.latest_intent_id` 表示最近挂入 Thread 的那个 `Intent`。
- `Thread.current_intent_id` 表示进入下一阶段时当前真正被选中的那个 `Intent`。

这一阶段的对象关系：

```text
Thread[L] ----------------current_intent_id / latest_intent_id------> Intent[S]
Thread[L] ----------------intents[].intent_id-----------------------> Intent[S]
Intent[S] ----------------parents-----------------------------------> Intent[S]
Intent[S] ----------------analysis_context_frames-------------------> ContextFrame[E]
Scheduler[L] -------------为后续 Plan / Task / Run 预留当前态槽位
ContextSnapshot[S] -------作为可选稳定基线存在
```

关系说明：

- `Thread.current_intent_id / latest_intent_id -> Intent` 表示 Thread 会记录当前聚焦的 `Intent`，以及最近挂入的 `Intent`。
- `Thread.intents[].intent_id -> Intent` 表示 Thread 维护了属于这一轮交互的全部 `Intent` 成员列表。
- `Intent.parents -> Intent` 表示新的 `Intent` revision 通过父边连接到更早的版本。
- `Intent.analysis_context_frames -> ContextFrame` 表示 `Intent` 可以引用分析它时使用到的上下文记录。
- `Scheduler` 在这一阶段还没有具体执行对象，但会预留后续 `Plan / Task / Run` 的当前态槽位。
- `ContextSnapshot` 在这一阶段如果出现，表示一份可选的稳定基线，而不是必须对象。

### 6.2 Phase 1：规划

对应英文：[agent-workflow.md Phase 1](./agent-workflow.md#phase-1-planning)。

Scheduler 基于当前 `Intent` 生成计划和任务：

1. 创建一个或多个 `Plan` Snapshot
2. 为可委派的工作单元创建 `Task` Snapshot
3. Libra 从 `Plan + Task` 推导出：
   - ready queue
   - parallel groups
   - checkpoints
   - selected plan head

重点是：

- 计划结构是不可变的
- 调度视图是可变的

如果把 `Phase 1` 的规划过程展开看，通常会经过下面几个步骤：

- 先读取当前被选中的 `Intent`
- 围绕这个 `Intent` 生成一个或多个 `Plan`
- 每个 `Plan` 都可以带上规划时使用到的 `ContextFrame`
- 再从被选中的 `Plan` 中拆出一组 `Task`
- `Scheduler` 不直接保存整个计划内容，而是记录当前选中的
  `selected_plan_id`、所有仍然活跃的 `current_plan_heads`，
  以及由 `Plan + Task` 推导出来的 ready queue 和 checkpoints

可以把这个过程理解成：

```text
Thread[L].current_intent_id
   |
   v
Intent[S] #2 (被选中进入规划)
   |
   +--> Plan[S] #1 (candidate A)
   |        |
   |        +--> context_frames -----------> ContextFrame[E] #2
   |        |
   |        +--> Task[S] #1 (from step A1)
   |        |
   |        +--> Task[S] #2 (from step A2)
   |
   +--> Plan[S] #2 (candidate B, optional)
            |
            +--> context_frames -----------> ContextFrame[E] #3
            |
            +--> Task[S] #3 (from step B1)

Scheduler[L].current_plan_heads ----> Plan[S] #1, Plan[S] #2
Scheduler[L].selected_plan_id  -----> Plan[S] #1
Scheduler[L].ready_queue       -----> Task[S] #1, Task[S] #2
```

上图说明：

- `Intent[S] #2` 表示当前被选中进入规划阶段的那个 `Intent`。
- `Plan[S] #1` 和 `Plan[S] #2` 表示同一个 `Intent` 下可以同时存在多个候选计划。
- `ContextFrame[E] #2 / #3` 表示规划某个 `Plan` 时使用到的上下文记录。
- `Task[S] #1 / #2 / #3` 表示从不同 `Plan` 的不同步骤拆出来的工作单元。
- `Scheduler.current_plan_heads` 表示当前仍然活跃、还没有被淘汰的计划分支头。
- `Scheduler.selected_plan_id` 表示当前真正被选中用于继续推进的计划。
- `Scheduler.ready_queue` 表示已经满足依赖、可以进入执行阶段的任务集合。

这一阶段的对象关系：

```text
Plan[S] ------------------intent_id---------------------------------> Intent[S]
Plan[S] ------------------parents-----------------------------------> Plan[S]
Plan[S] ------------------context_frames----------------------------> ContextFrame[E]
Task[S] ------------------intent_id---------------------------------> Intent[S]
Task[S] ------------------origin_step_id----------------------------> Plan[S].step_id
Task[S] ------------------parent_task_id----------------------------> Task[S]
Task[S] ------------------dependencies------------------------------> Task[S]
Scheduler[L] -------------selected_plan_id / current_plan_heads----> Plan[S]
Scheduler[L] -------------ready queue / checkpoints-----------------> 从 Plan[S] + Task[S] 推导
```

关系说明：

- `Plan.intent_id -> Intent` 表示每个 `Plan` 都是围绕某一个 `Intent` 生成的。
- `Plan.parents -> Plan` 表示 replan 或 merge 后的新 `Plan` 通过父边接到旧 `Plan` 上。
- `Plan.context_frames -> ContextFrame` 表示规划时使用到的上下文会被冻结引用在 `Plan` 上。
- `Task.intent_id -> Intent` 表示任务最终仍然属于某个 `Intent` 语义范围。
- `Task.origin_step_id -> Plan.step_id` 表示任务来源于 `Plan` 中的某一个逻辑步骤。
- `Task.parent_task_id -> Task` 表示任务之间可以形成父子层级。
- `Task.dependencies -> Task` 表示任务之间的前置依赖关系。
- `Scheduler.selected_plan_id / current_plan_heads -> Plan` 表示当前选中的计划，以及当前保留着的多个计划分支头。
- `Scheduler.ready queue / checkpoints` 不是单独对象，而是根据 `Plan + Task` 推导出来的运行时结构。

### 6.3 Phase 2：执行

对应英文：[agent-workflow.md Phase 2](./agent-workflow.md#phase-2-execution)。

对于每个 ready task 或并行组：

1. Libra 准备运行时上下文
2. 持久化 `Run` 和 `Provenance`
3. 追加 `TaskEvent` / `RunEvent` / `PlanStepEvent`
4. 记录 `ToolInvocation` / `Evidence` / `ContextFrame`
5. 生成一个或多个 `PatchSet`
6. 写入 `RunUsage`
7. Libra 更新 retry、staging、replan 等 mutable control state

这里要特别注意：

- 执行是事件流，不是覆盖式状态更新
- 候选补丁是新的 `PatchSet`，不是覆盖旧的 diff
- 上下文新增通过 `ContextFrame` 记录，而不是修改共享容器

对于并行组完成后的增量集成，还会发生：

1. Libra 将 staging 中的多个 `PatchSet` 合并回主 sandbox 视图
2. 校验 interface contracts，并运行 batch integration tests
3. 追加新的 `Evidence`、以及必要时的 `RunEvent` / `TaskEvent`
4. 如果剩余任务图已经失效，则写入新的 `Plan` revision，
   再由 `Scheduler` 更新后续调度视图

如果把 `Phase 2` 的执行过程展开看，通常会经过下面几个步骤：

- `Scheduler` 从 ready queue 中选出一个或一组 `Task`
- 为每个 `Task` 创建一次新的 `Run`
- 为这次 `Run` 记录 `Provenance`
- 执行过程中不断追加 `TaskEvent`、`RunEvent`、`PlanStepEvent`
- 工具调用、验证结果和新增上下文分别写成 `ToolInvocation`、
  `Evidence`、`ContextFrame`
- 每次执行可以产出一个或多个 `PatchSet`
- 如果执行改变了剩余计划结构，则生成新的 `Plan` revision，
  而不是修改旧计划

可以把这个过程理解成：

```text
Scheduler[L].ready_queue
   |
   v
Task[S] #1
   |
   +--> Run[S] #1
   |        |
   |        +--> Provenance[S] #1
   |        |
   |        +--> TaskEvent[E] #1 (task started)
   |        +--> RunEvent[E] #1 (run started)
   |        +--> PlanStepEvent[E] #1 (step advanced)
   |        +--> ToolInvocation[E] #1
   |        +--> ContextFrame[E] #4
   |        +--> Evidence[E] #1
   |        +--> PatchSet[S] #1
   |        +--> PatchSet[S] #2 (optional candidate)
   |        +--> RunUsage[E] #1
   |
   +--> Run[S] #2 (retry or re-execution, optional)

Scheduler[L].active_task_id ----> Task[S] #1
Scheduler[L].active_run_id  ----> Run[S] #1
Scheduler[L].live_context_window -> ContextFrame[E] #4
Plan[S] #3 (replan, optional) ----parents----> Plan[S] #1
```

上图说明：

- `Task[S] #1` 表示当前 ready queue 中被挑出来执行的任务。
- `Run[S] #1` 表示围绕这个任务产生的一次具体执行尝试。
- `Provenance[S] #1` 表示这次执行使用的模型、参数和执行来源信息。
- `TaskEvent[E] #1`、`RunEvent[E] #1`、`PlanStepEvent[E] #1` 表示任务、运行和步骤层面的推进变化。
- `ToolInvocation[E] #1` 表示执行过程中实际发生的工具调用。
- `ContextFrame[E] #4` 表示执行过程中新增的上下文记录。
- `Evidence[E] #1` 表示这次执行附带产生的验证证据。
- `PatchSet[S] #1 / #2` 表示一次执行可以产出一个或多个候选补丁。
- `RunUsage[E] #1` 表示本次执行对应的 usage / cost 记录。
- `Run[S] #2` 表示重试或再次执行时，会创建新的 `Run`，而不是覆盖旧的 `Run`。
- `Plan[S] #3 (replan)` 表示如果执行改变了剩余策略，会生成新的计划版本接回旧计划。

这一阶段的对象关系：

```text
Run[S] -------------------task_id-----------------------------------> Task[S]
Run[S] -------------------plan_id-----------------------------------> Plan[S]
Run[S] -------------------context_snapshot_id-----------------------> ContextSnapshot[S]
Provenance[S] ------------run_id------------------------------------> Run[S]
PatchSet[S] --------------run_id------------------------------------> Run[S]
TaskEvent[E] -------------task_id-----------------------------------> Task[S]
RunEvent[E] --------------run_id------------------------------------> Run[S]
RunUsage[E] --------------run_id------------------------------------> Run[S]
PlanStepEvent[E] ---------plan_id / step_id / run_id---------------> Plan[S] / Plan[S].step_id / Run[S]
ToolInvocation[E] --------run_id------------------------------------> Run[S]
Evidence[E] --------------run_id / patchset_id----------------------> Run[S] / PatchSet[S]
ContextFrame[E] ----------run_id / plan_id / step_id---------------> Run[S] / Plan[S] / Plan[S].step_id
Scheduler[L] -------------active_task_id / active_run_id-----------> Task[S] / Run[S]
Scheduler[L] -------------live_context_window-----------------------> ContextFrame[E]
Plan[S]（replan）---------parents-----------------------------------> 旧 Plan[S]
```

关系说明：

- `Run.task_id -> Task` 表示每次执行尝试都必须隶属于一个任务。
- `Run.plan_id -> Plan` 表示这次执行如果属于某个计划分支，会记录对应的 `Plan`。
- `Run.context_snapshot_id -> ContextSnapshot` 表示执行启动时可以绑定一份稳定上下文基线。
- `Provenance.run_id -> Run` 表示模型、参数、执行来源信息都附着在某次 `Run` 上。
- `PatchSet.run_id -> Run` 表示候选补丁一定是某次执行尝试的产物。
- `TaskEvent.task_id -> Task` 表示任务层面的变化历史附着在任务上。
- `RunEvent.run_id -> Run` 表示执行层面的变化历史附着在运行上。
- `RunUsage.run_id -> Run` 表示 usage / cost 归属于某次运行。
- `PlanStepEvent.plan_id / step_id / run_id -> Plan / Plan.step / Run` 表示步骤推进同时关联计划、步骤和具体执行尝试。
- `ToolInvocation.run_id -> Run` 表示工具调用是某次执行中的事实记录。
- `Evidence.run_id / patchset_id -> Run / PatchSet` 表示证据既可以归属于运行，也可以指向某个候选补丁。
- `ContextFrame.run_id / plan_id / step_id -> Run / Plan / Plan.step` 表示上下文增量可以挂在不同层级上。
- `Scheduler.active_task_id / active_run_id -> Task / Run` 表示当前被强调或正在执行的任务与运行。
- `Scheduler.live_context_window -> ContextFrame` 表示运行时实际可见的上下文窗口。
- `Plan(replan).parents -> 旧 Plan` 表示重新规划时生成的是新 `Plan`，而不是覆盖旧计划。

### 6.4 Phase 3：验证与审计

对应英文：[agent-workflow.md Phase 3](./agent-workflow.md#phase-3-system-level-validation-and-audit)。

系统在这一阶段做系统级检查：

- 测试
- 静态分析
- 安全审计
- 审核证据收集

输出表现为：

- 新的 `Evidence`
- 新的 `Decision`
- 终态 `TaskEvent` / `RunEvent`
- 可选最终 `ContextSnapshot`

Libra 在此基础上重建 release candidate view 和 audit view。

如果系统级验证或安全审计发现问题，控制流会回到 `Phase 2`：

- 保留已有 immutable audit trail
- 将问题写成新的 `Evidence`
- 必要时追加终态或失败态 `RunEvent` / `TaskEvent`
- 如果需要调整剩余方案，则持久化新的 `Plan` revision，
  再继续后续执行

如果把 `Phase 3` 的验证与审计过程展开看，通常会经过下面几个步骤：

- 先基于前面产生的 `Run`、`PatchSet` 和上下文结果做系统级验证
- 将测试、静态分析、安全审计的结果写成新的 `Evidence`
- 如果已经可以形成发布候选，则可选写入一个最终 `ContextSnapshot`
- 当需要表达“选中了哪个补丁”或“本轮是否通过”时，写入 `Decision`
- 如果发现问题无法直接放行，则通过新的 `Evidence`、终态事件和
  `Plan` revision 把控制流送回 `Phase 2`

可以把这个过程理解成：

```text
Run[S] #1 + PatchSet[S] #1
   |
   +--> Evidence[E] #2 (tests)
   +--> Evidence[E] #3 (static analysis)
   +--> Evidence[E] #4 (security audit)
   |
   +--> ContextSnapshot[S] #2 (release candidate, optional)
   |
   +--> Decision[E] #1 (choose PatchSet[S] #1, optional)
   |
   +--> RunEvent[E] #2 (completed / failed)
   +--> TaskEvent[E] #2 (completed / failed)
   +--> IntentEvent[E] #3 (optional terminal intent event)

如果验证失败：
Plan[S] #4 (replan) ----parents----> Plan[S] #3
   |
   +--> 返回 Phase 2
```

上图说明：

- `Evidence[E] #2 / #3 / #4` 表示同一个发布候选可以对应多类验证证据。
- `ContextSnapshot[S] #2` 表示当系统需要冻结一份 release candidate 基线时，会写入新的快照。
- `Decision[E] #1` 表示验证阶段已经可以产生“选中哪个补丁”或“本轮是否通过”的决策。
- `RunEvent[E] #2` 和 `TaskEvent[E] #2` 表示验证结束后，运行和任务会进入新的终态。
- `IntentEvent[E] #3` 表示在需要时，也可以把 `Intent` 推进到终态。
- `Plan[S] #4 (replan)` 表示如果验证失败，需要通过新 `Plan` revision 回到执行阶段，而不是回写旧计划。

这一阶段的对象关系：

```text
Evidence[E] --------------run_id------------------------------------> Run[S]
Evidence[E] --------------patchset_id-------------------------------> PatchSet[S]
Decision[E] --------------run_id / chosen_patchset_id-------------> Run[S] / PatchSet[S]
TaskEvent[E] -------------task_id-----------------------------------> Task[S]
RunEvent[E] --------------run_id------------------------------------> Run[S]
IntentEvent[E] -----------intent_id / next_intent_id---------------> Intent[S] / Intent[S]
ContextSnapshot[S] -------作为可选 release candidate 基线存在
Audit View[L] ------------由 Intent -> Plan -> Task -> Run -> PatchSet / Evidence / Decision 重建
Plan[S]（replan）---------parents-----------------------------------> 旧 Plan[S]
```

关系说明：

- `Evidence.run_id -> Run` 表示大多数验证结果首先归属于某次运行。
- `Evidence.patchset_id -> PatchSet` 表示证据也可以精确指向某个候选补丁。
- `Decision.run_id / chosen_patchset_id -> Run / PatchSet` 表示最终决策既关联执行过程，也关联被选中的候选补丁。
- `TaskEvent.task_id -> Task` 和 `RunEvent.run_id -> Run` 表示任务和执行的终态仍然通过事件记录下来。
- `IntentEvent.intent_id / next_intent_id -> Intent` 表示在验证之后，可以对某个 `Intent` 记录终态，或者指向下一步建议处理的 `Intent`。
- `ContextSnapshot` 如果出现在这里，表示 release candidate 阶段的稳定上下文基线。
- `Audit View` 不是新对象，而是从整条 `Intent -> Plan -> Task -> Run` 审计链重建出来的阅读视图。
- `Plan(replan).parents -> 旧 Plan` 表示如果验证失败触发重规划，仍然通过新 `Plan` revision 接回旧计划。

### 6.5 Phase 4：决策与发布

对应英文：[agent-workflow.md Phase 4](./agent-workflow.md#phase-4-decision-and-release)。

最后一阶段处理发布决策：

- 低风险任务可自动合并
- 高风险任务进入人工评审
- 最终决定写入 `Decision`
- 如有必要写入终态 `IntentEvent`
- Libra 推进当前 thread / workspace 指针

发布完成后，`Scheduler` 的活跃执行字段应退出活跃态：

- `active_task_id` 和 `active_run_id` 通常应清空为 `None`
- `selected_plan_id` 与 `current_plan_heads` 可以按产品策略保留为只读展示，
  或进入归档态，但不再表示“仍在执行”

如果把 `Phase 4` 的决策与发布过程展开看，通常会经过下面几个步骤：

- 基于 `Evidence`、`Decision`、风险等级和审计结果形成最终发布判断
- 如果风险低且条件满足，可以自动接受某个 `PatchSet`
- 如果风险高，则进入人工评审后再形成最终 `Decision`
- 发布完成后，可以为当前 `Intent` 追加终态 `IntentEvent`
- 同时更新 `Thread` 的当前焦点和 DAG heads，并清理 `Scheduler`
  的活跃执行字段

可以把这个过程理解成：

```text
PatchSet[S] #1 + Evidence[E] #2/#3/#4
   |
   +--> Decision[E] #2 (approve / reject / request changes)
   |
   +--> IntentEvent[E] #4 (completed / cancelled, optional)
   |
   +--> Thread[L].latest_intent_id
   +--> Thread[L].current_intent_id
   +--> Thread[L].intents[].is_head
   |
   +--> Scheduler[L].selected_plan_id      (可选保留为只读展示)
   +--> Scheduler[L].current_plan_heads    (保留或归档)
   +--> Scheduler[L].active_task_id = None
   +--> Scheduler[L].active_run_id  = None
```

上图说明：

- `Decision[E] #2` 表示发布阶段最终形成的接受、拒绝或要求修改的决定。
- `IntentEvent[E] #4` 表示发布完成后，可以把当前 `Intent` 标记为 completed 或 cancelled。
- `Thread.latest_intent_id` 表示最近一次被挂入 Thread 的 `Intent`。
- `Thread.current_intent_id` 表示发布后 Thread 当前仍然聚焦的那个 `Intent`，它可以保持不变，也可以切换到下一个推荐 `Intent`。
- `Thread.intents[].is_head` 表示在发布后哪些 `Intent` 仍然构成当前 DAG 的分支头。
- `Scheduler.selected_plan_id / current_plan_heads` 可以按产品策略继续保留用于只读展示。
- `Scheduler.active_task_id / active_run_id = None` 表示活跃执行态已经结束。

这一阶段的对象关系：

```text
Decision[E] --------------run_id------------------------------------> Run[S]
Decision[E] --------------chosen_patchset_id------------------------> PatchSet[S]
IntentEvent[E] -----------intent_id---------------------------------> Intent[S]
IntentEvent[E] -----------next_intent_id----------------------------> Intent[S]
Thread[L] ----------------current_intent_id / latest_intent_id------> Intent[S]
Thread[L] ----------------intents[].is_head-------------------------> 标记当前 Intent DAG heads
Scheduler[L] -------------selected_plan_id / current_plan_heads----> Plan[S]（可保留为只读展示）
Scheduler[L] -------------active_task_id / active_run_id-----------> 发布后清空为 None
```

关系说明：

- `Decision.run_id -> Run` 表示最终决策对应的是哪一次执行尝试。
- `Decision.chosen_patchset_id -> PatchSet` 表示如果最终选择了某个补丁，会明确指向该 `PatchSet`。
- `IntentEvent.intent_id -> Intent` 表示某个 `Intent` 在发布阶段可以进入 completed 或 cancelled 等终态。
- `IntentEvent.next_intent_id -> Intent` 表示发布结束后也可以建议下一个要处理的 `Intent`。
- `Thread.current_intent_id / latest_intent_id -> Intent` 表示 Thread 会更新当前聚焦点和最近挂入点。
- `Thread.intents[].is_head` 表示哪些 `Intent` 仍然是当前 DAG 的分支头。
- `Scheduler.selected_plan_id / current_plan_heads -> Plan` 表示计划层信息可以保留用于只读展示。
- `Scheduler.active_task_id / active_run_id -> None` 表示发布结束后，活跃执行态通常应被清空。

## 7. 重建与恢复规则

这是整个设计里非常关键的一条：

View 丢失不能阻塞读取。

换句话说：

- `Thread` 的拓扑字段可以从 `Intent` / `IntentEvent` / 相关关系重建
- `Scheduler` 的执行图关系可以从 `Plan` / `Task` / `Run` / `PlanStepEvent` 重建
- Query Index 丢了，可以重扫 Snapshot + Event 重新生成

更精确地说：

- 可以从 `git-internal` 严格重建的是对象图相关字段，例如：
  `intents`、branch heads、`latest_intent_id`、`Plan -> Task -> Run`
  的执行关系，以及各种 reverse indexes
- 属于 Libra 原生会话元数据或显式 UI 焦点的字段，例如：
  `title`、`owner`、`participants`、`metadata`、`current_intent_id`
  以及某些显式选择态，未必能从 immutable history 无损恢复

因此 View 丢失后的正确语义不是“对象不存在”，而是：

- 核心导航和执行关系应先尽量从 immutable history 重建
- 无法严格重建的 Libra 原生字段应允许退化为默认值，
  或依赖 Libra 自身持久化 / 备份恢复

因此：

- View 是当前态缓存
- Snapshot + Event 才是权威历史来源

## 8. 实现时应遵守的规则

### 8.1 Snapshot 只回答“它是什么”

不要把 mutable state 塞进 Snapshot。

### 8.2 Event 只回答“发生了什么”

Event 只追加，不回写父对象。

### 8.3 Libra 只回答“当前怎么看”

当前选中哪个 Plan、当前跑哪个 Task、当前哪个 Intent 是 head，
这些都属于 Libra。

### 8.4 缺失 View 记录不应视为对象不存在

读取 View 记录时，`None` 的含义应该优先理解为：

```text
View 缺失，可能需要 rebuild
```

而不是：

```text
这个 thread / scheduler 在语义上不存在
```

### 8.5 高速读写路径优先走 View

UI、MCP、调度器优先读 Libra View 和 Query Index；
缺失或不一致时再降级重建。

### 8.6 读写路径决策

建议实现成下面的决策顺序：

- 读路径：
  1. 先读取 View / Query Index
  2. 命中则直接返回
  3. 缺失或明显过期时，回退到 rebuild 或直接扫描 immutable history
  4. 将可重建结果回写 View
  5. 如果只缺失 Libra 原生元数据，则允许使用降级默认值返回；
     降级默认值的具体策略应由实现层按字段语义决定，
     不是所有字段都适合直接退化为零值
- 写路径：
  1. 先写 Snapshot / Event 这类 immutable fact
  2. 再 update 或 upsert View 记录
  3. 如果 View 记录缺失，通常直接 create / upsert 即可，
     不要求每次写入前都先做全量 rebuild
  4. 只有当写入依赖 View version 或当前态判断时，
     才需要先 reload 或 rebuild 再做条件更新

## 9. 一份简短总结

这套 Agent 设计的本质是把“定义”、“事件”和“当前态”彻底分离：

```text
Snapshot：定义与结构
Event：执行与审计事实
Libra：当前会话 / 调度 / 索引视图
```

其中：

- `Intent / Plan / Task / Run / PatchSet` 是不可变业务骨架
- `TaskEvent / RunEvent / ToolInvocation / Evidence / Decision / ContextFrame` 是执行事实流
- `Thread / Scheduler` 是当前系统视图
- `Query Index` 是为当前系统视图服务的加速辅助结构

只要坚持这条边界，系统就能同时满足：

- 可追溯
- 可审计
- 可恢复
- 可并行
- 可重建
- 可扩展

## 10. 多 Intent Thread 的重建

`Thread` 里有多个 `Intent` 并不会让重建失效。

真正能不能重建，关键不在于 `Intent` 的数量，而在于这些 `Intent`
之间有没有可追溯的 immutable 连接关系。

在当前设计里，主要依赖两类边：

- `Intent.parents`
- `IntentEvent.next_intent_id`

因此，重建时不应该把 `Thread` 理解成一条线性的链，而应该理解成一张
由多个 `Intent` 组成的关系图。

重建的基本过程是：

1. 选择一个起始 `Intent`
2. 沿着 `Intent.parents` 和 `IntentEvent.next_intent_id` 扩展
3. 找到同一个连通分量中的全部 `Intent`
4. 把这组 `Intent` 视为一个 `Thread` 的骨架
5. 再收集这组 `Intent` 派生出来的 `Plan`、`Task`、`Run`、`PatchSet`、
   `Evidence`、`Decision`、`ToolInvocation`、`ContextFrame` 等记录

因此：

- 多个 `Intent` 不是问题
- 分支和合流也不是问题
- 真正的问题是缺少 immutable linkage

如果若干 `Intent` 在语义上原本属于同一个 `Thread`，但它们之间没有留下
任何 immutable 边，那么在 View 全部丢失后，就无法再证明它们原本属于
同一个 `Thread`。

所以设计上必须坚持一条原则：

```text
凡是要被视为同一个 Thread 的多个 Intent，必须至少留下
一种可重建的 immutable 连接关系。
```
