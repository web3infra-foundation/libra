# 在 Libra 中集成 EntireIO 风格的"外部 Agent 会话捕获"能力（改进版）

> **文档定位**：把 EntireIO 的核心价值（把外部 Agent 的生命周期事件与原始 transcript 纳入版本控制）迁移到 Libra，在保留现有 `libra code` 行为不变的前提下，补齐"多外部 Agent、可恢复会话、可回放检查点、可追踪同步"的能力。
> 
> **改进原则**：
> 1. **Lean v1**：第一阶段只建骨架，不追求 7 个适配器全量上线。
> 2. **Git 原生优先**：Checkpoint 和 transcript 快照直接存为 Git 对象，SQLite 只做轻量索引，不做对象内容的二次镜像。
> 3. **编译时契约**：引入 `RedactedBytes` 类型，确保任何进入持久化路径的数据必须先过脱敏管线。
> 4. **可选接口模式**：放弃单一大 trait，采用"核心小 trait + 能力接口"的 EntireIO 注册表模式，降低新 Agent 接入成本。
> 5. **复用而非重复**：`SessionStore` 的 JSONL 事件流已足够承载 `agent_session_event`，不新建事件表。

---

## 1. 基线核对（必须先对齐）

### 1.1 现状可复用项（已存在）
- `HookProvider`、`LifecycleEvent`、`LifecycleEventKind`、`SessionHookEnvelope` 已存在于 `src/internal/ai/hooks/*`，可复用其解析、校验与生命周期落库逻辑。
- `src/internal/ai/hooks/runtime.rs::process_hook_event_from_stdin` 已具备：读取 stdin、payload 校验、`dedup`、事件落地、会话恢复、`SessionState` 持久化、`SessionEnd` 时写 `ai_session` blob 与 `AI_REF`。
- `HistoryManager::new_with_ref` 已存在，可对任意 orphan ref 写 CAS 追加。
- 会话锁路径为 `.libra/sessions/<session_id>.lock`（`SessionStore` 现状）。
- 迁移框架为 CEX-12.5 runner：当前注册版本 `2026050301`（`automation_log`）+ `2026050302`（`agent_usage_stats`）。
- `SessionHookEnvelope` 与 transcript_path 校验已做长度上限与空值防护。
- `object_index` 已支持 `blob`/`tree`/`commit`/`tag` 四种 `o_type`。

### 1.2 现状待修正项（文档中原先不准确）
- **CLI 入口**：当前 `src/cli.rs` 没有公开 `hooks` 或 `agent hooks` 子命令。providers 安装写入的 `libra hooks ...` 命令在当前基线不可解析，必须在本任务中补齐。
- **迁移接入方式**：`builtin_migrations()` 目前直接内嵌 SQL 字符串；**改进要求**：v1 新建 `sql/migrations/2026050303_agent_capture.sql`，`builtin_migrations()` 使用 `include_str!` 加载。这是 AGENTS.md 所述的演进方向，不应继续 inline。
- **迁移命名语义**：`sql/migrations/README.md` 需要更新为当前版本链路 + `include_str!` 加载规则。
- **分支保护范围**：`INTENT_BRANCH`（`intent`）保护现在只在 `checkout/switch` 两个命令明确触达，未在 `restore/reset` 等命令全面覆盖，不应宣称"全覆盖"。v1 需把 `agent-traces` 及 shadow ref 前缀加入 `is_locked_branch` 的保护清单，并在 `restore`/`reset` 等命令中增加对内部 ref 的拒绝逻辑。

---

## 2. v1 设计目标与边界

### 2.1 目标（v1）
1. 接入 **2 种外部 Agent**（Claude Code、Gemini）并持久化其原始 transcript；其余 5 种（Cursor、Codex、OpenCode、Copilot CLI、Factory AI Droid）在 v1 预留接口，v2 填充实现。
2. 保持与现有 `refs/libra/intent`/`ai_session` 体系兼容，不影响 `libra code` 行为。
3. v1 强调**可观测**：可追踪会话、可查看 checkpoint 列表、可提取 transcript 快照。
4. v1 **不做 rewind 的本地 transcript 覆写**（该能力需要各 Provider 实现 `TranscriptTruncator`，风险高，放入 v2）。
5. 与 `subagent-scaffold` 无关；默认构建可用，不引入额外 feature 依赖。

