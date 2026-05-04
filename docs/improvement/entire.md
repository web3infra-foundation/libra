# 在 Libra 中集成 EntireIO 风格的“外部 Agent 会话捕获”能力

> **文档定位**：把 EntireIO 的核心价值（把外部 Agent 的生命周期事件与原始 transcript 纳入版本控制）迁移到 Libra，在保留现有 `libra code` 行为不变的前提下，补齐“多外部 Agent、可恢复会话、可回放检查点、可追踪同步”的能力。

> **边界**：与 `agent.md` / `sandbox.md` 不重复。  
> - `agent.md` 仍负责 Libra 自身子 Agent（`libra code`）运行时。  
> - `sandbox.md` 仍聚焦工具执行安全。  
> - 本文仅覆盖 **外部 Agent 生命周期接入与会话对象化**。

## 1. 基线核对（必须先对齐）

### 现状可复用项（已存在）
- `HookProvider`、`LifecycleEvent`、`LifecycleEventKind`、`SessionHookEnvelope` 已存在于 `src/internal/ai/hooks/*`，可复用其解析、校验与生命周期落库逻辑。
- `src/internal/ai/hooks/runtime.rs::process_hook_event_from_stdin` 已具备：读取 stdin、payload 校验、`dedup`、事件落地、会话恢复、`SessionState` 持久化、`SessionEnd` 时写 `ai_session` blob 与 `AI_REF`。
- `HistoryManager::new_with_ref` 已存在，可对任意 orphan ref 写 CAS 追加。
- 会话锁路径为 `.libra/sessions/<session_id>.lock`（`SessionStore` 现状）。
- 迁移框架为 CEX-12.5 runner：
  - 当前注册版本：`2026050301`（`automation_log`）+ `2026050302`（`agent_usage_stats`）。
  - `tests/db_migration_test.rs` 对这两个版本和名称有硬编码断言，新增迁移需同步更新该测试。
- `SessionHookEnvelope` 与 transcript_path 校验已做长度上限与空值防护。

### 现状待修正项（文档中原先不准确）
- **CLI 入口**：当前 `src/cli.rs` 没有公开 `hooks` 或 `agent hooks` 子命令。providers 安装写入的 `libra hooks ...` 命令在当前基线不可解析，必须在本任务中补齐。
- **迁移接入方式**：`builtin_migrations()` 目前直接内嵌 SQL 字符串；尚未用 `include_str!` 加载 `sql/migrations/*.sql`。
- **迁移命名语义**：`sql/migrations/README.md` 仍描述了与当前代码不一致的“文件化 + 4 位版本”；需要更新为当前版本链路。
- **分支保护范围**：`INTENT_BRANCH`（`intent`）保护现在只在 `checkout/switch` 两个命令明确触达，未在 `restore/reset` 等命令全面覆盖，不应宣称“全覆盖”。

## 2. v1 设计目标与边界

### 目标（v1）
1. 接入 7 种外部 Agent（Claude Code、Cursor、Codex、Gemini、OpenCode、GitHub Copilot CLI、Factory AI Droid）并持久化其原始 transcript。
2. 保持与现有 `refs/libra/intent`/`ai_session` 体系兼容，不影响 `libra code` 行为。
3. v1 强调可观测与可恢复：可追踪会话、可 rewind、可重放 checkpoint。
4. 支持 EntireIO 风格的 Subagent（子代理）级联追踪。
5. 与 `subagent-scaffold` 无关；默认构建可用，不引入额外 feature 依赖。

### 不做（v1）
1. 不替换/重写 `HookProvider` 全量 API；以兼容方式扩展。
2. 不强制统一整库 transcript schema；仍保留 provider 原生格式（`jsonl/sqlite/markdown/binary`）的字节语义。
3. 不在 v1 引入复杂压缩索引（先以可用性和隔离为先）。

## 3. 存储与对象模型

