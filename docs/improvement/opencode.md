# `libra code` 多 LLM 编排落地方案（基于 opencode 本地架构）

> **定位**：本文是 [agent.md](agent.md) 的横向补充计划，目标是把 `libra code` 从「单 session、单 provider、单 agent」演进为「单 session、多 agent、多 provider 协同」。本文不替代 agent.md 的 Step / Phase 编号；所有实施项都必须挂接到 agent.md Step 1 / Step 2 已有契约上。
>
> **验证基线**：2026-05-05，本地 opencode 仓库 `/Volumes/Data/opencode`，分支 `dev`，HEAD `25ecf0af6`（`fix: retry server_is_overloaded errors`，本计划当日合入）；上游近 24 小时关键提交：
> - `25ecf0af6` provider retry policy（`server_is_overloaded` 视为可重试）
> - `576480b5d` Mistral medium 3.5/2604 reasoning variant 注册
> - `811954880` `fix(compaction): order compaction summary before retained tail` — `filterCompacted` 重排算法
> - `75d141b57` `fix(session): cancel subtask child sessions` — `TaskPromptOps.cancel` 改为可 await 的 Effect
> - `22a4a9df8` `feat(core): session warping`（多终端 session 同步，Libra 不在范围）
>
> Libra 仓库 `/Volumes/Data/GitMono/libra`。本文只改进文档，不宣称对应实现已经落地。

---

## 结论

opencode 可借鉴的是一组编排 primitive，而不是它的 TypeScript / Effect 实现本身。Libra 的落地路径必须先补三个 Interface，再接 Task 工具：

1. **Provider Runtime Adapter**：Libra 的 `CompletionModel` 不是 object-safe，不能按旧稿写成 `Box<dyn CompletionModel>`。第一阶段应做 `AnyCompletionModel` enum Adapter，统一 7 个 provider 的运行时选择。
2. **Executable Agent Profile**：现有 `AgentProfile` 只是 system prompt + tools hint；要升级为可执行的 `AgentExecutionSpec`，明确 `mode`、`model`、`variant`、`steps`、工具策略和权限策略。
3. **Task Dispatcher**：`task` 不能只做普通 `ToolHandler`。Libra 的 `ToolHandler::handle()` 只有单个 tool invocation，没有父会话 history、父模型、usage recorder 和 session store。必须在 `tool_loop` 层加 `SubAgentDispatcher` Interface，registry 只暴露 task schema。
4. **Context Handoff / Compaction**：Libra 已有 `ContextBudget`、`ContextFrameEvent`、`CompactionEvent`，缺的是 LLM summarizer 和跨 provider handoff 模板。不要新建平行 transcript 格式。
5. **Goal Supervisor**：Codex-like Goal 模式不能只靠 system prompt 约束"继续做"。必须有 runtime supervisor 持久化 `GoalSpec` / `GoalState`，在目标未完成时禁止进入普通 final/idle；只有完成验证通过、用户显式取消、或进入等待用户输入状态时才能停止推进。
6. **GA 配置面**：多 agent 配置、预算、Goal 模式、TUI 命令最后发布，并放在 `code.multi_agent.enabled = false` / `code.goal.enabled = false` 默认关闭路径后，确保旧用户路径零回归。

最小可落地路径是 OC-Phase 0 到 OC-Phase 6。每个 phase 都定义文件落点、测试、退出标准和回滚边界。

---

## 目录

