# Libra CLI 命令改进顺序计划

## Context

基于两份审计报告（CLI UX 对比研究 + CLIG 六维审计报告），结合当前代码库已实现的基础设施，制定命令级别的改进优先级。

**已完成的基础设施：**
- 全局 `--json`/`--machine`/`--quiet`/`--color`/`--no-pager`/`--progress`/`--exit-code-on-warning` 标志 (`src/cli.rs`)
- 稳定错误码体系 18 个错误码 (`src/utils/error.rs`)
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架 (`src/utils/output.rs`)
- `CommandOutput` trait 支持结构化输出
- 错误码文档 (`docs/error-codes.md`)
- `init` 命令主改造已落地：`run_init()`、顶层 human/JSON/machine 渲染、`InitProgress`、显式 `StableErrorCode`、嵌套 fetch 输出隔离均已就绪
- `add` 命令主改造已落地：`run_add()` → `AddOutput` 执行层/渲染层拆分、JSON/machine 输出、显式 `StableErrorCode`、warning 接入共享 tracker
- `status` 命令主改造已落地：`StatusData` 共享数据层、upstream tracking、`--exit-code` 标志、显式 `StableErrorCode`

**已有 JSON 输出的命令（面向终端用户的高层命令）：** commit, status, branch, config, init, clone, add, push, pull, switch（底层命令如 `cat-file`、`show-ref` 也已支持 JSON，但未纳入本优先级列表）
**已用 StableErrorCode 的命令：** init, clone, add, status, commit, push, pull, switch, shortlog, lfs, code（共 11 个）

---

## 改进顺序

### 批次依赖约束（避免文档状态冲突）

为避免子计划把“已确定方案”误写成“代码已落地”，第一批及后续批次统一遵循以下表述规则：

- **已完成/已落地**：仅表示当前代码库已经具备该能力，可被后续批次直接调用或复用。
- **前置依赖/由前一批次交付**：表示该能力已经在上游计划中确定，但必须等前一批次实施完成后，下一批次才能把它当作现成能力使用。
- **本批新增**：表示能力在当前批次内交付，不应被同一时点的其他文档写成“现已存在”，除非明确注明“依赖本批先落地后复用”。

第一批中的关键依赖链已经按以下顺序落地，后续文档应直接以此为基线，而不是继续写成“前置依赖”：

- `config` 已交付 `config_kv`、`resolve_env()`、vault key 管理与 `vault` 命令吸收；`init`、`clone`、`push/pull` 等命令已在此基础上切换读取链路。
- `init` 已交付 `run_init()`、顶层渲染层拆分、separate-layout 全链路移除，以及嵌套 fetch 的静默子级 `OutputConfig` 约束；后续文档可直接把这些能力写成“当前代码已具备”。
- `clone` 已整体落地，复用 `init` 的纯执行层与 `config` 的解析/认证基础设施。成功 schema（`CloneOutput`）、显式错误码、typed checkout 失败传播与 cleanup warning 可见性均已完成；详见 [commands/clone.md](commands/clone.md) 实施记录。性能优化留后续批次。

后续各命令子计划如依赖前一批次交付项，应在“已完成前置条件与当前代码状态”中明确区分：

- 哪些能力是当前仓库已存在的基线。
- 哪些能力是上游批次的**前置依赖**。
- 哪些约束只是沿用上游批次已确定的对外契约，而不是当前批次重新设计。

### 第一批：核心高频命令 + config（P0 阻断性）

