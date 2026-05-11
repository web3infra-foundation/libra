# 在 Libra 中集成 EntireIO 风格的"外部 Agent 会话捕获"能力

> **文档定位**：把 EntireIO 的核心价值（把外部 Agent 的生命周期事件与原始 transcript 纳入版本控制）迁移到 Libra，在保留现有 `libra code` 行为不变的前提下，补齐"多外部 Agent、可恢复会话、可回放检查点、可追踪同步"的能力。
>
> **与兄弟改进计划的边界**：
> - [agent.md](agent.md)（Part A/B/C）：Libra 自身 Agent 子系统（`libra code` 内部运行时、Step 1/2 sub-agent、TUI Automation Control）。本文不重复。
> - [sandbox.md](sandbox.md)：工具执行边界与权限。本文与之正交。
> - 本文：**外部 Agent 生命周期接入与会话对象化**。

> **改进原则（融合自前两轮迭代）**：
> 1. **Lean v1**：仅 Claude + Gemini 上线，其余 5 个 Agent 在 v1 占位为 preview，v2 填充。
> 2. **Git 原生优先**：Checkpoint 与 transcript 直接存为 Git 对象，SQLite 只做轻量索引——不再把 transcript_blob_oid 抄进 SQLite。
> 3. **编译时契约**：引入 `RedactedBytes` 类型，未脱敏字节在类型层面无法进入持久化路径。
> 4. **可选接口模式**：`ObservedAgent` 小核心 + `ObservedAgentHooks` / `TranscriptTruncator` / `TranscriptChunker` 能力 trait，降低新 Agent 接入成本。
> 5. **复用而非重复**：`SessionStore` 的 JSONL 事件流已足够承载会话事件，不另建 `agent_session_event` 表。

---

## 1. 基线核对（已对仓库验证）

### 1.1 现状可复用项

| 资产 | 位置 | 用途 |
|------|------|------|
| `HookProvider` trait + Claude/Gemini provider | [src/internal/ai/hooks/provider.rs](../../src/internal/ai/hooks/provider.rs)、[hooks/providers/](../../src/internal/ai/hooks/providers/) | 已能解析 hook envelope → `LifecycleEvent`，并安装 hook 配置 |
| `LifecycleEvent` / `LifecycleEventKind` / `SessionHookEnvelope` / `make_dedup_key` / `apply_lifecycle_event` / `validate_session_hook_envelope` / `append_raw_hook_event` | [hooks/lifecycle.rs](../../src/internal/ai/hooks/lifecycle.rs) | 完整的事件模型与状态机原语 |
| `process_hook_event_from_stdin` | [hooks/runtime.rs:139](../../src/internal/ai/hooks/runtime.rs) | stdin → envelope → dedup → apply → 写 ai_session blob 到 `AI_REF` |
| `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head` / `update_ref_if_matches` | [src/internal/ai/history.rs:176](../../src/internal/ai/history.rs) | 任意 orphan ref 上的 CAS 追加，已带 SQLite-busy 与 head-conflict 双重重试 |
| `SessionStore::lock_session` + `SessionFileLock` | [src/internal/ai/session/store.rs:338](../../src/internal/ai/session/store.rs)（`SESSION_LOCK_TIMEOUT = 5s`、`STALE_SESSION_LOCK_AGE = 30s`） | 跨进程会话文件锁，基于 `.libra/sessions/<id>.lock` |
| 分层存储 | [src/utils/client_storage.rs:336](../../src/utils/client_storage.rs) | 大 blob 自动按 `LIBRA_STORAGE_THRESHOLD` 推到 R2 |
| 云同步 | [src/command/cloud.rs:192](../../src/command/cloud.rs) | 增量按 `object_index` 表迭代 |
| Migration runner（CEX-12.5） | [src/internal/db/migration.rs:499](../../src/internal/db/migration.rs)、[sql/migrations/README.md](../../sql/migrations/README.md) | 已注册 `2026050301`(`automation_log`) + `2026050302`(`agent_usage_stats`)，inline SQL；`builtin_runner` / `run_builtin_migrations` 公开 API 可用 |
| `stash::build_tree_recursive` | [src/command/stash.rs](../../src/command/stash.rs) | 工作目录 → tree，已处理 index 合并、忽略文件、子模块 |
| `restore` 路径还原 | [src/command/restore.rs](../../src/command/restore.rs) | rewind 复用此路径 |
| `object_index` 表 | [src/utils/object.rs](../../src/utils/object.rs) + [src/internal/db.rs](../../src/internal/db.rs) | 自动驱动云同步 |

### 1.2 现状待修正项（前轮文档曾不准确）

| 现状 | 必须修正 |
|------|---------|
| `src/cli.rs` 的 `Commands` 枚举**无** `Hooks` 或 `Agent` 变体（grep 已确认） | 本任务必须新增 `Commands::Hooks(HooksArgs)`（兼容层）与 `Commands::Agent(AgentArgs)`（新顶层） |
| `builtin_migrations()` 当前**用 inline SQL 字符串**，未走 `include_str!`（[migration.rs:499-540](../../src/internal/db/migration.rs)） | v1 新建 `sql/migrations/2026050303_agent_capture.sql` 并改用 `include_str!` 加载——既符合 [sql/migrations/README.md](../../sql/migrations/README.md) 描述的演进方向，又把 SQL 与 Rust 解耦 |
| [sql/migrations/README.md](../../sql/migrations/README.md) 仍写"4 位版本号 NNNN"，与现网 `2026050301` 不一致 | 同步更新为"YYYYMMDDNN 形式 + `include_str!` 加载规则" |
| `is_locked_branch` 仅匹配 `DEFAULT_BRANCH \| INTENT_BRANCH`（[branch.rs:45](../../src/internal/branch.rs)） | 扩展为可配清单，加入 `agent-traces`；并在 `restore`/`reset` 也调用 `is_locked_branch`（目前仅 `checkout`/`switch` 调用） |
| `tests/db_migration_test.rs` **硬编码** `vec![2026050301, 2026050302]` 与 `vec!["automation_log", "agent_usage_stats"]`（[lines 47-48, 53, 985](../../tests/db_migration_test.rs)） | 新增迁移时必须同步把这三处更新到 `2026050303` / `agent_capture` |

---

## 2. v1 设计目标与边界

### 2.1 目标（v1）

1. 接入 **Claude Code、Gemini** 两种外部 Agent 并持久化其原始 transcript；其余 5 种（Cursor、Codex、OpenCode、GitHub Copilot CLI、Factory AI Droid）在 v1 注册为 **preview**，CLI 可见但走存根实现。
2. 保持 `refs/libra/intent` / `ai_session` / `libra code` 行为完全不变。
3. v1 强调**可观测**：可追踪会话、可查看 checkpoint 列表、可提取 transcript 快照。
4. v1 引入 EntireIO 风格的 Subagent（子代理）级联追踪元数据（仅记录 `parent_session_id`），实际嵌套语义随各 adapter 的 `SubagentExtractor` 后续补全。
5. 与 `subagent-scaffold` Cargo feature 无关；默认构建即可用。

### 2.2 不做（v1）

1. 不替换/重写 `HookProvider` 全量 API；以**新增并存**的方式扩展。
2. 不强制统一整库 transcript schema；保留 provider 原生格式（`jsonl/sqlite/markdown/binary`）的字节语义。
3. **不建 `agent_session_event` 表**——`SessionStore` 的 JSONL 已承载事件流。
4. **不建 shadow ref `refs/libra/agent-shadow/...`**——用 orphan commit 上的 `Libra-Scope: temporary` trailer 区分。
5. **不实装 `libra agent checkpoint rewind` 的 transcript 覆写**——仅工作树恢复，transcript 截断需要每个 Provider 实现 `TranscriptTruncator`，归 v2。v1 命令打印明显警告。
6. 不向 D1 同步全量 `agent_session_event`（事件流量大，v1 仅同步 session/checkpoint 摘要）。