- [已验证架构事实](#已验证架构事实)
- [Libra 当前差距](#libra-当前差距)
- [版本管理与 entire.md 兼容约束](#版本管理与-entiremd-兼容约束)
- [目标架构](#目标架构)
- [核心数据契约](#核心数据契约)
- [Permission Ruleset 与 Approval 反馈协议](#permission-ruleset-与-approval-反馈协议)
- [Tool Registry 预过滤合同](#tool-registry-预过滤合同)
- [Codex-like Goal 模式](#codex-like-goal-模式)
- [实施路线](#实施路线)
- [与 agent.md 的接口](#与-agentmd-的接口)
- [与 entire.md 的接口](#与-entiremd-的接口)
- [测试矩阵](#测试矩阵)
- [Acceptance Scenarios（端到端落地标准）](#acceptance-scenarios端到端落地标准)
- [PR 切片建议](#pr-切片建议)
- [风险与反模式](#风险与反模式)
- [非目标](#非目标)
- [Appendix A: opencode 源码锚点](#appendix-a-opencode-源码锚点)
- [Appendix B: Provider Quirks Inventory](#appendix-b-provider-quirks-inventory)
- [Changelog](#changelog)

---

## 已验证架构事实

### opencode Module Map

| Module | Interface | Implementation | Libra 可借鉴点 |
|--------|-----------|----------------|----------------|
| Config | `Config.Info.agent`、`ConfigAgent.Info` | `packages/opencode/src/config/config.ts`、`config/agent.ts` 解析 config 与 `{agent,agents}/**/*.md` | agent 配置是启动期事实，不是 prompt 片段 |
| Provider | `Provider.Service.getModel()`、`getLanguage()`、`defaultModel()` | `provider/provider.ts` 维护 provider/model registry，动态加载 SDK，合并 `models.dev`、config、auth、plugin | Libra 需要独立 Provider Runtime Adapter，而不是把 provider match 留在 `code.rs` |
| Model Metadata | `Provider.Model` | `provider/provider.ts` 中 `capabilities`、`cost`、`limit`、`variants`，来源为 `models.dev` + config override | capability matrix 应先做静态表，后续再支持远端刷新 |
| Provider Transform | `ProviderTransform.message/options/variants` | `provider/transform.ts` 统一处理 normalize、cache、temperature、topP、topK、reasoning variants、schema transform | quirks 应集中在一个 Module，避免 provider copy-paste |
| Agent | `Agent.Info` | `agent/agent.ts` 定义 build / plan / general / explore / compaction / title / summary，并合并用户配置 | agent 是一等运行实体，含 model 和 permission |
| Tool Registry | `ToolRegistry.tools()` | `tool/registry.ts` 合并内建、plugin、project tools，并按 provider/model 过滤 edit/apply_patch | 工具描述可动态注入可用 sub-agent 列表 |
| Task Tool | `TaskTool.Parameters` | `tool/task.ts` 请求 permission，创建 child session，选择 sub-agent model，调用 `SessionPrompt.prompt()` | task 是父 session 内的受控 dispatcher |
| Session Prompt | `SessionPrompt.Service.prompt()` | `session/prompt.ts` 负责 prompt parts、agent/model resolution、tool loop、compaction trigger | Libra 的 task dispatcher 应挂到 tool-loop/prompt 层 |
| LLM | `LLM.Service.stream()` | `session/llm.ts` 组合 system prompt、ProviderTransform、tools、plugin hooks，调用 AI SDK | Libra 应把 provider options / transform 放进 completion request 前后 |
| Processor | `SessionProcessor.Service` | `session/processor.ts` 消费 stream，写 message parts，执行 tools，处理 overflow | Task / Compaction 都应产生可 replay 事件 |
| Permission | `Permission.Ruleset` | `permission/index.ts` 支持 allow / deny / ask，按 ruleset + approved 计算 | 子 agent 权限必须是父权限的受限投影，不可扩大 |
| Compaction | `SessionCompaction.Service` | `session/compaction.ts` 保留 tail turns，调用 compaction agent，写 compaction marker 和 v2 events | Libra 已有 deterministic `CompactionEvent`，只缺 LLM summarizer |

### opencode 调用链

```text
Config.load
  -> Agent.Service list/get/defaultAgent
  -> ToolRegistry.tools(model, agent)
  -> SessionPrompt.prompt(input)
  -> Provider.getModel(providerID, modelID)
  -> LLM.stream({ model, agent, tools, messages })
  -> ProviderTransform.message/options/variants
  -> SessionProcessor.process(stream)
  -> Tool.execute(...)
     -> task tool creates child session and re-enters SessionPrompt.prompt(...)
     -> compaction creates synthetic compaction message and re-enters processor with no tools
```

这条链路说明两件事：

- opencode 的 Task tool 能运行，是因为 tool context 带有 `promptOps`，可以重新进入 `SessionPrompt.prompt()`。
- Libra 不能把 Task tool 只做成 `ToolHandler`，除非同时把父会话上下文塞进 `ToolRuntimeContext`。更清晰的落点是在 `tool_loop` 加 `SubAgentDispatcher`。

### Libra 已有基线

| 层 | 当前文件 | 现状 |
|----|----------|------|
| provider 入口 | [src/command/code.rs](../../src/command/code.rs) `CodeProvider` 与 `execute_tui()` | CLI 启动时 match provider，启动后单 provider / 单 model |
| completion trait | [src/internal/ai/completion/mod.rs](../../src/internal/ai/completion/mod.rs) | `CompletionModel: Clone + Send + Sync`，含 associated `Response` 与 RPITIT future，不可直接做 trait object |
| tool loop | [src/internal/ai/agent/runtime/tool_loop.rs](../../src/internal/ai/agent/runtime/tool_loop.rs) | 泛型 `M: CompletionModel`，记录 usage、context frame、hooks、allowed_tools、stream events |
| provider modules | [src/internal/ai/providers/](../../src/internal/ai/providers/) | 7 个 provider 各自处理 request/response mapping，缺统一 capability/transform Module |
| profile parser/router | [src/internal/ai/agent/profile/](../../src/internal/ai/agent/profile/) | frontmatter 只解析 `name` / `description` / `tools` / `model` 字符串；TUI 中只通过 slash command 选择 agent prompt |
| tools | [src/internal/ai/tools/](../../src/internal/ai/tools/) | 工具 registry 有 hardening、sandbox、approval；没有 task schema / dispatcher |
| context budget | [src/internal/ai/context_budget/](../../src/internal/ai/context_budget/) | `ContextBudget`、`ContextFrameEvent`、`CompactionEvent` 已有，可复用 |
| usage | [src/internal/ai/usage/](../../src/internal/ai/usage/) | `agent_usage_stats` 已按 provider/model 聚合，预留 `agent_run_id`，但没有 `agent_name` |
| sub-agent schema | [src/internal/ai/agent_run/](../../src/internal/ai/agent_run/) | `subagent-scaffold` feature 下有 schema-only `AgentTask` / `AgentRun` / `AgentRunEvent` / `AgentBudget` / `AgentPermissionProfile` |

---

## Libra 当前差距

| 差距 | 为什么会阻塞 | 先做什么 |
|------|--------------|----------|
| `CompletionModel` 不能动态分发 | 旧稿的 `Box<dyn CompletionModel>` 无法编译；`run_tool_loop` 需要固定 `M::Response: CompletionUsage` | `AnyCompletionModel` + `AnyCompletionRawResponse` enum Adapter |
| provider 构建散在 `code.rs` | task 派发时也需要按 `(provider, model)` 创建模型，不能复用 CLI match | `providers/factory.rs`，命令层只负责传 env / flags |
| profile 不可执行 | `model_preference` 只是字符串，TUI 只把 prompt 拼进 user message | `AgentExecutionSpec` + router 返回完整 spec |
| Task tool 缺父上下文 | 普通 `ToolHandler` 看不到 history、parent session、usage recorder、model binding | `SubAgentDispatcher` 挂到 `ToolLoopConfig` 或相邻 runtime |
| 权限合并语义未定 | 子 agent 可能绕过父 sandbox / approval | 明确父权限 ∩ 子权限，deny 优先，默认不允许 nested sub-agent |
| compaction 不是 LLM summarizer | 已有 `CompactionEvent` 只记录 summary 字段，但没有生成结构化 handoff 的 agent | 新增 `ContextHandoffBuilder`，复用 `ContextFrameEvent` 和 session JSONL |
| Goal 模式缺 runtime 监督 | 仅靠 prompt 说"完成前不要停"会被模型 final answer、turn budget、重复调用 abort 或上下文压缩打断 | 新增 `GoalSupervisor`，把"能否停止"变成 runtime 状态机与完成验证 |
| usage 缺 agent 维度 | 多 provider 后只按 provider/model 聚合不够定位成本 | migration 增加 `agent_name`，sub-agent 填 `agent_run_id` |

---

## 版本管理与 entire.md 兼容约束

结论：本文方案可以兼容 [entire.md](entire.md)，但前提是实现时保留 Libra 现有版本管理逻辑，并把"Libra 内部 sub-agent"与"外部 Agent 会话捕获"分开。opencode 的 task/sub-agent primitive 只能进入 `libra code` runtime，不得绕过 Libra 的 Snapshot / Event / SessionStore / Git-object 追加路径。

### 所有权边界

| 资产 | Owner | 本文约束 |
|------|-------|----------|
| `refs/libra/intent` / `AI_REF` | `libra code` 当前结构化 AI 制品 | 保持现有写入语义；本文不新增 parallel intent ref，也不改变 `refs/libra/intent` 的 commit 形态 |
| `refs/libra/agent-traces` | `entire.md` 的外部 Agent 捕获 | 本文内部 sub-agent **不得写入**；不得创建 `agent_session` / `agent_checkpoint` 行；只有 entire.md 的 `ObservedAgent` adapter 可以用 `agent_kind='opencode'` 捕获外部 OpenCode |
| `agent_run/` schema | `agent.md` Step 2 的 Libra 内部 sub-agent contract | OC-Phase 0/3 只复用或扩展这个 schema；默认 build 不直接依赖 `subagent-scaffold` gated 类型 |
| `SessionStore` JSONL | Libra runtime 会话事件流 | 内部 sub-agent 事件落在 `libra code` 的 session namespace；如果 entire.md 已引入 `.libra/sessions/code/` 与 `.libra/sessions/agent/`，本文只使用 `code/`，不写 `agent/` |
| `object_index` / cloud sync | Git object 与分层存储索引 | 若新增 blob/tree 持久化，必须通过现有 object 写入路径登记 `object_index`；不得在 SQLite 镜像 blob OID |
| SQLite migration version | built-in migration runner | entire.md 预留 `2026050303_agent_capture`。本文后续若要迁移 `agent_usage_stats.agent_name`，必须取 `max_registered_version() + 1` 的 `YYYYMMDDNN` 版本，不复用 `2026050303` |
| locked refs | branch protection | `intent` 与 entire.md 的 `agent-traces` 都是受保护 ref；sub-agent merge/rewind/restore 不得直接操作这些 refs 或用户分支 |

### 保留的版本管理不变量

1. `libra code` 的旧路径在 flag-off 下必须只写原有 session/event/usage 结构，不写 `agent_session`、`agent_checkpoint`、`refs/libra/agent-traces`。
2. 子 agent 输出默认是事件和受限 tool result；需要代码变更时只产生隔离 workspace / `AgentPatchSet` / evidence，最终 merge 归 agent.md CEX-S2-13，不在 OC-Phase 3 直接改主 worktree。
3. 任何可 replay 状态都必须是 append-only event 或 Snapshot；reader 必须保持 unknown-event-safe，不允许用临时内存状态作为恢复主路径。
4. 需要 Git-backed artifact 时，必须复用 `HistoryManager` / object writer 的 CAS 与 retry 语义，不手写 ref update。
5. `thread_id`、`agent_run_id`、`task_id` 的关联只能作为 metadata / index；不能把 external `agent_session.session_id` 当成 internal `AgentRunId`。
6. DB 迁移必须保持单调版本、幂等 up/down、更新 `tests/db_migration_test.rs` 的版本断言；新增 SQL 文件遵循 entire.md 的 `include_str!` 路线。

### 兼容性 Gate

OC-Phase 3 合入前必须有以下回归：

- flag-off 跑完一次 `libra code`，`refs/libra/agent-traces` tip 不变，`agent_session` / `agent_checkpoint` 行数不变。
- fake sub-agent E2E 只产生 `AgentRunEvent` / code session JSONL / usage row，不产生 external agent checkpoint。
- migration 测试证明 built-in 版本序列在 entire.md 的 `2026050303` 之后继续单调递增。
- 任一新增 blob 通过 `object_index` 可被云同步扫描到，且 SQLite 不保存 transcript/blob OID 镜像。
- restore/reset/switch/checkout 对 `intent` 与 `agent-traces` 的 locked-ref 保护仍然通过。

---

## 目标架构

```text
.libra/agents/*.md or .libra/agents.toml
  -> AgentExecutionSpec
  -> ModelBinding(provider_id, model_id, variant)
  -> ProviderFactory.build_any_completion_model(...)
  -> AnyCompletionModel implements CompletionModel
  -> GoalSupervisor optional wrapper
       -> GoalState projection from SessionJsonlStore
       -> run_tool_loop(...)
            -> normal tools via ToolRegistry.dispatch(...)
            -> task tool schema via ToolRegistry
            -> goal progress / completion tools when Goal mode is active
            -> task calls intercepted by SubAgentDispatcher
                 -> AgentRun / AgentTask schema
                 -> child SessionJsonlStore in the `libra code` namespace
                 -> ContextHandoff from ContextFrame + compaction agent
                 -> child run_tool_loop with restricted registry and model
                 -> ToolResult returned to parent
       -> GoalVerifier checks completion claim and evidence
       -> if incomplete: append GoalEvent and re-enter run_tool_loop with continuation prompt
```

### Module Depth 目标

- `providers/factory.rs` 应是深 Module：调用方只给 `ModelBinding`，不需要知道 env var、default model、provider client 类型。
- `agent/profile/spec.rs` 应是深 Module：调用方只拿 `AgentExecutionSpec`，不需要知道 frontmatter 兼容细节。
- `agent/runtime/sub_agent.rs` 或 `agent_run/dispatcher.rs` 应是深 Module：父 loop 只调用 `dispatch(TaskInvocation)`，不直接管理 child session、permission merge、handoff、usage。
- `providers/transform.rs` 应集中 provider quirks，提升 Locality。新增 provider 时必须在该 Module 增加 capability/transform 测试。
- `agent/runtime/goal.rs` 或 `goal/supervisor.rs` 应是深 Module：TUI / CLI 只发起、取消、查询 Goal；停止策略、completion gate、继续执行 prompt、budget 暂停和 resume replay 都在 supervisor 内部。

---

## 核心数据契约

### ModelBinding

```rust
pub struct ModelBinding {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
}
```

约束：

- 字符串格式接受 `provider/model`，其中 model 允许继续包含 `/`，对齐 opencode `Provider.parseModel()`。
- `variant` 不拼进 `model_id`，由 provider transform / request option 单独处理。
- `codex` managed runtime 不进入本契约，保持外部进程路径。

### AgentExecutionSpec

```rust
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

pub enum ToolSelection {
    Inherit,
    Allow(Vec<String>),
    Deny(Vec<String>),
}

pub enum ApprovalRoutingSpec {
    Layer1Human,
    SessionPreApproved,
}

pub struct AgentPermissionSpec {
    pub allowed_tools: BTreeSet<String>,
    pub denied_tools: BTreeSet<String>,
    pub allowed_source_slugs: BTreeSet<String>,
    pub approval_routing: ApprovalRoutingSpec,
    pub may_spawn_sub_agents: bool,
}

pub struct AgentExecutionSpec {
    pub name: String,
    pub description: String,
    pub mode: AgentMode,
    pub model: Option<ModelBinding>,
    pub system_prompt: String,
    pub tools: ToolSelection,
    pub permission: AgentPermissionSpec,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_steps: Option<u32>,
}
```

兼容规则：

- 旧 `model: default` / `model: fast` 保持为 `model_preference`，不强行解析。
- 只有 `model` 形如 `provider/model` 时 lift 为 `ModelBinding`。
- 现有 `.libra/agents/*.md` 没有 `mode` 时默认 `Primary`，避免旧 profile 突然出现在 sub-agent 列表。
- `tools` 先复用现有 allow-list；权限 ruleset 进入 OC-Phase 3 后再强制。
- `AgentPermissionSpec` 是 default build 可用的 profile 配置 schema，字段形状刻意对齐 feature-gated `agent_run::permission::AgentPermissionProfile`。OC-Phase 3 才做转换，不允许 default build 直接依赖 `subagent-scaffold` 模块。
- `ToolSelection::Inherit` 对 primary agent 表示沿用 session allowed tools；对 sub-agent 表示默认空 allow-list。任何 `Deny` 都优先于 allow 和父会话继承。

### AnyCompletionModel

不能使用：

```rust
// 不可落地：CompletionModel 不是 object-safe
Box<dyn CompletionModel>
```

第一版应使用 enum Adapter：

```rust
pub enum AnyCompletionModel {
    Gemini(gemini::CompletionModel),
    OpenAi(openai::CompletionModel),
    Anthropic(anthropic::CompletionModel),
    DeepSeek(deepseek::CompletionModel),
    Kimi(kimi::CompletionModel),
    Zhipu(zhipu::CompletionModel),
    Ollama(ollama::CompletionModel),
    #[cfg(feature = "test-provider")]
    Fake(fake::CompletionModel),
}

pub enum AnyCompletionRawResponse {
    Gemini(gemini::GenerateContentResponse),
    OpenAi(openai_compat::ChatResponse),
    Anthropic(anthropic::AnthropicResponse),
    DeepSeek(openai_compat::ChatResponse),
    Kimi(openai_compat::ChatResponse),
    Zhipu(openai_compat::ChatResponse),
    Ollama(openai_compat::ChatResponse),
    #[cfg(feature = "test-provider")]
    Fake(fake::FakeRawResponse),
}
```

约束：

- `AnyCompletionModel` 实现 `CompletionModel<Response = AnyCompletionRawResponse>`。
- `AnyCompletionRawResponse` 实现 `CompletionUsage`，内部 match 到各 provider response。
- `set_run_id()` 必须 match 转发，保持现有 workflow 链接能力。
- `run_tui_with_model(AnyCompletionModel, ...)` 不需要改泛型签名。

### TaskInvocation

```rust
pub struct TaskInvocation {
    pub description: String,
    pub prompt: String,
    pub subagent_type: String,
    pub task_id: Option<String>,
}

pub struct TaskResult {
    pub task_id: String,
    pub agent_name: String,
    pub provider_id: String,
    pub model_id: String,
    pub final_text: String,
    pub steps_used: u32,
    pub usage: CompletionUsageSummary,
}
```

约束：

- `task_id` 复用只允许同一 parent thread。
- `subagent_type` 必须解析到 `AgentExecutionSpec`，且 `mode` 为 `Subagent` 或 `All`。
- 默认 `max_subagent_depth = 1`，默认 `max_concurrent_subagents = 1`。
- 子 agent 默认不允许再调用 `task`。

### ContextHandoff

```rust
pub struct ContextHandoff {
    pub summary: String,
    pub recent_tail: Vec<ContextFrameSegment>,
    pub attachment_refs: Vec<ContextAttachmentRef>,
    pub source_frame_id: Uuid,
    pub remaining_budget_tokens: u64,
}
```

约束：

- `summary` 由 `compaction` agent 生成，模板必须严格匹配 [OC-Phase 4 的 8 段固定布局](#literal-summary-template)（Goal / Constraints & Preferences / Progress(Done, In Progress, Blocked) / Key Decisions / Next Steps / Critical Context / Relevant Files）。
- `recent_tail` 取自最新 `ContextFrameEvent`，不是 raw transcript 全量复制。
- 写入 `CompactionEvent`，可由 session JSONL replay。

---

## Permission Ruleset 与 Approval 反馈协议

opencode 的多 agent 编排能落地的关键不是「谁能调谁」，而是 **`Permission.Ruleset`、`evaluate()`、`merge()`、三态 Reply** 这一组语义足够明确、足够可测试的协议。本节把这套协议的 Libra 等价形态固定下来，作为 OC-Phase 2 / 3 / 5 共同依赖的合同。

### 类型与语义

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionRule {
    /// permission key, e.g. "edit" / "bash" / "task" / "external_directory"
    pub permission: String,
    /// glob pattern; "*" means any. ~/, $HOME/ are expanded at parse time.
    pub pattern: String,
    pub action: PermissionAction,
}

pub type PermissionRuleset = Vec<PermissionRule>;

/// User reply to a "ask" prompt. Mirrors opencode's three-state reply.
pub enum PermissionReply {
    Once,
    Always,
    Reject { feedback: Option<String> },
}
```

约束：

- `Ruleset` 是 **顺序敏感的扁平向量**，不是 set。`merge` 等价于按顺序串联：`merge(a, b, c) == [a..., b..., c...]`，**不去重、不排序**。
- `evaluate(permission, pattern, ...rulesets)` 必须返回 `findLast(rule => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern))`。**最后命中的规则胜出**，与 opencode `permission/evaluate.ts` 保持一致。
- 任意一个 `permission` 上的 `Deny` 规则只要在 ruleset 中存在且能 match 当前 pattern 集合中**任意一个** pattern，整次 ask 立即失败（与 `Permission.ask()` 在 opencode 一致）。
- "Ask" 表示需要交互；如果所有 pattern 都通过 allow，则跳过交互。
- pattern 的 home 展开（`~`, `~/`, `$HOME`, `$HOME/`）在 `from_config()` 阶段完成，runtime 不再展开。

### Reply 三态与持久化

| Reply | 行为 | 持久化范围 |
|-------|------|------------|
| `Once` | 当次 ask 通过；不写 approved table | 仅当前 ask |
| `Always` | 当次通过；按 `request.always` 中的每个 pattern 追加一条 `{permission, pattern, action: Allow}` 到 `approved` ruleset；同时把 pending 队列里所有「permission 相同、所有 pattern 都被新 approved 包含」的请求一并放行 | project-scope（写入 SQLite，重启后保留） |
| `Reject { feedback }` | 当次失败；同 session 的其它 pending ask 全部以 Reject 失败；如果 `feedback` 非空，错误以 `PermissionCorrectedError { feedback }` 形态返回模型，模型必须把 feedback 视为用户指令调整后续动作 | 不持久化，但失败链一致 |

Libra 现有 `ApprovalCachePolicy`（[src/internal/ai/sandbox/mod.rs:136](../../src/internal/ai/sandbox/mod.rs)）覆盖了 TTL、Scope、Sensitivity Tier，但**没有 `Always` 与 `request.always[]` 的 pattern-level 持久化合同**。OC-Phase 2 必须在 `ApprovalCachePolicy` 之上新增 `ApprovedRuleset` 投影：

```rust
pub struct ApprovedRuleset {
    pub project_id: ProjectId,
    pub rules: PermissionRuleset, // 从 SQLite approved_permission 表读取
}
```

migration（OC-Phase 5）新增 `approved_permission` 表，列：`project_id` / `permission` / `pattern` / `created_at`。版本号必须晚于 entire.md 的 `2026050303_agent_capture`。

### Sub-Agent 权限继承算法

opencode 的 task tool [tool/task.ts:73-101](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/task.ts) 用了一个非平凡的过滤器：

```ts
permission: [
  ...(parent.permission ?? []).filter(
    (rule) => rule.permission === "external_directory" || rule.action === "deny",
  ),
  ...(canTodo ? [] : [{ permission: "todowrite", pattern: "*", action: "deny" }]),
  ...(canTask ? [] : [{ permission: id, pattern: "*", action: "deny" }]),
  ...(cfg.experimental?.primary_tools?.map(item => ({pattern:"*", action:"allow", permission:item})) ?? []),
]
```

翻译为 Libra 算法（必须逐字实现，不要简化）：

```text
fn child_ruleset(parent: &PermissionRuleset, sub_spec: &AgentExecutionSpec) -> PermissionRuleset:
  1. base = []
  2. for rule in parent:
       if rule.permission == "external_directory" OR rule.action == Deny:
         base.push(rule)               # 继承父的 external_dir 规则与 deny 规则
  3. base.extend(sub_spec.permission)  # 子 agent 自己的规则（全部）
  4. if "task" not in sub_spec.permission allowed:
       base.push({permission:"task", pattern:"*", action:Deny})  # 默认禁套娃
  5. if "todowrite" not in sub_spec.permission allowed:
       base.push({permission:"todowrite", pattern:"*", action:Deny})
  6. extend with primary_tools allow rules (experimental, default 空)
  7. return base
```

特别注意：

- **不**把父的 `Allow` 规则继承下来。子 agent 的能力由它自己 spec 定义，父只贡献 `external_directory`（路径范围）+ `Deny`（硬禁令）。
- 子 agent 不能为自己批准 ask；任何 `Ask` action 必须冒泡回父 session（即 controller / Layer 1 human）的 permission service。OC-Phase 3 的 `SubAgentDispatcher::dispatch()` 必须把 child session 的 `permission.ask()` 接到 **父 session 的 PermissionService instance**，而不是创建独立 service。
- 子 agent 想 `apply_patch` / `write` / `edit` 时，必须父 spec 的 `permission` 显式 allow `edit` permission；OC-Phase 3 默认 `edit: deny`。

测试用例（[tests/ai_subagent_permission_test.rs]）必须覆盖：

| 场景 | 父 ruleset | 子 spec | 期望 effective ruleset |
|------|-----------|--------|------------------------|
| 父全开，子限制 read | `[{*:allow}]` | `{permission: [{read:allow},{*:deny}]}` | `[{*:deny}, {read:allow},{*:deny}]` → evaluate(`read`)=allow，evaluate(`shell`)=deny |
| 父禁 shell，子未提 | `[{shell:deny}]` | `{permission: []}` | `[{shell:deny},{task:*:deny},{todowrite:*:deny}]` |
| 父限 ext_dir，子无 ext_dir | `[{external_directory: {/tmp/*: allow}}]` | `{permission: []}` | 父的 ext_dir 规则被继承 |
| 子尝试套娃 | 任意 | `{permission: [{task:*:allow}]}` | 子规则保留，但被显式拒绝写入 `code.multi_agent.max_subagent_depth=1` 检查 |
| 子提升 edit | `[{edit:deny}]` | `{permission: [{edit:allow}]}` | `[{edit:deny}, {edit:allow}, {task:*:deny}, {todowrite:*:deny}]` → evaluate(`edit`) findLast = allow，**但 OC-Phase 3 必须额外 enforce「子 ruleset 中的 allow 不能扩大父 deny」的 spec 时检查**（在 dispatch 入口断言：若父 effective=deny 且子 effective=allow，则 dispatch 失败并写 `PermissionEscalationDenied` event） |

最后一条是 opencode 没有的额外保护——opencode 让 `findLast` 直接覆盖父 deny。Libra 的安全模型不允许这样：父 deny 必须始终胜出。OC-Phase 3 因此必须在 `effective_ruleset` 计算之后，对每条 `permission`+`pattern` 组合都跑一次：

```text
for (perm, pattern) in cartesian(perm_keys, pattern_samples):
  parent_action = evaluate(perm, pattern, parent_ruleset)
  effective_action = evaluate(perm, pattern, effective_ruleset)
  if parent_action == Deny and effective_action != Deny:
    fail with PermissionEscalationDenied { perm, pattern }
```

### Libra 当前 `ApprovalCachePolicy` 与 ruleset 的对接

| 现有概念 | OC-Phase 引入 | 转换规则 |
|----------|--------------|----------|
| `AskForApproval { Never, OnFailure, OnRequest, UnlessTrusted }` | session-level default | `Never` → ruleset 添加 `[{*:allow}]` 兜底；`OnRequest` → `[{*:ask}]`；`UnlessTrusted` → 维持 `Ask` 但工具 `trust_tier == Trusted` 时 fast-path allow；`OnFailure` 当前语义保留作为 **post-tool retry** 决策，不进入 pre-tool ruleset |
| `ApprovalScope { Session, Project, User }` | `Always` reply 写入位置 | `Project`（默认） → `approved_permission` 表；`Session` → in-memory ruleset；`User` 暂不支持，OC-Phase 5 评估 |
| `ApprovalSensitivityTier` | rule pattern 风格 | 高敏感工具映射到 `permission = sensitive_tool_name`，低敏感映射到 `permission = "*"` 子树 |

OC-Phase 2 的退出标准必须包含「`AskForApproval::OnRequest` 在新 ruleset 路径下行为等价于旧路径」的 byte-level fixture（捕获若干 fake provider script 的 stdout / event 序列做对照）。

---

## Tool Registry 预过滤合同

opencode 在把 tool list 交给模型前先用 `Permission.disabled(tools, ruleset)` 把"任何 pattern=`*` action=`deny`"的工具从 schema 中**完全移除**：模型甚至看不到这些工具的 name。这是关键的可用性 / 安全交叉点——一旦模型看到工具，它会反复尝试调用，而 deny-after-call 会在 transcript 中留下大量噪音并消耗 tokens。

### Libra 落点

| 现有 | OC-Phase 改动 |
|------|--------------|
| [src/internal/ai/tools/registry.rs](../../src/internal/ai/tools/registry.rs) `ToolRegistry` / `ToolHandler` | 新增 `ToolRegistry::available_for(agent: &AgentExecutionSpec, ruleset: &PermissionRuleset) -> Vec<ToolSpec>` |
| 现有 `ToolRegistryBuilder` | 不变，但所有 `dispatch()` / `available_tools()` 入口改为接受 ruleset 参数 |
| [src/internal/ai/agent/runtime/tool_loop.rs](../../src/internal/ai/agent/runtime/tool_loop.rs) | tool list 的构造放到 loop 启动前；ruleset 变化（approval 写入）触发 list 重建 |

### 算法

```text
fn disabled(all_tools: &[&str], ruleset: &PermissionRuleset) -> HashSet<String>:
  let edit_tools = ["apply_patch", "write_file", "patch"]; // Libra 等价
  let mut disabled = HashSet::new();
  for tool in all_tools:
    let permission_key = if edit_tools.contains(tool) { "edit" } else { tool };
    let last = ruleset.iter().rev().find(|r| Wildcard::match(permission_key, &r.permission));
    if let Some(rule) = last:
      if rule.pattern == "*" && rule.action == Deny:
        disabled.insert(tool.to_string());
  return disabled;
```

`ToolRegistry::available_for()` 返回 `all_tools.filter(|t| !disabled.contains(t))`。

### 测试

- 给定 `[{edit:deny}]`，`apply_patch` / `write_file` / `patch` 全部不出现在 schema 中。
- 给定 `[{edit:deny}, {edit: { "src/**": "allow" }}]`，工具仍然出现（因为 `pattern=*` 的 deny 没有命中）。
- 给定 `[{*:deny}, {grep:allow}]`，只 `grep` 出现在 schema 中（每个工具单独 evaluate；`grep` 的最后命中是 allow）。
- flag-off 下 `available_for()` 等价于现有 `available_tools()` 的输出（fixture）。

### 与 OC-Phase 5 的耦合

`/agents` slash command 必须显示当前 agent 的 effective ruleset 与 disabled tool 列表。这是用户配置出错时唯一直观的诊断面。

---

## Codex-like Goal 模式

### GoalSpec / GoalState / GoalEvent

Codex-like Goal 模式是一层 runtime contract，不是 agent prompt 风格。它的核心语义是：**只要 Goal 仍为 active，普通 assistant final answer 不能让会话进入 idle；runtime 必须继续推进、要求用户输入、或等待显式取消。**

```rust
pub struct GoalSpec {
    pub goal_id: Uuid,
    pub thread_id: String,
    pub session_id: String,
    pub objective: String,
    pub acceptance_criteria: Vec<GoalCriterion>,
    pub constraints: Vec<String>,
    pub evidence_policy: GoalEvidencePolicy,
    pub budget: GoalBudget,
    pub created_at: DateTime<Utc>,
    pub created_by: GoalActor,
}

pub struct GoalCriterion {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub verifier_hint: Option<String>,
}

pub enum GoalStatus {
    Active,
    Running,
    AwaitingUser,
    Blocked,
    CompletionClaimed,
    Completed,
    Cancelled,
}

pub struct GoalState {
    pub spec: GoalSpec,
    pub status: GoalStatus,
    pub plan: Vec<GoalPlanStep>,
    pub completed_criteria: BTreeSet<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub blockers: Vec<GoalBlocker>,
    pub last_assistant_summary: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub enum GoalEvent {
    Created(GoalSpec),
    PlanUpdated { steps: Vec<GoalPlanStep> },
    StepStarted { step_id: String },
    StepCompleted { step_id: String, evidence_refs: Vec<GoalEvidenceRef> },
    ProgressRecorded { summary: String, evidence_refs: Vec<GoalEvidenceRef> },
    Blocked { reason: GoalBlockReason, requested_input: Option<String> },
    CompletionClaimed(GoalCompletionClaim),
    CompletionRejected { missing: Vec<String>, reason: String },
    Completed(GoalCompletionReport),
    Cancelled { reason: String, cancelled_by: GoalActor },
}
```

约束：

- Goal 只能由显式入口创建：`libra code --goal "..."`、TUI `/goal start ...`、Code Control `goal.start`，或 automation 明确声明 `goal = true`。普通用户消息不得被自动推断成 Goal。
- `GoalEvent` 写入现有 `SessionEvent` envelope，新增 `SessionEvent::Goal(GoalEventEnvelope)`；reader 必须 unknown-event-safe。
- `GoalStatus::Completed` 只能由 runtime 写入，模型不能直接写最终状态。
- `GoalStatus::Cancelled` 只能由用户 / automation owner / control lease owner 显式触发；budget 用尽、max turn、重复调用、provider error 都不能伪装成 completed 或 cancelled。
- `GoalStatus::AwaitingUser` 是"暂停推进等待外部输入"，不是完成。恢复输入后 supervisor 必须继续 active Goal。
- `GoalStatus::Blocked` 只用于 runtime 已知缺口，例如 approval 被拒、预算需要用户确认、scope 不足、外部依赖不可达；blocked Goal 在 `/goal status` 中仍显示为未完成。

### Goal 工具

Goal 模式下 registry 额外暴露两个 runtime 工具：

```rust
pub struct UpdateGoalProgressArgs {
    pub summary: String,
    pub completed_criteria: Vec<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub next_steps: Vec<String>,
}

pub struct SubmitGoalCompleteArgs {
    pub summary: String,
    pub completed_criteria: Vec<String>,
    pub evidence_refs: Vec<GoalEvidenceRef>,
    pub verification: Vec<GoalVerificationRecord>,
    pub residual_risks: Vec<String>,
}
```

约束：

- `update_goal_progress` 非 terminal tool，只记录进度和证据。
- `submit_goal_complete` 是 Goal 模式的唯一 completion claim tool；它不直接终止会话，只把状态推进到 `CompletionClaimed`。
- `submit_task_complete` 在 Goal 模式下仍可用于子任务 / orchestrator task，但不能结束 active Goal。
- Goal completion claim 必须至少包含：目标摘要、所有 required criteria 的满足说明、证据引用、验证命令或人工验证说明、剩余风险。
- evidence ref 第一版允许引用 `ContextFrameEvent`、tool call id、文件路径 + content hash、测试输出 attachment、`AgentRunEvent`。不得把大段 transcript 原文塞进 GoalEvent。

### GoalSupervisor 停止策略

```rust
pub enum GoalLoopDecision {
    Continue { prompt: String },
    AwaitUser { question: String },
    Completed { report: GoalCompletionReport },
    Cancelled,
}

pub struct GoalSupervisor {
    pub stop_policy: GoalStopPolicy,
    pub verifier: GoalVerifier,
    pub continuation_prompt: GoalContinuationPromptBuilder,
}

pub enum GoalStopPolicy {
    Normal,
    GoalBound { goal_id: Uuid },
}
```

运行规则：

1. Goal 创建后先写 `GoalEvent::Created`，再注入 system/user preamble：当前目标、验收标准、约束、可用工具、完成协议。
2. 每次 `run_tool_loop` 返回后，supervisor replay 最新 `GoalState`。
3. 如果没有 active Goal，保持现有行为。
4. 如果 active Goal 仍未完成，且 assistant 只是给出 final text，supervisor 追加 `GoalEvent::ProgressRecorded`，构造 continuation prompt，重新进入 `run_tool_loop`。
5. 如果模型调用 `submit_goal_complete`，supervisor 调用 `GoalVerifier`。通过则写 `GoalEvent::Completed` 并允许会话进入 idle；失败则写 `CompletionRejected` 并继续执行。
6. 如果达到单轮 `max_turns`、repeat abort、provider error 或 context overflow，supervisor 不结束 Goal；它要么压缩 context 后继续，要么进入 `AwaitingUser` / `Blocked` 并给出最小可执行问题。
7. 如果 budget 达到 warn 阈值，supervisor 继续但提示；达到 hard cap 时写 `Blocked { reason: BudgetApprovalRequired }`，等待用户追加预算或取消。
8. 如果用户输入 `/goal cancel` 或 control API cancel，写 `Cancelled`，停止 Goal-bound loop。

"不完成不停止"在实现上定义为：

- `Completed` 和 `Cancelled` 是 terminal boundary。
- `AwaitingUser` 和 `Blocked` 是 non-terminal pause boundary；UI 可以停止当前 turn 等用户输入，但 Goal 仍 active，不能显示为 completed。
- `max_continuation_loops` 只是单次无人值守推进上限；命中后写 `Blocked { reason: LoopLimitNeedsUser }`，不能写 `Completed`。
- 普通 assistant final text、`submit_task_complete`、turn budget、repeat abort、context compaction 都不是 Goal terminal boundary。

### GoalVerifier

第一版 verifier 是 deterministic + evidence-based，不调用模型做最终裁决：

- required acceptance criteria 全部在 `completed_criteria` 中出现。
- 每个 required criterion 至少有一个 evidence ref。
- 如果有文件改动，必须有 `git status --short` 或等价 VCS 状态 evidence。
- 如果 `verification` 为空，completion 被拒绝，除非 goal 标记为 documentation-only / analysis-only 且 evidence policy 允许人工说明。
- 若最近一轮 tool result 有 failed / denied / timeout，completion 被拒绝，除非 residual risk 明确解释且 criterion 不依赖该工具。
- 对实现类 Goal，若没有 successful write 或明确 `no_changes_needed` evidence，completion 被拒绝。

后续可增加 LLM reviewer，但只能作为 advisory evidence；runtime verifier 仍是最终 gate。

### 用户接口

| 入口 | 行为 |
|------|------|
| `libra code --goal "<objective>"` | 启动 TUI 并创建 active Goal |
| `/goal start <objective>` | 当前 session 创建 Goal；若已有 active Goal，要求先 complete 或 cancel |
| `/goal status` | 展示 objective、criteria、当前步骤、证据、blocker、预算 |
| `/goal criteria add <text>` | 用户显式增加验收标准，写 `PlanUpdated` / spec revision event |
| `/goal cancel <reason>` | 显式取消 active Goal |
| `/budget goal approve <amount>` | 对 budget blocker 追加用户批准的 Goal 预算 |
| `libra code --resume <thread>` | replay GoalState；若 active Goal 未完成，默认显示恢复提示，用户确认后继续 Goal-bound loop |
| Code Control `goal.start/status/cancel` | automation 使用同一 contract；必须持有当前 controller lease |

TUI 状态不新增"看起来完成但其实未完成"的 idle 状态。active Goal 的底栏至少显示：goal id 短码、status、当前 step、tokens/cost、blocker 或 next action。

---

## 实施路线

### OC-Phase 0：契约冻结与错误路径排除

**目标**：把本文转成 PR 可引用的契约，先修正不可编译或缺上下文的设计。

**文件落点**

| 文件 | 改动 |
|------|------|
| `docs/improvement/opencode.md` | 本文作为权威横向计划 |
| `docs/improvement/agent.md` | 增加本文链接，标明 OC-Phase 3 与 CEX-S2-12 共用 runtime schema |
| `docs/improvement/entire.md` | 不改 schema；如需交叉链接，只说明本文不写 `agent-traces` / `agent_session` / `agent_checkpoint` |
| `src/internal/ai/agent/profile/spec.rs` | 新建，仅定义 `AgentExecutionSpec` / `ModelBinding` / `AgentMode` / `ToolSelection` / `AgentPermissionSpec`，不接 runtime |
| `src/internal/ai/agent/profile/mod.rs` | re-export spec |
| `tests/ai_agent_profile_spec_test.rs` 或 profile 模块测试 | schema round-trip，旧 `model_preference` 兼容 |

**退出标准**

- `cargo test agent_profile` 通过。
- 默认 build 无行为变化。
- 文档明确禁止 `Box<dyn CompletionModel>` 路线。
- 文档明确 Task tool 需要 `SubAgentDispatcher`，不是普通 handler-only。
- 文档明确保留 entire.md 的版本管理所有权边界：内部 sub-agent 不写 `refs/libra/agent-traces`。

**回滚边界**

- 纯 schema / docs，可直接 revert，无 runtime 状态。

### OC-Phase 1：Provider Runtime Adapter 与 Capability Matrix

**目标**：让 Libra 能在运行时按 `(provider_id, model_id)` 构建模型，同时保持现有 generic fast path。

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/providers/runtime.rs` | 新增 `AnyCompletionModel` / `AnyCompletionRawResponse` |
| `src/internal/ai/providers/factory.rs` | 新增 `ProviderFactory`，输入 `ModelBinding` 和 env lookup，输出 `AnyCompletionModel` |
| `src/internal/ai/providers/capability.rs` | 新增静态 `ModelCapability` 表和 provider 默认能力 |
| `src/internal/ai/providers/mod.rs` | re-export runtime / factory / capability |
| `src/command/code.rs` | 抽出现有 provider match 的共享构建逻辑；主 TUI 可先继续走旧 match |
| `tests/ai_provider_factory_test.rs` | fake provider + env lookup 单测 |

**设计细节**

- `ProviderFactory` 不直接读 `CodeArgs`。命令层把 api base、Ollama compact-tools、dotenv map 变成 `ProviderBuildOptions`。
- `AnyCompletionModel` 的 Response 统一为 `AnyCompletionRawResponse`，这样 `run_tool_loop` 的 `M::Response: CompletionUsage` 继续成立。
- `ModelCapability` 第一版静态维护，字段包括 `supports_tool_calls`、`supports_streaming`、`supports_vision`、`supports_reasoning`、`supports_interleaved`、`context_window`、`output_limit`、`cost`。
- capability 不准作为安全边界，只作为提前拒绝和用户错误提示。

**测试**

- `build_any_completion_model(fake/default)` 可返回 fake model。
- unknown provider / unknown model 给 user-friendly error，带 available suggestions。
- `AnyCompletionRawResponse::usage_summary()` 覆盖 fake、OpenAI-compatible、Anthropic、Gemini response。
- `run_tui_with_model(AnyCompletionModel, ...)` 编译通过。

**退出标准**

- `cargo test ai_provider_factory`
- `cargo test --lib any_completion`
- `cargo clippy --all-targets --all-features -- -D warnings`

### OC-Phase 2：Agent Profile 可执行化

**目标**：让 `.libra/agents/*.md` 的 agent 可以绑定 provider/model，但暂不启用 sub-agent。

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/agent/profile/parser.rs` | 解析 `mode`、`variant`、`temperature`、`top_p`、`steps`、`model: provider/model` |
| `src/internal/ai/agent/profile/router.rs` | 返回 `AgentExecutionSpec` 或 `AgentRoute`，保留 `get(name)` |
| `src/internal/tui/app.rs` | slash command 选中 agent 时，不再只拼 system prompt；同时可切换 model binding |
| `src/command/code.rs` | 启动时构建 `ProviderFactory` 并交给 TUI runtime |
| `tests/ai_agent_profile_test.rs` | frontmatter 兼容与模型绑定测试 |
| `tests/ai_agent_route_test.rs` | slash command agent model override 测试 |

**行为规则**

- 没有 `model` 的 agent 继续使用 CLI/default model。
- `--provider` / `--model` 显式传入时作为 session default；command-selected agent 的 `model` 只在 `code.multi_agent.enabled = true` 或 `--agent <name>` 明确选择时生效，避免旧 slash command 静默换 provider。
- plain user message 不自动 route，沿用现状；隐式路由留到 OC-Phase 5。

**测试**

- 旧 embedded profiles 全部可 parse。
- project `.libra/agents/planner.md` 覆盖 embedded planner，且 `model: anthropic/claude-...` lift 为 `ModelBinding`。
- `model: fast` 仍保留为 preference，不触发 provider factory。
- 选中 agent 后 usage context 使用 agent provider/model。

**退出标准**

- 未配置 agent model 时，`libra code` 行为与当前一致。
- 配置 agent model 且显式选择 agent 时，provider/model 出现在 `agent_usage_stats` 对应行。

### OC-Phase 3：Task 工具与单 sub-agent dispatcher

**目标**：在 feature flag 后开放一个 `task` 工具，使父 agent 能派发一个 sub-agent 并拿回结果。此 phase 对齐 agent.md `CEX-S2-12 Single sub-agent behind flag`。

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/tools/spec.rs` | 增加 `ToolSpec::task()` schema |
| `src/internal/ai/agent/runtime/tool_loop.rs` | `ToolLoopConfig` 增加 `subagent_dispatcher: Option<Arc<dyn SubAgentDispatcher>>`；tool name 为 `task` 时走 dispatcher |
| `src/internal/ai/agent/runtime/sub_agent.rs` 或 `src/internal/ai/agent_run/dispatcher.rs` | 新增 `SubAgentDispatcher` Interface 与 implementation |
| `src/internal/ai/agent_run/event.rs` | 复用现有 `AgentRunEvent`，必要时追加 `Delegated` / `BudgetExceeded` 等 snake_case variant，保持 unknown-event-safe envelope |
| `src/internal/ai/agent_run/permission.rs` | 父权限与子权限 merge：deny 优先，子权限不可扩大父权限 |
| `src/command/code.rs` | `code.multi_agent.enabled` 为 true 时注册 task schema 与 dispatcher |
| `tests/ai_subagent_single_test.rs` | fake provider E2E |
| `tests/ai_subagent_flag_off_regression_test.rs` | flag-off 行为等价 |

**SubAgentDispatcher Interface**

`SubAgentDispatcher` 是这一阶段唯一新增的运行时 trait。它**不是**普通 `ToolHandler`：handler 只能看到工具参数与 sandbox，看不到父会话 history、父 model、父 usage recorder、permission service、abort handle。dispatcher 必须挂在 `tool_loop` 一侧，处于 `ToolLoopConfig` 的同级位置。

```rust
pub struct DispatchContext<'a> {
    pub parent_thread_id: &'a str,
    pub parent_session_id: &'a SessionId,
    pub parent_agent: &'a AgentExecutionSpec,
    pub parent_ruleset: &'a PermissionRuleset,
    pub parent_model_binding: &'a ModelBinding,
    pub parent_message_id: MessageId,         // 触发 task 的 assistant message id
    pub permission_service: &'a PermissionService,
    pub session_store: &'a SessionJsonlStore,
    pub provider_factory: &'a ProviderFactory,
    pub usage_recorder: &'a UsageRecorder,
    pub context_frame_loader: &'a ContextFrameLoader,
    pub abort_token: AbortToken,              // 父 abort 树的子节点
    pub depth: u8,                            // 调用栈深度，0 = 主 agent，1 = 第一层 sub
}

pub trait SubAgentDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        ctx: DispatchContext<'a>,
        invocation: TaskInvocation,
        entry_kind: TaskEntryKind,
    ) -> BoxFuture<'a, Result<TaskResult, TaskFailure>>;
}

pub enum TaskEntryKind {
    /// 父 agent 模型主动 emit task tool call
    LlmInitiated,
    /// 用户在 TUI / API 显式提交 SubtaskPart（对应 opencode 的 MessageV2.SubtaskPart）
    UserInitiated { bypass_permission_ask: bool },
}

pub enum TaskFailure {
    FeatureDisabled,                                      // step 1: code.multi_agent.enabled = false（Libra 新增）
    UnknownSubagent { name: String, suggestions: Vec<String> },
    DepthExceeded { current: u8, limit: u8 },
    ConcurrencyExceeded { current: u32, limit: u32 },
    PermissionEscalationDenied { permission: String, pattern: String },
    SafetyDenied(SafetyDecisionDenial),
    ApprovalRejected { feedback: Option<String> },        // 父 PermissionService 返回 Reject
    BudgetExceeded(BudgetExceededReason),
    ContextHandoffFailed(ContextHandoffError),
    ProviderError(CompletionError),
    ChildToolLoopFailed(ToolLoopError),
    Cancelled { source: CancellationSource },
    Timeout { wall_clock_ms: u64 },
}
```

**dispatcher 主流程**（必须严格按此顺序，每步对应一条 PR 测试）：

```text
 1. validate feature flag (code.multi_agent.enabled)
 2. validate ctx.depth + 1 <= max_subagent_depth
 3. validate concurrent count + 1 <= max_concurrent_subagents
 4. resolve subagent_type via AgentExecutionSpec registry
    - mode must be Subagent or All; Primary -> UnknownSubagent error
 5. SafetyDecision::evaluate(SubAgentSpawn { name, prompt_digest })
 6. compute effective_ruleset via child_ruleset(parent.ruleset, sub_spec)
 7. assert no permission escalation (Permission Escalation Gate)
 8. PermissionService.ask(SubAgentSpawn) ONLY if entry_kind == LlmInitiated
    - UserInitiated { bypass_permission_ask: true } 跳过此步
 9. build child AnyCompletionModel via provider_factory
10. resolve parent ContextFrame -> ContextHandoff (OC-Phase 4)
11. create-or-resume child AgentRun + child JSONL session under `libra code` namespace
12. write AgentRunEvent::Spawned (with prompt_digest, model binding, depth)
13. spawn child run_tool_loop:
      - registry = ToolRegistry::available_for(sub_spec, effective_ruleset)
      - permission_service forwarded (子 ask -> 父 service)
      - usage_recorder.attach(agent_run_id = child_run_id, agent_name = sub_spec.name)
      - abort = abort_token.child()
14. on completion / failure:
      - write AgentRunEvent::Completed | Failed
      - flush usage rows
15. return TaskResult { task_id, ..., final_text, usage }
```

**两类入口（Two Entry Points）**

opencode 在 [session/prompt.ts:548 `handleSubtask`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/prompt.ts) 与 task tool（LLM-initiated）共用同一个 dispatcher，区别只在 `bypassAgentCheck`。Libra 必须显式区分：

| 入口 | TaskEntryKind | permission ask | 用例 |
|------|---------------|----------------|------|
| 父 agent 模型 emit `task(subagent_type=...)` | `LlmInitiated` | 父 PermissionService.ask({permission:"task", patterns:[subagent_type]}) | 父 agent 用 task 工具委派 |
| 用户在 TUI 输入 `/task <agent> <prompt>`、Code Control `task.dispatch`、或带 SubtaskPart 的用户消息 | `UserInitiated { bypass_permission_ask: true }` | 跳过 permission ask（用户已经显式选择） | 用户直接派发 |

`UserInitiated` 仍要走 SafetyDecision、depth、concurrency 检查；只跳过对话框 ask。两条入口在 OC-Phase 3 都必须有 fake-provider E2E 测试（`tests/ai_subagent_llm_initiated_test.rs` + `tests/ai_subagent_user_initiated_test.rs`）。

**Cancel / Abort 传播合同**

opencode 最近（PR #25798，2026-05-04）专门修了 task tool 的 cancel 语义。修复前 `TaskPromptOps.cancel(sessionID)` 是同步 `() => void`，父 abort fire-and-forget；修复后改为 `Effect.Effect<void>`（异步可 await），并通过 `EffectBridge.make()` 把 cancel fork 到独立 runtime：

```ts
// task.ts (post-#25798)
const runCancel = yield* EffectBridge.make()
const cancel = ops.cancel(nextSession.id)            // returns Effect, not void
function onAbort() { runCancel.fork(cancel) }        // fork into bridge runtime
ctx.abort.addEventListener("abort", onAbort)
// On exit:
if (Exit.hasInterrupts(exit)) yield* cancel          // AWAIT cancel completion before release
```

Libra 必须采用同等语义。`tokio_util::sync::CancellationToken` 树的核心约束不仅是「父 cancel triggers 子 token」，还包括「父 abort 必须 await 子 cancel **完整 completion**」，否则 child JSONL 可能漏写 `AgentRunEvent::Failed { reason: Cancelled }`：

```text
AbortToken (root, owned by `libra code` runtime)
 ├── parent session AbortToken
 │    └── parent run_tool_loop AbortToken
 │         └── child SubAgentDispatcher::dispatch AbortToken (passed via ctx.abort_token.child())
 │              └── child run_tool_loop AbortToken
```

约束（粗体为 #25798 修复后必须 enforce 的项）：

- 父 cancel（`Ctrl-C`、`/cancel`、Code Control `code.cancel`、`prompt.cancel(parent_id)`）**立即** trigger 子 token，并用 `tokio::select!` / `CancellationToken::cancelled()` 让 reqwest / tokio I/O 短路。
- **dispatcher 在 cleanup 阶段必须 `yield* cancel` 等价的「await 子 cancel 完成」**：
  ```rust
  // 伪代码：dispatcher cleanup 必须实现
  let mut child_handle = spawn_child_tool_loop(...);
  tokio::select! {
      result = &mut child_handle => result,
      _ = parent_abort.cancelled() => {
          child_abort.cancel();
          // CRITICAL: await child to fully drain & write Failed event
          let _ = child_handle.await;
          Err(TaskFailure::Cancelled { source: ParentAbort })
      }
  }
  ```
  不允许直接 `child_handle.abort()` 然后立即返回；child JSONL 必须看到 `AgentRunEvent::Failed { reason: Cancelled }` 才算 cleanup 完成。
- 子 token 单独 cancel（如 budget hard cap）不影响父 session；父继续走，task tool 返回 `TaskFailure::Cancelled { source: BudgetHardCap }`。
- 父 abort 时，dispatcher 必须保证 **至少一条** `AgentRunEvent::Failed { reason: Cancelled }` 写入子 JSONL，且子 SessionStatus 投影必须从 `busy` 转为 `idle`，便于 resume。
- `AbortToken::child()` 是 single-direction：子 abort 不会冒泡回父。
- **`UserInitiated` 入口的 cancel 必须等价工作**：opencode 的 `tests/session/prompt.test.ts:861-895` 测试验证「slash command subtask 创建后调用 `prompt.cancel(parent_chat_id)`，parent 与 child SessionStatus 都必须从 `busy` 变为 `idle`」。Libra 的 `tests/ai_subagent_user_initiated_cancel_test.rs` 必须复刻此场景：
  - 创建父 session，调 `task.dispatch(parent_id, agent, prompt)` 派发 sub-agent
  - 让 fake provider hang
  - 断言 parent.status = busy AND child.status = busy
  - 调 `code_control::cancel(parent_thread_id)`
  - await dispatcher fiber 退出
  - 断言 parent.status = idle AND child.status = idle
  - 断言 child JSONL 末尾事件是 `AgentRunEvent::Failed { reason: Cancelled }`

**Permission Escalation Gate（强制）**

OC-Phase 3 的 dispatcher 在第 7 步必须运行下列断言（即上文「Sub-Agent 权限继承算法」末尾的逻辑）。这是与 opencode 偏离的 Libra-only 安全门：

```rust
fn assert_no_escalation(
    parent: &PermissionRuleset,
    effective: &PermissionRuleset,
    perm_keys: &[&str],
    pattern_samples: &[&str],
) -> Result<(), TaskFailure> {
    for &perm in perm_keys {
        for &pattern in pattern_samples {
            let parent_action = evaluate(perm, pattern, &[parent]);
            let effective_action = evaluate(perm, pattern, &[effective]);
            if parent_action.action == Deny && effective_action.action != Deny {
                return Err(TaskFailure::PermissionEscalationDenied {
                    permission: perm.into(),
                    pattern: pattern.into(),
                });
            }
        }
    }
    Ok(())
}
```

`perm_keys` 与 `pattern_samples` 取自 `(builtin_tool_names ∪ child_spec.permission.iter().map(|r| &r.permission)) × ("*" ∪ child_spec.permission.iter().map(|r| &r.pattern))`。

测试 fixture 见 [tests/ai_subagent_permission_test.rs] 的「父 deny + 子 allow」案例。

**Sub-Agent Worktree 合同（OC-Phase 3 临时态）**

`agent.md` CEX-S2-13 「Isolated Workspace & Tool Boundary」尚未落地。OC-Phase 3 的 sub-agent **不能**写主 worktree；这是临时合同，CEX-S2-13 落地后会被替换为完整的 isolated workspace。

| 工具类 | OC-Phase 3 行为 | 后续（CEX-S2-13 之后） |
|--------|-----------------|--------------------------|
| 只读（grep/glob/read/list/web_search） | 直接走父 cwd，readonly | 同左 |
| `apply_patch` / `write_file` / `edit` | dispatcher 在 `available_for()` 阶段过滤；模型看不到 | 走 isolated worktree，patch 由 CEX-S2-15 merge |
| `shell` / `bash` | dispatcher 在 `available_for()` 阶段过滤，除非父 spec 显式 allow + sub-spec 显式 allow + sandbox=ReadOnly + network=Deny | 同上但解除 readonly 限制 |
| MCP bridge | source-pool trust_tier 决定；OC-Phase 3 默认只暴露 trust_tier=`builtin` | 不变 |

OC-Phase 3 的 PR 必须有 `tests/ai_subagent_worktree_readonly_test.rs`：fake sub-agent 试图调用 `apply_patch`，应在 `available_for()` 阶段被过滤，模型不会看到该工具。

**硬约束**

- `code.multi_agent.enabled = false` 时 `ToolRegistry::available_for()` 不暴露 `task`，与现状字节级等价。
- 默认 `max_subagent_depth = 1`、`max_concurrent_subagents = 1`；即使 spec 显式允许，OC-Phase 3 内不放开。
- 子 agent 默认 readonly；任何写文件能力都需 sub_spec 显式 allow + 父 effective allow。
- 子 agent 不能批准自己的 approval；`PermissionService` 必须使用父 instance。
- sub-agent 结果不得直接 merge 到主 worktree；写入隔离 workspace 或返回 patch/evidence，merge 归 agent.md CEX-S2-13。
- sub-agent 不得写 `refs/libra/agent-traces`，也不得创建 entire.md 的 `agent_session` / `agent_checkpoint`。外部 Agent 捕获由 `libra agent` / `ObservedAgent` 路径负责。
- 如果 entire.md 的 session namespace 隔离已落地，child JSONL 必须位于 `code/` namespace，而不是 `agent/` namespace。
- `task_id` 复用必须验证 `parent_thread_id` 一致，否则返回 `TaskFailure::UnknownSubagent`。

**测试**

- fake parent 调 `task`，fake child 返回文本，父工具结果包含 `task_id` 和 `<task_result>`。
- unknown `subagent_type` 返回 structured error。
- `mode = Primary` 的 agent 不能作为 sub-agent。
- 子 agent 尝试调用未授权工具，返回 deny，不执行 handler。
- 子 agent 写权限大于父权限时被裁剪。
- `task_id` resume 校验 parent thread，不允许串 session 复用。
- failure / timeout / cancel / budget exceeded 各有 event fixture。

**退出标准**

- `cargo test ai_subagent_single`
- `cargo test ai_subagent_flag_off_regression`
- `cargo test ai_subagent_contract`
- `cargo clippy --all-targets --all-features -- -D warnings`

### OC-Phase 4：Provider Transform 与 Context Handoff / Compaction

**目标**：把 provider quirks 集中到 transform Module，并让 sub-agent 接收稳定、结构化、可 audit 的 handoff。

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/providers/transform.rs` | 新增 request/response transform pipeline |
| `src/internal/ai/providers/*/completion.rs` | 调用 transform before_send / after_receive |
| `src/internal/ai/context_budget/handoff.rs` | 新增 `ContextHandoffBuilder` |
| `src/internal/ai/context_budget/compaction.rs` | 增加 LLM-generated summary source 标识，继续使用 `CompactionEvent` |
| `src/internal/ai/agent/profile/embedded/compaction.md` | 新增内建 compaction agent prompt |
| `tests/ai_provider_transform_test.rs` | provider quirks 单测 |
| `tests/ai_context_handoff_test.rs` | handoff 结构与 JSONL replay 测试 |

**Provider Transform 第一版范围**

- OpenAI-compatible 家族：tool_call_id、empty assistant content、reasoning field。
- Anthropic：empty content 过滤、tool_use/tool_result 顺序、cache control、thinking block。
- Gemini：system instruction / tools shape / safety error。
- Ollama：`think` 字段与本地/cloud auth 差异。

**Provider Error Taxonomy & Retry Policy（基于 opencode `provider/error.ts`）**

opencode 在 [provider/error.ts:105-202](https://github.com/sst/opencode/blob/dev/packages/opencode/src/provider/error.ts) 定义了一组**结构化** provider error 解析。Libra 必须复刻这套 taxonomy 而不是把所有 provider 错误折叠成 `CompletionError::ProviderFailed(String)`，因为不同错误类型对应**不同的恢复策略**：

```rust
// Libra 落点：src/internal/ai/providers/error.rs
pub enum ProviderError {
    /// 输入超过 model context window；触发 OC-Phase 4 compaction，**不**做 backoff retry
    ContextOverflow { message: String, response_body: Option<String> },
    /// 一般 API 错误；is_retryable 决定是否走指数退避
    ApiError {
        message: String,
        status_code: Option<u16>,
        is_retryable: bool,
        response_headers: HashMap<String, String>,
        response_body: Option<String>,
    },
    /// stream 协议错误（mid-stream JSON event 含 error code）
    StreamError {
        kind: StreamErrorKind,
        message: String,
        response_body: String,
    },
}

pub enum StreamErrorKind {
    /// retry 后通常恢复（含 server_overloaded、server_error）
    Transient,
    /// 用户必须修复（quota 用尽、订阅未升级）
    UserActionRequired,
    /// 模型输入有问题（invalid_prompt、bad request）
    BadInput,
    /// context window 超限（同 ContextOverflow，但 mid-stream 检测）
    ContextOverflow,
}
```

**错误码到 StreamErrorKind 的映射表**（必须严格复刻 opencode `parseStreamError`）：

| opencode error code | Libra `StreamErrorKind` | is_retryable | 恢复策略 |
|---------------------|--------------------------|--------------|----------|
| `context_length_exceeded` | `ContextOverflow` | false | 触发 compaction，重新请求 |
| `insufficient_quota` | `UserActionRequired` | false | 直接返回 user-facing error；TUI 显示充值 hint |
| `usage_not_included` | `UserActionRequired` | false | 同上，提示订阅升级 |
| `invalid_prompt` | `BadInput` | false | 直接失败；不重试；保留给模型作为 tool result（如果有） |
| `server_is_overloaded`（PR #25888 新增） | `Transient` | true | 指数退避重试 |
| `server_error` | `Transient` | true | 指数退避重试 |
| HTTP 413 / context_length_exceeded body | `ContextOverflow` | false | 触发 compaction |

**HTTP-level 错误**（`parseAPICallError`）也必须按相同逻辑分类：

```text
1. if message includes "context_length_exceeded" or status==413 -> ContextOverflow
2. else if status in {502, 503, 504} -> ApiError { is_retryable: true }
3. else if status == 429 -> ApiError { is_retryable: true } and respect Retry-After header
4. else if provider_id startsWith "openai" -> use isOpenAiErrorRetryable() heuristic
5. else -> ApiError { is_retryable: response.is_retryable }
```

**Retry Policy 实现要求**

OC-Phase 4 的 PR P4.\* 必须在 `tool_loop` 一侧（**不是** dispatcher）实现统一 retry：

| 配置项 | 默认值 | 来源 |
|--------|--------|------|
| `max_retries` | 3 | OC-Phase 5 `[code.provider.retry]` 段 |
| `base_delay_ms` | 1000 | 同上 |
| `max_delay_ms` | 30000 | 同上 |
| `backoff_factor` | 2.0 | 指数 jitter；`delay = min(max, base * 2^attempt + rand(0..base/2))` |
| `respect_retry_after` | true | 优先使用 `Retry-After` header（秒或 HTTP date） |

不重试的错误：

- `ContextOverflow` → 调 compaction agent，重建 transcript 后重试（一次，不计入 max_retries）
- `UserActionRequired` → 直接返回；TUI 显示 actionable error（如「ChatGPT Plus 升级链接」）
- `BadInput` → 直接返回；模型必须看到错误细节（作为 tool result）以自我修正
- 任何 `is_retryable: false` → 直接返回

**测试**：

- `tests/ai_provider_error_taxonomy_test.rs`：每个 error code → 对应 `StreamErrorKind` + `is_retryable` 期望值（fixture-based）
- `tests/ai_provider_retry_policy_test.rs`：fake provider 连续返回 `server_overloaded`，至 max_retries 后失败；中途返回成功后 retry 停止
- `tests/ai_provider_context_overflow_compact_loop_test.rs`：fake provider 返回 `context_length_exceeded` → 触发 compaction → 第二次请求成功；不计入 retry budget

**Variants 表维护模式（基于 opencode `transform.ts:761-770`）**

opencode 的 `variants(model)` 函数对每个 provider 维护一个**模型 ID 子串列表**，用 `mistralId.includes(id)` 做匹配。这是新模型版本上线时的最低维护成本路径。Libra 落点 [src/internal/ai/providers/capabilities.rs](../../src/internal/ai/providers/) 必须采用同样模式：

```rust
const ANTHROPIC_REASONING_IDS: &[&str] = &[
    "claude-3-7-sonnet",
    "claude-opus-4",
    "claude-sonnet-4",
];

const MISTRAL_REASONING_IDS: &[&str] = &[
    "mistral-small-2603",
    "mistral-small-latest",
    "mistral-medium-3.5",
    "mistral-medium-2604",  // PR #25887 新加
];

pub fn supports_reasoning(provider_id: &str, model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    match provider_id {
        "anthropic" => ANTHROPIC_REASONING_IDS.iter().any(|s| id.contains(s)),
        "mistral" => MISTRAL_REASONING_IDS.iter().any(|s| id.contains(s)),
        ...
    }
}
```

每次新增 reasoning 能力的模型，**只需在对应常量列表追加 ID**；Provider Transform 通过 `supports_reasoning()` 自动启用 variants。这条规则写入 [docs/agent/provider-capability-update-guide.md](../agent/) 作为协作约定（OC-Phase 5 GA 前完成）。

<a id="literal-summary-template"></a>

**Literal Compaction Template（必须按字面落库）**

下面是 `compaction` agent 的 system / user prompt 末尾必须严格附加的模板。模板与 opencode `session/compaction.ts:43-78` 的 `SUMMARY_TEMPLATE` 字面一致；**不允许中文化、不允许字段重排、不允许把空段省略**：

```text
Output exactly the Markdown structure shown inside <template> and keep the section order unchanged. Do not include the <template> tags in your response.
<template>
## Goal
- [single-sentence task summary]

## Constraints & Preferences
- [user constraints, preferences, specs, or "(none)"]

## Progress
### Done
- [completed work or "(none)"]

### In Progress
- [current work or "(none)"]

### Blocked
- [blockers or "(none)"]

## Key Decisions
- [decision and why, or "(none)"]

## Next Steps
- [ordered next actions or "(none)"]

## Critical Context
- [important technical facts, errors, open questions, or "(none)"]

## Relevant Files
- [file or directory path: why it matters, or "(none)"]
</template>

Rules:
- Keep every section, even when empty.
- Use terse bullets, not prose paragraphs.
- Preserve exact file paths, commands, error strings, and identifiers when known.
- Do not mention the summary process or that context was compacted.
```

落库要求：

- 模板放在 [src/internal/ai/agent/profile/embedded/compaction.md](../../src/internal/ai/agent/profile/embedded/) 的 system prompt 部分。
- `ContextHandoff::summary` 字段必须能解析出 8 个标题段（`## Goal` / `## Constraints & Preferences` / `## Progress` 含 3 个 `### Done|In Progress|Blocked` / `## Key Decisions` / `## Next Steps` / `## Critical Context` / `## Relevant Files`）。
- `tests/ai_context_handoff_test.rs` 的 fake compaction agent 必须 echo 一份 `lipsum` 填入这 8 段；测试用 `parse_handoff_template()` 严格校验段数与顺序。
- 如果 compaction agent 漏了任何一段，`ContextHandoffError::SchemaMismatch { missing_sections: Vec<String> }` 必须被记录；不允许 silent fallback 到 raw transcript。

**Libra Compaction 默认常量（与 opencode 对齐）**

| Libra 常量 | 默认值 | opencode 来源 |
|------------|--------|---------------|
| `PRUNE_MINIMUM` | 20_000 tokens | `session/compaction.ts:36` |
| `PRUNE_PROTECT` | 40_000 tokens | `session/compaction.ts:37` |
| `TOOL_OUTPUT_MAX_CHARS` | 2_000 chars | `session/compaction.ts:38` |
| `PRUNE_PROTECTED_TOOLS` | `["skill"]`（Libra 加 `["skill", "submit_intent_draft", "submit_plan_draft"]`） | `session/compaction.ts:39` |
| `DEFAULT_TAIL_TURNS` | 2 | `session/compaction.ts:40` |
| `MIN_PRESERVE_RECENT_TOKENS` | 2_000 | `session/compaction.ts:41` |
| `MAX_PRESERVE_RECENT_TOKENS` | 8_000 | `session/compaction.ts:42` |
| `preserve_recent_budget(model)` | `min(MAX, max(MIN, floor(usable(model) * 0.25)))` | `session/compaction.ts:137-142` |

落库位置：[src/internal/ai/context_budget/compaction.rs](../../src/internal/ai/context_budget/compaction.rs) 的常量段。所有常量必须 `pub const`，且通过 `[code.compaction]` TOML 段允许 override（OC-Phase 5）。

**Compaction 触发判定**

opencode 在 [`session/overflow.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/overflow.ts) 用 `usable(model)` = `model.limit.context - model.limit.output - safety_margin`，并在 `tokens.input >= usable(model)` 时触发。Libra 的 `ContextBudget`（[src/internal/ai/context_budget/budget.rs](../../src/internal/ai/context_budget/budget.rs)）已有 `window_tokens()` / `output_reserve()`，必须实现等价 `is_overflow()` 函数：

```rust
impl ContextBudget {
    pub fn usable(&self) -> u64 {
        self.window_tokens
            .saturating_sub(self.output_reserve)
            .saturating_sub(self.safety_margin)
    }
    pub fn is_overflow(&self, input_tokens: u64) -> bool {
        input_tokens >= self.usable()
    }
}
```

`safety_margin` 第一版默认 1024 tokens。

**Compaction Message Ordering（PR #25851，2026-05-05）**

opencode 在 PR #25851（合入于本计划当日，6 小时前）修了一个长期被忽视的语义错误：compaction 之后展示给模型的消息顺序应该是 **「compaction marker → summary message → retained tail」**，而不是 chronological order 的 「retained tail → compaction marker → summary」。这个修复直接体现在 [session/message-v2.ts:1106-1133 `filterCompacted`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/message-v2.ts) 的尾部重排逻辑：

```ts
// post-#25851 logic
const compactionIndex = result.findLastIndex(msg => has compaction-with-tail_start_id);
const summaryIndex = result.findIndex((m, i) => i > compactionIndex && m.role==="assistant" && m.summary && m.parentID === compaction.id);
const tailIndex = result.findIndex(m => m.id === compaction.tail_start_id);
if (tailIndex >= 0 && tailIndex < compactionIndex && summaryIndex > compactionIndex) {
  return [
    ...result.slice(compactionIndex, summaryIndex + 1),  // compaction marker + summary
    ...result.slice(tailIndex, compactionIndex),         // retained tail
    ...result.slice(summaryIndex + 1),                   // post-summary content
  ];
}
```

为什么这样的顺序对模型更友好：

1. **首先看到「曾经发生过 compaction」**（marker），告诉模型早期 transcript 已被压缩；
2. **然后看到 summary**（assistant message with `summary: true`），即 SUMMARY_TEMPLATE 8 段产物；
3. **最后看到 retained tail**（最近 N 轮原始消息），保留 raw fidelity；
4. 再后是 compaction 之后产生的新消息。

**`CompactionPart` schema（必须实现）**

为支持上述重排，Libra 的 `CompactionEvent` schema 必须新增 `tail_start_id` 字段，对齐 opencode `CompactionPart`：

```rust
pub struct CompactionPart {
    pub id: PartId,
    pub message_id: MessageId,
    pub session_id: SessionId,
    pub kind: CompactionKind,            // Auto | UserRequested
    pub overflow: bool,                  // 是否由 overflow 触发
    pub tail_start_id: Option<MessageId>,// 指向 retained tail 的第一条消息 id；None 表示无 tail（全压缩）
    pub source_frame_id: Uuid,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub created_at: DateTime<Utc>,
}
```

`tail_start_id` 是这次 PR #25851 之后最关键的字段：没有它就无法实现重排，filterCompacted 也无从识别 retained tail 的边界。Libra 的 [src/internal/ai/context_budget/compaction.rs](../../src/internal/ai/context_budget/compaction.rs) 必须将该字段加入持久化 schema，并在 OC-Phase 4 PR P4.3 之前**先完成 schema migration**（不依赖运行时实现，schema 兼容是 P4.\* 的前置）。

**filterCompacted 等价函数**

Libra 落点：[src/internal/ai/context_budget/projection.rs](../../src/internal/ai/context_budget/) 新文件，实现：

```rust
pub fn filter_compacted(messages: &[SessionMessage]) -> Vec<&SessionMessage> {
    // Step 1: walk backwards, collect messages until hit a compaction-with-tail.
    //         If compaction has tail_start_id, retain everything up to that id.
    //         If no tail_start_id, retain only messages newer than the compaction.
    // Step 2: reverse to chronological order.
    // Step 3: if (tail_index < compaction_index < summary_index): reorder to
    //         [compaction..summary, tail..compaction-1, rest]
    // Otherwise: return chronological order unchanged.
}
```

测试用例（`tests/ai_compaction_filter_test.rs`）必须覆盖：

| 输入 transcript（idx: kind） | 期望输出顺序 |
|------------------------------|--------------|
| `0:user, 1:assistant, 2:compaction(tail=1), 3:assistant(summary, parent=2), 4:user, 5:assistant` | `[2, 3, 1, 4, 5]`（marker, summary, tail-1, rest） |
| `0:user, 1:assistant, 2:compaction(tail=None), 3:assistant(summary, parent=2)` | `[0, 1, 2, 3]`（无 tail，保持原序） |
| `0:user, 1:assistant, 2:assistant(summary, parent=0), 3:compaction(tail=1)` | `[0, 1, 2, 3]`（summary 在 compaction 之前，不触发重排） |

**Pruning 算法（与 SUMMARY 互补）**

opencode 在 token 接近 `usable * 0.5` 但未到 overflow 时跑一次 prune（[`session/compaction.ts` `prune` 函数]），删除「**单条 tool 输出 > TOOL_OUTPUT_MAX_CHARS** 且不在 `PRUNE_PROTECTED_TOOLS`」的旧 tool result。Libra 必须复用 `ContextFrameEvent` 的 `attachment_refs`：

- 大型 tool 输出 在写入 `ContextFrame` 时附 `attachment_id`（指向外部 blob，agent.md Step 1.9 已落地）。
- prune 阶段把 inline tool output 替换为 `<pruned attachment_id="..." length="...">`，让模型仍能看到 attachment 入口但不再消耗 inline tokens。
- 不修改原始 SessionJsonl bytes；只在内存 transcript 投影里替换。

**Context Handoff 规则**

- 输入来自最新 `ContextFrameEvent`，保留 non-compressible/system rules。
- compaction agent 不允许工具。
- compaction model 默认继承父 model，可由配置 override。
- 如果 compaction 失败，Task dispatcher 返回明确错误；不 silently 传 raw transcript。
- `CompactionEvent` 必须记录 tokens_before / tokens_after / source frame / attachment refs。

**测试**

- `ContextHandoff` 必含 7 个标题段。
- 超 budget 时保留 tail，历史头部进入 summary。
- provider 不支持对应 capability 时提前报错。
- transform 对 fake provider 的模拟 quirk 生效。

**退出标准**

- `cargo test ai_provider_transform`
- `cargo test ai_context_frame`
- `cargo test ai_context_handoff`

### OC-Phase 5：声明式配置、预算与 TUI 基线

**目标**：把多 agent 能力发布成可维护用户接口，并给 Goal 模式提供预算、usage 和 TUI 基线。

**配置面**

第一版优先 TOML，兼容现有 markdown profile：

```toml
[code.multi_agent]
enabled = false
max_subagent_depth = 1
max_concurrent_subagents = 1

[code.goal]
enabled = false
auto_continue_on_resume = "ask"
max_continuation_loops = 50
require_completion_evidence = true

[code.agents.planner]
mode = "primary"
model = "anthropic/claude-3-5-sonnet-latest"
tools = ["read_file", "list_dir", "grep_files", "task"]
steps = 30

[code.agents.explorer]
mode = "subagent"
model = "deepseek/deepseek-chat"
tools = ["read_file", "list_dir", "grep_files"]
permission = { write = "deny", shell = "deny" }
steps = 20

[code.compaction]
model = "deepseek/deepseek-chat"
tail_turns = 3
preserve_recent_tokens = 4000

[code.budget]
max_session_cost_usd = 5.0
warn_session_cost_usd = 2.0
max_session_tokens = 1000000

[code.budget.goal]
warn_cost_usd = 2.0
max_cost_usd = 5.0
warn_wall_clock_minutes = 30
max_wall_clock_minutes = 120

[code.budget.per_agent.explorer]
max_cost_usd = 1.0
max_steps = 20
```

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/agent/profile/config.rs` | `AgentsConfig` TOML schema + validation |
| `src/internal/ai/usage/recorder.rs` | `UsageContext` 增加 `agent_name` |
| `src/internal/db/migration.rs` | additive migration 为 `agent_usage_stats` 增加 `agent_name` + index；版本号必须晚于 entire.md 的 `2026050303_agent_capture` |
| `src/internal/ai/usage/query.rs` | 支持 `--by=agent` 或 `(agent, provider, model)` 聚合 |
| `src/internal/tui/slash_command.rs` / app 相关文件 | `/agents`、`/budget`、`/usage`、`/goal status` 只读入口扩展 |
| `docs/` / examples | 多 agent 示例与迁移说明 |

**配置 Gate**

- flag-off old path 通过 agent.md CP-S2-3 等价测试。
- `examples/agents.toml` 可完成 planner -> explorer -> reviewer 的 fake E2E。
- `/usage` 能按 agent/provider/model 展示。
- budget 超限错误有 `StableErrorCode` 与 actionable hint。
- README / docs 更新配置字段与安全限制。

### OC-Phase 6：Codex-like Goal 模式

**目标**：实现一个显式开启、可恢复、完成前不进入 idle 的 Goal 模式。此 phase 不是多 agent 的替代品；它是父 session 的监督层，可以调用普通工具、Task dispatcher、compaction 和 usage/budget。

**文件落点**

| 文件 | 改动 |
|------|------|
| `src/internal/ai/goal/mod.rs` | 新模块入口，re-export spec / event / state / supervisor / verifier |
| `src/internal/ai/goal/spec.rs` | `GoalSpec`、`GoalCriterion`、`GoalBudget`、`GoalEvidencePolicy` |
| `src/internal/ai/goal/event.rs` | `GoalEvent` envelope，保持 unknown-event-safe |
| `src/internal/ai/goal/state.rs` | `GoalState` replay / projection |
| `src/internal/ai/goal/supervisor.rs` | `GoalSupervisor` 主循环，包裹 `run_tool_loop` |
| `src/internal/ai/goal/verifier.rs` | deterministic `GoalVerifier` 与 rejection reason |
| `src/internal/ai/goal/prompt.rs` | Goal preamble / continuation prompt 模板 |
| `src/internal/ai/session/jsonl.rs` | 增加 `SessionEvent::Goal`，replay 时不影响旧 `SessionState` |
| `src/internal/ai/tools/spec.rs` | 增加 `update_goal_progress`、`submit_goal_complete` schema |
| `src/internal/ai/tools/handlers/goal.rs` | 两个 Goal 工具 handler，只写事件，不直接改 final status |
| `src/internal/ai/agent/runtime/tool_loop.rs` | 增加 `GoalStopPolicy` 或等价 `LoopExitPolicy`，确保 Goal 模式下普通 final text 不被视为完成 |
| `src/command/code.rs` | `--goal` CLI、config wiring、GoalSupervisor 启动 |
| `src/internal/tui/slash_command.rs` | `/goal start/status/cancel/criteria` |
| `src/internal/tui/app.rs` / bottom pane | active Goal 状态、blocker、budget 提示 |
| `src/command/code_control.rs` | `goal.start`、`goal.status`、`goal.cancel` NDJSON 方法 |
| `docs/commands/code.md` | Goal 模式用户文档 |

**Supervisor 主循环**

```text
libra code --goal "..."
  -> GoalSpecBuilder parses objective and optional acceptance criteria
  -> append GoalEvent::Created
  -> GoalSupervisor::run_until_goal_boundary(...)
       -> build Goal-bound ToolLoopConfig
       -> expose update_goal_progress + submit_goal_complete
       -> run_tool_loop(...)
       -> replay GoalState
       -> if Completed or Cancelled: return final response
       -> if CompletionClaimed: GoalVerifier::verify(...)
            -> pass: append Completed, return final response
            -> fail: append CompletionRejected, continue
       -> if AwaitingUser or Blocked: render blocker, stop interactive turn without marking complete
       -> otherwise: build continuation prompt and loop
```

**完成判定**

- `submit_goal_complete` 成功执行只是"声称完成"。
- `GoalVerifier` 通过后才写 `GoalEvent::Completed`。
- TUI / Code Control / session status 只有看到 `GoalEvent::Completed` 才显示 Goal complete。
- 如果 assistant 没有调用 `submit_goal_complete`，即使 final text 写"完成了"，supervisor 也必须继续。
- 如果 verifier 拒绝 completion，下一轮 prompt 必须列出缺失 criteria、缺失 evidence、最近失败工具和最小下一步。

**暂停 / 阻塞边界**

| 场景 | 状态 | 用户可见行为 |
|------|------|--------------|
| 需要用户选择方案 | `AwaitingUser` | 提出 1 个具体问题；回答后继续 |
| approval 被拒 | `Blocked` | 显示被拒 tool / scope；用户可修改 scope 或 cancel |
| hard budget 用尽 | `Blocked` | 显示已用成本、剩余任务、追加预算命令 |
| provider 连续失败 | `Blocked` | 显示 provider / model / retry count；可切换 model 或 cancel |
| context 溢出 | `Running` | 先 compaction；compaction 失败才 `Blocked` |
| 用户 Ctrl-C / cancel turn | `Active` | 当前 loop 停止，Goal 仍 active；resume 后继续 |

**测试**

- 创建 Goal 后写入 `GoalEvent::Created`，`GoalState` replay 正确。
- active Goal 下 assistant final text 不会让 session idle；supervisor 生成 continuation prompt。
- `submit_goal_complete` 缺 required criterion 被 `CompletionRejected`，并继续下一轮。
- `submit_goal_complete` 满足 criteria + evidence + verification 后写 `Completed`，TUI 状态回 idle。
- budget hard cap 进入 `Blocked(BudgetApprovalRequired)`，不写 `Completed` / `Cancelled`。
- `/goal cancel` 写 `Cancelled`，停止 Goal-bound loop。
- `--resume <thread>` replay active Goal，默认 ask；用户确认后继续。
- flag-off 下 `update_goal_progress` / `submit_goal_complete` 不出现在 registry，旧 session JSONL 字节级等价。
- `submit_task_complete` 在 active Goal 下不能结束 Goal。
- malformed / unknown future Goal event 不破坏 session replay。

**退出标准**

- `cargo test ai_goal_state`
- `cargo test ai_goal_supervisor`
- `cargo test ai_goal_completion_gate`
- `cargo test ai_goal_resume`
- `cargo test ai_goal_flag_off_regression`
- `cargo test ai_usage_tui_test`
- `cargo clippy --all-targets --all-features -- -D warnings`

**GA Gate**

- `code.goal.enabled = false` 默认关闭至少一个 release cycle。
- Goal 模式用户文档明确：不会因为预算耗尽自动标记完成；用户取消会留下 `Cancelled` 事件。
- Goal 模式与 multi-agent 同时开启时，sub-agent 不能完成父 Goal，只能返回 evidence。
- Goal completion report 包含 changed files、verification、residual risk、budget summary。

---

## 与 agent.md 的接口

| agent.md 章节 | 本文对应 |
|---------------|----------|
| Step 1.1 SafetyDecision | OC-Phase 3 的 `SubAgentSpawn` 必须走同一 safety gate |
| Step 1.6 Approval TTL | 子 agent approval 复用父会话 approval store；子 agent 不能 approve 自己 |
| Step 1.8 JSONL Session | child run 写入 `libra code` SessionStore namespace；GoalEvent 也必须复用 `SessionEvent` envelope，reader 使用 unknown-event-safe 语义 |
| Step 1.9 ContextBudget / ContextFrame | OC-Phase 4 handoff 输入必须来自 `ContextFrameEvent`；Goal continuation / resume 也必须从最新 ContextFrame 和 CompactionEvent 重建上下文 |
| Step 1.11 Usage | OC-Phase 1 保持 provider/model 记录；OC-Phase 5 增加 agent 维度；OC-Phase 6 Goal loop 使用独立 `request_kind = goal_continuation` 或等价字段 |
| Step 2.1 CEX-S2-10 | 本文 OC-Phase 0/3 复用 `agent_run` schema，不复制业务对象 |
| Step 2.3 CEX-S2-12 | 本文 OC-Phase 3 是 user-facing Task 工具形态，runtime 仍归 CEX-S2-12 |
| Step 2.4 CEX-S2-13/14 | 并行 sub-agent 和 merge candidate 不在 OC-Phase 3 开启 |

冲突规则：

- Schema / Event 以 agent.md 为准。
- 用户可见配置、CLI/TUI 命令、错误文案以本文为准，但必须同步回 agent.md readiness matrix。
- 若本文提出的新字段与 `agent_run` 现有 schema 冲突，优先扩展现有 schema，不新建平行 schema。

---

## 与 entire.md 的接口

| entire.md 章节 | 本文兼容规则 |
|----------------|--------------|
| 3.1 Transcript 落盘 | 本文不持久化 provider 原始 transcript；内部 sub-agent handoff 只记录摘要、tail 和 attachment refs |
| 3.2 `refs/libra/agent-traces` | 该 ref 归外部 Agent 捕获；OC-Phase 3/4 不写入、不清理、不 rewind |
| 3.4 `agent_session` / `agent_checkpoint` | 这两张表只服务 external `libra agent`；内部 `AgentRun` 不复用这些主键或表 |
| 3.6 `agent_run/` 关系 | 本文正好落在 Libra 自家 sub-agent；继续使用 `agent_run/`，并保持 default build 不直接链接 gated schema |
| 4 migration strategy | 后续 `agent_name` migration 使用 `YYYYMMDDNN`，版本晚于已注册最大值，不占用 `2026050303` |
| 7 Checkpoint / Rewind | 内部 sub-agent 不提供 checkpoint rewind；工作树恢复和 patch merge 归 agent.md CEX-S2-13 |
| 10 cloud sync | 新 artifact 必须走 object writer + `object_index`；不向 D1 同步内部事件流 |
| 11 与 `libra code` 共存 | 本文保持 `libra code` 为 code namespace；不进入 `.libra/sessions/agent/` |
| Goal 模式 | `GoalEvent` 只进入 code session JSONL；不写 `agent_session` / `agent_checkpoint` / `agent-traces`；进程退出后不后台继续执行 |

冲突规则：

- 外部 OpenCode 捕获属于 entire.md 的 `ObservedAgent` adapter；内部多 LLM 编排属于本文。
- 若某个实现同时需要"运行 OpenCode 作为外部进程"和"在 `libra code` 内派发 sub-agent"，必须拆成两个 PR，分别修改 `observed_agents/` 与 `agent_run/`。
- 任一 PR 只要写 `agent_session` / `agent_checkpoint` / `agent-traces`，就必须按 entire.md 的 redaction、checkpoint、locked-ref、cloud sync gate 评审；本文不能降低这些门槛。

---

## 测试矩阵

| 层 | 必测内容 | 建议测试 |
|----|----------|----------|
| Provider Runtime | `AnyCompletionModel` dispatch、usage summary、unknown provider/model error | `cargo test ai_provider_factory` |
| Profile | frontmatter 兼容、`provider/model` lift、旧 `model: fast` 不误解析 | `cargo test agent_profile` |
| Task Dispatcher | success / unknown agent / permission deny / resume / budget / timeout | `cargo test ai_subagent_single` |
| Flag-off | 没有 task / goal tool，旧 TUI、slash command、usage 行为不变 | `cargo test ai_subagent_flag_off_regression` / `cargo test ai_goal_flag_off_regression` |
| EntireIO 兼容 | flag-off 和 fake sub-agent 不写 `agent-traces` / `agent_session` / `agent_checkpoint` | `cargo test ai_subagent_entire_compat` |
| Permission | 父 ∩ 子、deny 优先、nested task deny、self-approval deny | `cargo test ai_subagent_permission` |
| Context Handoff | 7 段 summary、tail 保留、attachment refs、replay | `cargo test ai_context_handoff` |
| Transform | Anthropic / OpenAI-compatible / Deepseek / Gemini / Ollama quirks | `cargo test ai_provider_transform` |
| Usage | `agent_name` migration、按 agent 聚合、失败行 | `cargo test ai_usage_stats_test` |
| Goal State | GoalEvent replay、status transition、unknown event、resume | `cargo test ai_goal_state` |
| Goal Supervisor | final text 不停止、completion gate、budget blocked、cancel | `cargo test ai_goal_supervisor` |
| CLI/TUI | `/agents`、`/budget`、`/usage`、`/goal` 快照 | `cargo test ai_usage_tui_test` |

每个 runtime PR 必须至少跑：

```bash
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

---

## Acceptance Scenarios（端到端落地标准）

每个 phase 的退出条件除 cargo test 外，必须把以下场景在 `--features test-provider` 下能 deterministic 跑通。每个场景都是「fake provider script + 输入 → 期望 event/usage 序列」格式，断言 byte-level 或 schema-level 对比，避免依赖模型实际输出。

### S1 — Provider Factory（OC-Phase 1）

| 步骤 | 输入 | 期望 |
|------|------|------|
| 1 | `ModelBinding { provider_id: "fake", model_id: "default", variant: None }` 调 `factory.build(...)` | 返回 `AnyCompletionModel::Fake(...)` |
| 2 | 同步调 fake `completion(req)` | 返回 `AnyCompletionRawResponse::Fake(...)`，`usage_summary()` 输出 fixture 一致的 (input, output, total) tuple |
| 3 | 用 unknown provider 调 `factory.build(...)` | 返回 `ProviderFactoryError::UnknownProvider { available: [...] }`，`available` 包含全部 8 个 provider id（含 fake） |
| 4 | 用合法 provider + 不存在 model | 返回 `UnknownModel { provider, suggestions }`，`suggestions` 至少 1 个 |
| 5 | `set_run_id("run-X")` 后 fake response 中 metadata `run_id == "run-X"` | 通过 |

测试文件：`tests/ai_provider_factory_test.rs`。

### S2 — Agent Profile lift（OC-Phase 2）

输入：

```yaml
# .libra/agents/planner.md frontmatter
---
name: planner
mode: primary
model: anthropic/claude-3-5-sonnet-latest
temperature: 0.5
steps: 30
permission:
  edit: deny
  task: allow
---
You are a planner.
```

期望：

- `parse_agent_profile()` 返回 `AgentExecutionSpec { mode: Primary, model: Some(ModelBinding { provider_id: "anthropic", model_id: "claude-3-5-sonnet-latest", .. }), permission: [{edit:*:deny}, {task:*:allow}], steps: Some(30), .. }`。
- 旧 `model: fast` 不被解析为 ModelBinding；保留为 `model_preference: "fast"`，`AgentExecutionSpec.model = None`。
- 旧 frontmatter 缺 `mode` 字段时，`AgentExecutionSpec.mode = Primary`。
- `cargo run -- code --agent planner` 在 fake provider 下启动并显示 `provider=anthropic` 的 usage row（fake provider 模拟 anthropic 包装）。

测试文件：`tests/ai_agent_profile_test.rs` + `tests/ai_agent_route_test.rs`。

### S3 — Single sub-agent dispatch（OC-Phase 3）

`fake-providers` 脚本：

- parent agent (`build`, fake provider) 收到 user message `"Find all TODOs"`
- parent emits exactly: `tool_call(name="task", args={subagent_type:"explore", description:"find TODOs", prompt:"grep TODO src/"})` 一次，然后 emits final `"Done."`
- sub-agent (`explore`) 收到 prompt 后 emits: `tool_call(name="grep_files", args={pattern:"TODO", path:"src/"})`，再 emits text `"Found 3 TODOs in 2 files."`，结束

期望（断言 JSONL 事件序列）：

```text
parent.session.jsonl:
  UserMessage(text="Find all TODOs")
  AssistantMessage { agent: "build", model: "fake/default" }
  ToolCall { name: "task", call_id: "..." }
  AgentRunEvent::Spawned { agent_run_id, parent_thread_id, subagent_name: "explore" }
  ... (child events appear in child JSONL)
  AgentRunEvent::Completed { agent_run_id, usage: {...} }
  ToolResult { name: "task", call_id, output: "task_id: ...\n<task_result>\nFound 3 TODOs in 2 files.\n</task_result>" }
  AssistantMessage { text: "Done." }
  Final { stop_reason: "end_turn" }

child.session.jsonl (libra code namespace):
  UserMessage(text="grep TODO src/", from: SubAgentSpawn)
  AssistantMessage { agent: "explore", model: "fake/default" }
  ToolCall { name: "grep_files" }
  ToolResult { name: "grep_files" }
  AssistantMessage { text: "Found 3 TODOs in 2 files." }
  Final { stop_reason: "end_turn" }

usage rows (agent_usage_stats):
  { thread_id, agent_run_id: NULL, agent_name: "build",   provider: "fake", model: "default", input: x, output: y }
  { thread_id, agent_run_id: <run>,  agent_name: "explore", provider: "fake", model: "default", input: x, output: y }
```

flag-off 等价：把 `code.multi_agent.enabled = false`；同样输入下 `task` tool 不出现，parent 只 emits final text；`agent_usage_stats` 只 1 行；`agent-traces` ref tip 不变。

测试文件：`tests/ai_subagent_single_test.rs` + `tests/ai_subagent_flag_off_regression_test.rs`。

### S4 — Permission escalation gate（OC-Phase 3）

设置：

- 父 ruleset `[{edit: "*": deny}]`
- 子 spec permission `[{edit: "*": allow}]`

输入：parent emits task 调 `explore-with-edit` sub-agent。

期望：

- `SubAgentDispatcher::dispatch()` 在 `assert_no_escalation()` 阶段失败，返回 `TaskFailure::PermissionEscalationDenied { permission: "edit", pattern: "*" }`。
- 写入 `AgentRunEvent::Failed { reason: PermissionEscalationDenied }`。
- 父收到 `ToolResult { name: "task", error: "<structured>" }`。

测试文件：`tests/ai_subagent_permission_test.rs`。

### S5 — Compaction handoff（OC-Phase 4）

输入：parent session token 用量 > `usable * 0.5`，触发 prune；之后超 `usable`，触发 compact。

期望：

- prune 阶段：大 tool result 被 inline 替换为 `<pruned attachment_id="..." length="...">`，原始 JSONL 字节不变。
- compact 阶段：调 `compaction` agent (fake provider script echoes 8-section template with fixture)；写 `CompactionEvent { tokens_before, tokens_after, source_frame_id, summary }`。
- handoff 给下一轮的 parent prompt 包含 8 段标题且顺序一致；`ContextHandoff::summary` parse 通过。
- compaction agent fake script 故意省略 `## Critical Context` 段时，dispatcher 必须返回 `ContextHandoffError::SchemaMismatch { missing_sections: ["Critical Context"] }`，不写入 `CompactionEvent`。

测试文件：`tests/ai_context_handoff_test.rs` + `tests/ai_context_compaction_prune_test.rs`。

### S6 — Goal supervisor non-completion（OC-Phase 6）

输入：

- `libra code --goal "Add unit test for utils::path::join"`
- parent agent fake script：emit final text `"I think we're done."` 不调用 `submit_goal_complete`

期望：

- `GoalSupervisor` 检测到无 completion claim，写 `GoalEvent::ProgressRecorded { summary: "I think we're done." }`。
- 构造 continuation prompt（包含 missing criteria + last assistant summary），重新进入 `run_tool_loop`。
- 如 fake script 第二轮 emit `submit_goal_complete` with **不完整 evidence**，verifier 拒绝并写 `CompletionRejected { missing: [...] }`。
- 第三轮 fake emit 完整 evidence + verification，verifier 通过，写 `Completed`。

第二个变体：`max_continuation_loops = 3` 命中后写 `Blocked { reason: LoopLimitNeedsUser }`，**不**写 `Completed` / `Cancelled`。TUI / `/goal status` 仍显示 active。

测试文件：`tests/ai_goal_supervisor_test.rs` + `tests/ai_goal_completion_gate_test.rs`。

### S7 — Multi-agent declarative config E2E（OC-Phase 5）

输入：`examples/multi_agent.toml` 声明 planner (anthropic-fake) → coder (deepseek-fake) → reviewer (openai-fake)；用户输入 `"Implement function X"`。

期望（高 level，不要求字节对齐）：

- 三个 agent_run rows 写入 `agent_usage_stats`，`agent_name` 区分。
- TUI `/usage` 输出按 agent / provider 分组的表格。
- Budget 设置 `max_session_cost_usd = 0.001` 时，fake script 计算成本 > 阈值，下一次 task dispatch 在第 4 步（concurrency 检查后）失败为 `BudgetExceeded`，错误码 `StableErrorCode::AgentBudgetExceeded`，TUI 显示 hint。

测试文件：`tests/ai_multi_agent_e2e_test.rs`（gated on `test-provider` feature）。

---

## PR 切片建议

每个 OC-Phase 不应作为单 PR 落地。下表给出推荐拆分；每个 PR 都有独立的可验收范围、严格小于 ~600 行 diff（含测试），并独立通过 CI。

### OC-Phase 0（契约冻结）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P0.1 | `docs/improvement/opencode.md` 本文 + agent.md / entire.md cross-link | docs only |
| P0.2 | `src/internal/ai/agent/profile/spec.rs`（schema 定义 + serde + round-trip 测试） | ~200 |

### OC-Phase 1（Provider Adapter）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P1.1 | `providers/runtime.rs` 定义 `AnyCompletionModel` + `AnyCompletionRawResponse` + `CompletionUsage` 转发，但不接入任何调用方 | ~350 |
| P1.2 | `providers/factory.rs` + `providers/capability.rs`（静态 capability 表）；fake provider 单测 | ~300 |
| P1.3 | `code.rs` 主路径切换到 factory；保留 `enum CodeProvider` 作为 provider_id 枚举 | ~250 |

### OC-Phase 2（Agent Profile 可执行化）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P2.1 | `agent/profile/parser.rs` frontmatter 扩展 + 兼容回归测试 | ~300 |
| P2.2 | `agent/profile/router.rs` 返回 `AgentExecutionSpec`；TUI slash command 适配 | ~250 |
| P2.3 | `tools/registry.rs::available_for(spec, ruleset)` + `Permission.disabled()` 算法 | ~250 |
| P2.4 | `code.rs` 启动注入 ProviderFactory；`--agent <name>` 选择 spec.model | ~200 |
| P2.5 | `ApprovalCachePolicy` 增加 `ApprovedRuleset` 投影；migration 加 `approved_permission` 表 | ~300 |

### OC-Phase 3（Sub-Agent Dispatcher）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P3.1 | `tools/spec.rs` 增加 `task` schema；flag-off 默认不暴露 | ~150 |
| P3.2 | `agent/runtime/sub_agent.rs` 定义 `SubAgentDispatcher` trait + `DispatchContext` + `TaskFailure` | ~250 |
| P3.3 | dispatcher 实现：steps 1-7（feature flag / depth / concurrency / SafetyDecision / permission merge / escalation gate） | ~350 |
| P3.4 | dispatcher 实现：steps 8-13（PermissionService.ask / handoff / model build / child loop） | ~400 |
| P3.5 | `AgentRunEvent` Spawned / Completed / Failed wire-up；fake E2E（S3） | ~350 |
| P3.6 | UserInitiated 入口 + `bypass_permission_ask`；fake E2E（user-initiated 变体） | ~250 |
| P3.7 | Cancel / abort 传播；timeout；budget hard cap | ~300 |
| P3.8 | flag-off 字节级回归 fixture（S3 flag-off 变体） | ~150 |

### OC-Phase 4（Provider Transform + Compaction）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P4.1 | `providers/transform.rs` trait + 6 个 provider transform 移植（每个 provider 半天） | ~500 |
| P4.2 | Anthropic / OpenAI / Deepseek 的 quirks 集中并补 fake quirk 测试 | ~400 |
| P4.3 | `context_budget/handoff.rs` 与 8 段 SUMMARY parser；schema mismatch error | ~300 |
| P4.4 | embedded `compaction` agent prompt + LLM-summary path | ~250 |
| P4.5 | prune 算法集成到 `ContextFrameEvent` attachment_refs；非破坏性 inline replace | ~300 |
| P4.6 | E2E（S5） | ~150 |

### OC-Phase 5（声明式配置 + 预算 + TUI）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P5.1 | `agent/profile/config.rs` `AgentsConfig` schema + TOML loader + validation | ~350 |
| P5.2 | `agent_usage_stats` migration `agent_name` + index；query API 扩展 | ~300 |
| P5.3 | budget enforcement（per-session / per-agent）；`StableErrorCode::AgentBudgetExceeded` | ~400 |
| P5.4 | TUI `/agents` `/budget` `/usage` slash command | ~350 |
| P5.5 | E2E（S7）+ examples + docs | ~250 |

### OC-Phase 6（Goal mode）

| PR | 范围 | 估算 LOC |
|----|------|----------|
| P6.1 | `goal/spec.rs` `goal/event.rs` `goal/state.rs`（schema only） | ~300 |
| P6.2 | `goal/verifier.rs` deterministic completion gate | ~250 |
| P6.3 | `goal/supervisor.rs` 主循环 + continuation prompt builder | ~400 |
| P6.4 | `tools/handlers/goal.rs`（`update_goal_progress` / `submit_goal_complete`） | ~250 |
| P6.5 | `--goal` CLI + `/goal start/status/cancel` TUI | ~300 |
| P6.6 | Code Control `goal.start/status/cancel` NDJSON | ~200 |
| P6.7 | E2E（S6）+ resume replay + flag-off regression | ~350 |

**总估算**：约 9 500 LOC（含测试），分布在 35 个 PR；按每周 2 个 PR 节奏需要约 18 周。Goal 模式在 GA 前要至少 1 个 release cycle 的 unstable flag，故实际从启动到 GA 约 6-9 个月。

### PR 依赖图（critical path）

```text
P0.* ─→ P1.1 ─→ P1.2 ─→ P1.3 ─┐
                              │
                              ├─→ P2.1 ─→ P2.2 ─→ P2.3 ─→ P2.4 ─→ P2.5
                              │
                              └─────────────────────────→ P3.1
                                                          ↓
                            P3.2 ─→ P3.3 ─→ P3.4 ─→ P3.5 ─→ P3.6 ─→ P3.7 ─→ P3.8
                                                              │
                                                              └→ P4.1 ─→ ... ─→ P4.6
                                                                                  │
                                                                                  └→ P5.1 ─→ ... ─→ P5.5
                                                                                                      │
                                                                                                      └→ P6.* (并行支线)
```

P6.\* 依赖 P5.\* 提供 budget / usage TUI 基线，但内部 PR 之间可以与 P4.\* 并行开发（P6 不阻塞 P4，因为 Goal mode 的 supervisor 不强依赖 LLM compaction，只共享同一 ContextFrame 接口）。

---

## 风险与反模式

| 风险 / 反模式 | 影响 | 处理 |
|---------------|------|------|
| `Box<dyn CompletionModel>` | 当前 trait 不 object-safe，方案不能编译 | 使用 `AnyCompletionModel` enum Adapter |
| Task 只做普通 `ToolHandler` | 缺父 history / model / session / usage 上下文，无法安全派发 | 在 `tool_loop` 增加 `SubAgentDispatcher` |
| 内部 sub-agent 混写 entire.md 的 `agent-traces` | 外部 Agent 捕获与内部 runtime 状态混淆，checkpoint / rewind / redaction gate 被绕过 | 内部只写 `AgentRunEvent` 与 code SessionStore；external capture 只走 `ObservedAgent` |
| 子 agent 直接写主 worktree | 可能造成未审查修改、数据丢失 | OC-Phase 3 禁止；merge 归 CEX-S2-13 |
| 子权限大于父权限 | 权限绕过 | 父 ∩ 子，deny 优先，默认 readonly |
| compaction 失败后 raw transcript 直传 | 泄露和超 context 风险 | 返回 structured error，要求用户重试或缩小范围 |
| Goal 模式变成无限循环 | 成本失控、用户无法接管 | hard budget / wall-clock / continuation loop cap 进入 `Blocked`，用户可追加预算或 cancel；不能伪造 completed |
| Goal 完成只靠模型自述 | 未完成工作被标记完成 | `submit_goal_complete` + deterministic `GoalVerifier` + evidence refs 三段 gate |
| Goal blocker 被当成失败终止 | 用户以为任务结束，实际未完成 | `AwaitingUser` / `Blocked` 都是非完成状态，resume 必须提示 active Goal |
| provider transform 分散在各 completion.rs | quirks 难维护，新增 provider 易回归 | 集中到 `providers/transform.rs` 并加 fake quirk tests |
| 多 agent 成本失控 | 用户经济损失 | per-session + per-agent budget，超限前检查 |
| 配置面过早 GA | 长期兼容负担 | `code.multi_agent.enabled` 至少一个 release cycle 默认 false |

---

## 非目标

1. 不新增 provider 数量。
2. 不改造 Codex managed runtime。
3. 不做自动 model selection；默认不根据 prompt 猜 provider。
4. 不开放并行 sub-agent；并行依赖 agent.md CEX-S2-13/14。
5. 不在 OC-Phase 3 合并 sub-agent patch 到主 worktree。
6. 不引入远端 cloud orchestrator。
7. 不替代已有 MCP / SourcePool / hooks 体系。
8. 不实现 entire.md 的外部 Agent 捕获、transcript checkpoint、rewind 或 `refs/libra/agent-traces` 清理。
9. 不把 Goal 模式做成后台 daemon；进程退出后只持久化 active Goal，继续执行必须通过 `--resume` 或 control lease 显式恢复。
10. **不实现 opencode 的 Session Warping**（PR #25768 `feat(core): session warping`）：opencode 在 2026-05-04 引入了「workspace + sync_owner」体系，让用户在多个客户端 / 设备间同步同一 session 状态。这是 opencode 独立的 SaaS 多终端协同需求，与 Libra 的 multi-LLM 编排目标正交。Libra 的 session 始终绑定单一 `libra code` 进程；远端协同由 entire.md 的云同步路径独立处理，不复用 opencode 的 sync schema 或 control-plane 协议。

---

## Appendix A: opencode 源码锚点

本节是架构事实来源，不要求 Libra 逐行仿写。

| 主题 | opencode 文件 |
|------|---------------|
| provider SDK 注册 | `/Volumes/Data/opencode/packages/opencode/src/provider/provider.ts` `BUNDLED_PROVIDERS` |
| model metadata schema | `/Volumes/Data/opencode/packages/opencode/src/provider/models.ts` |
| runtime provider model | `/Volumes/Data/opencode/packages/opencode/src/provider/provider.ts` `Provider.Model`、`getModel()`、`getLanguage()` |
| provider transform | `/Volumes/Data/opencode/packages/opencode/src/provider/transform.ts` |
| agent schema 和内建 agent | `/Volumes/Data/opencode/packages/opencode/src/agent/agent.ts` |
| config agent schema | `/Volumes/Data/opencode/packages/opencode/src/config/agent.ts` |
| config root | `/Volumes/Data/opencode/packages/opencode/src/config/config.ts` |
| task tool | `/Volumes/Data/opencode/packages/opencode/src/tool/task.ts` |
| tool registry | `/Volumes/Data/opencode/packages/opencode/src/tool/registry.ts` |
| session prompt loop | `/Volumes/Data/opencode/packages/opencode/src/session/prompt.ts` |
| LLM stream | `/Volumes/Data/opencode/packages/opencode/src/session/llm.ts` |
| session processor | `/Volumes/Data/opencode/packages/opencode/src/session/processor.ts` |
| compaction | `/Volumes/Data/opencode/packages/opencode/src/session/compaction.ts` |
| permission ruleset | `/Volumes/Data/opencode/packages/opencode/src/permission/index.ts` |
| useful tests | `/Volumes/Data/opencode/packages/opencode/test/provider/transform.test.ts`、`test/tool/task.test.ts`、`test/permission-task.test.ts`、`test/session/messages-pagination.test.ts` |

---

## Appendix B: Provider Quirks Inventory

| Provider | Quirk | Libra 落点 |
|----------|-------|------------|
| Anthropic | empty content 拒绝、tool_use/tool_result 顺序敏感、tool id 字符集、cache_control、thinking blocks | `AnthropicTransform` |
| OpenAI | responses/chat 差异、tool_call_id、reasoning_effort、部分模型 system message 限制 | `OpenAiTransform` |
| DeepSeek | `reasoning_content` 独立字段、assistant empty content、OpenAI-compatible 但 reasoning round-trip 特殊 | `OpenAiCompatTransform` + deepseek capability |
| Kimi | OpenAI-compatible，stream 默认与 thinking 控制不同 | `OpenAiCompatTransform` + kimi options |
| Zhipu | OpenAI-compatible，部分模型 stream / model id 行为特殊 | `OpenAiCompatTransform` + zhipu options |
| Gemini | system instruction、tool schema、safety block、media modality | `GeminiTransform` |
| Ollama | local/cloud auth、`think` 字段、tool schema compact mode、本地模型能力未知 | `OllamaTransform` + conservative capability |
| Fake | 测试用 scripted response、usage summary fixture | `FakeTransform` 仅用于 tests |

---

## Changelog

- **2026-05-05（upstream-sync pass，HEAD `25ecf0af6`）**：根据 opencode dev 分支过去 24 小时的 5 个相关提交同步本计划。新增 / 修改如下：
  - **OC-Phase 3 Cancel/Abort 合同升级**（依据 PR #25798 `fix(session): cancel subtask child sessions`）：opencode 把 `TaskPromptOps.cancel` 从 `() => void` 改为 `Effect.Effect<void>`，配合 `EffectBridge.make()` 与 `if (Exit.hasInterrupts(exit)) yield* cancel` 实现「父 abort **必须 await 子 cancel 完成** 才能 release」。Libra 必须用 `tokio::select!` + `child_handle.await` 复刻；child JSONL 必须看到 `AgentRunEvent::Failed { reason: Cancelled }` 后才算 cleanup 完成。新增 `tests/ai_subagent_user_initiated_cancel_test.rs` 场景，复刻 opencode `tests/session/prompt.test.ts:861-895` 的 SubtaskPart 取消验证。
  - **OC-Phase 4 Compaction 重排算法**（依据 PR #25851 `fix(compaction): order compaction summary before retained tail`）：opencode 修了一个潜在语义错误——compaction 之后展示给模型的顺序应是 `[compaction marker, summary, retained tail]`，不是 chronological。新增「`CompactionPart` schema 必含 `tail_start_id`」要求、`filter_compacted()` 等价函数与 3 条 ordering test fixture。Libra OC-Phase 4 PR P4.3 之前必须先完成 schema migration。
  - **新增 Provider Error Taxonomy & Retry Policy**（依据 PR #25888 `fix: retry server_is_overloaded errors` 与 `provider/error.ts:105-202` 整体）：定义 `ProviderError { ContextOverflow, ApiError, StreamError }` 与 `StreamErrorKind { Transient, UserActionRequired, BadInput, ContextOverflow }`；6 个 opencode error code 到 Libra 类型的映射表；HTTP-level retry 优先级（HTTP 413 / 429 / 5xx 处理规则）；`tool_loop` 一侧的统一 retry 配置（`max_retries`, `base_delay_ms`, `respect_retry_after`）；3 条 fixture 测试。`ContextOverflow` 不计入 retry budget，触发 compaction 后重试一次。
  - **新增 Variants 表维护模式**（依据 PR #25887 `fix: mistral medium 3.5/2604`）：opencode 用「per-provider 模型 ID 子串列表 + `String.contains()` 匹配」维护 reasoning capability。Libra `providers/capabilities.rs` 必须采用同样模式（`ANTHROPIC_REASONING_IDS` / `MISTRAL_REASONING_IDS` 等常量），新增 reasoning 模型只需追加 ID。约定写入 `docs/agent/provider-capability-update-guide.md`。
  - **新增「非目标」第 10 条**：opencode PR #25768 `feat(core): session warping` 引入的多终端 session 同步与 Libra 多 LLM 编排目标正交，明确不复用其 sync schema / control-plane 协议。
  - **顶部「验证基线」更新**：HEAD 从 `773078e81` 推进到 `25ecf0af6`，列明 5 个关键 upstream commit。
- **2026-05-05（landability pass）**：把方案推到「可直接拆 PR」状态。基于 `/Volumes/Data/opencode/packages/opencode/src/permission/index.ts`、`tool/task.ts`、`session/prompt.ts:548 handleSubtask`、`session/compaction.ts:43-78 SUMMARY_TEMPLATE` 的逐行细节，新增以下 4 大类落地合同：
  - 新增 [Permission Ruleset 与 Approval 反馈协议](#permission-ruleset-与-approval-反馈协议)：`PermissionAction` / `PermissionRule` / `PermissionRuleset` 类型与 `evaluate(findLast)` / `merge(flat)` 语义；三态 Reply（`Once` / `Always` / `Reject{feedback}`）与 `approved_permission` SQLite 表；Sub-Agent 权限继承算法（与 opencode `task.ts:73-101` 字面对齐）；额外 Libra-only `Permission Escalation Gate` 阻止「父 deny 被子 allow 覆盖」；与现有 `ApprovalCachePolicy` 的对接表。
  - 新增 [Tool Registry 预过滤合同](#tool-registry-预过滤合同)：`ToolRegistry::available_for(spec, ruleset)` + `Permission.disabled()` 算法；模型只能看到 effective ruleset 允许的工具；测试 fixture。
  - 增强 OC-Phase 3：`SubAgentDispatcher` trait + `DispatchContext` + `TaskFailure` 完整类型；14 步主流程；区分 `LlmInitiated` 与 `UserInitiated { bypass_permission_ask }` 两类入口；`tokio_util::CancellationToken` 树形 abort 传播合同；Sub-Agent Worktree 临时合同（OC-Phase 3 readonly，CEX-S2-13 之后启用 isolated workspace）；`assert_no_escalation` 实现伪代码与测试矩阵。
  - 增强 OC-Phase 4：字面级 `SUMMARY_TEMPLATE` 8 段固定布局（不允许中文化）；Libra `usable() / is_overflow() / preserve_recent_budget()` 等价实现；与 opencode 对齐的 8 个常量默认值（`PRUNE_MINIMUM=20_000` 等）；prune 算法与 `ContextFrameEvent.attachment_refs` 的非破坏性 inline replace。
  - 新增 [Acceptance Scenarios](#acceptance-scenarios端到端落地标准) S1-S7：每个 phase 的 fake-provider deterministic E2E 场景，含期望 JSONL event 序列和 usage row。
  - 新增 [PR 切片建议](#pr-切片建议)：35 个 PR 的拆分计划，约 9 500 LOC（含测试），含 critical-path 依赖图与每周 2-PR 节奏估算。
- **2026-05-05**：补充 Codex-like Goal 模式设计。新增 `GoalSpec` / `GoalState` / `GoalEvent` / `GoalSupervisor` / `GoalVerifier` 契约，明确 active Goal 未完成时普通 final text 不能让 session idle；completion 必须经 `submit_goal_complete` 与 deterministic verifier；budget / provider failure / approval denial 只能进入 `Blocked` 或 `AwaitingUser`，不能伪装完成；增加 OC-Phase 6、测试矩阵、风险和非目标。
- **2026-05-05**：将初稿改为可落地方案。基于 `/Volumes/Data/opencode` 实际架构补充 Module Map 与调用链；修正 `Box<dyn CompletionModel>` 不可编译的问题，改为 `AnyCompletionModel` enum Adapter；修正 Task tool handler-only 不足，改为 `SubAgentDispatcher`；将 Goal 前基础路线重排为 OC-Phase 0 到 OC-Phase 5，并补齐文件落点、测试、退出标准和风险反模式。
- **2026-05-05**：补充与 [entire.md](entire.md) 的版本管理兼容约束。明确内部 sub-agent 不写 `refs/libra/agent-traces`、`agent_session`、`agent_checkpoint`；child JSONL 进入 `libra code` SessionStore namespace；后续 migration 版本必须晚于 `2026050303_agent_capture`；增加 `ai_subagent_entire_compat` gate。