这些命令覆盖最基本的工作流（config → init → clone → add → status → commit → push/pull），使用频率最高，审计报告指出的问题最严重。config 放在首位，因为它是 vault 加密基础设施和环境变量解析的根基，其他命令依赖 config 提供的 `resolve_env()` 等能力。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **1** | `config` | ✅ 已落地 | vault-backed 存储；子命令风格；SSH/GPG key 管理；env vault；吸收 vault 命令功能（详见下方专节） |
| **2** | `init` | ✅ 已落地 | 作为 `clone` / 转发路径的已交付基线；后续仅维护回归测试与文档同步 |
| **3** | `clone` | ✅ 已落地 | 结构化成功输出（`--json`/`--machine`）；显式 `StableErrorCode`；network/auth hint；checkout 失败传播；cleanup warning 可见性；阶段性 human 进度（详见 [commands/clone.md](commands/clone.md)） |
| **4** | `add` | ✅ 已落地 | 执行层/渲染层拆分（`run_add()` → `AddOutput`）；JSON/machine 结构化输出；成功摘要；`--dry-run`/`--verbose` 输出经 `OutputConfig` 管控；显式 `StableErrorCode`；ignored/failed 按 warning 处理并接入 `--exit-code-on-warning`（详见 [add.md](add.md)） |
| **5** | `status` | ✅ 已落地 | `StatusData` 共享数据层消除重复计算；upstream tracking（human/JSON/porcelain v2）；显式 `StableErrorCode`；新增 `--exit-code` dirty → exit `1`；颜色控制统一到 `OutputConfig`（详见 [status.md](status.md)） |
| **6** | `commit` | ✅ 已落地 | `CommitError`（18 变体）typed enum + 显式 `StableErrorCode`；`run_commit()` + `render_commit_output()` 执行/渲染拆分；JSON 向后兼容扩展（+`branch`/`amend`/`signoff`/`conventional`/`signed`）；hook I/O 隔离；`--help` EXAMPLES（详见 [commit.md](commit.md)） |
| **7** | `push` | ✅ 已落地 | 修复 refspec 语法；10s 连接/空闲超时；human 进度输出；JSON 输出；错误码。**前置依赖**：需在 `protocol/` 建立可替换 transport seam 供超时/auth/protocol 测试 |
| **8** | `pull` | ✅ 已落地 | 聚合 fetch + fast-forward/up-to-date 结果；修复 upstream tracking；JSON 输出；错误码（non-fast-forward merge 留 merge 批次）。**前置依赖**：需在 `fetch.rs`/`merge.rs` 建立 pull 可复用的最小 typed helper（完整 JSON/进度改造留第五/六批） |

**理由：** config 是基础设施层，vault 加密存储和 `resolve_env()` 被其他命令（push 认证、code AI provider）依赖，必须最先完成。init/clone 是入口命令（审计指出 init 耗时 ~6s 严重违反 CLIG "100ms 内打印内容"原则）；add 是 commit 前的必经步骤；push 是审计中"最严重的三个缺陷"之一。

**第一批内部依赖说明：**

- `config` 已是第一批内部的已落地基线；`init`/`clone` 文档应直接在其上描述现状与剩余收尾项。
- `init` 已落地并成为 `clone` 的直接基线；`clone` 对 `run_init()`、separate-layout 移除、嵌套 fetch 静默规则的引用，应统一写成“当前代码已具备”。
- `clone` 已整体落地：执行层/渲染层拆分、`CloneOutput` 结构化输出、显式 `StableErrorCode` 映射、typed `RestoreError` checkout、`RemoteSpecErrorKind` 分类、cleanup warning 可见性均已完成。性能优化目标仍保留为后续独立批次。
- `add` 已整体落地：`run_add()` → `AddOutput` 执行层/渲染层拆分；JSON/machine 结构化输出（含 `added`/`modified`/`removed`/`refreshed`/`ignored`/`failed` 分类）；显式 `StableErrorCode` 映射（`AddError → CliError`）；`--dry-run`/`--verbose` 输出经 `OutputConfig` 管控；ignored/failed 接入共享 warning tracker。
- `status` 已整体落地：`StatusData` 共享数据层消除 `execute_to()` 与 `collect_status_json()` 的逻辑重复；upstream tracking（human/JSON/short/porcelain v2）；显式 `StatusError → CliError` 映射；新增 `--exit-code` 标志（dirty → exit `1`）；颜色控制统一到 `OutputConfig`。
- `commit` 已整体落地：`CommitError`（18 变体）typed enum + 显式 `StableErrorCode` 映射；`run_commit()` + `render_commit_output()` 执行/渲染拆分；JSON 向后兼容扩展（+`branch`/`amend`/`signoff`/`conventional`/`signed`）；hook I/O 隔离（JSON 模式 `Stdio::piped()`）；全部 18 变体单元映射测试 + 11 CLI 级集成测试 + 9 JSON schema 稳定性测试。