### 3.1 transcript 落盘：化繁为简，依赖 Git Blob 与 Packfile
> 改进分析：EntireIO 的 transcript 常以 append-only 的 JSONL 或 SQLite 存在。原计划中设计的 `agent_transcript_blob` 手动 `chunk_seq` 分块是反 Git 模式的。
我们改为在 `TurnEnd` 或 `Checkpoint` 节点，将整个 transcript 文件作为一个新的 Git Blob 写入，并将 `blob_oid` 记录在 `agent_checkpoint` 中。Git 底层的 Packfile 机制会自动且高效地对这些连续快照进行 Delta 压缩，避免了手动维护 chunk 的复杂性。
- 大文件自然流转至 R2（`client_storage::ClientStorage` + `LIBRA_STORAGE_THRESHOLD`）
- 与 `object_index` 一体化，`agent` 对象可自然复用现有云同步路径
- `object_index.o_type` 新增 `agent_transcript`

### 3.2 refs 与并行历史
- 新增并行 orphan ref：`refs/libra/agent-traces`
- 保留现有 ref：`refs/libra/intent`（`AI_REF`）
- 新增临时快照 ref：`refs/libra/agent-shadow/<session_id>/<checkpoint_id>`
- 与 `libra/intent` 完全平行，避免覆盖行为。

### 3.3 目录树（建议）

```
refs/libra/agent-traces
└── session/
    └── <session-uuid>.json        # manifest + 会话状态快照（小）
└── checkpoint/
    └── <id[:2]>/<id[2:]>/
        ├── metadata.json
        └── tree/tree.txt          # 可选：用于审计展示，不参与核心读取
```

### 3.4 会话状态与 checkpoint 表模型

新增 3 张表（移除了冗余的 `agent_transcript_blob`）；名称与字段对齐既有项目风格：

```sql
CREATE TABLE IF NOT EXISTS `agent_session` (
    `session_id` TEXT PRIMARY KEY,        -- UUIDv4
    `agent_kind` TEXT NOT NULL,           -- claude_code|cursor|codex|gemini|opencode|copilot|factory_ai
    `provider_session_id` TEXT NOT NULL,   -- provider native session id
    `thread_id` TEXT,                     -- ai_thread 映射（v2/可选）
    `state` TEXT NOT NULL CHECK(`state` IN ('pending','active','condensed','stopped','quarantined')),
    `working_dir` TEXT NOT NULL,
    `worktree_id` TEXT,
    `parent_commit` TEXT,
    `parent_session_id` TEXT,             -- 关联 Subagent 的父会话 (EntireIO 兼容)
    `metadata_json` TEXT NOT NULL DEFAULT '{}',
    `redaction_report` TEXT NOT NULL DEFAULT '{}',
    `started_at` INTEGER NOT NULL,
    `last_event_at` INTEGER NOT NULL,
    `stopped_at` INTEGER,
    `schema_version` INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS `agent_session_event` (
    `session_id` TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `seq` INTEGER NOT NULL,
    `kind` TEXT NOT NULL,
    `dedup_key` TEXT,
    `recorded_at` INTEGER NOT NULL,
    `payload_json` TEXT NOT NULL,
    PRIMARY KEY (`session_id`, `seq`)
);

CREATE TABLE IF NOT EXISTS `agent_checkpoint` (
    `checkpoint_id` TEXT PRIMARY KEY,
    `session_id` TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `parent_checkpoint_id` TEXT,
    `scope` TEXT NOT NULL CHECK(`scope` IN ('temporary','committed','subagent')),
    `parent_commit` TEXT NOT NULL,
    `tree_oid` TEXT NOT NULL,
    `transcript_blob_oid` TEXT,           -- 记录该 Checkpoint 节点时完整的 Transcript 快照 OID
    `metadata_blob_oid` TEXT NOT NULL,
    `shadow_ref` TEXT,
    `traces_commit` TEXT,
    `tool_use_id` TEXT,
    `subagent_session_id` TEXT,           -- 当 scope='subagent' 时，关联拉起的子会话 ID
    `description` TEXT,
    `created_at` INTEGER NOT NULL
);
```

### 3.5 与 projection 的关系
本层与现有 `projection/` 解耦；不在 projection 模块扩散新实体。新表放在独立数据访问层（例如 `observed_agents/storage.rs`）。

