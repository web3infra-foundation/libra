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
| `process_hook_event_from_stdin` | [hooks/runtime.rs:157](../../src/internal/ai/hooks/runtime.rs) | stdin → envelope → dedup → apply → 写 ai_session blob 到 `AI_REF` |
| `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head` / `update_ref_if_matches` | [src/internal/ai/history.rs](../../src/internal/ai/history.rs)（`new_with_ref` :176、`resolve_history_head` :459、`create_append_commit` :601、`update_ref_if_matches` :745） | 任意 orphan ref 上的 CAS 追加，已带 SQLite-busy 与 head-conflict 双重重试 |
| `SessionStore::lock_session` + `SessionFileLock` | [src/internal/ai/session/store.rs:440](../../src/internal/ai/session/store.rs)（`SessionFileLock` 类型在 store.rs:44；`SESSION_LOCK_TIMEOUT = 5s`、`STALE_SESSION_LOCK_AGE = 30s`） | 跨进程会话文件锁，基于 `.libra/sessions/<id>.lock` |
| 分层存储 | [src/utils/client_storage.rs:351](../../src/utils/client_storage.rs)（`LIBRA_STORAGE_THRESHOLD` 解析）+ `put()`（client_storage.rs:500） | 大 blob 自动按 `LIBRA_STORAGE_THRESHOLD` 推到 R2 |
| 云同步 | [src/command/cloud.rs::run_cloud_sync](../../src/command/cloud.rs)（当前位于 cloud.rs:872；`ensure_object_index_table` 在 :893 起 driver query） | 增量按 `object_index` 表迭代 |
| Migration runner（CEX-12.5） | [src/internal/db/migration.rs:532](../../src/internal/db/migration.rs)、[sql/migrations/README.md](../../sql/migrations/README.md) | 当前注册表共 7 条迁移（`automation_log` / `agent_usage_stats` / `agent_capture` / `agent_checkpoint_parent_nullable` / `approved_permission` / `agent_usage_stats_agent_name` / `source_call_log`），全部走 `include_str!` 加载（v0.17.400 起 inline SQL 已抽取到文件）；`run_builtin_migrations`（migration.rs:652）公开 API 可用 |
| `stash::build_tree_recursive` | [src/command/stash.rs](../../src/command/stash.rs) | 工作目录 → tree，已处理 index 合并、忽略文件、子模块 |
| `restore` 路径还原 | [src/command/restore.rs](../../src/command/restore.rs) | rewind 复用此路径 |
| `object_index` 表 | [src/utils/object.rs](../../src/utils/object.rs) + [src/internal/db.rs](../../src/internal/db.rs) | 自动驱动云同步 |

### 1.2 现状待修正项（前轮文档曾不准确）

| 现状 | 必须修正 |
|------|---------|
| `src/cli.rs` 的 `Commands` 枚举**无** `Hooks` 或 `Agent` 变体（grep 已确认） | ✅ 已落地：`Commands::Agent(command::agent::AgentArgs)`（cli.rs:422）与 `Commands::Hooks(command::hooks::HooksArgs)`（cli.rs:434，`#[command(hide = true)]` 兼容层）已实现，并在 dispatch match 中接入（cli.rs:1198 / :1199） |
| `builtin_migrations()` 历史上**用 inline SQL 字符串**，未走 `include_str!`（曾位于 migration.rs:499-540） | ✅ 已落地（v0.17.400）：`2026050301_automation_log` / `2026050302_agent_usage_stats` 已抽取到 `sql/migrations/2026050301_automation_log{,_down}.sql` 与 `2026050302_agent_usage_stats{,_down}.sql`，与 `2026050303_agent_capture` 起的后续迁移一致走 `include_str!`；[sql/migrations/README.md](../../sql/migrations/README.md) 注册表已同步标记两条 SQL 文件来源。当前 builtin_migrations 位于 migration.rs:532 |
| [sql/migrations/README.md](../../sql/migrations/README.md) 仍写"4 位版本号 NNNN"，与现网 `2026050301` 不一致 | ✅ 已落地：README 已改为 `YYYYMMDDNN` 形式说明，并明确所有迁移走 `include_str!`（v0.17.400 起注册表的两条 inline 来源已抽取为文件） |
| `is_locked_branch` 仅匹配 `DEFAULT_BRANCH \| INTENT_BRANCH`（曾位于 branch.rs:45） | ✅ 已落地：`AGENT_TRACES_BRANCH` 已加入 `is_locked_branch`（[branch.rs:51](../../src/internal/branch.rs)）；`branch`（create / delete / rename）、`switch`（create）已直接检查；`restore` 通过 [restore.rs:198-202](../../src/command/restore.rs) 调用 `is_locked_revision` 拒绝 `agent-traces` / `agent-traces~1` 等所有源 revision，`reset` 通过 [reset.rs:335](../../src/command/reset.rs) 同样拒绝 `agent-traces` / `agent-traces^` 等所有目标 revision。回归覆盖：[tests/command/restore_test.rs:305](../../tests/command/restore_test.rs)（`--source agent-traces~1`）+ [tests/command/reset_test.rs:207/339](../../tests/command/reset_test.rs) |
| `tests/db_migration_test.rs` **硬编码** `vec![2026050301, 2026050302]` 与 `vec!["automation_log", "agent_usage_stats"]`（[lines 47-63, 68, 1040](../../tests/db_migration_test.rs)） | ✅ 已落地：注册表回归测试已扩展到**当前全部 9 条迁移**（`2026050301`..`2026060401`：automation_log / agent_usage_stats / **agent_capture** / agent_checkpoint_parent_nullable / approved_permission / agent_usage_stats_agent_name / source_call_log / source_call_log_agent_run_id / cherry_pick_state），`max_registered_version() == Some(2026060401)`；新增迁移仍需同步更新这三处断言 |

### 1.3 实现状态总览（2026-06-05 全量核对）

> **本节是"计划 ↔ 代码"对照的权威结论**，对 `/Volumes/Data/entireio/cli`（Go 参考实现）与 libra 最新代码逐项核对后给出。图例：✅ = 完整落地且有测试；🟡 = 核心已落地但与本文早期描述有差异（差异已在对应小节就地订正）；⏳ = 明确推迟到 v2/后续阶段。**实现已越过本文最初定义的 v1/v2 边界**——Phase 4（原"本次不做"）的外部 RPC、派生 ToolCallRecord、5 个 preview adapter 提升为 stable 均已落地。

