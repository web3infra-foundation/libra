# Libra CLI 命令改进顺序计划

## Context

基于两份审计报告（CLI UX 对比研究 + CLIG 六维审计报告），结合当前代码库已实现的基础设施，制定命令级别的改进优先级。

**已完成的基础设施：**
- 全局 `--json`/`--machine`/`--quiet`/`--color`/`--no-pager`/`--progress`/`--exit-code-on-warning` 标志 (`src/cli.rs`)
- 稳定错误码体系 16 个错误码 (`src/utils/error.rs`)
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架 (`src/utils/output.rs`)
- `CommandOutput` trait 支持结构化输出
- 错误码文档 (`docs/error-codes.md`)
- `init` 命令主改造已落地：`run_init()`、顶层 human/JSON/machine 渲染、`InitProgress`、显式 `StableErrorCode`、嵌套 fetch 输出隔离均已就绪

**已有 JSON 输出的命令（面向终端用户的高层命令）：** commit, status, branch, config, init（底层命令如 `cat-file`、`show-ref` 也已支持 JSON，但未纳入本优先级列表；`switch` 仅用 `is_json()` 抑制 human 输出，但 `--json` 模式下不产生结构化 stdout，不计入）
**已用 StableErrorCode 的命令：** commit, init, shortlog, lfs, code（共 5 个）

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
- `clone` 已开始复用 `init` 的纯执行层与 `config` 的解析/认证基础设施，但 clone 命令**整体尚未落地**；其成功 schema、错误码、checkout 失败传播与 cleanup 收尾项继续在 `clone.md` 中维护。

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
| **3** | `clone` | 进行中：已切换到 `run_init()`，但成功 JSON / 显式错误码 / checkout 失败传播尚未落地 | 结构化成功输出；显式 `StableErrorCode`；network/auth hint；checkout 失败传播；cleanup warning |
| **4** | `add` | 与 Git 一致，无 JSON | JSON 输出（变更文件列表）；--dry-run 支持；错误信息包含文件名 |
| **5** | `status` | 有 JSON + porcelain，无 hint | 添加下一步命令建议（"use libra add..."）；补齐 StableErrorCode |
| **6** | `commit` | ✅ 已完成（金标准） | 作为参考模板，无需改动 |
| **7** | `push` | 功能失败/60s 超时/无 JSON | 修复 refspec 语法；10s 超时；进度输出；JSON 输出；错误码 |
| **8** | `pull` | 级联失败/无 JSON | 修复 upstream tracking；JSON 输出；错误码 |

**理由：** config 是基础设施层，vault 加密存储和 `resolve_env()` 被其他命令（push 认证、code AI provider）依赖，必须最先完成。init/clone 是入口命令（审计指出 init 耗时 ~6s 严重违反 CLIG "100ms 内打印内容"原则）；add 是 commit 前的必经步骤；push 是审计中"最严重的三个缺陷"之一。

**第一批内部依赖说明：**

- `config` 已是第一批内部的已落地基线；`init`/`clone` 文档应直接在其上描述现状与剩余收尾项。
- `init` 已落地并成为 `clone` 的直接基线；`clone` 对 `run_init()`、separate-layout 移除、嵌套 fetch 静默规则的引用，应统一写成“当前代码已具备”。
- `clone` 尚未整体落地；README 只把它视为“已接入 init/config 基线、仍在收尾”的命令，不应写成已完成。
- `clone` 的性能优化目标仍保留为后续独立批次，不覆盖 `clone.md` 中“本批不做性能优化”的执行边界。

### 第二批：状态变更确认命令（P0 消灭"沉默"）

审计报告核心发现："成功时沉默、等待时沉默、失败时沉默"。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **8** | `switch` | 有 JSON + 确认消息 | 补齐 StableErrorCode；切换不存在分支时提示 `did you mean -c` |
| **9** | `reset` | 有确认消息，无 JSON | 输出 "HEAD is now at \<SHA\> \<msg\>"；JSON 输出；错误码 |
| **10** | `tag` | 有短标志 -l/-d/-m/-a | 补齐 JSON 输出；重复创建时 hint；退出码对齐 exit 1 |
| **11** | `branch` | 有 JSON | 补齐 StableErrorCode；退出码对齐（删除不存在分支 exit 1） |

**理由：** 这些命令改变仓库状态，必须告知用户发生了什么。

### 第三批：历史查询命令（P1 结构化输出）

使用频率高，AI Agent 场景依赖结构化输出。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **12** | `log` | 明确拒绝 --json | 实现 JSON 输出（结构化提交列表）；保持 --oneline/--graph |
| **13** | `diff` | 无 JSON | JSON 输出（hunk 级别结构化）；--numstat/--name-only |
| **14** | `show` | 有 --oneline/-s | JSON 输出；错误码 |
| **15** | `blame` | 与 Git 一致 | JSON 输出 |

**理由：** Agent 需要从历史/差异中提取结构化信息来决策。log --json 是 MCP 维度最关键的改进。

### 第四批：暂存与撤销命令（P1 一致性修复）

审计报告指出的跨命令一致性问题集中在这些命令。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **16** | `stash` | 有 -m，有子命令 | JSON 输出（stash list）；保存确认和 stash 编号 |
| **17** | `restore` | 无确认/无 JSON | 确认消息；退出码对齐 exit 1；错误码 |
| **18** | `revert` | 有确认消息，有 -n | 补齐 --no-edit；JSON 输出；错误码 |
| **19** | `cherry-pick` | 与 Git 一致 | JSON 输出；错误码 |

**理由：** 撤销操作的错误反馈尤为重要，用户需要知道操作是否成功。

### 第五批：远程管理（P1 对齐）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **20** | `remote` | 有子命令，无 JSON | JSON 输出；退出码对齐（重复添加 exit 3 或 exit 1） |
| **21** | `fetch` | 与 Git 一致 | JSON 进度事件；错误码 |

### 第六批：辅助命令（P2 增强）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **23** | `reflog` | 子命令结构偏离 Git | 重构为 `libra reflog [-n N]`；JSON 输出 |
| **24** | `describe` | 有 --abbrev/--tags | 补齐 --always；JSON 输出 |
| **25** | `shortlog` | 已有错误码 | 补齐 revision 位置参数；JSON 输出 |
| **26** | `clean` / `checkout` / `rebase` / `merge` | 与 Git 语法一致 | JSON 输出；merge 冲突结构化输出 |

### 第七批：全局层面改进（贯穿所有命令）

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

## 命令改进详细计划

- [Config 命令改进详细计划](config.md)
- [Init 命令改进详细计划](init.md)
- [Clone 命令改进详细计划](clone.md)

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