## 4. 迁移策略（与当前基线一致）

### 4.1 先决更新
- 结合当前 Libra 代码基线的 `src/internal/db/migration.rs::builtin_migrations()`，已存在 `2026050301` 和 `2026050302`。新增一个 migration 注册：`2026050303_agent_capture`。
- 文档 `sql/migrations/README.md` 同步更新（版本序列规则、注册方式）。

### 4.2 迁移实现方案（按现网实际）
当前 `builtin_migrations()` 用 inline SQL，因此 v1 可继续内嵌常量以最小化改动。
- 必须包含 `up` 与对应的 `down`（用于回滚）。
- 兼容 legacy DB 的幂等性。

### 4.3 测试覆盖
- `tests/db_migration_test.rs`：
  1) `builtin_migrations_register_current_schema_migrations` 断言更新为 `2026050303`  
  2) `run_pending` 场景确认新增 3 张表已创建  
  3) `run_pending` 幂等（重连后仍 no-op）
- 新增 migration 专用测试确保回滚无副作用（有 `down` 时）或明确注释不可回退。

## 5. 适配层与抽象重构

### 5.1 不删 `HookProvider`，新增 `ObservedAgentAdapter`
`HookProvider` 仅承载“hook 事件解析 + hook 安装 + 幂等判断”作为底座；新增 `ObservedAgentAdapter` 补充 transcript 截断与抽象能力。

> 改进分析：EntireIO 具备截断回放功能（如 `TruncateTranscriptAtUUID`）。在 Libra 中做 Rewind 操作时，必须让各 Provider 实现自己的解析逻辑，找出对应的节点并对本地的 transcript 文件进行截断覆写，以确保 Agent 的上下文也同步被回滚。

```rust
pub trait ObservedAgentAdapter: Send + Sync {
    fn provider_kind(&self) -> AgentKind;
    fn provider_name(&self) -> &'static str;
    fn parse_hook_event(&self, name: &str, env: &SessionHookEnvelope) -> Result<LifecycleEvent>;
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn command_set(&self) -> &'static [ProviderHookCommand];

    fn transcript_reader(&self, session: &AgentSessionCtx) -> Result<Option<Box<dyn TranscriptReader>>>;
    fn session_dir(&self, session: &AgentSessionCtx) -> Option<PathBuf>;
    fn resolve_session_file(&self, session: &AgentSessionCtx) -> Option<PathBuf>;
    
    // 新增：根据特定的 Checkpoint (如 tool_use_id) 截断 Transcript 数据
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>>;

    fn post_process(&self, event: &LifecycleEvent, session: &mut AgentSessionCtx, storage: &Path) -> Result<()>;
}
```

### 5.2 7 个 v1 适配器
`claude`、`gemini` 可直接改造为“继承/组合”现有 provider；`cursor`/`codex`/`opencode`/`copilot`/`factory_ai` 为 v1 新增适配器。

### 5.3 现有 provider 的迁移建议
- 保留现有 `src/internal/ai/hooks/providers/claude/*`、`gemini/*` 行为，补充实现为“既支持现有 `libra hooks`，又可由新的 `libra agent hooks` 复用”。
- 避免一刀切 trait 重命名导致安装与回放链路大规模改动。

## 6. Hook 入口与摄入流程

### 6.1 CLI 入口设计
新增两层命令：
1. `libra hooks <provider> <subcommand>`：保持与现有 claude/gemini 安装兼容（临时兼容层）
2. `libra agent hooks <provider> <subcommand>`：对外对齐外部 agent 视图（`--hidden`）

#### 命令执行路径
- 两条路径都最终走 `process_hook_event_from_stdin`，通过内部参数化版本选择写入目标：
  - `AiIntent`：现网既有 `refs/libra/intent`（`AI_REF`）
  - `AgentTraces`：新增 `refs/libra/agent-traces`

### 6.2 改造点
- 将 `process_hook_event_from_stdin` 抽离为：
  - `process_hook_event_with_target(..., target: HookTarget) -> Result<()>`
  - 保留原公开 API 做 1:1 向后兼容包装。