### 第二批：状态变更确认命令（P0 消灭"沉默"）

审计报告核心发现："成功时沉默、等待时沉默、失败时沉默"。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **9** | `switch` | ✅ 已落地 | 第二批主改造已落地；后续仅维护回归测试、文档同步与大仓库切换性能观察（详见 [switch.md](switch.md)） |
| **9a** | `checkout`（兼容收口） | ✅ 第二批兼容收口已落地 | 已完成 `SwitchError` 变体匹配适配与 `--help` EXAMPLES；**不是完整现代化**——`CheckoutError` / JSON / render split 仍留第六批（详见 [checkout.md](checkout.md)） |
| **10** | `reset` | 部分已落地：已有确认消息、JSON/machine、显式 `StableErrorCode`、`run_reset()` / `render_reset_output()` | 补齐 `ResetError` typed enum；移除 string-based runtime 错误分类与直写 warning；补齐 `--help` EXAMPLES（详见 [reset.md](reset.md)） |
| **11** | `tag` | 部分已落地：已有 JSON/machine、显式 `StableErrorCode`、重复创建 hint | 补齐 `TagError` typed enum；统一 run/render 分层；收口 list/show 路径的显式错误码与 human 确认消息；补齐 `--help` EXAMPLES（详见 [tag.md](tag.md)） |
| **12** | `branch` | 部分已落地：JSON 已覆盖 list/create/delete/rename/set-upstream/show-current，`StableErrorCode` 已大体补齐 | 补齐 `BranchError` typed enum；统一 run/render 分层；补齐 create/force-delete 确认消息、fuzzy suggestion 与 `--help` EXAMPLES（详见 [branch.md](branch.md)） |

**理由：** 这些命令改变仓库状态，必须告知用户发生了什么。`checkout` 的兼容收口随 `switch` 一起落地，因为 `switch` 的 `ensure_clean_status()` 签名变更强制要求 `checkout` 同步适配。

### 第三批：历史查询命令（P1 结构化输出）

使用频率高，AI Agent 场景依赖结构化输出。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **13** | `log` | 明确拒绝 --json | 实现 JSON 输出（结构化提交列表）；保持 --oneline/--graph |
| **14** | `diff` | 无 JSON | JSON 输出（hunk 级别结构化）；--numstat/--name-only |
| **15** | `show` | 有 --oneline/-s | JSON 输出；错误码 |
| **16** | `blame` | 与 Git 一致 | JSON 输出 |

**理由：** Agent 需要从历史/差异中提取结构化信息来决策。log --json 是 MCP 维度最关键的改进。

### 第四批：暂存与撤销命令（P1 一致性修复）

审计报告指出的跨命令一致性问题集中在这些命令。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **17** | `stash` | 有 -m，有子命令 | JSON 输出（stash list）；保存确认和 stash 编号 |
| **18** | `restore` | 无确认/无 JSON | 确认消息；退出码对齐 exit 1；错误码 |
| **19** | `revert` | 有确认消息，有 -n | 补齐 --no-edit；JSON 输出；错误码 |
| **20** | `cherry-pick` | 与 Git 一致 | JSON 输出；错误码 |

**理由：** 撤销操作的错误反馈尤为重要，用户需要知道操作是否成功。

### 第五批：远程管理（P1 对齐）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **21** | `remote` | 有子命令，无 JSON | JSON 输出；退出码对齐（重复添加 exit 3 或 exit 1） |
| **22** | `fetch` | 与 Git 一致 | JSON 进度事件；错误码 |