### 2.2 不做（v1）
1. 不替换/重写 `HookProvider` 全量 API；以兼容方式扩展。
2. 不强制统一整库 transcript schema；仍保留 provider 原生格式（`jsonl/sqlite/markdown/binary`）的字节语义。
3. **不做 `agent_checkpoint` 表的 `transcript_blob_oid` 字段**（见 §3.1 改进）。
4. 不引入复杂压缩索引。
5. **不做 `libra agent checkpoint rewind`**（本地覆写 transcript 风险高，且需要每个 Provider 实现截断逻辑，放入 v2）。
6. 不做 D1 全量 `agent_session_event` 同步（事件量可能极大，v1 仅同步 `agent_session` 摘要 + checkpoint commit）。

---

## 3. 存储与对象模型（核心改进）

### 3.1 transcript 落盘：Git Blob 直接快照，SQLite 不镜像 OID
> **改进分析（重大）**：原方案设计 `agent_checkpoint.transcript_blob_oid` 把 Git OID 再抄进 SQLite，形成双重维护。Git 本身就是对象数据库，checkpoint 的 commit 树已经包含了 transcript blob 的引用。我们改为：
> - 在 `TurnEnd`/`SessionEnd` 时，将**脱敏后的**完整 transcript 作为一个 Git Blob 写入。
> - 将该 blob 挂入 checkpoint commit 的树中（路径：`transcript/<provider>`）。
> - **SQLite 的 `agent_checkpoint` 表仅记录 `traces_commit`（即 checkpoint commit 的 hash），不记录 `transcript_blob_oid`**。需要读取 transcript 时，从 commit 的树中解析 `transcript/<provider>` 条目即可。
> 
> 这消除了 OID 漂移风险，且与 `stash.rs` 的树构建逻辑完全一致。

- 大文件自然流转至 R2（`client_storage::ClientStorage` + `LIBRA_STORAGE_THRESHOLD`）。
- `object_index.o_type` 新增 `agent_transcript`（仅用于统计和云同步过滤，不影响读取路径）。

### 3.2 refs 与并行历史
- **新增并行 orphan ref**：`refs/libra/agent-traces`
- **保留现有 ref**：`refs/libra/intent`（`AI_REF`）
- **删除原方案的 shadow ref 设计**：`refs/libra/agent-shadow/<session_id>/<checkpoint_id>` 过于复杂且 Git ref 不适合高频动态创建（ref 更新是全局锁）。改为：
  - Temporary checkpoint 以 **orphan commit** 形式存在 `refs/libra/agent-traces` 的历史中，但用 `agent_checkpoint.scope='temporary'` 标记。
  - Committed checkpoint 同样以 orphan commit 存在，用 `scope='committed'` 标记。
  - 清理时遍历 `agent_checkpoint` 表，按 `scope='temporary'` 且 `session.state='stopped'` 的条件，从 `agent-traces` 的 commit 历史中移除对应节点（通过重写 orphan branch 的 tip）。

### 3.3 目录树（建议）

```
refs/libra/agent-traces  (orphan branch)
└── checkpoint/
    └── <id[:2]>/<id[2:]>/
        ├── metadata.json      # checkpoint 元数据（小）
        └── tree/              # 可选：用于审计展示，不参与核心读取
```

每个 checkpoint 对应一个 orphan commit：
- `tree` 包含：`metadata.json`、`transcript/<provider>`（blob）、`files/`（可选的树快照）。
- `commit message` 包含 trail metadata：
  - `Libra-Session: <uuid>`
  - `Libra-Agent: <provider>`
  - `Libra-Parent-Commit: <oid>`
  - `Libra-Checkpoint-ID: <id>`
  - `Libra-Scope: temporary|committed`

### 3.4 会话状态与 checkpoint 表模型（精简为 2 张表）

> **改进分析**：原方案 3 张表中的 `agent_session_event` 与现有 `SessionStore` 的 JSONL 事件流功能重叠。`SessionStore` 已经以 append-only JSONL 形式存储了所有原始 hook 事件，且已有文件锁和恢复机制。v1 不复建一张 SQLite 事件表，而是：
> - 继续用 `SessionStore` 存储活跃会话的事件流。
> - 在 `SessionEnd` 时，把 `SessionStore` 的 JSONL 文件整体作为 Git Blob 写入，并关联到最终的 committed checkpoint commit。
> - 仅在 SQLite 中保留轻量的 `agent_session`（会话摘要）和 `agent_checkpoint`（checkpoint 索引）。

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