---

## 3. 存储与对象模型

### 3.1 Transcript 落盘：Git Blob 直接快照，SQLite 不镜像 OID

> **改进分析**：原方案曾设计 `agent_checkpoint.transcript_blob_oid` 把 OID 抄进 SQLite，造成 Git 与 SQLite 双重维护、漂移风险。Git 本身就是对象数据库——checkpoint 的 commit tree 已经引用了 transcript blob。改为：
> - 在 `TurnEnd`/`SessionEnd` 时，把**脱敏后的**完整 transcript 作为 Git Blob 写入 `.libra/objects/`
> - 把该 blob 挂在 checkpoint commit 的 tree 中（路径 `transcript/<provider>`）
> - SQLite `agent_checkpoint` **只记录 `traces_commit`**（commit hash），需要 transcript 时从 commit tree 解析
> - Git Packfile 自动对连续快照做 delta 压缩，性能与体积都不需要额外手工分块
- 大文件自然按 `LIBRA_STORAGE_THRESHOLD` 流转到 R2
- `object_index.o_type` 新增 `agent_transcript`（仅用于云同步过滤与统计，不是读取主路径）

### 3.2 Refs 与并行历史

- **新增并行 orphan ref**：`refs/libra/agent-traces`（与 `refs/libra/intent` 平行）
- **不引入 shadow ref**：原方案的 `refs/libra/agent-shadow/<session>/<checkpoint>` 高频动态 ref 创建会撞上 Git ref 全局锁。改为：
  - Temporary checkpoint 也是 `agent-traces` 上的 orphan commit，但 commit message 含 `Libra-Scope: temporary` trailer，且 `agent_checkpoint.scope='temporary'`
  - Committed checkpoint 同样位于 `agent-traces`，`scope='committed'`
  - `libra agent clean` 时按 `scope='temporary'` 与 `agent_session.state='stopped'` 联合查询，重写 `agent-traces` orphan branch tip 移除对应 commit；底层 blob/tree 由 `git gc` 回收

### 3.3 目录树（refs/libra/agent-traces 上的 tree 布局）

```
refs/libra/agent-traces  (orphan branch)
└── checkpoint/
    └── <id[:2]>/<id[2:]>/         # UUIDv4 前两位分片
        ├── metadata.json          # checkpoint 元数据（小）
        ├── transcript/<provider>  # 脱敏后的 transcript blob（大；自动入 R2）
        └── tree/                  # 可选：审计用的 tree 快照
```

每个 checkpoint = 一个 orphan commit，commit message 含 trailer：

```
Libra-Session: <session-uuid>
Libra-Agent: claude_code
Libra-Parent-Commit: <user-branch-head-oid>
Libra-Checkpoint-ID: <checkpoint-uuid>
Libra-Scope: temporary | committed | subagent
```

### 3.4 SQLite Schema（仅 2 张表）

```sql
-- sql/migrations/2026050303_agent_capture.sql

CREATE TABLE IF NOT EXISTS `agent_session` (
    `session_id`           TEXT PRIMARY KEY,           -- UUIDv4
    `agent_kind`           TEXT NOT NULL,              -- claude_code|cursor|codex|gemini|opencode|copilot|factory_ai
    `provider_session_id`  TEXT NOT NULL,              -- agent 原生 session id
    `thread_id`            TEXT,                       -- v2 才填；FK ai_thread
    `state`                TEXT NOT NULL CHECK(`state` IN ('pending','active','condensed','stopped','quarantined')),
    `working_dir`          TEXT NOT NULL,
    `worktree_id`          TEXT,                       -- 多 worktree 区分
    `parent_commit`        TEXT,                       -- SessionStart 时的 HEAD oid
    `parent_session_id`    TEXT,                       -- Subagent 父会话（self-FK）
    `metadata_json`        TEXT NOT NULL DEFAULT '{}',
    `redaction_report`     TEXT NOT NULL DEFAULT '{}',
    `started_at`           INTEGER NOT NULL,
    `last_event_at`        INTEGER NOT NULL,
    `stopped_at`           INTEGER,
    `schema_version`       INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE SET NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS `idx_agent_session_provider`
    ON `agent_session`(`agent_kind`, `provider_session_id`);
CREATE INDEX IF NOT EXISTS `idx_agent_session_active`
    ON `agent_session`(`state`, `working_dir`) WHERE `state` = 'active';
CREATE INDEX IF NOT EXISTS `idx_agent_session_thread` ON `agent_session`(`thread_id`);

CREATE TABLE IF NOT EXISTS `agent_checkpoint` (
    `checkpoint_id`        TEXT PRIMARY KEY,           -- UUIDv4
    `session_id`           TEXT NOT NULL REFERENCES `agent_session`(`session_id`) ON DELETE CASCADE,
    `parent_checkpoint_id` TEXT,                       -- 子 agent 嵌套
    `scope`                TEXT NOT NULL CHECK(`scope` IN ('temporary','committed','subagent')),
    `parent_commit`        TEXT NOT NULL,              -- 用户分支当时 HEAD
    `tree_oid`             TEXT NOT NULL,              -- checkpoint commit 的根树 OID
    `metadata_blob_oid`    TEXT NOT NULL,              -- metadata.json blob OID
    `traces_commit`        TEXT NOT NULL,              -- agent-traces 上的 orphan commit
    `tool_use_id`          TEXT,                       -- 触发的 tool 调用
    `subagent_session_id`  TEXT,                       -- scope='subagent' 时关联子会话
    `description`          TEXT,
    `created_at`           INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_session`
    ON `agent_checkpoint`(`session_id`, `created_at`);
CREATE INDEX IF NOT EXISTS `idx_agent_checkpoint_scope` ON `agent_checkpoint`(`scope`);
```

> **不建 `agent_session_event` 表**：原方案的事件表与 `SessionStore` 的 JSONL 流功能重叠。`SessionStore` 已经在 `.libra/sessions/<id>/events.jsonl` 写 append-only 事件，且有文件锁与恢复机制。`SessionEnd` 时把整个 JSONL 作为 Git Blob 写入并挂在 committed checkpoint 的 tree 中（`events/<provider>.jsonl`）即可。SQLite 只保留 session 摘要 + checkpoint 索引。

> **不在 `agent_checkpoint` 写 `transcript_blob_oid`**：transcript 路径从 `traces_commit` → tree → `transcript/<provider>` blob 解析得到，避免 Git 与 SQLite 间的 OID 漂移。

### 3.5 与 `projection/` 模块的关系

[projection/](../../src/internal/ai/projection/) 是 runtime projections 层，处理 `ai_thread`/`ai_index_*` 等结构化 AI 制品。本计划的 `agent_session` 与 `agent_checkpoint` 是**对等 sibling**——独立放在 `src/internal/ai/observed_agents/storage.rs`，不进 projection 模块；通过可空 `agent_session.thread_id` 弱关联。

### 3.6 与 `agent_run/` 模块的关系

[`agent_run/`](../../src/internal/ai/agent_run/) 是 Step 2.1（CEX-S2-10）的 sub-agent contracts schema scaffold，**完全 gated 在 `subagent-scaffold` Cargo feature 后**，默认构建不链接。它关注**Libra 自家** sub-agent；本计划关注**外部独立 Agent**——表/ref/schema 不重叠。但**借鉴**它的 unknown-event-safe envelope 模式（`tag = "kind", content = "payload"` + `untagged` 包装）作为 `agent_session.metadata_json` 与 `metadata.json` blob 的演进方向。

---

## 4. 迁移策略：推动 `include_str!()` 演进

### 4.1 新建迁移文件

新建 `sql/migrations/2026050303_agent_capture.sql`（`up`）和 `sql/migrations/2026050303_agent_capture_down.sql`（`down`）。

`down.sql`：
```sql
DROP INDEX IF EXISTS `idx_agent_checkpoint_scope`;
DROP INDEX IF EXISTS `idx_agent_checkpoint_session`;
DROP TABLE IF EXISTS `agent_checkpoint`;