### 第六批：辅助命令（P2 增强）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **23** | `reflog` | 子命令结构偏离 Git | 重构为 `libra reflog [-n N]`；JSON 输出 |
| **24** | `describe` | 有 --abbrev/--tags | 补齐 --always；JSON 输出 |
| **25** | `shortlog` | 已有错误码 | 补齐 revision 位置参数；JSON 输出 |
| **26** | `clean` / `checkout` / `rebase` / `merge` | 与 Git 语法一致 | JSON 输出；merge 冲突结构化输出（pull 依赖的 three-way merge 能力在此批次统一实现）。**说明**：`checkout` 的兼容性收口（`SwitchError` 变体匹配适配、`--help` EXAMPLES）已随第二批 `switch` 提前落地（见 [checkout.md](checkout.md)）；本批次负责 `checkout` 的完整现代化（`CheckoutError` typed enum、JSON 输出、执行/渲染拆分） |

### 第七批：工作树操作与文件管理命令（P2 补齐）

已在代码库中实现但未纳入前六批改进计划的用户可见命令。这些命令已具备基本功能，但缺少结构化输出、显式错误码和现代化的执行/渲染分离。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **27** | `mv` | 无 JSON，无 StableErrorCode（492 行） | 执行层/渲染层拆分；JSON 输出（移动/重命名结果）；显式 `StableErrorCode`；成功确认消息 |
| **28** | `rm` | 无 JSON，无 StableErrorCode（376 行） | 执行层/渲染层拆分；JSON 输出（删除文件列表）；显式 `StableErrorCode`；`--dry-run` 结构化输出 |
| **29** | `worktree` | 无 JSON，无 StableErrorCode（745 行） | JSON 输出（worktree list 结构化）；显式 `StableErrorCode`；子命令 add/list/lock/unlock/remove 补齐确认消息 |
| **30** | `open` | 无 JSON，无 StableErrorCode（172 行） | 显式 `StableErrorCode`；无远程 URL 时 hint |

**理由：** `mv`/`rm` 是文件管理基本操作，静默成功违反 CLIG 原则；`worktree` 是多任务并行开发的重要命令；`open` 体量小，可顺带收尾。

### 第八批：底层与特殊用途命令（P2 增强）

底层管道命令和特殊用途命令。这些命令主要面向脚本和内部基础设施，部分已有 JSON 支持。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **31** | `cat-file` | 有 JSON（1,329 行） | 补齐 `StableErrorCode`；统一错误输出格式 |
| **32** | `show-ref` | 有 JSON（193 行） | 补齐 `StableErrorCode` |
| **33** | `index-pack` | 无 JSON，无 StableErrorCode（313 行） | 显式 `StableErrorCode`；JSON 进度/结果输出 |
| **34** | `lfs` | 无 JSON，已有 StableErrorCode（357 行） | 补齐 JSON 输出（batch、lock 子命令） |
| **35** | `cloud` | 无 JSON，无 StableErrorCode（937 行） | JSON 输出（backup/restore 结果）；显式 `StableErrorCode`；进度输出 |

**不纳入改进计划的模块：**
- `web_assets.rs`（11 行）：纯资源嵌入模块，无命令逻辑
- `claude_sdk.rs`（3,576 行）：Claude Agent SDK managed-mode 命令面，属于独立子系统，改进节奏由 SDK 自身演进决定
- `code.rs`（1,153 行）：`libra code` TUI/Web/MCP 入口，已有 StableErrorCode，改进节奏由 AI Agent 子系统自身演进决定

### 全局层面改进（贯穿所有命令）

这些改进不针对单个命令，而是全局性的：

| 顺序 | 改进项 | 优先级 |
|------|--------|--------|
| **A** | 退出码三级模型统一对齐（0/128/129） | 与各命令改进同步进行 |
| **B** | 每个子命令 --help 添加 EXAMPLES 段 | 与各命令改进同步进行 |
| **C** | `NO_COLOR` / `TERM=dumb` / `--no-color` 颜色控制 | 独立改进 |
| **D** | log/diff/blame/show TTY 下使用 pager | 独立改进 |
| **E** | 顶层 help 按场景分组 | 独立改进 |
| **F** | 拼写纠错建议（确认 clap suggest 已启用） | 独立改进 |
| **G** | 意外错误时输出 GitHub Issues URL | 独立改进 |
| **H** | **In-process SSH Client**：使用 Rust SSH 库（`russh`）替换外部 `ssh` 进程调用，实现 SSH 私钥纯内存传递（不落盘），消除临时文件泄漏风险和文件系统依赖。解除 Agent blocker | 后续批次优先 |