CREATE TABLE IF NOT EXISTS `agent_checkpoint` (
    `checkpoint_id` TEXT PRIMARY KEY,
    `session_id` TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `parent_checkpoint_id` TEXT,
    `scope` TEXT NOT NULL CHECK(`scope` IN ('temporary','committed','subagent')),
    `parent_commit` TEXT NOT NULL,
    `tree_oid` TEXT NOT NULL,             -- checkpoint commit 的根树 OID
    `metadata_blob_oid` TEXT NOT NULL,    -- metadata.json blob OID
    `traces_commit` TEXT NOT NULL,        -- 对应 refs/libra/agent-traces 上的 orphan commit hash
    `tool_use_id` TEXT,
    `subagent_session_id` TEXT,           -- 当 scope='subagent' 时，关联拉起的子会话 ID
    `description` TEXT,
    `created_at` INTEGER NOT NULL
);
```

**删除原方案中的 `agent_session_event` 表**。

### 3.5 与 projection 的关系
本层与现有 `projection/` 解耦；不在 projection 模块扩散新实体。新表放在独立数据访问层（例如 `src/internal/ai/observed_agents/storage.rs`）。

---

## 4. 迁移策略（推动向 `include_str!` 演进）

### 4.1 先决更新
- 新建 `sql/migrations/2026050303_agent_capture.sql`，内容包含上述 2 张表的 DDL + 索引。
- `src/internal/db/migration.rs::builtin_migrations()` 中新增：
  ```rust
  Migration {
      version: 2026050303,
      name: "agent_capture",
      up: include_str!("../../../sql/migrations/2026050303_agent_capture.sql"),
      down: Some(include_str!("../../../sql/migrations/2026050303_agent_capture_down.sql")),
  }
  ```
- 同步新建 `2026050303_agent_capture_down.sql`（DROP INDEX + DROP TABLE）。
- 更新 `sql/migrations/README.md`：说明文件化迁移的命名规则（`<version>_<name>.sql`）、`include_str!` 加载方式、以及版本序列规则。

### 4.2 兼容性说明
- `up` DDL 必须保持幂等（`CREATE TABLE IF NOT EXISTS`、`CREATE INDEX IF NOT EXISTS`）。
- 对于 legacy DB（已存在旧表名），若未来与第三方 schema 冲突，使用 `IF NOT EXISTS` 可安全跳过。

### 4.3 测试覆盖
- `tests/db_migration_test.rs`：
  1. `builtin_migrations_register_current_schema_migrations` 断言更新为 `2026050303`，且 runner 长度变为 3。
  2. `run_pending` 场景确认新增 2 张表已创建。
  3. `run_pending` 幂等（重连后仍 no-op）。
- 新增 `tests/agent_capture_migration_test.rs`：验证 `up`/`down` 的往返正确性，且 `down` 不遗留索引。

---

## 5. 适配层与抽象重构（重大改进）

### 5.1 放弃 `ObservedAgentAdapter` 大单 trait，采用可选接口模式
> **改进分析**：原方案的 `ObservedAgentAdapter` 是一个 10+ 方法的大 trait，任何新 Agent 必须一次性实现所有方法。这与 EntireIO 的"核心小接口 + 可选能力接口"模式相反，会严重阻碍社区贡献新 Agent。我们改为：

```rust
/// 核心 trait：每个被观测的 Agent 必须实现的最小集合。
/// 仅负责"身份"和"transcript 读取"。
pub trait ObservedAgent: Send + Sync {
    fn provider_kind(&self) -> AgentKind;
    fn provider_name(&self) -> &'static str;
    
    /// 读取该 Agent 的原始 transcript 字节。
    /// 返回 None 表示当前无可用 transcript（例如会话尚未产生文件）。
    fn read_transcript(&self, session: &AgentSessionCtx) -> Result<Option<Vec<u8>>>;
    
    /// 返回该 Agent 在本仓库中的保护目录（如 `.claude`、`.gemini`）。
    /// 用于 rewind/clean 时的安全边界。
    fn protected_dirs(&self) -> &'static [&'static str];
}

/// Hook 支持：仅当 Agent 提供生命周期 hook 时才实现。
pub trait ObservedAgentHooks: ObservedAgent {
    fn supported_commands(&self) -> &'static [ProviderHookCommand];
    fn parse_hook_event(&self, name: &str, env: &SessionHookEnvelope) -> Result<LifecycleEvent>;
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()>;
    fn uninstall_hooks(&self) -> Result<()>;
    fn hooks_are_installed(&self) -> Result<bool>;
}