DROP INDEX IF EXISTS `idx_agent_session_thread`;
DROP INDEX IF EXISTS `idx_agent_session_active`;
DROP INDEX IF EXISTS `idx_agent_session_provider`;
DROP TABLE IF EXISTS `agent_session`;
```

### 4.2 改造 `builtin_migrations()`

[`src/internal/db/migration.rs:499`](../../src/internal/db/migration.rs) 当前用 inline SQL。新增条目改用 `include_str!`：

```rust
pub fn builtin_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 2026050301,
            name: "automation_log",
            up:   /* 现状 inline 保留，避免破坏现有读路径 */,
            down: /* 现状保留 */,
        },
        Migration {
            version: 2026050302,
            name: "agent_usage_stats",
            up:   /* 现状保留 */,
            down: /* 现状保留 */,
        },
        Migration {
            version: 2026050303,
            name: "agent_capture",
            up:   include_str!("../../../sql/migrations/2026050303_agent_capture.sql"),
            down: Some(include_str!("../../../sql/migrations/2026050303_agent_capture_down.sql")),
        },
    ]
}
```

> 注意：`include_str!` 的相对路径以 `migration.rs` 所在目录为基准——即 `src/internal/db/`，三段 `..` 退回到 repo 根，再下行 `sql/migrations/`。已与 [sql/migrations/README.md](../../sql/migrations/README.md) 给出的路径规则一致。

### 4.3 文档同步

更新 [sql/migrations/README.md](../../sql/migrations/README.md)：
- 把"4 位版本号 NNNN"改为 `YYYYMMDDNN`（与现状 `2026050301`/`2026050302` 一致）
- 给出 `include_str!` 加载示例与相对路径规则
- 注明现存的两个迁移仍 inline，新增的迁移走文件化路径，以增量演进

### 4.4 测试覆盖

修改 [`tests/db_migration_test.rs`](../../tests/db_migration_test.rs)：
- 第 47-48 行：`vec![2026050301, 2026050302, 2026050303]` / `vec!["automation_log", "agent_usage_stats", "agent_capture"]`
- 第 53 行：`max_registered_version() == Some(2026050303)`
- 第 985 行：`applied == vec![2026050301, 2026050302, 2026050303]`
- 新增断言：`table_exists(&conn, "agent_session").await` 与 `table_exists(&conn, "agent_checkpoint").await`

新增 [`tests/agent_capture_migration_test.rs`](../../tests/agent_capture_migration_test.rs)：
- `test_up_creates_tables_and_indexes`
- `test_up_is_idempotent`（连跑两次 `run_pending` 第二次为 no-op）
- `test_down_rolls_back_cleanly`（`rollback_to(2026050302)` 后两表与索引都消失）
- `test_legacy_schema_compatible`（先用 `sqlite_20260309_init.sql` 初始化 legacy DB，再 `run_pending` 不报错）

---

## 5. 适配层抽象：核心小 trait + 能力 trait

### 5.1 设计

> **改进分析**：原方案的 `ObservedAgentAdapter` 是 10+ 方法的大 trait，新增 Agent 一次实现所有方法门槛过高。EntireIO 用"核心 `Agent` + 可选能力接口"的注册表模式，社区贡献成本低。Libra 直接借鉴：

```rust
// src/internal/ai/observed_agents/adapter.rs

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode, Cursor, Codex, Gemini, OpenCode, Copilot, FactoryAi,
}

/// 核心 trait：每个被观测 Agent 必须实现的最小集合。
pub trait ObservedAgent: Send + Sync {
    fn provider_kind(&self) -> AgentKind;
    fn provider_name(&self) -> &'static str;

    /// 读取 Agent 原生 transcript 字节。`None` = 当前无可用 transcript。
    /// **返回的字节尚未脱敏**——调用方负责走 `Redactor` 后才能持久化。
    fn read_transcript(&self, session: &AgentSessionCtx) -> anyhow::Result<Option<Vec<u8>>>;

    /// Agent 的保护目录（如 `.claude`、`.gemini`），用于 rewind/clean 的安全边界。
    fn protected_dirs(&self) -> &'static [&'static str];
}

/// 可选能力：Hook 支持。仅当 Agent 提供生命周期 hook 时实现。
/// 与现有 `HookProvider` 是**并存**关系——内置实现可同时实现两者。
pub trait ObservedAgentHooks: ObservedAgent {
    fn supported_commands(&self) -> &'static [ProviderHookCommand];
    fn parse_hook_event(&self, name: &str, env: &SessionHookEnvelope) -> anyhow::Result<LifecycleEvent>;
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn install_hooks(&self, options: &ProviderInstallOptions) -> anyhow::Result<()>;
    fn uninstall_hooks(&self) -> anyhow::Result<()>;
    fn hooks_are_installed(&self) -> anyhow::Result<bool>;
}

/// 可选能力：transcript 截断（v2 才需要）。
pub trait TranscriptTruncator: ObservedAgent {
    /// 输入完整 transcript 字节（已从 Git Blob 读出），按 checkpoint 边界截断。
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> anyhow::Result<Vec<u8>>;
}

/// 可选能力：分块（超大 transcript 用）。v2 候选。
pub trait TranscriptChunker: ObservedAgent {
    fn chunk_transcript(&self, content: &[u8], max_size: usize) -> anyhow::Result<Vec<Vec<u8>>>;
    fn reassemble_transcript(&self, chunks: &[Vec<u8>]) -> anyhow::Result<Vec<u8>>;
}
```

### 5.2 v1 适配器范围

| Agent | 状态 | 实现路径 |
|-------|------|---------|
| Claude Code | **stable** | 在 `observed_agents/builtin/claude_code.rs` 新增 `ClaudeObservedAgent`，内部组合现有 `ClaudeProvider`（继续实现 `HookProvider`），只新增 `read_transcript` 与 `protected_dirs` |
| Gemini CLI | **stable** | 同上，`gemini.rs` |
| Cursor | **preview** | 仅占位：`provider_kind()` / `provider_name()` 返回常量，`read_transcript()` 返回 `Err(AgentNotYetImplemented)`。CLI `libra agent enable cursor` 可见、可安装 hook 桩，但 hook 触发时打印明显告警并不写 traces |
| Codex | **preview** | 同上 |
| OpenCode | **preview** | 同上 |
| GitHub Copilot CLI | **preview** | 同上 |
| Factory AI Droid | **preview** | 同上 |

> v1 不动现有 [hooks/providers/claude/](../../src/internal/ai/hooks/providers/claude/) 与 [hooks/providers/gemini/](../../src/internal/ai/hooks/providers/gemini/) 的 `HookProvider` 实现——避免破坏现有 hook 安装/调用链路。新 wrapper 在 `observed_agents/builtin/` 中以**组合**复用旧 provider。

---

## 6. Hook 入口与摄入流程

### 6.1 CLI 入口

新增两层命令（**两条都最终走 `process_hook_event_with_target`**）：

| CLI | 用途 | 写入目标 |
|-----|------|---------|
| `libra hooks <provider> <subcommand>` | 兼容现有 claude/gemini hook 安装（providers 在历史 hook 配置里写过这条命令） | `HookTarget::AiIntent` → `refs/libra/intent` |
| `libra agent hooks <agent> <subcommand>` | 新顶层视图，外部 Agent 用此安装 | `HookTarget::AgentTraces` → `refs/libra/agent-traces` |

两个变体都加到 [`src/cli.rs`](../../src/cli.rs) 的 `Commands` 枚举：`Commands::Hooks(HooksArgs)` 与 `Commands::Agent(AgentArgs)`。`Hooks` 子命令对外文档可标 `hidden`，但必须可解析（现存 provider hook 配置依赖之）。

### 6.2 摄入函数参数化

把现 [hooks/runtime.rs:139](../../src/internal/ai/hooks/runtime.rs) 的 `process_hook_event_from_stdin` 抽离为内部参数化函数：

```rust
pub enum HookTarget { AiIntent, AgentTraces }