---

## 命令改进详细计划进展

- [Config 命令改进详细计划](config.md) ✅ 已落地
- [Init 命令改进详细计划](init.md) ✅ 已落地
- [Clone 命令改进详细计划](clone.md) ✅ 已落地
- [Add 命令改进详细计划](add.md) ✅ 已落地
- [Status 命令改进详细计划](status.md) ✅ 已落地
- [Commit 命令改进详细计划](commit.md) ✅ 已落地
- [Push 命令改进详细计划](push.md) ✅ 已落地
- [Pull 命令改进详细计划](pull.md) ✅ 已落地
- [Switch 命令改进详细计划](switch.md) ✅ 已落地
- [Checkout 命令改进详细计划（第二批兼容收口）](checkout.md) ✅ 已落地（完整现代化留第六批）
- [Reset 命令改进详细计划](reset.md)
- [Tag 命令改进详细计划](tag.md)
- [Branch 命令改进详细计划](branch.md)
- [Log 命令改进详细计划](log.md)
- [Diff 命令改进详细计划](diff.md)
- [Show 命令改进详细计划](show.md)
- [Blame 命令改进详细计划](blame.md)

## 命令改进实施记录

- [Clone 命令改进实施记录](commands/clone.md)

---

## 收尾工作（所有命令改进完成后）

以下工作依赖所有命令批次全部完成，作为改进计划的最终收口：

### Legacy Config 清理

| 清理项 | 说明 | 来源 |
|--------|------|------|
| 删除旧 `config` 表 schema | 从 `sql/sqlite_20260309_init.sql` 中移除 `CREATE TABLE config` 定义 | config.md 特性 1 |
| 删除旧 `Config` API | 移除 `src/internal/config.rs` 中所有标记 `#[deprecated]` 的旧公共 API（`get`/`get_all`/`insert`/`update`/`remove`/`remove_config`/`list_all`/`remote_config`/`all_remote_configs`/`get_remote`/`get_remote_url`/`branch_config` 等） | config.md 验证方式 |
| 删除旧 SeaORM entity | 移除 `src/internal/model/config.rs` 中标记 `#[deprecated]` 的 `Model`/`Entity`/`Column`/`ActiveModel` | config.md 验证方式 |
| 原始 SQL 最终清扫 | `rg -i '(FROM\|INTO\|UPDATE\|DELETE\s+FROM)\s+["\x60]?config["\x60]?\b' src/ --type rust` 确认零结果（deprecated 定义文件一并删除后不再有例外） | config.md 验证方式 |

### AI Provider `from_env()` → `resolve_env()` 改造

config.md 设计原则 #5 明确将 `src/internal/ai/providers/*/client.rs` 的 `from_env()` → `resolve_env()` 改造**留到后续批次**。当前涉及 6 个 provider（gemini、openai、anthropic、deepseek、zhipu、ollama）共 12 个文件。所有命令改进完成后，应统一将 AI provider 的 API key / base URL 读取切换到 `resolve_env()`，使 `vault.env.*` 配置对 AI provider 生效。

### 验收标准

- `cargo clippy --all-targets --all-features -- -D warnings` 通过（旧 API 删除后不再有 deprecated 引用）
- 原始 SQL 检查零结果
- `cargo test --all` 全部通过
- 旧 `config` 表在新建仓库中不再被创建

---

## 每次改进质量验收
1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. **测试覆盖规则**：凡纳入迁移范围的命令、内部模块和转发路径，都必须有对应的集成测试覆盖新 config_kv 读写链路。不维护固定测试列表，以迁移范围清单为准