| 计划区域 | 状态 | 代码位置 / 说明 |
|---------|------|----------------|
| 迁移 `2026050303_agent_capture` + 注册表 | ✅ | `builtin_migrations()`（[migration.rs:532](../../src/internal/db/migration.rs)）现共 **9** 条（…→`2026060401`）；`tests/agent_capture_migration_test.rs` 覆盖 fresh / legacy / up→down→up |
| 适配层 trait（核心 + 能力） | ✅ | [adapter.rs](../../src/internal/ai/observed_agents/adapter.rs)：`ObservedAgent` + `ObservedAgentHooks` + `TranscriptTruncator` + `TranscriptChunker` |
| 7-Agent 矩阵（read_transcript） | ✅（越界） | Phase 4.4 已把 Cursor/Codex/OpenCode/Copilot/FactoryAi 由 preview **提升为 stable**（[builtin/stable_promoted.rs](../../src/internal/ai/observed_agents/builtin/stable_promoted.rs)，`read_transcript` 真实读盘）；`PREVIEW_SPECS` 现为空、`is_preview()` 恒 `false`（见 §5.2 订正） |
| 5 promoted adapter 的 `HookProvider`（自动 hook 安装） | ⏳ | **手动捕获可用**；缺自动 hook 安装——这 5 个无 `HookProvider`，`enable` 仍只装 claude/gemini。5 种机制形态各异（Go 端 342–483 行/个，含 merge 幂等），且无真实 agent 不可端到端验证，刻意未仓促移植；**完整实装契约见 §2.1 订正表**（专属文件 vs merge、subagent-hook declined） |
| `RedactedBytes` 编译契约 + Redactor | ✅ | [redaction.rs](../../src/internal/ai/observed_agents/redaction.rs)：`pub(crate)` 构造 + 两个 `compile_fail` doctest；25+ 高置信规则、幂等、误报回归 |
| Hook 摄入参数化 `HookTarget` | ✅ | [runtime.rs](../../src/internal/ai/hooks/runtime.rs) `process_hook_event_with_target`，旧 API 1:1 包装；`AgentTraces` 路径强制脱敏 + upsert + checkpoint，经 `libra agent hooks <agent> …` 触发 |
| Checkpoint commit 生成 | ✅ | `metadata.json`（BTreeMap → 天然 canonical-sorted）+ `transcript/<provider>` blob + `Libra-*` trailer + CAS 追加 `agent-traces` + `agent_checkpoint` 行；`tree/` 工作树快照**刻意不做**（避免把 gitignored 机密带入 R2-同步的 agent-traces，rewind 从 `parent_commit` 恢复）、`events.jsonl` 对 agent 捕获 N/A（事件落 SQLite）——见 §7.1 订正 |
| Checkpoint scope 三态 | ✅ | 三态均有生产侧产生点：`committed`（TurnEnd/SessionEnd）、`subagent`（`PostToolUse[Task]`）、`temporary`（`PostToolUse[TodoWrite]`，`clean` 移除）；`checkpoint_scope_for_tool` + 参数化 `write_checkpoint`，仅 Task/TodoWrite 产生 checkpoint 避免泛滥（见 §7.2 订正） |
| `checkpoint show` | ✅ | metadata + **transcript 字节长度 + tree 摘要**（2026-06-05 补齐：[checkpoint.rs](../../src/command/agent/checkpoint.rs) `summarize_checkpoint_tree`） |
| `checkpoint rewind` dry-run/apply | ✅ | 恢复工作树到 `parent_commit`；Claude Code 截断本地 transcript，其它 kind 明确告警 |
| `clean [--all]` | ✅ | stopped-only、删 `temporary` 行、重写 `agent-traces` orphan tip |
| `doctor` | ✅ | hook 安装状态、孤儿 checkpoint、**stuck 会话检测**（2026-06-05 补齐，§13#8） |
| `session list/show/stop/resume` + `--extract-transcript` | ✅ | [session.rs](../../src/command/agent/session.rs)；**新增 `--worktree` 过滤**（2026-06-05） |
| `worktree_id` 落库 + 过滤 | ✅ | upsert 按 cwd 解析 worktree 根目录名落库（`runtime.rs::resolve_worktree_id`）；`session list --worktree`（§3.4 / §13#9） |
| 云同步（R2 + D1） | ✅ | `o_type='agent_transcript'`（仅 transcript blob）；[cloud.rs](../../src/command/cloud.rs) `sync_agent_capture_tables` + `restore_agent_capture_from_d1`；[d1_client.rs](../../src/utils/d1_client.rs) ensure/upsert（见 §10.1 订正） |
| `agent push` | ✅ | 固定 refspec `agent-traces:refs/libra/agent-traces`，不创建远端 `refs/heads/agent-traces` |
| 分支保护 | ✅ | `is_locked_branch` + restore/reset/switch/checkout |
| **Phase 4** 外部 RPC | ✅（越界） | [observed_agents/rpc.rs](../../src/internal/ai/observed_agents/rpc.rs) + [command/agent/rpc.rs](../../src/command/agent/rpc.rs)：`rpc list` / `rpc invoke` |
| **Phase 4** 派生 ToolCallRecord | ✅（越界） | [derived.rs](../../src/internal/ai/observed_agents/derived.rs) + `session derive-tool-calls` |
| **Phase 4** 跨体系 promote | ✅（越界） | `session promote --as-intent`（→ `refs/libra/intent`） |
| 资源隔离（SessionStore 子目录） | 🟡 | `agent` 用 `sessions/agent/`；`libra code` 仍用 `sessions/` 根（目标"避免锁冲突"已达成，见 §11.5 订正） |
| 会话文件锁（agent 路径） | ✅ | `ingest_agent_traces_payload` 在 `Some(repo_path)` 下取 `SessionStore` 文件锁（`sessions/agent/<id>.traces` 锁，与 AiIntent 的 `.lock` 不同名以避免自死锁），覆盖 concurrent-check → UPSERT → checkpoint 临界区；`None`（单测）路径无锁、确定性（见 §13#6 订正） |
| 熵 / 通用 URI / DB-DSN / bounded-KV / placeholder / JSON-aware / config-mode 脱敏 | ✅ | 分层引擎落地（[redaction.rs](../../src/internal/ai/observed_agents/redaction.rs)）：Shannon 熵、任意-scheme credential-URI、JDBC/DSN/semicolon 连接串、vendor-前缀 K/V、placeholder 白名单、`redact_jsonl` 字段跳过、`RedactionMode redact\|warn\|off` + `agent.redaction.*` 配置；55 单测全绿；§8.3 transcript blob 恒强制脱敏（见 §8 订正） |
| gitleaks 260+ 全规则矩阵 / PII address | ⏳ | 当前以 25+ 高置信规则近似 betterleaks（不引入第三方依赖）；PII 仅 email/phone，address 留作后续 |
| 非 Claude `TranscriptTruncator` | 🟡 | **Gemini 已落地**（[builtin/gemini.rs](../../src/internal/ai/observed_agents/builtin/gemini.rs)：解析单 JSON 文档、按 `messages[].timestamp` 截断、保留无时间戳消息；`truncator_for` 现为 `{ClaudeCode, Gemini}`）；Cursor(SQLite)/Codex/OpenCode/Copilot/FactoryAi 仍待 per-format 截断 |
| `storage.rs` / `registry.rs` 独立文件 | 🟡 | **从未创建**：`agent_session` / `agent_checkpoint` 的 SQL 内联在调用点（runtime.rs / command/agent/*），adapter 注册表为 `observed_agents::mod.rs::agent_for()`（见 §16 订正） |

---

## 2. v1 设计目标与边界

### 2.1 目标（v1）

1. 接入 **Claude Code、Gemini** 两种外部 Agent 并持久化其原始 transcript；其余 5 种（Cursor、Codex、OpenCode、GitHub Copilot CLI、Factory AI Droid）在 v1 注册为 **preview**，CLI 可见但走存根实现。
   > **2026-06-05 订正**：Phase 4.4 已将这 5 个 preview adapter **提升为 stable**（`builtin/stable_promoted.rs`，`read_transcript` 按 `AgentSessionCtx.transcript_path` 真实读盘，16 MiB 上限），`PREVIEW_SPECS` 现为空、`is_preview()` 恒 `false`。这意味着**手动捕获已可用**（用户把 libra 指向 transcript 即可）；**仍缺的只是自动 hook 安装**——这 5 个在 ClaudeCode/Gemini 之外**没有 `HookProvider`**，故 `libra agent enable <slug>` 仍只为 claude-code/gemini 安装 hook（其余按"无 HookProvider"跳过；[`command/agent/mod.rs`](../../src/command/agent/mod.rs) `STABLE_AGENT_SLUGS` 与 `provider_for_slug`）。
   >
   > **为何 v2 / 实装契约（已 ground-truth 核对 EntireIO Go `cmd/entire/cli/agent/<a>/hooks.go`）**：5 个 agent 的 hook 机制**形态各异**，逐个需独立的 settings 安装器（Go 端各 342–483 行，含 read-merge-write 幂等 + uninstall 保留他人 hook 的逻辑），且**无法在缺少这 5 个真实 agent 的环境中端到端验证**（写错 merge 会污染用户配置、写错 shape 会静默不触发）。故本轮**刻意不仓促移植 ~2000 行不可验证安装器**；下表是实装契约，可据此逐个落地（每个：新 `providers/<a>/{mod,parser,settings}.rs` 实现 `HookProvider` + `find_provider`/`STABLE_AGENT_SLUGS` 注册 + `agent hooks <slug>` 子命令 + 单测断言生成的配置匹配 Go 形态）：
   >
   > | agent | 配置文件 | 形态 | 安全性 | 备注 |
   > |-------|---------|------|--------|------|
   > | Copilot CLI | `.github/hooks/entire.json` | libra **专属文件** | 覆写安全（非 merge） | 最易落地：写/删整文件 + 存在性检查 |
   > | OpenCode | `.opencode/plugins/libra.ts` | libra **专属 TS 插件**（`include_str!` 模板，把 `entire hooks opencode` 换成 libra 二进制 + `agent hooks opencode`） | 覆写安全 | 机制不同（TS 插件回调，非 JSON envelope）；`template/opencode_plugin.ts` 新增 |
   > | Codex | `.codex/hooks.json` | Claude-matcher shape（PascalCase `SessionStart`/`UserPromptSubmit`/`PostToolUse`/`Stop`） | 需 **merge**（保留用户既有 hook） | 可复用 claude `settings.rs` 的 merge 思路 |
   > | Cursor | `.cursor/hooks.json` | `{version:1,hooks:{...}}`，**camelCase** key（`sessionStart`/`beforeSubmitPrompt`/`stop`/`preCompact`；`subagentStart`/`subagentStop` 无 libra 映射→不装） | 需 **merge** | drop `ModelUpdate`（Cursor 无） |
   > | Factory AI Droid | `.factory/settings.json` | Claude-matcher shape | 需 **merge**（与用户 factory 设置共存） | 暴露 model+compaction hook |
   >
   > **跨 agent 取舍（已定）**：subagent 生命周期 hook（Cursor `subagentStart/Stop`、Copilot `subagentStop`）在 libra `LifecycleEventKind`/`ProviderHookCommand` **无对应 kind**，故**不安装**（或映射到 `TurnEnd`）——列为明确 declined-feature，矩阵不宣称 100% parity。`ModelUpdate`/`Compaction` 覆盖不均（仅 Gemini/Factory 暴露）。per-agent `TranscriptTruncator`（Cursor 为 SQLite，非 JSONL，截断非平凡）同属此 v2 批次。
2. 保持 `refs/libra/intent` / `ai_session` / `libra code` 行为完全不变。
3. v1 强调**可观测**：可追踪会话、可查看 checkpoint 列表、可提取 transcript 快照。
4. v1 引入 EntireIO 风格的 Subagent（子代理）级联追踪元数据（仅记录 `parent_session_id`），实际嵌套语义随各 adapter 的 `SubagentExtractor` 后续补全。
5. 与 `subagent-scaffold` Cargo feature 无关；默认构建即可用。

### 2.2 不做（v1）

1. 不替换/重写 `HookProvider` 全量 API；以**新增并存**的方式扩展。
2. 不强制统一整库 transcript schema；保留 provider 原生格式（`jsonl/sqlite/markdown/binary`）的字节语义。
3. **不建 `agent_session_event` 表**——`SessionStore` 的 JSONL 已承载事件流。
4. **不建 shadow ref `refs/libra/agent-shadow/...`**——用 orphan commit 上的 `Libra-Scope: temporary` trailer 区分。
5. **不承诺统一 transcript 覆写**——v1 只对已实现 `TranscriptTruncator` 的 Agent 执行本地 transcript 截断（当前为 Claude Code）；其它 provider 仍只恢复工作树并打印明显 warning。
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

[projection/](../../src/internal/ai/projection/) 是 runtime projections 层，处理 `ai_thread`/`ai_index_*` 等结构化 AI 制品。本计划的 `agent_session` 与 `agent_checkpoint` 是**对等 sibling**，不进 projection 模块；通过可空 `agent_session.thread_id` 弱关联。（**2026-06-05 订正**：原文称这两表"独立放在 `observed_agents/storage.rs`"——该 DAO 模块从未创建，SQL 直接内联在 `hooks/runtime.rs` 与 `command/agent/*.rs` 调用点。）

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

[`src/internal/db/migration.rs:532`](../../src/internal/db/migration.rs) 的 `builtin_migrations()` 现在全部走 `include_str!`（v0.17.400 起 inline SQL 已抽取）。新增条目继续沿用 `include_str!`：

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
- 第 47-63 行：`vec![2026050301, …, 2026052301]` / `vec!["automation_log", …, "source_call_log"]`
- 第 68 行：`max_registered_version() == Some(2026052301)`
- 第 1040 行：`applied == vec![2026050301, …, 2026052301]`
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

把现 [hooks/runtime.rs:157](../../src/internal/ai/hooks/runtime.rs) 的 `process_hook_event_from_stdin` 抽离为内部参数化函数：

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

> **2026-06-05 订正（以代码为准）**：实际 `agent_session.state` 机（[runtime.rs](../../src/internal/ai/hooks/runtime.rs) 的 `new_state` match）将 **`TurnEnd` 也映射为 `stopped`**（回合间"空闲"），下一个 `TurnStart` 再回到 `active`，只有 `SessionEnd`/`TurnEnd` 写 `stopped_at`。这与上表"TurnEnd active 保持"不同，但**是有意设计且被回归测试 pin 住**（`runtime.rs` 的 "Stop/TurnEnd must create an intermediate checkpoint" 测试断言 `state=='stopped'`），并与独立的 `SessionPhase` live-status 模型（"TurnEnd parks at Stopped"）一致。语义：`active` = 回合进行中，`stopped` = 回合间空闲 / 会话结束。`clean` 只清 `stopped` 会话，因此该映射使"回合进行中的会话不会被误清"。**`ToolUse` 不创建 checkpoint**（`subagent`/`temporary` 生产侧未接线，见 §7.2）。`AgentTraces` 路径**不取 `lock_session`**（见 §13#6 订正）。

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

> **2026-06-05 订正（以代码为准）**：实际生产路径（[runtime.rs](../../src/internal/ai/hooks/runtime.rs) `write_checkpoint` → [history.rs](../../src/internal/ai/history.rs) `append_checkpoint_commit`）落地了第 3/5/6/7 步，三态 scope 也已接线（§7.2）。对第 1/2/4 步的现状逐项核对：
> - **canonical JSON 已满足**：metadata 用 `serde_json::to_vec_pretty(&Value)`，而本仓 `serde_json` **未启用 `preserve_order`**（无 `indexmap`），故底层为 `BTreeMap`、键按字典序输出——即天然 canonical-sorted、字节稳定。无需额外排序代码。
> - **`tree/` 工作树快照：刻意不做（安全取舍，非缺口）**。`refs/libra/agent-traces` 经 §14.3 自动同步到 R2；把任意工作树字节快照进该 ref 会有把 `.env` 等 gitignored 机密带入持久层的 P0 风险。`rewind --apply` 本就从 checkpoint 的 **`parent_commit`**（摄入时用户分支 HEAD，已受版本控制+脱敏约束）恢复工作树，不依赖 `tree/` 快照；审计需求由 `parent_commit` + 脱敏后的 `transcript/<provider>` 满足。`protected_dirs()` 由 `rewind`/`clean` 的安全语义消费。本文 §3.3/§7.1 将 `tree/` 标注为"可选"，此处据安全考量明确**不实装**。
> - **`events.jsonl`：对外部 Agent 捕获不适用（N/A）**。Agent 捕获路径不写 `SessionStore` JSONL（事件落 SQLite `agent_session` + `redaction_report`），故无 JSONL 可挂载；`append_checkpoint_commit` 的 `events_jsonl` 形参保留但恒传 `None`。事件流的等价信息已在 `agent_session` 行与 transcript blob 中。
> - `checkpoint show` 现已补齐 transcript **字节长度** + tree 摘要（§7.3 / §1.3）。

### 7.2 Temporary vs Committed

- **`scope='temporary'`**：`PostTool[Task]` 等临时点，commit message 也带 `Libra-Scope: temporary`，`libra agent clean` 会重写 `agent-traces` tip 移除（git gc 后自动回收 blob/tree）
- **`scope='committed'`**：`TurnEnd`/`SessionEnd`，长期保留
- **`scope='subagent'`**：子 Agent 嵌套，关联到 `parent_checkpoint_id` 与 `subagent_session_id`

> **2026-06-05 订正（生产侧产生点已落地）**：三态 checkpoint 均有生产路径。`write_checkpoint`（原 `write_committed_checkpoint`）现接 `scope: CheckpointScope` 参数；`ingest_agent_traces_payload` 在 `Some(repo_path)` 下按事件分派：`TurnEnd`/`SessionEnd → committed`；`PostToolUse` 经 `checkpoint_scope_for_tool(tool_name)` 把 `Task → subagent`、`TodoWrite → temporary`，其它工具不产生 checkpoint（避免每次 Read/Edit 都落一个 commit）。`agent_checkpoint.tool_use_id` 列随之写入（metadata 也带 `tool_use_id`/`scope`）。回归：`ingest_tool_use_produces_subagent_and_temporary_checkpoints`。**仍属后续**：`subagent` checkpoint 的完整 `parent_checkpoint_id`/`subagent_session_id` 关联与嵌套子会话 transcript 解析（EntireIO 的 pre-task marker + 文件 diff 机制），当前只记录 scope 与 tool_use_id。

### 7.3 Rewind（v1 dry-run / read-only）

- `libra agent checkpoint show <id>`：展示 metadata + transcript 长度 + tree 摘要
- `libra agent checkpoint rewind <id> --dry-run`：默认 dry-run，打印将影响的文件列表
- `libra agent checkpoint rewind <id> --apply`（不带 `--apply` 时拒绝执行）：恢复**工作树**（复用 `restore` 路径），HEAD 与 `refs/heads/*` 不动；若 checkpoint 属于 Claude Code 且 `metadata_json.transcript_path` 可用，则把本地 transcript 截断到 checkpoint boundary；其它 agent kind 明确打印 warning 并保持 transcript 不变：
  ```
  Note: agent_kind '<provider>' has no TranscriptTruncator adapter yet;
  the agent's local transcript was left untouched.
  ```

### 7.4 清理

`libra agent clean [--all]`：
1. 默认查最近一个 `state='stopped'` session；`--all` 才扩大到所有 stopped session（v0.17.1115 已落地）。active session 永不清理，避免删掉仍在运行的外部 Agent 临时 checkpoint。
2. 对选中 session，查 `agent_checkpoint WHERE session_id=? AND scope='temporary'`
3. 删 `agent_checkpoint` 对应行（v0.17.1115 已加回归测试覆盖 default/`--all` 的 stopped-only 语义）
4. v0.17.1117 已补齐 `refs/libra/agent-traces` orphan branch rewrite：clean 会按保留的 checkpoint catalog 重建可达 commit 链，跳过被删除的 temporary checkpoint；若仓库只有历史 DB 行且 ref 为空，则保持 SQLite-only cleanup。
5. 不主动 `git gc`——交底层自然回收

### 7.5 与现有命令交互

- ✅ 扩展 [`is_locked_branch`](../../src/internal/branch.rs)：已新增匹配 `AGENT_TRACES_BRANCH` 常量（branch.rs:42 / :51）；`branch`（create / delete）、`switch`（create / existing target）与 `checkout` 均已调用
- ✅ 锁定分支互操作已收口：[`command/restore.rs`](../../src/command/restore.rs) 通过 `RestoreError::LockedSource` 守 `--source <locked-ref>`，并通过 `RestoreError::LockedCurrentBranch` 拦截当前位于 `intent` / `agent-traces` 时的 worktree restore；[`command/reset.rs`](../../src/command/reset.rs) 通过 `ResetError::LockedTarget` 守 `reset <locked-ref>`，并通过 `ResetError::LockedCurrentBranch` 拦截当前位于 `intent` / `agent-traces` 时的整树 reset。`main` 仍只在 branch-management 语义下锁定，不阻塞普通用户工作树。
- ✅ `git log refs/libra/agent-traces` 直接可用

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

> **2026-06-05 订正（以代码为准，分层引擎已落地）**：[redaction.rs](../../src/internal/ai/observed_agents/redaction.rs) 现为**分层检测引擎**（参照 EntireIO `redact/redact.go`），`RedactedBytes` 编译契约 + 以下检测层全部落地并有单测覆盖（55 个单测全绿）：
> - **Layer 1 静态规则**（25+ 高置信前缀规则，幂等、丰富误报回归、`compile_fail` doctest）；
> - **Layer 2 熵检测**（§8.2 P1）：Shannon entropy > 4.5 + `[A-Za-z0-9+_=-]{10,}` 候选 + JSON-escape 守卫（`refine_entropy_span`），git SHA / UUID / 路径不误伤；
> - **通用 credential-URI**：原固定 scheme 白名单已泛化为任意 scheme（`redis://:pass@`、自定义 scheme 均覆盖）；
> - **Layer 3 DB 连接串**（§8.2 P1）：JDBC / keyword-DSN / semicolon-conn 三类，配 `has_secret` placeholder 感知门控（`<password>` 占位不误伤）；
> - **Layer 4 bounded K/V**（§8.2 P2）：vendor 前缀 `dbPasswordKeyShape`，短值也命中、placeholder 白名单门控；
> - **Layer 5 PII opt-in**（§8.2 P3，默认关）：email（含 noreply 白名单）+ phone，经 `PiiConfig`/`agent.redaction.pii.*` 开启；address 暂缺。
> - **JSON-aware 替换**（`redact_jsonl`）：整体单 JSON 值或逐行 JSONL 解析，**跳过** `*id`/`*ids`/`filepath`/`cwd`/`path`/`signature` 字段与 `type:image`/`base64` 对象，仅替换 value 位置（`replace_keyed_json_value`），非 JSON 行回退标量脱敏。
> - **`[agent.redaction] mode = redact|warn|off`**（§8.4）：`RedactionMode` 枚举 + `from_config_str`（未知值安全回退 `redact`）；runtime 经 `build_configured_redactor` 从 `config_kv` 读取 `agent.redaction.mode` 与 `pii.*`。
> - **§8.3 P0 安全边界**：`mode` 只影响 `agent_session` 行的 prompt/tool_input 字段是否替换；写入 `refs/libra/agent-traces` 的**完整 transcript blob 与 prompt 回退恒为强制脱敏**（`build_checkpoint_transcript_redacted` 始终用 `RedactionMode::Redact` + `redact_jsonl`），`warn`/`off` 永远不能让未脱敏字节落入持久层。
> 仍属后续：完整 gitleaks 260+ 规则矩阵（当前以 25+ 高置信规则近似，不引入 betterleaks 依赖）、PII address 检测。

---

## 9. CLI 命令面（v1）

加到 [`src/cli.rs`](../../src/cli.rs) `Commands` 枚举：`Commands::Hooks(HooksArgs)` + `Commands::Agent(AgentArgs)`。

```
libra hooks <provider> <subcommand>      # hidden 兼容入口（仅 claude/gemini）
libra agent
  enable [--agent <name> ...]            # 安装 hook，注册 agent
  disable [--agent <name> ...]           # 卸载 hook
  status                                 # 活跃会话计数 + 最近 checkpoint
  session list [--agent <name>] [--state <s>] [--worktree <id>]   # --worktree 为 2026-06-05 新增
  session show <id> [--extract-transcript <path>]
  session stop <id>                      # 标记 metadata state=stopped
  session resume <id>                    # 标记 metadata state=active，不恢复 transcript 上下文
  session promote <id> [--as-intent] [--prompt <text>] [--dry-run]  # Phase 4.2：提升到 refs/libra/intent（已实现）
  session derive-tool-calls <id>         # Phase 4.3：从归一化事件派生 ToolCallRecord（已实现）
  checkpoint list [--session <id>]
  checkpoint show <id>                   # metadata + transcript 字节长度 + tree 摘要
  checkpoint rewind <id> [--dry-run|--apply]   # --apply 恢复工作树；Claude Code transcript 会按 checkpoint 截断
  clean [--all]
  doctor                                 # 诊断 hook 安装、stuck 会话、孤儿 checkpoint
  push [--remote <name>]                 # 推送 refs/libra/agent-traces
  rpc list                               # Phase 4.5：发现 PATH 上的 libra-agent-<name> RPC 二进制（已实现）
  rpc invoke <slug> <method> [--params <json>]   # Phase 4.5：调用单个 JSON-RPC 方法（已实现）
  hooks <agent> <subcommand>             # hidden；agent hooks 调用入口
```

> **2026-06-05 订正**：原"v1 不实现"清单中的 `session promote --as-intent`（Phase 4.2）与外部 `libra-agent-<name>` RPC（Phase 4.5）**均已落地**，另新增 `session derive-tool-calls`（Phase 4.3）。`checkpoint explain` 仍按计划合并进 `checkpoint show`。仍未实现（v2）：
> - 非 Claude Code provider 的 `checkpoint rewind --apply` transcript 截断（需各 provider 接 `TranscriptTruncator`）

### 9.1 初始化

`libra init` 在初始化路径中追加：
```rust
HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch().await?;
```
与现有 `refs/libra/intent` 初始化并列。

### 9.2 分支保护

已落地在 [`src/internal/branch.rs:51`](../../src/internal/branch.rs)：
```rust
pub fn is_locked_branch(name: &str) -> bool {
    name == DEFAULT_BRANCH || name == INTENT_BRANCH || name == AGENT_TRACES_BRANCH
}
```

其中 `AGENT_TRACES_BRANCH` 作为常量在 branch.rs:42 定义为 `"agent-traces"`，避免把 ref 字面值散落在多个调用点。

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

Transcript blob、metadata blob、events blob 都走 `write_git_object` → `object_index` → 现有 [cloud.rs::run_cloud_sync](../../src/command/cloud.rs) 增量同步。零代码即得云备份。

> **2026-06-05 订正**：`o_type='agent_transcript'` **仅打在 transcript blob 上**（metadata / events blob 用普通 `"blob"`，见 [history.rs](../../src/internal/ai/history.rs) `append_checkpoint_commit`）。云**同步不按 `o_type` 过滤**（只按 `is_synced`）；该 tag 仅用于 `cloud status` 的按类型统计与下游枚举（与本节"用于过滤"措辞不同）。

### 10.2 D1 表同步

[d1_client.rs](../../src/utils/d1_client.rs) 新增：
- `ensure_agent_session_table`
- `ensure_agent_checkpoint_table`
- `upsert_agent_session(session)` / `upsert_agent_checkpoint(checkpoint)`

`libra cloud sync` 流程末尾追加 `agent_*` 表的增量上行。**v1 不同步**事件流（无 `agent_session_event`，且 `SessionStore` JSONL 已作 blob 同步——可从 commit tree 反解）。

### 10.3 推送独立 remote

`libra agent push [--remote <name>]`：scope 限定 `refs/libra/agent-traces`；默认 remote 为 `origin`，如需专用 remote 可传 `--remote agent-traces` 并在 `.libra/config` 配置 `[remote "agent-traces"]`。v0.17.1114 已落地为现有 push 机器的薄包装，无新 transport：本地 `agent-traces` tip 通过固定 refspec `agent-traces:refs/libra/agent-traces` 推送，且不会在远端创建 `refs/heads/agent-traces`。

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
| 孤儿 ref CAS | `HistoryManager::new_with_ref` / `create_append_commit` / `resolve_history_head` / `update_ref_if_matches` | [src/internal/ai/history.rs](../../src/internal/ai/history.rs)（:176 / :459 / :601 / :745） |
| Hook 摄入流水线 | `process_hook_event_from_stdin` → 抽离为 `process_hook_event_with_target` + 旧 API 包装 | [src/internal/ai/hooks/runtime.rs:157](../../src/internal/ai/hooks/runtime.rs) |
| 事件模型 | `LifecycleEvent` / `LifecycleEventKind` / `make_dedup_key` / `normalize_json_value` / `validate_session_hook_envelope` / `apply_lifecycle_event` / `append_raw_hook_event` | [hooks/lifecycle.rs](../../src/internal/ai/hooks/lifecycle.rs) |
| Unknown-event-safe envelope 模式 | 借鉴 `AgentRunEvent` / `AgentRunEventEnvelope`（`agent_run/` gated 在 `subagent-scaffold`，**不直接依赖**） | [agent_run/event.rs](../../src/internal/ai/agent_run/event.rs) |
| Migration runner | `MigrationRunner::register` / `run_pending` / `builtin_migrations()` | [migration.rs](../../src/internal/db/migration.rs) |
| 文件锁 | `SessionStore::lock_session` + `SessionFileLock`（5s timeout、30s stale） | [session/store.rs:440](../../src/internal/ai/session/store.rs) |
| 工作树 → tree | `build_tree_recursive` | [stash.rs](../../src/command/stash.rs) |
| 文件还原 | restore 的 path-walking | [restore.rs](../../src/command/restore.rs) |
| 分支保护 | `is_locked_branch`（扩展） / `INTENT_BRANCH` 拒绝模式 | [branch.rs:51](../../src/internal/branch.rs)、[checkout.rs:219/224/351](../../src/command/checkout.rs)（三处 INTENT_BRANCH/AGENT_TRACES match arm，分别在 :219 / :224 / :351）、[switch.rs:36/266](../../src/command/switch.rs)（`is_locked_branch` 调用 + INTENT_BRANCH 字面比较） |
| 分层存储 | `TieredStorage` + `LIBRA_STORAGE_THRESHOLD` 路由 | [client_storage.rs:351/500](../../src/utils/client_storage.rs) |
| 对象 I/O | `write_git_object` / `read_git_object` | [object.rs](../../src/utils/object.rs) |
| 云同步 | `object_index` 迭代 | [cloud.rs::run_cloud_sync (line 872)](../../src/command/cloud.rs) |
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
| 10 | **v1 过度承诺 rewind** | P1 | `checkpoint rewind` 在 v1 默认 dry-run；`--apply` 只对已实现 `TranscriptTruncator` 的 Agent 改写 transcript（当前 Claude Code），其它 agent kind 保持 transcript 不变并打印明显 warning |
| 11 | **Cursor SQLite transcript 取查** | P2 | `libra agent session show --extract-transcript <path>` 把 SQLite blob 物化到指定路径，文档说明用 `sqlite3` 查看 |
| 12 | **Unknown-event-safe envelope 兼容** | P2 | 老 reader 读包含未来虚构事件 `kind=future_event_xyz` 的 metadata.json，应落到 `Unknown(Value)` 分支不报错（与 [agent_run/event.rs](../../src/internal/ai/agent_run/event.rs) 对外承诺一致） |

> **2026-06-05 验收状态**：
> - #1 ✅ `RedactedBytes` 契约（compile-fail doctest）+ AKIA 回归。
> - #2 ✅ Claude/Gemini hook 安装/调用幂等，旧入口不变。
> - #3 ✅ 注册表断言已随迁移扩展（**当前到 `2026060401`**，非 #3 原文的 `2026050303`）；`tests/agent_capture_migration_test.rs` 覆盖 fresh/legacy/up-down-up。
> - #4 🟡 transcript blob 超阈值入 R2 ✅；`clean` 重写 `agent-traces` ✅；但生产侧不产生 `temporary`（§7.2），故"移除 temporary"常态无对象。
> - #5 ✅ restore/reset/switch/checkout 锁定回归绿。
> - #6 🟡 `concurrent_active` 标志 ✅；但 `AgentTraces` 路径**不取 `SessionStore` 锁**（并发以 `agent-traces` CAS + UPSERT 幂等兜底），"5s 锁超时恢复"仅 `AiIntent` 路径具备。
> - #7 ✅ CAS 重试（`HISTORY_HEAD_CONFLICT_MAX_RETRIES`）已就绪。
> - #8 ✅（2026-06-05 补齐）`doctor` 现报告 **stuck 会话**（active 且 6h 无事件 → 多半 agent 未发 SessionEnd）；真正"未捕获 session"无 DB 行、无法计数，以 stuck 检测作为等价信号。
> - #9 ✅（2026-06-05 全部满足）`worktree_id` 按 cwd 落库 + `session list --worktree` 过滤；**"DB 必须在 `--git-common-dir`"这一子项结构上已满足**：libra worktree 的 `<wt>/.libra` 是指向主 `.libra` 的**符号链接**，`util::try_get_storage_path` 经 `fs::canonicalize`（[util.rs](../../src/utils/util.rs)）解析到唯一物理目录，DB 连接缓存按规范化路径键控（[db.rs](../../src/internal/db.rs)），故所有 worktree 的 `agent_session` 写入同一共享 `libra.db`。无需 `git rev-parse --git-common-dir`（libra 无独立 per-worktree gitdir，符号链接目标即单一真相源）。
> - #10 ✅ rewind 默认 dry-run；`--apply` 仅 Claude Code 截断，其它 kind 告警。
> - #11 ✅ `session show --extract-transcript` 物化 transcript（从最近 checkpoint 的 Git tree 解析）。
> - #12 ⏳ 借鉴 `agent_run/event.rs` 的 unknown-safe 模式；当前 metadata.json 为普通 JSON，未针对未来事件做 `Unknown(Value)` 兜底（低优先）。

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
3. **CLI**：实现 `libra agent session list/info/show/stop/resume`、`libra agent checkpoint list/show`、`libra agent doctor`、`libra agent push`（v0.17.1114：`agent push` 已接入 `refs/libra/agent-traces` 推送）
4. **清理**：`libra agent clean`：v0.17.1115 已按 `state='stopped' AND scope='temporary'` 删除 SQLite catalog 行，且 `--all` 不再触碰 active session；v0.17.1117 已补齐 agent-traces rewrite，temporary checkpoint commit 会从可达链移除，保留 checkpoint 的 `traces_commit` / `tree_oid` 同步改写。
5. **Rewind dry-run / apply**：`libra agent checkpoint rewind <id> --dry-run` 列出将影响的文件；`--apply` 调 `restore` 路径还原工作树，并在 Claude Code checkpoint 上截断本地 transcript；其它 agent kind 打印 transcript 不变更 warning
6. **TUI/MCP 扩展（可选）**：在现有 [tui/](../../src/internal/tui/) 加一个最小 view 展示 `agent_session` 列表

**阶段 2 验收**：完整 Claude/Gemini session 能生成多个 checkpoint commit；`libra agent checkpoint list` 列出按时间排序；`git log refs/libra/agent-traces` 可读；`libra agent clean` 后 temporary commit 从 `agent-traces` 移除且 SQLite 索引同步清理。

### 阶段 3（脱敏完善 + Preview 适配器 + 云同步，2-3 周）

**目标**：脱敏达到生产可用；其余 5 个 Agent 在 CLI 可见；云同步走通。

1. **Redaction 完善**：完整 gitleaks 规则集；可选 PII 检测；`agent_session.redaction_report` 与 metadata blob 摘要落地；新增 known-bad 字符串矩阵的单元测试
2. **Preview 适配器**：在 `observed_agents/builtin/{cursor,codex,opencode,copilot,factory_ai}.rs` 写存根：`provider_kind/provider_name/protected_dirs` 返回常量，`read_transcript` 返回 `Err(AgentNotYetImplemented)`；CLI `libra agent enable` 列出全部 7 个 agent，preview 标注；`hooks <preview-agent>` 调用时打印告警并不写 traces
3. **云同步**：`d1_client` 新增 `ensure_agent_session_table` / `ensure_agent_checkpoint_table` / `upsert_*`；`libra cloud sync` 流程末尾追加；`object_index.o_type='agent_transcript'` 走正常 R2 同步；`libra agent push` scope 限定 `refs/libra/agent-traces`
4. **资源隔离**：`SessionStore` 路径分两个子目录（`code/` vs `agent/`）

**阶段 3 验收**：脱敏对模拟矩阵的命中率 100%；7 个 agent 都能 `enable`（preview 有告警）；R2 + D1 凭据配置后 `libra cloud sync` 完成且远端可见；另一台机器 `libra cloud restore` 后 `libra agent session list` 能看到原 session。

### 阶段 4（v2）

> **2026-06-05 订正**：本阶段原标"本次不做"，但 2/3/4/5 项均已落地——本计划实现进度已越过原 v1/v2 边界。

1. 🟡 非 Claude Code provider 的 `TranscriptTruncator` 实装 + `checkpoint rewind --apply` transcript 覆写 —— **Gemini 已落地**（`truncator_for` 现为 `{ClaudeCode, Gemini}`；Gemini 按单 JSON 文档 `messages[].timestamp` 截断）；Cursor(SQLite)/Codex/OpenCode/Copilot/FactoryAi 仍待 per-format 实现
2. ✅ `session promote <id> --as-intent` 跨体系提升 —— 已实现（[session.rs](../../src/command/agent/session.rs) `promote`）
3. ✅ 从 `agent_session` 反算 `ToolCallRecord`（`session derive-tool-calls`）—— 已实现（[derived.rs](../../src/internal/ai/observed_agents/derived.rs)）
4. ✅ 5 个 preview adapter → stable —— 已实现（Phase 4.4，[builtin/stable_promoted.rs](../../src/internal/ai/observed_agents/builtin/stable_promoted.rs)）
5. ✅ 外部 `libra-agent-<name>` 二进制 RPC 支持（与 EntireIO 对等）—— 已实现（[observed_agents/rpc.rs](../../src/internal/ai/observed_agents/rpc.rs) + `libra agent rpc list/invoke`）

**已补齐（2026-06-05 本轮）**：分层脱敏（熵/通用 URI/DB-DSN/bounded-KV/placeholder/JSON-aware/config-mode，§8 订正）、Gemini `TranscriptTruncator`（§14.4#1）、生产侧三态 checkpoint 产生点（§7.2 订正）、`AgentTraces` 会话文件锁（§13#6）、canonical JSON（BTreeMap 天然满足）+ `agent_session` 共享 DB（符号链接，§13#9）均落地。**仍属后续（Phase 3+/v2）**：gitleaks 260+ 全规则矩阵与 PII address（§8）、Cursor(SQLite)/Codex/OpenCode/Copilot/FactoryAi 的 per-format `TranscriptTruncator`（§14.4#1）、这 5 个 promoted adapter 的 `HookProvider`（§2.1 订正，`enable` 仍只装 claude/gemini hook）、`subagent` checkpoint 的完整嵌套子会话关联（§7.2）。

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

> **2026-06-05 订正（实际落地与本清单的差异）**：
> - **`storage.rs` 从未创建**：`agent_session` / `agent_checkpoint` 没有独立 DAO 模块，SQL 直接内联在调用点（`hooks/runtime.rs` 的 upsert/insert、`command/agent/*.rs` 的查询）。§3.5 的 "独立放在 `observed_agents/storage.rs`" 描述同样作废。
> - **`registry.rs` 从未创建**：adapter 查找是 `observed_agents::mod.rs::agent_for(AgentKind) -> &'static dyn ObservedAgent`（穷尽 match），preview 占位逻辑在 `preview.rs`（现为空）+ `builtin/stable_promoted.rs`。
> - **preview 存根已被 `builtin/stable_promoted.rs` 取代**（Phase 4.4 提升为 stable）。
> - **`tests/redaction_contract_test.rs` 非 trybuild**：编译期保证由 `RedactedBytes` 上的两个 `compile_fail` doctest 提供（功能等价）。
> - 实际新增还包括：`observed_agents/{derived,preview,rpc}.rs`、`builtin/stable_promoted.rs`、`command/agent/{rpc}.rs`、`tests/command/agent_{checkpoint,clean,push,session,help}_test.rs`。

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
| `src/internal/db/migration.rs:532` `builtin_migrations()` | ✅ `2026050303_agent_capture` 条目已加入并使用 `include_str!("../../../sql/migrations/...")`；本表保留作为历史改造记录 |
| `src/internal/branch.rs::is_locked_branch` | ✅ 已加 `\|\| name == AGENT_TRACES_BRANCH`（branch.rs:51 起的 helper） |
| `src/command/restore.rs` | 入口处加 `is_locked_branch` 检查，命中拒绝（restore 已通过 `RestoreError::LockedSource` 守 `--source`；reset / restore 命令在 cwd 上的拦截属于行为变更，留作独立切片） |
| `src/command/reset.rs` | 同上 |
| `src/internal/ai/hooks/runtime.rs:157` `process_hook_event_from_stdin` | 抽离为 `process_hook_event_with_target(..., target: HookTarget)`；旧函数 1:1 包装传 `AiIntent` |
| `src/command/init.rs`（或对应初始化路径） | 调 `HistoryManager::new_with_ref("refs/libra/agent-traces").init_branch()` |
| `src/command/agent/push.rs` + `src/command/push.rs` | ✅ v0.17.1114：`libra agent push` 复用 push 传输并允许显式 `refs/libra/*` 远端目的 ref；回归测试确认只写 `refs/libra/agent-traces`，不创建 `refs/heads/agent-traces` |
| `src/internal/ai/session/store.rs`（路径子目录） | 新增 `code/` vs `agent/` 子目录区分 |
| `tests/db_migration_test.rs:50 / :56-61 / :1040` | ✅ 已落地：注册表回归测试硬编码断言已扩展到全部七个迁移版本（`2026050301..2026052301`，含 v0.17.800 source_call_log）与对应表名 |
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
  - **v1 rewind**：默认 dry-run；`--apply` 恢复工作树，Claude Code transcript 可截断，其它 provider transcript 覆写归 v2
  - **Migration 文件化**：`include_str!` 加载新增 `2026050303_agent_capture.sql`
  - **`is_locked_branch` 扩展**：加 `agent-traces`，并在 `restore`/`reset` 调用
  - **Phase 1 文件级落地清单**：新增第 16 章，把抽象计划落到具体文件
  - **EntireIO 差异决策表**：第 15 章
  - **基线核对验证**：所有引用的现状（迁移版本、`is_locked_branch` 范围、`Commands` 枚举无 `Hooks`/`Agent`、`tests/db_migration_test.rs` 硬编码断言行号）已对仓库做 grep 验证
- **2026-06-05（全量核对 + 补齐）**：对照 `/Volumes/Data/entireio/cli`（Go 参考实现）与 libra 最新代码逐项核对，新增 **§1.3 实现状态总览**（权威"计划 ↔ 代码"对照表），并就地订正多处与现状不符的描述：
  - **越界落地**：Phase 4 的外部 RPC（`rpc.rs` + `libra agent rpc`）、派生 ToolCallRecord（`derived.rs` + `session derive-tool-calls`）、`session promote --as-intent`、5 个 preview adapter **提升为 stable**（`builtin/stable_promoted.rs`）均已实现（§2.1 / §5.2 / §9 / §14.4 订正）。
  - **行为订正**：`agent_session.state` 机将 `TurnEnd` 映射为 `stopped`（回合间空闲）而非"active 保持"，且被回归测试 pin 住（§6.3）；checkpoint commit 未做工作树快照 / `protected_dirs` 排除 / `events.jsonl` / canonical JSON（§7.1）；生产仅产生 `committed` scope（§7.2）；`o_type='agent_transcript'` 仅用于统计/枚举、云同步不按其过滤（§1.3 / §10）；`SessionStore` 仅 `agent` 分目录、`code` 仍在根（§11.5）；`storage.rs` / `registry.rs` 从未创建、DAO SQL 内联（§16）；`redaction_contract_test` 用 `compile_fail` doctest 而非 trybuild（§16）。
  - **本次补齐的功能**：① `checkpoint show` 增加 transcript 字节长度 + tree 摘要（§7.3）；② `doctor` 增加 stuck 会话检测（§13#8）；③ `worktree_id` 按 cwd 解析落库 + `session list --worktree` 过滤（§3.4 / §13#9）；④ 清理两处过期/误导性 doc 注释（`HookTarget`、`command/agent/mod.rs`）。新增测试：`tests/command/agent_session_test.rs` + `agent_checkpoint_test.rs::…show_reports_tree_summary…` + runtime `resolve_worktree_id` 单测；`cargo +nightly fmt`、`cargo clippy --all-targets --all-features -D warnings`、相关 `cargo test` 全绿。
  - **仍属后续**：gitleaks 260+ 全规则矩阵与 PII address（§8）、Cursor/Codex/OpenCode/Copilot/FactoryAi 的 per-format `TranscriptTruncator` 与 `HookProvider`（§14.4#1 / §2.1）、`subagent` checkpoint 的完整嵌套子会话关联（§7.2）。`tree/` 工作树快照按安全取舍刻意不做（§7.1）。