pub async fn process_hook_event_from_stdin(  // 公开 API 保留
    command: ProviderHookCommand,
    expected_kind: LifecycleEventKind,
    provider: &dyn HookProvider,
) -> anyhow::Result<()> {
    process_hook_event_with_target(command, expected_kind, provider, HookTarget::AiIntent).await
}

async fn process_hook_event_with_target(
    command: ProviderHookCommand,
    expected_kind: LifecycleEventKind,
    provider: &dyn HookProvider,
    target: HookTarget,
) -> anyhow::Result<()> {
    // 现有 6 步前后插入：
    //   Step 0:  AgentTraces 路径执行 redaction（强制）
    //   Step 5a: AgentTraces 路径 upsert agent_session
    //   Step 5b: AgentTraces 路径在 TurnEnd / SessionEnd 创建 checkpoint commit
    //   Step 6:  按 target 写 ai_session（AiIntent）或 traces commit（AgentTraces）
}
```

公开 API 1:1 向后兼容：现有 Claude/Gemini provider 与 hook 配置无感。

### 6.3 状态机

| 事件 | state 转换 | 副作用 |
|------|----------|--------|
| `SessionStart` | `→ active` | 建 `agent_session` 行；获取 `.libra/sessions/<id>.lock` |
| `TurnStart`（UserPromptSubmit） | active 保持 | 检查 `agent_session WHERE state='active' AND working_dir=?`，>0 时记 `concurrent_active=true`，**不阻塞** |
| `Compaction` | `→ condensed`，`CompactionCompleted → active` | 计数器 |
| `ToolUse` | active 保持 | 若 adapter 提供子代理识别则插入 `scope='subagent'` checkpoint |
| `TurnEnd`（Stop） | active 保持 | 创建 `scope='committed'` checkpoint：脱敏 transcript → 写 blob → 拼 commit → 追加到 `agent-traces` |
| `SessionEnd` | `→ stopped` | 最终 committed checkpoint，关闭 `SessionStore` 并释放锁 |

### 6.4 Hook 配置安装

`libra agent enable --agent claude-code` 调 `ObservedAgentHooks::install_hooks`，写出形如：
```json
{"hooks": {"Stop": [{"command": "libra agent hooks claude-code stop || true"}]}}
```
`|| true` 作为硬不变量保证 Libra 不可用时**永不破坏 Agent 自身**。可选 ship `libra-hook-shim` no-op 二进制覆盖纯 JSON hook 配置场景（不允许 shell 拼接的）。

---

## 7. Checkpoint / Rewind（v1 简化）

### 7.1 Checkpoint 生成（在 `TurnEnd`/`SessionEnd`）

1. **构建 tree**：`build_tree_recursive`（[stash.rs](../../src/command/stash.rs)）；范围排除每个 adapter 的 `protected_dirs()`
2. **构建 metadata blob**（canonical JSON）：
   ```json
   {
     "checkpoint_id": "<uuid>",
     "session_id": "<uuid>",
     "agent_kind": "claude_code",
     "parent_commit": "<oid>",
     "scope": "committed",
     "tool_use_id": "...",
     "subagent_checkpoint_ids": [],
     "model_info": {...},
     "redaction_report": {...},
     "schema_version": 1
   }
   ```
3. **构建 transcript blob**：`adapter.read_transcript()` → `Redactor::redact()` → `RedactedBytes` → `write_git_object` 得 OID
4. **构建 events blob**（可选）：把 `SessionStore` 的 JSONL 字节流也作 blob，挂在 tree `events/<provider>.jsonl`
5. **构建 checkpoint tree**（路径 `checkpoint/<id[:2]>/<id[2:]>/{metadata.json, transcript/<provider>, events/<provider>.jsonl, tree/}`），写入 Git
6. **追加 commit** 到 `refs/libra/agent-traces`：commit message 含 `Libra-*` trailer；`HistoryManager::create_append_commit` + `update_ref_if_matches` 处理 CAS
7. **插 `agent_checkpoint`**：`scope`、`traces_commit`、`tree_oid`、`metadata_blob_oid`

### 7.2 Temporary vs Committed

- **`scope='temporary'`**：`PostTool[Task]` 等临时点，commit message 也带 `Libra-Scope: temporary`，`libra agent clean` 会重写 `agent-traces` tip 移除（git gc 后自动回收 blob/tree）
- **`scope='committed'`**：`TurnEnd`/`SessionEnd`，长期保留
- **`scope='subagent'`**：子 Agent 嵌套，关联到 `parent_checkpoint_id` 与 `subagent_session_id`

### 7.3 Rewind（v1 dry-run / read-only）

- `libra agent checkpoint show <id>`：展示 metadata + transcript 长度 + tree 摘要
- `libra agent checkpoint rewind <id> --dry-run`：默认 dry-run，打印将影响的文件列表
- `libra agent checkpoint rewind <id> --apply`（不带 `--apply` 时拒绝执行）：仅恢复**工作树**（复用 `restore` 路径），HEAD 与 `refs/heads/*` 不动；**不**覆写本地 transcript 文件——v1 命令明确打印：
  ```
  Note: Transcript truncation for <provider> is not yet implemented in v1.
  The Agent's local transcript file remains unchanged. Re-running the Agent
  may produce inconsistent context.
  ```

### 7.4 清理

`libra agent clean [--all]`：
1. 查 `agent_session WHERE state='stopped'`
2. 对每个，查 `agent_checkpoint WHERE session_id=? AND scope='temporary'`
3. 重写 `refs/libra/agent-traces` orphan branch tip 跳过这些 commit
4. 删 `agent_checkpoint` 对应行
5. 不主动 `git gc`——交底层自然回收

### 7.5 与现有命令交互

- 扩展 [`is_locked_branch`](../../src/internal/branch.rs)：新增匹配 `agent-traces`
- 在 [`command/restore.rs`](../../src/command/restore.rs) 与 [`command/reset.rs`](../../src/command/reset.rs) 的入口增加 `is_locked_branch(target_branch_name)` 检查并拒绝
- `git log refs/libra/agent-traces` 直接可用

---

## 8. 脱敏（RedactedBytes 编译时契约）

### 8.1 编译时安全契约

> **改进分析**：EntireIO 的 `redact.RedactedBytes` 是零开销包装，强制要求任何持久化目标只接受脱敏后的字节。直接采纳：

```rust
// src/internal/ai/observed_agents/redaction.rs

/// 已脱敏的字节数据。消费者（checkpoint store、cloud sync、history append）
/// 只接受此类型，从而在编译时强制脱敏契约。
pub struct RedactedBytes {
    data: Vec<u8>,
}

impl RedactedBytes {
    /// 仅供 redaction 模块内部和可信回读路径使用。
    pub(crate) fn new_unchecked(data: Vec<u8>) -> Self {
        Self { data }
    }
    pub fn bytes(&self) -> &[u8] { &self.data }
    pub fn len(&self) -> usize { self.data.len() }
}
```

**强制接受 `RedactedBytes` 的写路径**：
- `observed_agents::checkpoint::write_transcript_blob(redacted: RedactedBytes, ...)` 而非 `&[u8]`
- `observed_agents::traces::append_checkpoint_commit(...)` 内部调 `HistoryManager::create_append_commit` 时只传已封装类型
- Cloud sync 的 `agent_transcript` 上传封装层也接 `RedactedBytes`

### 8.2 Redaction 引擎

新建 [`src/internal/ai/observed_agents/redaction.rs`](../../src/internal/ai/observed_agents/redaction.rs)：

| 检测器 | 描述 | 优先级 |
|-------|------|--------|
| Gitleaks 派生规则 | 260+ 已知 secret 格式（API key / token / SSH / JWT / AWS / GCP / 连接串） | P0 |
| 熵检测 | Shannon entropy > 4.5 的 alphanumeric ≥10 字符 | P1 |
| Credential URI | `postgres://user:pass@host/db` 内嵌密码 URL | P1 |
| DB Connection String | JDBC、keyword DSN、semicolon-separated | P1 |
| Bounded K/V | `DB_PASSWORD=...` 等键值对 | P2 |
| PII（opt-in） | email、phone、address；默认关闭 | P3 |

**JSON-aware 替换**：
- JSONL 输入逐行解析、递归 walk；**跳过** `*id`/`*ids`/`filepath`/`cwd`/`path` 字段；**跳过** image 对象（`type: image`、`base64`）
- 命中替换为 `<REDACTED:rule_id>`
- Placeholder 白名单：`changeme`、`placeholder`、`<password>`、`***`、`${VAR}`、已有的 `REDACTED`

API：
```rust
pub struct Redactor { /* ... */ }
pub struct RedactionRule {
    pub id: &'static str,
    pub regex: regex::Regex,
    pub replacement: &'static str,
}
pub struct RedactionReport {
    pub matches: Vec<RedactionMatch>,
    pub bytes_scanned: usize,
    pub bytes_redacted: usize,
}
impl Redactor {
    pub fn new_default() -> Self;       // 内置规则集
    pub fn redact(&self, input: &[u8]) -> (RedactedBytes, RedactionReport);
}
```

### 8.3 强制扫描路径

无视配置 mode、强制走脱敏：
- `prompt.body`（`TurnStart`）
- `tool_use.input.command`（Bash 工具）
- transcript 全文（`TurnEnd` / `SessionEnd` 写 blob 之前）

### 8.4 配置

`.libra/config` 的 `[agent.redaction]` 段：
- `mode = "redact"`（默认）：替换匹配为 `<REDACTED:...>`
- `mode = "warn"`：仅记录到 `RedactionReport`，不替换（审计用）
- `mode = "off"`：跳过——文档明确警告，不推荐

### 8.5 结果存证

- `agent_session.redaction_report`：累计的 JSON（按规则 id 计数 + 替换偏移摘要）
- `metadata.json` blob：按 checkpoint 范围切片的 RedactionReport

---

## 9. CLI 命令面（v1）

加到 [`src/cli.rs`](../../src/cli.rs) `Commands` 枚举：`Commands::Hooks(HooksArgs)` + `Commands::Agent(AgentArgs)`。

```
libra hooks <provider> <subcommand>      # hidden 兼容入口（仅 claude/gemini）
libra agent
  enable [--agent <name> ...]            # 安装 hook，注册 agent
  disable [--agent <name> ...]           # 卸载 hook
  status                                 # 活跃会话计数 + 最近 checkpoint
  session list [--agent <name>] [--state <s>]
  session show <id> [--extract-transcript <path>]
  session stop <id>
  session resume <id>                    # v1 仅恢复 metadata，不恢复 transcript 上下文
  checkpoint list [--session <id>]
  checkpoint show <id>                   # metadata + diff 摘要（合并自 explain）
  checkpoint rewind <id> [--dry-run|--apply]   # v1: --apply 仅恢复工作树，不覆写 transcript
  clean [--all]
  doctor                                 # 诊断 hook 安装、stuck 状态、孤儿 checkpoint
  push [--remote <name>]                 # 推送 refs/libra/agent-traces
  hooks <agent> <subcommand>             # hidden；agent hooks 调用入口