- 在 `AgentTraces` 路径新增：
  1. redaction（见 8）
  2. `agent_session` + `agent_session_event` upsert
  3. checkpoint 写入点（TurnEnd / SessionEnd / PostTool，写入时读取完整的 transcript 并快照为 Git Blob）

### 6.3 状态机（v1）
- `SessionStart` -> `active`（创建/恢复会话，落锁）
- `TurnStart` -> `active`
- `Compaction` -> `condensed`，`CompactionCompleted` -> `active`
- `ToolUse` -> `active`（可触发子会话，触发 Subagent Checkpoint）
- `TurnEnd` -> `stopped`（暂存 checkpoint，写入 transcript_blob_oid）
- `SessionEnd` -> `stopped`（补齐并标记可重用）

### 6.4 并发会话检测（轻量）
`TurnStart` 时查询同工作目录下同一 `state='active'` 会话数。  
> `>0` 时仅告警与记录 `concurrent_active=true`，不阻塞（避免合法并发回归）。

## 7. Checkpoint / rewind

### 7.1 Temporary checkpoint
在每个 `TurnEnd`/`tool-use` 关键点：
- `build_tree_recursive` 构造树快照（重用 `src/command/stash.rs`）
- 生成 `refs/libra/agent-shadow/<session>/<checkpoint>` commit
- 在 `agent_checkpoint.scope='temporary'` 记录

### 7.2 Committed checkpoint
`TurnEnd`/`SessionEnd` 生成可回放 checkpoint：
- `tree_oid`/`transcript_blob_oid`/`metadata_blob_oid` 统一写入 `agent_checkpoint.scope='committed'`
- 将 `agent_checkpoint.traces_commit` 与 `refs/libra/agent-traces` 的提交对应。
- `agent_traces` commit message 附带：
  - `Libra-Session: <uuid>`
  - `Libra-Agent: <provider>`
  - `Libra-Parent-Commit: <oid>`

### 7.3 Rewind
- `libra agent checkpoint rewind <id>` 涉及两步核心操作：
  1. 仅恢复工作树，不更新 HEAD，不切换 refs（非破坏性）。依赖 `restore` 的路径恢复逻辑。
  2. **覆写 Transcript**：从 `agent_checkpoint` 获取对应的 `transcript_blob_oid`，读取 Blob，通过 `ObservedAgentAdapter::truncate_transcript` 根据 Checkpoint ID 进行内容截断，最后将截断后的内容覆写回本地的 Session 文件，保证 Agent 再次运行时上下文正确回滚。

### 7.4 清理
- `libra agent clean` 删除 stopped 会话对应且已落 committed 的 `refs/libra/agent-shadow/*`。

## 8. 脱敏（redaction）与隐私边界

### 8.1 规则
新增 `src/internal/ai/observed_agents/redaction.rs`：
- 内置规则集（含 gitleaks 衍生）
- 支持 `warn` / `off` / `redact`（默认为 `redact`）

### 8.2 强制扫描路径
- `prompt`
- `tool_use.input.command`
- transcript 文件快照转换 Git Blob 前的流式替换

### 8.3 结果存证
- `agent_session.redaction_report` 保存累计结果
- checkpoint metadata 记录扫描与替换摘要（便于审计）

## 9. CLI 命令面

新增顶层：

```
libra agent
  enable [--agent <name> ...]
  disable [--agent <name> ...]
  status
  session list [--agent <name>] [--state <s>]
  session show <id> [--extract]
  session stop <id>
  session resume <id>
  session promote <id> --as-intent
  checkpoint list [--session <id>]
  checkpoint explain <id>
  checkpoint rewind <id>
  clean [--all]
  doctor
  push [--remote <name>]
  hooks <agent> <subcommand>   # hidden; 兼容入口
```

### 9.1 初始化流程
- 新增 `libra init` 时，若 `agent-traces` 未初始化，`HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch()` 在初始化链路中执行（与 `intent` 同步）。