/// Transcript 截断支持：v2 才需要，用于 rewind 时覆写本地 transcript。
pub trait TranscriptTruncator: ObservedAgent {
    /// 返回截断后的 transcript 字节。
    /// 输入为完整 transcript（已由调用者从 Git Blob 读取）。
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>>;
}

/// Transcript 分块支持：用于云同步或超大文件处理。
pub trait TranscriptChunker: ObservedAgent {
    fn chunk_transcript(&self, content: &[u8], max_size: usize) -> Result<Vec<Vec<u8>>>;
    fn reassemble_transcript(&self, chunks: &[Vec<u8>]) -> Result<Vec<u8>>;
}
```

### 5.2 v1 适配器范围缩小
- **v1 必实现**：`claude`、`gemini`。将现有 `src/internal/ai/hooks/providers/claude/*` 和 `gemini/*` 重构为同时实现 `HookProvider`（向后兼容）和 `ObservedAgent`/`ObservedAgentHooks`。
- **v1 预留接口**：`cursor`、`codex`、`opencode`、`copilot`、`factory_ai`。在注册表中占位，返回 `Err(AgentNotYetImplemented)` 或空操作，但 CLI 的 `libra agent enable` 可列出它们（标记为 preview）。

### 5.3 现有 provider 的迁移建议
- 保留现有 `src/internal/ai/hooks/providers/claude/*`、`gemini/*` 的 `HookProvider` 实现。
- 新增一个轻量的 `ObservedAgent` wrapper（例如 `ClaudeObservedAgent`），内部持有现有的 `ClaudeProvider`，将 `read_transcript` 代理到 `ClaudeProvider` 的 transcript 路径解析逻辑。
- 避免 trait 重命名导致安装与回放链路大规模改动。

---

## 6. Hook 入口与摄入流程

### 6.1 CLI 入口设计
新增两层命令：
1. `libra hooks <provider> <subcommand>`：保持与现有 claude/gemini 安装兼容（临时兼容层）。
2. `libra agent hooks <provider> <subcommand>`：对外对齐外部 agent 视图（`--hidden`）。

#### 命令执行路径
- 两条路径都最终走 `process_hook_event_from_stdin`，通过内部参数化版本选择写入目标：
  - `AiIntent`：现网既有 `refs/libra/intent`（`AI_REF`）
  - `AgentTraces`：新增 `refs/libra/agent-traces`

### 6.2 改造点
- 将 `process_hook_event_from_stdin` 抽离为：
  - `process_hook_event_with_target(..., target: HookTarget) -> Result<()>`
  - 保留原公开 API 做 1:1 向后兼容包装。
- 在 `AgentTraces` 路径新增：
  1. redaction（见 §8）
  2. `agent_session` upsert（**不**写 `agent_session_event` 表，事件继续落 `SessionStore` JSONL）
  3. checkpoint 写入点（TurnEnd / SessionEnd / PostTool）：
     - 读取完整 transcript（通过 `ObservedAgent::read_transcript`）
     - 过 redaction 管线 → 得到 `RedactedBytes`
     - 将 redacted transcript 写为 Git Blob
     - 构造 checkpoint commit（树含 `metadata.json` + `transcript/<provider>`）
     - 将 commit append 到 `refs/libra/agent-traces`（通过 `HistoryManager::new_with_ref`）
     - 在 `agent_checkpoint` 表插入索引记录（`traces_commit` 指向该 commit）

### 6.3 状态机（v1）
- `SessionStart` -> `active`（创建/恢复会话，落锁）
- `TurnStart` -> `active`
- `Compaction` -> `condensed`，`CompactionCompleted` -> `active`
- `ToolUse` -> `active`（可触发子会话，触发 Subagent Checkpoint）
- `TurnEnd` -> `stopped`（生成 committed checkpoint，写入 transcript blob，append 到 `agent-traces`）
- `SessionEnd` -> `stopped`（同上，并删除 `SessionStore` 缓存）

### 6.4 并发会话检测（轻量）
`TurnStart` 时查询同工作目录下同一 `state='active'` 会话数。  
> `>0` 时仅告警与记录 `concurrent_active=true`，不阻塞（避免合法并发回归）。

---

## 7. Checkpoint / rewind（v1 简化）

### 7.1 Checkpoint 生成流程
在每个 `TurnEnd`/`SessionEnd` 关键点：
1. `build_tree_recursive` 构造树快照（复用 `src/command/stash.rs` 的树构建逻辑，但范围限定在 `protected_dirs` 之外的用户代码）。
2. 构造 checkpoint commit：
   - `metadata.json` blob：含 checkpoint_id、session_id、scope、timestamp、description。
   - `transcript/<provider>` blob：redacted transcript（从 `ObservedAgent::read_transcript` 读取并脱敏）。
   - `tree/`（可选）：工作目录树快照 blob。
3. 写 orphan commit 到 `refs/libra/agent-traces`（`HistoryManager` CAS 追加）。
4. 在 `agent_checkpoint` 表插入记录，`traces_commit` 字段指向该 commit hash。

### 7.2 Temporary vs Committed 的区分
- **Temporary**：`scope='temporary'`，在 `agent_checkpoint` 表中标记，但在 `agent-traces` 的 commit message 中同样带有 `Libra-Scope: temporary`。v1 的 temporary checkpoint 也写入 `agent-traces`（而非 shadow ref），只是会在 `clean` 时被 GC。
- **Committed**：`scope='committed'`，长期保留。

### 7.3 Rewind（v1 仅查询，v2 实现截断）
- `libra agent checkpoint show <id>`：展示 checkpoint 的 metadata、关联的 transcript 长度、tree 差异摘要。
- `libra agent checkpoint rewind <id>`：v1 **仅支持恢复工作树**（调用 `restore` 逻辑，不更新 HEAD）。**不支持覆写本地 transcript**（需要 `TranscriptTruncator` 接口，放入 v2）。
- 在 v1 的 CLI 中，`rewind` 命令加 `--dry-run` 默认行为，并打印警告："Transcript truncation for <provider> is not yet implemented in v1."。

### 7.4 清理
- `libra agent clean`：
  1. 查询 `agent_session` 中 `state='stopped'` 的会话。
  2. 对这些会话，查询 `agent_checkpoint` 中 `scope='temporary'` 的记录。
  3. 从 `refs/libra/agent-traces` 的 commit 历史中移除这些 temporary checkpoint（通过重写 orphan branch tip 到前一个非 temporary commit）。
  4. 删除 `agent_checkpoint` 表中对应记录。
  5. **不**删除 Git 对象（由 `git gc` 自然回收）。

---

## 8. 脱敏（redaction）与隐私边界（重大改进）

### 8.1 引入 `RedactedBytes` 编译时契约
> **改进分析**：EntireIO 的 `redact.RedactedBytes` 是一个零开销包装类型，确保任何进入 checkpoint 存储或云同步路径的数据必须先经过脱敏。原方案把 redaction 作为"可选管线"描述，没有强制契约。我们改为：

```rust
/// 已脱敏的字节数据。消费者（checkpoint store、cloud sync、history append）
/// 只接受此类型，从而在编译时强制脱敏契约。
pub struct RedactedBytes {
    data: Vec<u8>,
}

impl RedactedBytes {
    /// 仅供 redaction 模块内部和可信的持久化回读路径使用。
    pub(crate) fn new_unchecked(data: Vec<u8>) -> Self { ... }
    pub fn bytes(&self) -> &[u8] { &self.data }
    pub fn len(&self) -> usize { self.data.len() }
}
```

**强制接受 `RedactedBytes` 的函数**：
- `HistoryManager::append` 在 `AgentTraces` 路径的调用
- `write_git_object` 的 transcript 写入封装层
- Cloud sync 的 `agent_transcript` 对象上传

### 8.2 Redaction 引擎设计
新建 `src/internal/ai/observed_agents/redaction.rs`，直接参考 EntireIO 的 `redact/` 包实现：

| 检测器 | 说明 | 优先级 |
|---|---|---|
| `betterleaks` | 260+ 已知 secret 格式（API key、token 等） | P0 |
| 熵检测 | Shannon entropy > 4.5 的 alphanumeric ≥10 字符 | P1 |
| Credential URI | `postgres://user:pass@host/db` 等内嵌密码 URL | P1 |
| DB Connection String | JDBC、keyword DSN、semicolon-separated | P1 |
| Bounded K/V | `DB_PASSWORD=...` 等键值对 | P2 |
| PII（opt-in）| email、phone、address（默认关闭） | P3 |

**JSON-aware 替换**：
- 对 JSON/JSONL 输入，逐行解析，递归 walk 字段。
- 跳过 ID 字段（`*id`、`*ids`）和路径字段（`filepath`、`cwd`、`path`）。
- 跳过 image 对象（`type: image`、`base64`）。
- 替换为 `REDACTED` 或 `[REDACTED_<label>]`（PII 场景）。
- Placeholder 白名单：`changeme`、`placeholder`、`<password>`、`***`、`${VAR}`、已有的 `REDACTED`。

### 8.3 扫描路径与结果存证
- **强制扫描路径**：
  - `prompt`（`TurnStart` 事件中的 prompt 字段）
  - `tool_use.input.command`
  - transcript 文件快照转换为 Git Blob **之前**的流式替换
- **结果存证**：
  - `agent_session.redaction_report` 保存累计结果（JSON：各检测器命中次数、替换位置摘要）。
  - checkpoint 的 `metadata.json` 中记录扫描摘要（便于审计）。

### 8.4 配置
- `settings.redaction` 字段（`warn` / `off` / `redact`），默认 `redact`。
- `warn` 模式：只记录 `redaction_report`，不实际替换（用于审计）。
- `off` 模式：跳过 redaction（不推荐，需用户显式开启）。

---

## 9. CLI 命令面（v1 精简）

新增顶层：

```
libra agent
  enable [--agent <name> ...]      # 安装 hook，注册 agent
  disable [--agent <name> ...]     # 卸载 hook
  status                           # 显示当前活跃的会话和最近 checkpoint
  session list [--agent <name>] [--state <s>]
  session show <id> [--extract-transcript <path>]
  session stop <id>
  session resume <id>              # v1 仅恢复 metadata，不恢复 transcript 上下文
  checkpoint list [--session <id>]
  checkpoint show <id>             # v1 替代 explain，展示 metadata + diff 摘要
  checkpoint rewind <id> --dry-run # v1 仅 dry-run，实际 rewind 放入 v2
  clean [--all]
  doctor
  push [--remote <name>]
  hooks <agent> <subcommand>       # hidden; 兼容入口
```

**v1 删除的命令**：
- `session promote <id> --as-intent`（v2 再做跨体系提升）
- `checkpoint explain <id>`（合并到 `checkpoint show`）

### 9.1 初始化流程
- 新增 `libra init` 时，若 `agent-traces` 未初始化，`HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch()` 在初始化链路中执行（与 `intent` 同步）。

### 9.2 分支保护
- 将 `INTENT_BRANCH` 的保护策略迁移为**可配置清单**（包含：`intent`、`agent-traces`、`agent-shadow/*` 前缀）。
- 当前代码仅保护 `checkout/switch`，因此 v1 测试必须覆盖 `restore/reset` 不应误触及这些内部 ref。
- 在 `src/internal/branch.rs` 的 `is_locked_branch` 中新增：
  ```rust
  pub fn is_locked_branch(name: &str) -> bool {
      name == DEFAULT_BRANCH
          || name == INTENT_BRANCH
          || name == "agent-traces"
          || name.starts_with("agent-shadow/")
  }
  ```

---

## 10. 云同步（v1 最小可行）

### 10.1 object_index
- transcript blob 与 `agent_transcript` 会自然走现有 `write_git_object` + `object_index` 路径。
- `command/cloud.rs` 同步逻辑里追加 `agent` 对象索引：
  - 同步 `agent_session` 表（仅摘要，不含事件明细）。
  - 同步 `agent_checkpoint` 表（仅索引，不含 blob 内容）。
  - `object_index` 中 `o_type='agent_transcript'` 的 blob 走正常对象同步。

### 10.2 D1
- `d1_client` 新增 `ensure_agent_session_table` 和 `ensure_agent_checkpoint_table`。
- **v1 不同步 `agent_session_event`**：原因：
  - 事件量可能极大（每次 turn 产生多条）。
  - 完整 transcript 已作为 blob 同步，事件明细可从 blob 反解。
  - D1 有行数限制和查询成本，全量事件同步不经济。

### 10.3 推送
- `libra agent push`：限定推送 refs 为 `refs/libra/agent-traces`；**不推送 shadow refs**（v1 已无 shadow refs）。
- 写入 `.libra/config` 维护 remote 配置（非强制默认值）。

---

## 11. 与现有 `libra code` 共存（v1）

1. `libra code` 仍仅写 `AI_REF` 与原生 `agent` 行为。
2. `libra agent` 仅写 `agent-traces` 与 `agent_*` 表。
3. `agent_session.thread_id` 在 v1 默认 `NULL`，除非明确 `promote`（v2）。
4. v1 不做跨模型转换；v2 可从 `agent_session` 反算 `ToolCallRecord` 并回写到 `ai_thread`。
5. **资源隔离保证**：`libra code` 的 `SessionStore` 路径（`.libra/sessions/`）与 `libra agent` 的 `SessionStore` 路径使用不同的子目录（`.libra/sessions/agent/` vs `.libra/sessions/code/`），避免文件锁冲突。

---

## 12. 与基线可复用列表（降风险实现）

- `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head`（`history.rs`）
- `process_hook_event_from_stdin`（`runtime.rs`）—— 提取为 `process_hook_event_with_target`
- `LifecycleEvent` 与 `make_dedup_key`（`lifecycle.rs`）
- `append_raw_hook_event` / `apply_lifecycle_event`（事件更新）
- `SessionStore` 文件锁语义（`session/store.rs`）—— v1 继续使用 JSONL，不复建事件表
- `cloud sync` 的对象索引上报链路（`command/cloud.rs`）
- `stash.rs` 的 tree 构建逻辑（checkpoint 重建）
- `object_index` CRUD（`internal/db.rs`）—— 追加 `agent_transcript` o_type

---

## 13. 风险清单与验收标准（v1 修订）

| 风险 | 严重程度 | 验收标准 |
|---|---|---|
| **兼容层空转** | P1 | `Claude`/`Gemini` 现有 hook 安装与事件调用在新增命令后保持可执行，含幂等检查。旧 `libra hooks claude session-start` 行为不变。 |
| **迁移注册断层** | P1 | `tests/db_migration_test.rs` 里版本断言覆盖 `2026050303`；`run_pending` 对 clean DB / reopen DB 都通过；`down` 往返测试通过。 |
| **Redaction 遗漏导致 secret 泄漏** | P0 | 所有进入 `agent-traces` 的 transcript blob 必须通过 `RedactedBytes` 类型；新增 `tests/redaction_contract_test.rs` 验证：未脱敏的 `Vec<u8>` 无法传入 checkpoint writer（编译失败或运行时 panic）。 |
| **Git 对象膨胀** | P1 | 每个 checkpoint 的 transcript blob 大小被监控；`clean` 能清理 temporary checkpoint 的 commit 和 blob；`object_index` 中 `agent_transcript` 类型可被 `git gc` 回收。 |
| **分支保护遗漏** | P1 | 增加回归测试，证明 `intent` 与 `agent-traces` 不被 `switch/checkout` 错误操作污染；`restore/reset` 若涉及分支参数则给出拒绝提示。 |
| **并发会话锁冲突** | P1 | 同工作目录并发 `TurnStart` 不阻塞，但记录 `concurrent_active=true`；`SessionStore` 的锁超时后触发恢复逻辑，不丢事件。 |
| **云同步可观测性缺失** | P2 | `libra cloud sync` 后，`agent_session` / `agent_checkpoint` 摘要和 `object_index` 中 `agent_transcript` 出现在同步结果中。 |
| **v1 过度承诺 rewind** | P1 | `checkpoint rewind` 在 v1 明确为 `--dry-run` 或不可用，不误导用户。 |

---

## 14. 分阶段实现建议（从小到大，进一步细化）

### 阶段 1（兼容与基础接入，约 2-3 周）
1. **迁移基础设施**：
   - 新建 `sql/migrations/2026050303_agent_capture.sql` 和 `down` 文件。
   - 改造 `builtin_migrations()` 使用 `include_str!`。
   - 更新 `tests/db_migration_test.rs` 和 `sql/migrations/README.md`。
2. **Redaction 骨架**：
   - 新建 `src/internal/ai/observed_agents/redaction.rs`，实现 `RedactedBytes` 类型 + 基础规则（betterleaks 集成 + 熵检测）。
   - 新增 `tests/redaction_contract_test.rs`（编译时契约测试）。
3. **CLI 兼容层**：
   - 在 `src/cli.rs` 新增 `Agent` 命令枚举（`Agent(AgentArgs)`）。
   - 实现 `libra hooks <provider> <subcommand>` 路由（向后兼容）。
   - 实现 `libra agent hooks <provider> <subcommand>` 路由（hidden）。
4. **Hook 摄入改造**：
   - 将 `process_hook_event_from_stdin` 提取为 `process_hook_event_with_target(..., target: HookTarget)`。
   - 保留原 API 做向后兼容包装。
   - 新增 `AgentTraces` 路径的 `agent_session` upsert（不写事件表，继续用 `SessionStore`）。
5. **分支保护**：
   - 扩展 `is_locked_branch` 保护 `agent-traces`。
   - 在 `restore`/`reset` 中增加对 locked branch 的拒绝逻辑。
   - 新增回归测试。

**阶段 1 验收**：Claude/Gemini 的现有 hook 安装和调用不受影响；`agent_session` 表可创建；`libra agent status` 可列出活跃会话。

### 阶段 2（Checkpoint 与 Git 存储，约 2-3 周）
1. **Checkpoint commit 生成**：
   - 复用 `stash.rs` 的树构建逻辑，构造 checkpoint commit（含 `metadata.json` + `transcript/<provider>`）。
   - 用 `HistoryManager::new_with_ref("refs/libra/agent-traces")` 做 CAS 追加。
   - 在 `TurnEnd`/`SessionEnd` 触发写入。
2. **`agent_checkpoint` 表**：
   - 仅做索引（`traces_commit` 指向 orphan commit hash）。
   - 支持 `scope='temporary'` 和 `'committed'`。
3. **CLI 命令**：
   - `libra agent checkpoint list`
   - `libra agent checkpoint show <id>`（展示 metadata + diff 摘要）
   - `libra agent session show <id> --extract-transcript <path>`
4. **清理逻辑**：
   - `libra agent clean`：GC temporary checkpoint commits，重写 `agent-traces` orphan branch tip。

**阶段 2 验收**：完整会话可生成多个 checkpoint；`libra agent checkpoint list` 可见；`clean` 后 temporary checkpoint 从 `agent-traces` 移除且 SQLite 索引同步清理。

### 阶段 3（扩展与云同步，约 2 周）
1. **Redaction 完善**：
   - 接入完整的 betterleaks 规则集。
   - PII 检测（opt-in）。
   - `redaction_report` 和 checkpoint metadata 摘要。
2. **新 Agent 适配器**：
   - 实现 `cursor`、`codex`、`opencode`、`copilot`、`factory_ai` 的 `ObservedAgent` 接口。
   - 实现 `ObservedAgentHooks`（如有 hook 支持）。
   - CLI `libra agent enable` 可列出全部 7 个 agent（已实现标 stable，未实现标 preview）。
3. **云同步**：
   - `d1_client` 同步 `agent_session` + `agent_checkpoint` 摘要。
   - `object_index` 中 `agent_transcript` blob 走正常同步链路。
   - `libra agent push` 推送 `refs/libra/agent-traces`。

**阶段 3 验收**：7 个 agent 均可被 `enable` 列出；redaction 对模拟 transcript 的命中测试通过；cloud sync 后 D1 可见 agent 数据。

### 阶段 4（Rewind 与跨体系集成，v2 规划）
1. 实现 `TranscriptTruncator` 接口。
2. `libra agent checkpoint rewind <id>` 支持本地 transcript 覆写回滚。
3. `session promote --as-intent` 跨体系提升。
4. 从 `agent_session` 反算 `ToolCallRecord` 回写 `ai_thread`。

---

## 15. 与 EntireIO 的差异化设计决策记录

| 决策点 | EntireIO 做法 | Libra 改进做法 | 理由 |
|---|---|---|---|
| Checkpoint 存储 | Shadow branch (`entire/<hash>`) + committed branch (`entire/checkpoints/v1`) | 单 orphan ref (`refs/libra/agent-traces`)，scope 用 metadata 区分 | 减少 ref 数量，避免 Git ref 全局锁瓶颈；清理时只需重写一个 orphan branch tip |
| 事件存储 | `.git/entire-sessions/<id>.json`（共享 across worktrees） | 继续用 `SessionStore` JSONL（`.libra/sessions/`），但子目录隔离 | 复用现有文件锁和恢复机制，减少新路径 |
| Redaction 契约 | `RedactedBytes` 编译时类型 | **直接引入** `RedactedBytes` 编译时类型 | 这是 EntireIO 最成功的安全设计，不应丢弃 |
| Agent 接口 | 核心 `Agent` + 可选接口（`HookSupport`、`TranscriptAnalyzer` 等） | **直接引入** 可选接口模式 | 降低社区贡献门槛，避免大 trait 负担 |
| Transcript 截断 | 各 Agent 实现 `TruncateTranscriptAtUUID` | v1 不实现，v2 通过 `TranscriptTruncator` 可选接口引入 | 降低 v1 风险，先验证存储和观测链路 |
| SQLite 角色 | 仅 session 状态，checkpoint 全在 Git | 同上：SQLite 只做轻量索引 | 避免双重维护 OID |

---

*文档版本：改进版 v1.0*
*基线：Libra @ 2026-05-04，EntireIO @ 2026-05-04*