```

**v1 不实现**：
- `session promote <id> --as-intent`（v2 跨体系提升）
- `checkpoint explain <id>`（合并到 `checkpoint show`）
- `checkpoint rewind <id> --apply` 的 transcript 截断（v2 接 `TranscriptTruncator`）

### 9.1 初始化

`libra init` 在初始化路径中追加：
```rust
HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch().await?;
```
与现有 `refs/libra/intent` 初始化并列。

### 9.2 分支保护

修改 [`src/internal/branch.rs:45`](../../src/internal/branch.rs)：
```rust
pub fn is_locked_branch(name: &str) -> bool {
    name == DEFAULT_BRANCH
        || name == INTENT_BRANCH
        || name == "agent-traces"
}
```

并在 [`src/command/restore.rs`](../../src/command/restore.rs)、[`src/command/reset.rs`](../../src/command/reset.rs) 入口增加：
```rust
if let Some(branch) = target_branch_name() {
    if internal::branch::is_locked_branch(branch) {
        bail!("refusing to {} locked branch '{}'", op_name, branch);
    }
}
```

回归测试覆盖 `restore`/`reset` 拒绝触及 `intent` / `agent-traces`。

---

## 10. 云同步（v1 最小可行）

### 10.1 自动入云

Transcript blob、metadata blob、events blob 都走 `write_git_object` → `object_index` → 现有 [cloud.rs:192](../../src/command/cloud.rs) 增量同步。**新增 `o_type='agent_transcript'`** 仅用于过滤与统计。零代码即得云备份。

### 10.2 D1 表同步

[d1_client.rs](../../src/utils/d1_client.rs) 新增：
- `ensure_agent_session_table`
- `ensure_agent_checkpoint_table`
- `upsert_agent_session(session)` / `upsert_agent_checkpoint(checkpoint)`

`libra cloud sync` 流程末尾追加 `agent_*` 表的增量上行。**v1 不同步**事件流（无 `agent_session_event`，且 `SessionStore` JSONL 已作 blob 同步——可从 commit tree 反解）。

### 10.3 推送独立 remote

`libra agent push [--remote <name>]`：scope 限定 `refs/libra/agent-traces`，对应 `[remote "agent-traces"]` 配置写在 `.libra/config`。薄包装现 push 机器，无新 transport。

---

## 11. 与 `libra code` 共存（v1 完全隔离）

1. `libra code` 仅写 `refs/libra/intent` 与 Intent/Task/Run 等结构化 AI 制品（保持现状）
2. `libra agent` 仅写 `refs/libra/agent-traces` 与 `agent_*` 表
3. `agent_session.thread_id` 在 v1 默认 NULL，除非显式 `promote`（v2）
4. v1 不做跨模型转换；v2 可从 `agent_session` 反算 `ToolCallRecord` 回写到 `ai_thread`
5. **资源隔离**：`SessionStore` 路径区分——`libra code` 用 `.libra/sessions/code/`，`libra agent` 用 `.libra/sessions/agent/`，避免文件锁冲突

---

## 12. 与基线可复用清单

| 目的 | 复用对象 | 位置 |
|------|---------|------|
| 孤儿 ref CAS | `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head` / `update_ref_if_matches` | [src/internal/ai/history.rs:176](../../src/internal/ai/history.rs) |
| Hook 摄入流水线 | `process_hook_event_from_stdin` → 抽离为 `process_hook_event_with_target` + 旧 API 包装 | [src/internal/ai/hooks/runtime.rs:139](../../src/internal/ai/hooks/runtime.rs) |
| 事件模型 | `LifecycleEvent` / `LifecycleEventKind` / `make_dedup_key` / `normalize_json_value` / `validate_session_hook_envelope` / `apply_lifecycle_event` / `append_raw_hook_event` | [hooks/lifecycle.rs](../../src/internal/ai/hooks/lifecycle.rs) |
| Unknown-event-safe envelope 模式 | 借鉴 `AgentRunEvent` / `AgentRunEventEnvelope`（`agent_run/` gated 在 `subagent-scaffold`，**不直接依赖**） | [agent_run/event.rs](../../src/internal/ai/agent_run/event.rs) |
| Migration runner | `MigrationRunner::register` / `run_pending` / `builtin_migrations()` | [migration.rs](../../src/internal/db/migration.rs) |
| 文件锁 | `SessionStore::lock_session` + `SessionFileLock`（5s timeout、30s stale） | [session/store.rs:338](../../src/internal/ai/session/store.rs) |
| 工作树 → tree | `build_tree_recursive` | [stash.rs](../../src/command/stash.rs) |
| 文件还原 | restore 的 path-walking | [restore.rs](../../src/command/restore.rs) |
| 分支保护 | `is_locked_branch`（扩展） / `INTENT_BRANCH` 拒绝模式 | [branch.rs:45](../../src/internal/branch.rs)、[checkout.rs:71-83](../../src/command/checkout.rs)、[switch.rs:35,265](../../src/command/switch.rs) |
| 分层存储 | `TieredStorage` + `LIBRA_STORAGE_THRESHOLD` 路由 | [client_storage.rs:336](../../src/utils/client_storage.rs) |
| 对象 I/O | `write_git_object` / `read_git_object` | [object.rs](../../src/utils/object.rs) |
| 云同步 | `object_index` 迭代 | [cloud.rs:192](../../src/command/cloud.rs) |
| 现 Claude/Gemini provider | 保留 `HookProvider`，新加 `ObservedAgent` wrapper 组合复用 | [hooks/providers/](../../src/internal/ai/hooks/providers/) |
| Projection 层 | **不直接复用** —— 独立 storage.rs，弱关联 | [projection/](../../src/internal/ai/projection/) |

---

## 13. 风险与验收标准

| # | 风险 | 严重 | 验收标准 |
|---|------|------|---------|
| 1 | **Redaction 遗漏导致 secret 泄漏** | **P0** | 所有写入 `agent-traces` 的 transcript blob 必须通过 `RedactedBytes` 类型；新增 `tests/redaction_contract_test.rs` 编译时验证未脱敏 `Vec<u8>` 不能直接传入 checkpoint writer；模拟 transcript 中故意塞 `AKIA...` AWS key 后，`git cat-file -p <oid>` 输出包含 `<REDACTED:aws-access-key-id>` 而非原值 |
| 2 | **兼容层空转** | P1 | Claude/Gemini 现有 hook 安装与事件调用在新增命令后保持可执行，含幂等检查；旧 `libra hooks claude session-start` 行为不变（hidden 不影响） |
| 3 | **迁移注册断层** | P1 | `tests/db_migration_test.rs` 三处版本断言更新为 `2026050303`；新建 `tests/agent_capture_migration_test.rs` 覆盖 fresh DB / legacy DB / `up→down→up` 往返；`run_pending` 幂等 |
| 4 | **Git 对象膨胀** | P1 | 每个 checkpoint transcript blob 大小被监控（超过 `LIBRA_STORAGE_THRESHOLD` 入 R2）；`clean` 能从 `agent-traces` 移除 temporary commit；`git gc` 后底层 blob/tree 回收 |
| 5 | **分支保护遗漏** | P1 | 回归测试证明 `restore <ref>` 与 `reset <ref>` 在 `<ref>` ∈ {`intent`, `agent-traces`} 时拒绝；`switch`/`checkout` 现有保护回归绿 |
| 6 | **并发会话锁冲突** | P1 | 同 `working_dir` 并发 `TurnStart` 不阻塞但记 `concurrent_active=true`；`SessionStore` 锁超时（5s）后触发恢复逻辑、不丢事件 |
| 7 | **CAS 风暴** | P1 | 模拟单 session 100 events/s 持续 60s，`agent-traces` 写入成功率 100%；必要时调整 `HISTORY_HEAD_CONFLICT_MAX_RETRIES`（现 32） |
| 8 | **Hook 失败安全** | P1 | 集成测试：安装 hook 后 `mv $(which libra) /tmp` 卸载 libra，Claude Code 仍能完整跑完一个 session（`|| true` 吃错误）；libra 还原后 `libra agent doctor` 报告"未捕获的 session N 个" |
| 9 | **多 worktree SQLite 共享** | P2 | `agent_session` 必须存于 `git rev-parse --git-common-dir` 对应的 SQLite；`worktree_id` 列允许 `list` 按需过滤；多 worktree 试跑 |
| 10 | **v1 过度承诺 rewind** | P1 | `checkpoint rewind` 在 v1 默认 dry-run；`--apply` 时不动 transcript 文件且打印明显警告 |
| 11 | **Cursor SQLite transcript 取查** | P2 | `libra agent session show --extract-transcript <path>` 把 SQLite blob 物化到指定路径，文档说明用 `sqlite3` 查看 |
| 12 | **Unknown-event-safe envelope 兼容** | P2 | 老 reader 读包含未来虚构事件 `kind=future_event_xyz` 的 metadata.json，应落到 `Unknown(Value)` 分支不报错（与 [agent_run/event.rs](../../src/internal/ai/agent_run/event.rs) 对外承诺一致） |

---

## 14. 分阶段实现

### 阶段 1（兼容层 + 基础接入，2-3 周）

**目标**：迁移落地、CLI 可解析、Claude/Gemini 走新路径但行为未变。

1. **迁移基础设施**：新建 `sql/migrations/2026050303_agent_capture.sql` + `_down.sql`；改 `builtin_migrations()` 用 `include_str!` 加 `agent_capture` 条目；更新 `tests/db_migration_test.rs` 三处断言；新增 `tests/agent_capture_migration_test.rs`；更新 `sql/migrations/README.md`
2. **Redaction 骨架**：新建 `src/internal/ai/observed_agents/redaction.rs` 含 `RedactedBytes` + 基础规则（gitleaks 派生 + 熵检测 + URI/DSN）；新增 `tests/redaction_contract_test.rs`（包含编译时不允许 `Vec<u8>` 直传 checkpoint writer 的 trybuild 测试）
3. **CLI 兼容层**：`src/cli.rs` 新增 `Commands::Hooks(HooksArgs)` 与 `Commands::Agent(AgentArgs)`；实现 `libra hooks <provider> <subcommand>` 路由（向后兼容）与 `libra agent hooks <agent> <subcommand>` 路由（hidden）
4. **Hook 摄入改造**：`process_hook_event_from_stdin` 抽离为 `process_hook_event_with_target(..., target)`；保留旧 API 1:1 包装；`AgentTraces` 路径走 redaction → `agent_session` upsert（不写事件表，事件继续落 `SessionStore` JSONL）
5. **分支保护**：扩展 `is_locked_branch` 加 `agent-traces`；在 `restore`/`reset` 入口增加拒绝逻辑；新增回归测试
6. **`libra init` 初始化 `agent-traces`** orphan branch
7. **`ObservedAgent` 与 `ObservedAgentHooks` trait 定义**；Claude/Gemini wrapper 组合复用现 `HookProvider`

**阶段 1 验收**：`cargo test --all` 全绿；`tests/db_migration_test.rs` 与 `tests/agent_capture_migration_test.rs` 全通过；Claude/Gemini 旧 hook 路径调用无回归；`libra agent status` 在新仓库可显示 0 active sessions；`libra agent enable claude-code` 写出有效的 `.claude/settings.json`。

### 阶段 2（Checkpoint Git 存储 + UX，2-3 周）

**目标**：完整会话能在 `refs/libra/agent-traces` 上生成可见的 checkpoint commit 链。

1. **Checkpoint commit 生成**：复用 `stash::build_tree_recursive`（排除 `protected_dirs`）构造 tree；构 metadata.json blob；transcript 经 `Redactor` → `RedactedBytes` → `write_git_object`；events.jsonl 同样作 blob；构 checkpoint tree；`HistoryManager::new_with_ref("refs/libra/agent-traces")` 追加 commit；commit message 含 `Libra-*` trailer
2. **`agent_checkpoint` 表写入**：`scope` ∈ {temporary, committed}；`traces_commit` 指向 orphan commit；`tree_oid` / `metadata_blob_oid`
3. **CLI**：实现 `libra agent session list/info/show/stop/resume`、`libra agent checkpoint list/show`、`libra agent doctor`、`libra agent push`
4. **清理**：`libra agent clean`：按 `state='stopped' AND scope='temporary'` 重写 `agent-traces` tip 移除对应 commit
5. **Rewind dry-run**：`libra agent checkpoint rewind <id> --dry-run` 列出将影响的文件；`--apply` 调 `restore` 路径仅还原工作树并打印 transcript 不变更警告
6. **TUI/MCP 扩展（可选）**：在现有 [tui/](../../src/internal/tui/) 加一个最小 view 展示 `agent_session` 列表

**阶段 2 验收**：完整 Claude/Gemini session 能生成多个 checkpoint commit；`libra agent checkpoint list` 列出按时间排序；`git log refs/libra/agent-traces` 可读；`libra agent clean` 后 temporary commit 从 `agent-traces` 移除且 SQLite 索引同步清理。

### 阶段 3（脱敏完善 + Preview 适配器 + 云同步，2-3 周）

**目标**：脱敏达到生产可用；其余 5 个 Agent 在 CLI 可见；云同步走通。

1. **Redaction 完善**：完整 gitleaks 规则集；可选 PII 检测；`agent_session.redaction_report` 与 metadata blob 摘要落地；新增 known-bad 字符串矩阵的单元测试
2. **Preview 适配器**：在 `observed_agents/builtin/{cursor,codex,opencode,copilot,factory_ai}.rs` 写存根：`provider_kind/provider_name/protected_dirs` 返回常量，`read_transcript` 返回 `Err(AgentNotYetImplemented)`；CLI `libra agent enable` 列出全部 7 个 agent，preview 标注；`hooks <preview-agent>` 调用时打印告警并不写 traces
3. **云同步**：`d1_client` 新增 `ensure_agent_session_table` / `ensure_agent_checkpoint_table` / `upsert_*`；`libra cloud sync` 流程末尾追加；`object_index.o_type='agent_transcript'` 走正常 R2 同步；`libra agent push` scope 限定 `refs/libra/agent-traces`
4. **资源隔离**：`SessionStore` 路径分两个子目录（`code/` vs `agent/`）

**阶段 3 验收**：脱敏对模拟矩阵的命中率 100%；7 个 agent 都能 `enable`（preview 有告警）；R2 + D1 凭据配置后 `libra cloud sync` 完成且远端可见；另一台机器 `libra cloud restore` 后 `libra agent session list` 能看到原 session。

### 阶段 4（v2，本次不做）

1. `TranscriptTruncator` 实装 + `checkpoint rewind --apply` 支持 transcript 覆写
2. `session promote <id> --as-intent` 跨体系提升
3. 从 `agent_session` 反算 `ToolCallRecord` 回写 `ai_thread`
4. 5 个 preview adapter → stable
5. 外部 `libra-agent-<name>` 二进制 RPC 支持（与 EntireIO 对等）

---

## 15. 与 EntireIO 的差异化决策

| 决策点 | EntireIO | Libra | 理由 |
|-------|----------|-------|------|
| Checkpoint ref 拓扑 | shadow + committed 两个 ref | 单 orphan ref `refs/libra/agent-traces`，scope 用 commit trailer 区分 | 减少 ref 数，避免 Git ref 全局锁瓶颈；清理只重写 orphan tip |
| Session 事件存储 | `.git/entire-sessions/<id>.json` 跨 worktree 共享 | 复用 `SessionStore` JSONL（`.libra/sessions/agent/<id>/`） | 复用既有锁与恢复机制，零增量代码 |
| Redaction 契约 | `RedactedBytes` 编译时类型 | **直接采纳** | 最成功的安全设计 |
| Adapter 接口 | 核心 `Agent` + 可选 `HookSupport`/`TranscriptAnalyzer` 等 | **直接采纳**（核心 `ObservedAgent` + 可选 `ObservedAgentHooks` / `TranscriptTruncator` / `TranscriptChunker`） | 降低社区贡献门槛 |
| Transcript 截断 | 各 Agent 实现 `TruncateTranscriptAtUUID` | v1 不实现，v2 通过 `TranscriptTruncator` 引入 | 降 v1 风险，先把存储与观测打通 |
| SQLite 角色 | 仅 session 状态 | 同上：SQLite 不存 OID 镜像 | 避免 Git 与 SQLite 双重维护 |
| ID 形态 | `YYYY-MM-DD-<UUID>` + 12-char hex | UUIDv4（Libra 原生） | 与 Libra 既有命名一致；如需互通再写一次性 `libra agent import-entireio` |
| Ref 命名前缀 | `entire/...` | `refs/libra/agent-*` | 命名空间隔离 |

---

## 16. Phase 1 文件级落地清单（可执行）

### 16.1 新增

| 文件 | 内容要点 |
|------|---------|
| `sql/migrations/2026050303_agent_capture.sql` | 第 3.4 节的两张表 DDL（idempotent） |
| `sql/migrations/2026050303_agent_capture_down.sql` | 第 4.1 节的 down DDL |
| `src/internal/ai/observed_agents/mod.rs` | 模块入口，re-export |
| `src/internal/ai/observed_agents/adapter.rs` | `AgentKind` enum、`ObservedAgent` / `ObservedAgentHooks` trait（v1 必备）、`TranscriptTruncator` / `TranscriptChunker` trait 占位（v2 才实装） |
| `src/internal/ai/observed_agents/redaction.rs` | `RedactedBytes` 类型 + `Redactor` + 默认规则集 |
| `src/internal/ai/observed_agents/storage.rs` | `agent_session` / `agent_checkpoint` 的 sea-orm DAO |
| `src/internal/ai/observed_agents/registry.rs` | 按 `AgentKind` 查找 adapter（含 5 个 preview 占位） |
| `src/internal/ai/observed_agents/builtin/claude_code.rs` | `ClaudeObservedAgent` wrapper，组合现 `ClaudeProvider` |
| `src/internal/ai/observed_agents/builtin/gemini.rs` | 同上，wrap 现 `GeminiProvider` |
| `src/internal/ai/observed_agents/builtin/{cursor,codex,opencode,copilot,factory_ai}.rs` | preview 存根（仅 `provider_kind` / `provider_name` / `protected_dirs`，其它返回 `Err`） |
| `src/command/agent/mod.rs` + `agent/{enable,disable,status,session,checkpoint,clean,doctor,push,hooks}.rs` | 子命令分发与实现 |
| `src/command/hooks.rs` | 兼容层，调 `process_hook_event_with_target(..., AiIntent)` |
| `tests/agent_capture_migration_test.rs` | fresh / legacy / up-down-up 往返 |
| `tests/redaction_contract_test.rs` | 含 trybuild compile-fail 用例验证未脱敏字节不可写 checkpoint |

### 16.2 修改

| 文件 | 改动 |
|------|------|
| `src/cli.rs` | `Commands` 枚举加 `Hooks(HooksArgs)` 与 `Agent(AgentArgs)`；`match` 分支加路由 |
| `src/internal/db/migration.rs:499` `builtin_migrations()` | 加 `2026050303_agent_capture` 条目，`up`/`down` 用 `include_str!("../../../sql/migrations/...")` |
| `src/internal/branch.rs:45` `is_locked_branch` | 加 `\|\| name == "agent-traces"` |
| `src/command/restore.rs` | 入口处加 `is_locked_branch` 检查，命中拒绝 |
| `src/command/reset.rs` | 同上 |
| `src/internal/ai/hooks/runtime.rs:139` `process_hook_event_from_stdin` | 抽离为 `process_hook_event_with_target(..., target: HookTarget)`；旧函数 1:1 包装传 `AiIntent` |
| `src/command/init.rs`（或对应初始化路径） | 调 `HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch()` |
| `src/internal/ai/session/store.rs`（路径子目录） | 新增 `code/` vs `agent/` 子目录区分 |
| `tests/db_migration_test.rs:47-48,53,985` | `2026050303` / `agent_capture` 加进硬编码断言 |
| `sql/migrations/README.md` | 版本号规则改 `YYYYMMDDNN`、`include_str!` 加载示例 |
| `Cargo.toml` | 如需 `regex`、`once_cell`、`fs2` 等新依赖在此声明 |

### 16.3 不动（明确边界）

- `src/internal/ai/agent_run/`（gated 在 `subagent-scaffold` feature）
- `src/internal/ai/projection/`
- `src/internal/ai/hooks/providers/{claude,gemini}/` 的现有 `HookProvider` 实现（仅在外面包 wrapper）
- `refs/libra/intent` 写入路径与 `AI_REF` 常量
- `sqlite_20260309_init.sql` / `sqlite_20260415_ai_runtime_contract.sql`
- 现有 `libra code` / TUI / MCP 行为

---

## 17. 端到端验证

不仅靠单元测试构成"做完了"。每个阶段必须有以下一组通过：

1. **基础回归**：`cargo +nightly fmt --all --check` && `cargo clippy --all-targets --all-features -- -D warnings` && `cargo test --all` 全绿
2. **真实 Claude Code 试跑**（阶段 2 末）：
   - `libra init && libra agent enable --agent claude-code`
   - 在 Claude Code 跑一个 session（含 tool 调用）
   - `libra agent session list` 显示 1 行 active；Claude 退出后转 stopped
   - `libra agent session show <id> --extract-transcript /tmp/x.jsonl` 输出可读 JSONL
   - `git cat-file -p refs/libra/agent-traces` 能看到 commit 链
3. **Rewind dry-run**（阶段 2 末）：
   - 跑两次 session；`libra agent checkpoint rewind <第一次的 id> --dry-run` 列出将影响文件
   - `--apply` 后工作树回到第一次状态；`git status` 在用户分支干净
   - 输出包含 transcript 未覆写的明显警告
4. **Redaction**（阶段 1 末）：
   - prompt 故意塞 fake `AKIA...`；`git cat-file -p <transcript blob oid>` 看到 `<REDACTED:aws-access-key-id>`
   - `agent_session.redaction_report` JSON 含一条匹配
5. **并发会话**（阶段 2 末）：
   - 同仓库并行两个 Claude Code；第二个 `TurnStart` 时事件 payload 含 `concurrent_active=true`
   - 两条 session 都正常完成
6. **Hook 失败安全**（阶段 1 末）：
   - 安装 hook 后 `mv $(which libra) /tmp/libra-removed`
   - Claude Code 仍跑完一个 session
   - 还原 libra 后 `libra agent doctor` 报"未捕获的 session N 个"
7. **云同步**（阶段 3 末）：
   - 配 R2/D1 凭据；触发 session 后 `libra cloud sync`
   - D1 控制台见 `agent_session` 行；R2 见 transcript blob
   - 另一台 `libra cloud restore --repo-id <id>` 后 `libra agent session list` 见原 session
8. **Unknown-event-safe envelope**（阶段 1 末）：
   - 老 reader 读包含未来虚构事件 `kind=future_event_xyz` 的 metadata blob，落到 `Unknown(Value)` 分支不报错

---

## Changelog

- **2026-05-04（初稿）**：基于 EntireIO（Go）的 Agent 抽象与 checkpoint 模型，结合 Libra 现有 `HookProvider`/`HistoryManager`/分层存储/云同步基座，设计 v1 7-Agent 矩阵 + Libra 原生格式 + 与 `libra code` 完全隔离的整合方案
- **2026-05-04（v2）**：按 main 分支状态修订——迁移文件采用 `sql/migrations/NNNN_name.sql` 命名；区分 `agent_run/`（gated 在 `subagent-scaffold`，Libra 自家 sub-agent）与本计划的关注边界；交叉引用 `agent.md`/`sandbox.md`
- **2026-05-05（v3，融合 revised 后定稿）**：合并 entire_revised.md 的精炼方案——
  - **Lean v1**：仅 Claude + Gemini 上线，其余 5 个 preview
  - **Git 原生优先**：删除 SQLite 中的 `transcript_blob_oid`；transcript 路径从 `traces_commit` → tree 解析
  - **删除 `agent_session_event` 表**：`SessionStore` JSONL 已承载事件流
  - **删除 shadow ref**：用 `agent-traces` 上的 orphan commit + scope trailer
  - **`RedactedBytes` 编译契约**：未脱敏字节类型层面无法进入持久化
  - **Adapter trait 拆分**：`ObservedAgent` 核心 + `ObservedAgentHooks` / `TranscriptTruncator` / `TranscriptChunker` 可选
  - **v1 rewind 仅 dry-run / 工作树**：transcript 覆写归 v2
  - **Migration 文件化**：`include_str!` 加载新增 `2026050303_agent_capture.sql`
  - **`is_locked_branch` 扩展**：加 `agent-traces`，并在 `restore`/`reset` 调用
  - **Phase 1 文件级落地清单**：新增第 16 章，把抽象计划落到具体文件
  - **EntireIO 差异决策表**：第 15 章
  - **基线核对验证**：所有引用的现状（迁移版本、`is_locked_branch` 范围、`Commands` 枚举无 `Hooks`/`Agent`、`tests/db_migration_test.rs` 硬编码断言行号）已对仓库做 grep 验证