### 9.2 分支保护
- 将 `INTENT_BRANCH` 的保护策略迁移为可配置清单（包含：`intent` 与 `agent-traces` 的 shadow）。  
- 当前代码仅保护 `checkout/switch`，因此 v1 测试必须覆盖 `restore/reset` 不应误触及这些内部 ref。

## 10. 云同步

### 10.1 object_index
- transcript blob 与 `agent_transcript` 会自然走现有 `write_git_object` + `object_index` 路径。
- `command/cloud.rs` 同步逻辑里追加 `agent` 会话对象索引（至少包括：
  `agent_session`、`agent_session_event` 关键状态字段）。

### 10.2 D1
- `d1_client` 新增 `ensure_agent_session_table` 相关方法。
- `cloud sync` 在 D1/metadata 处理流程中同步 `agent_*` 表与 `o_type='agent_transcript'`。

### 10.3 推送
- `libra agent push`：限定推送 refs 为 `refs/libra/agent-traces` + `refs/libra/agent-shadow/*`；
- 写入 `.libra/config` 维护 remote 配置（非强制默认值）。

## 11. 与现有 `libra code` 共存（v1）

1. `libra code` 仍仅写 `AI_REF` 与原生 `agent` 行为。  
2. `libra agent` 仅写 `agent-traces` 与 `agent_*` 表。  
3. `agent_session.thread_id` 在 v1 默认 `NULL`，除非明确 `promote`。  
4. v1 不做跨模型转换；v2 可从 `agent_session` 反算 `ToolCallRecord` 并回写到 `ai_thread`。

## 12. 与基线可复用列表（降风险实现）

- `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head`（`history.rs`）
- `process_hook_event_from_stdin`（`runtime.rs`）
- `LifecycleEvent` 与 `make_dedup_key`（`lifecycle.rs`）
- `append_raw_hook_event` / `apply_lifecycle_event`（事件更新）
- `SessionStore` 文件锁语义（`session/store.rs`）
- `cloud sync` 的对象索引上报链路（`command/cloud.rs`）
- `stash.rs` 的 tree 构建逻辑（checkpoint 重建）

## 13. 风险清单与验收标准（v1）

1. **高风险：兼容层空转**  
   - 验收：`Claude`/`Gemini` 现有 hook 安装与事件调用在新增命令后保持可执行，含幂等检查。
2. **高风险：迁移注册断层**  
   - 验收：`tests/db_migration_test.rs` 里版本断言覆盖新增版本；`run_pending` 对 clean DB / reopen DB 都通过。
3. **高风险：错误恢复路径缺口**  
   - 验收：`SessionEnd`/`sqlite locked`/`history CAS` 冲突均保留告警与可重试元数据；不会丢事件。
4. **高风险：分支保护遗漏**  
   - 验收：增加回归测试，证明 `intent` 与新增 `agent-traces` 相关 ref 不被 `switch/checkout` 错误操作污染；`restore/reset` 若涉及分支参数则给出拒绝提示。
5. **高风险：云同步可观测性缺失**  
   - 验收：`libra cloud sync` 后，新增 `agent_*` 与 `object_index` 中 `agent_transcript` 出现在同步结果中，且可按 `agent_session` 回放查询。

## 14. 分阶段实现建议（从小到大）

### 阶段 1（兼容与基础接入）
1. 实现 `libra hooks` + `libra agent hooks` 路由并打通 claude/gemini（兼容第一方 provider）
2. 将 hook 摄入提取为 target-aware 版本（AiIntent / AgentTraces）
3. 落地 `agent_session` 表 + `agent_session_event`，通过 Git Blob 生成 transcript 快照。
4. 完成迁移版本注册和测试

### 阶段 2（checkpoint 与会话）
1. 新增 `agent_checkpoint` 表，建立 `transcript_blob_oid` 机制。
2. 实现 temporary + committed checkpoint 生成
3. 实现 `session list/resume/checkpoint rewind`（包含 Transcript 本地覆写截断）。

### 阶段 3（隐私与扩展）
1. 上线 redaction 管线
2. 新增 cursor/codex/opencode/copilot/factory-ai 适配器（包含解析/截断 transcript 的能力）
3. 完成 `clean/doctor/push`，增强 cloud 同步
