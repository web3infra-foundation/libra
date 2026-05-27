# Libra CLI 命令改进顺序计划

## Context

基于两份审计报告（CLI UX 对比研究 + CLIG 六维审计报告），结合当前代码库已实现的基础设施，制定命令级别的改进优先级。

**已完成的基础设施：**
- 全局 `--json`/`--machine`/`--quiet`/`--color`/`--no-pager`/`--progress`/`--exit-code-on-warning` 标志 (`src/cli.rs`)
- 稳定错误码体系 22 个错误码 (`src/utils/error.rs`)
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架 (`src/utils/output.rs`)
- `CommandOutput` trait 支持结构化输出
- 错误码文档 (`docs/error-codes.md`)
- `init` 命令主改造已落地：`run_init()`、顶层 human/JSON/machine 渲染、`InitProgress`、显式 `StableErrorCode`、嵌套 fetch 输出隔离均已就绪
- `add` 命令主改造已落地：`run_add()` → `AddOutput` 执行层/渲染层拆分、JSON/machine 输出、显式 `StableErrorCode`、warning 接入共享 tracker
- `status` 命令主改造已落地：`StatusData` 共享数据层、upstream tracking、`--exit-code` 标志、显式 `StableErrorCode`

**已有 JSON 输出的命令（面向终端用户的高层命令）：** commit, status, branch, config, init, clone, add, push, pull, switch, reset, tag，第三批已落地的 `log` / `diff` / `show` / `blame`，第四批已落地的 `stash` / `restore` / `revert` / `cherry-pick`，第五批已落地的 `remote` / `fetch`，以及本轮新增的 `describe` / `shortlog` / `clean` / `open`（底层命令中的 `show-ref` / `index-pack` / `cat-file` 也已具备 JSON 契约）
**主要错误路径已接入 StableErrorCode 的命令：** init, clone, add, status, commit, push, pull, switch, reset, tag, branch, show, log, diff, blame, stash, restore, revert, cherry-pick, remote, fetch, describe, shortlog, clean, open, show-ref, index-pack, lfs, code

---

## 计划可信度与事实复核协议

本文档和 `docs/improvement/**` 中的子计划不能靠“此前 Review 已通过”来获得长期可信度。任何计划在进入实现或继续标记为“已落地”前，必须重新跑一轮事实复核；没有新证据时只能写“待核”或“依赖某批次交付”，不能写成代码事实。

**必须复核的证据：**
- **代码事实**：用 `rg` / `cargo test` / `cargo run -- <cmd> --help` 直接验证。所有“已落地”“已存在”“无冲突”声明必须能指向当前工作树中的文件、测试或命令输出。
- **链接事实**：相对链接必须指向现存文件；已合并或删除的旧计划（例如原 `code.md` / `tui.md`）必须改指向 [agent.md](agent.md) 的对应 Part。
- **占位事实**：可执行脚本、CI 命令、迁移版本、baseline commit、GitHub required-check 名称不得以 placeholder 形式进入可落地步骤。确实需要未来值时，该步骤必须 fail-fast 并把“需要维护者填值”列为阻塞验收项。
- **验收事实**：每个计划必须有可运行的验证命令、涉及文件、风险/非目标边界；涉及安全、迁移、数据写入、外部 API 或兼容 surface 的计划还必须有负向测试和 rollback/cleanup 说明。
- **生产错误处理**：任何实施计划触及 Rust 生产代码时，必须把 `unwrap()` / `expect()` / `panic!()` 扫描纳入验收；除测试或带 `// INVARIANT:` 的显然不可能失败路径外，新增或保留都视为 P1。已完成生产 unwrap 审计的文件由 `tests/compat/*_production_unwrap_guard.rs` 防御性保护：单独 guard 覆盖 `src/internal/protocol/lfs_client.rs`（v0.17.260）、`src/internal/config.rs`（v0.17.261）、`src/internal/head.rs`（v0.17.262）、`src/utils/util.rs` 与 `src/utils/client_storage.rs`（v0.17.264）；合并 guard `compat_extra_production_unwrap_guard`（v0.17.266）覆盖 `src/utils/lfs.rs`、`src/utils/object.rs`、`src/utils/storage/local.rs`、`src/utils/storage/tiered.rs`、`src/utils/path_ext.rs`、`src/git_protocol.rs`、`src/lfs_structs.rs`、`src/command/reflog.rs`；最后 `compat_all_production_unwrap_guard`（v0.17.268）作为兜底守卫遍历整个 `src/` 树（v0.17.268 时是 410 个 `.rs` 文件，截至 v0.17.842 已增至 427 个；guard 仍按发现的所有文件扫描，不依赖固定列表，所以新增模块自动纳入扫描），任何新增 bare `.unwrap()` 都会被它捕获。新增生产 `.unwrap()` 会让对应 guard 测试失败，且失败消息会指向 `.expect("INVARIANT: ...")` 或 typed-error sibling 作为修复路径。

**循环退出条件：**

只有当本轮复核没有发现新的 P0/P1 漏洞、所有 P2/P3 都被明确登记为非阻塞 follow-up、且上面的证据全部可复现时，才能把计划称为“本轮可执行”。不要在文档中写“100% 完成/无风险/无未决”这类不可证伪表述；用“本轮事实复核通过”替代。

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
- `clone` 已整体落地，复用 `init` 的纯执行层与 `config` 的解析/认证基础设施。成功 schema（`CloneOutput`）、显式错误码、typed checkout 失败传播与 cleanup warning 可见性均已完成；详见 [docs/commands/clone.md](../commands/clone.md) 实施记录。性能优化留后续批次。

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
| **3** | `clone` | ✅ 已落地 | 结构化成功输出（`--json`/`--machine`）；显式 `StableErrorCode`；network/auth hint；checkout 失败传播；cleanup warning 可见性；阶段性 human 进度（详见 [docs/commands/clone.md](../commands/clone.md)） |
| **4** | `add` | ✅ 已落地 | 执行层/渲染层拆分（`run_add()` → `AddOutput`）；JSON/machine 结构化输出；成功摘要；`--dry-run`/`--verbose` 输出经 `OutputConfig` 管控；显式 `StableErrorCode`；ignored/failed 按 warning 处理并接入 `--exit-code-on-warning`（详见 [add.md](add.md)） |
| **5** | `status` | ✅ 已落地 | `StatusData` 共享数据层消除重复计算；upstream tracking（human/JSON/porcelain v2）；显式 `StableErrorCode`；新增 `--exit-code` dirty → exit `1`；颜色控制统一到 `OutputConfig`（详见 [status.md](status.md)） |
| **6** | `commit` | ✅ 已落地 | `CommitError`（19 变体）typed enum + 显式 `StableErrorCode`；`run_commit()` + `render_commit_output()` 执行/渲染拆分；JSON 向后兼容扩展（+`branch`/`amend`/`signoff`/`conventional`/`signed`）；hook I/O 隔离；`--help` EXAMPLES（详见 [commit.md](commit.md)） |
| **7** | `push` | ✅ 已落地 | 修复 refspec 语法；60s 连接/空闲超时；human 进度输出；JSON 输出；错误码。**前置依赖**：需在 `protocol/` 建立可替换 transport seam 供超时/auth/protocol 测试 |
| **8** | `pull` | ✅ 已落地 | 聚合 fetch + fast-forward/up-to-date 结果；修复 upstream tracking；JSON 输出；错误码（non-fast-forward merge 留 merge 批次）。**前置依赖**：需在 `fetch.rs`/`merge.rs` 建立 pull 可复用的最小 typed helper（完整 JSON/进度改造留第五/六批） |

**理由：** config 是基础设施层，vault 加密存储和 `resolve_env()` 被其他命令（push 认证、code AI provider）依赖，必须最先完成。init/clone 是入口命令（审计指出 init 耗时 ~6s 严重违反 CLIG "100ms 内打印内容"原则）；add 是 commit 前的必经步骤；push 是审计中"最严重的三个缺陷"之一。

**第一批内部依赖说明：**

- `config` 已是第一批内部的已落地基线；`init`/`clone` 文档应直接在其上描述现状与剩余收尾项。
- `init` 已落地并成为 `clone` 的直接基线；`clone` 对 `run_init()`、separate-layout 移除、嵌套 fetch 静默规则的引用，应统一写成“当前代码已具备”。
- `clone` 已整体落地：执行层/渲染层拆分、`CloneOutput` 结构化输出、显式 `StableErrorCode` 映射、typed `RestoreError` checkout、`RemoteSpecErrorKind` 分类、cleanup warning 可见性均已完成。性能优化目标仍保留为后续独立批次。
- `add` 已整体落地：`run_add()` → `AddOutput` 执行层/渲染层拆分；JSON/machine 结构化输出（含 `added`/`modified`/`removed`/`refreshed`/`ignored`/`failed` 分类）；显式 `StableErrorCode` 映射（`AddError → CliError`）；`--dry-run`/`--verbose` 输出经 `OutputConfig` 管控；ignored/failed 接入共享 warning tracker。
- `status` 已整体落地：`StatusData` 共享数据层消除 `execute_to()` 与 `collect_status_json()` 的逻辑重复；upstream tracking（human/JSON/short/porcelain v2）；显式 `StatusError → CliError` 映射；新增 `--exit-code` 标志（dirty → exit `1`）；颜色控制统一到 `OutputConfig`。
- `commit` 已整体落地：`CommitError`（19 变体）typed enum + 显式 `StableErrorCode` 映射；`run_commit()` + `render_commit_output()` 执行/渲染拆分；JSON 向后兼容扩展（+`branch`/`amend`/`signoff`/`conventional`/`signed`）；hook I/O 隔离（JSON 模式 `Stdio::piped()`）；全部 19 变体单元映射测试 + 11 CLI 级集成测试 + 9 JSON schema 稳定性测试。

### 第二批：状态变更确认命令（P0 消灭"沉默"）

审计报告核心发现："成功时沉默、等待时沉默、失败时沉默"。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **9** | `switch` | ✅ 已落地 | 第二批主改造已落地；后续仅维护回归测试、文档同步与大仓库切换性能观察（详见 [switch.md](switch.md)） |
| **9a** | `checkout`（兼容收口） | ✅ 第二批兼容收口已落地；第 30 批补充结构化输出和 typed error 收口 | 已完成 `SwitchError` 变体匹配适配、`--help` EXAMPLES、`CheckoutOutput`、JSON/machine 成功路径、checkout-owned stable code，以及完整 `CheckoutError` typed enum（含 branch-store 与 remote-sync 细分映射）；后续仅维护回归测试和文档同步（详见 [checkout.md](checkout.md)） |
| **10** | `reset` | ✅ 主改造已落地：已有确认消息、JSON/machine、显式 `StableErrorCode`、`ResetError`、warning 管线、`run_reset()` / `render_reset_output()` | 后续仅维护 rollback / warning / pathspec corruption 边界回归与文档示例（详见 [reset.md](reset.md)） |
| **11** | `tag` | ✅ 主改造已落地：已有 JSON/machine、显式 `StableErrorCode`、`TagError`、run/render 分层、重复创建 hint 与统一 human 确认消息 | 后续仅维护 lightweight tag 的 human / machine 双契约、边界回归与文档同步（详见 [tag.md](tag.md)） |
| **12** | `branch` | ✅ 已落地：JSON 已覆盖 list/create/delete/rename/set-upstream/show-current，`BranchError` typed enum、run/render 分层、确认消息、fuzzy suggestion 与 `--help` EXAMPLES 已就绪；v0.17.211 → v0.17.217 已把 `tests/command/*` 全部迁到 `*_result` 并删除 8 个 lossy wrapper（详见 [branch.md](branch.md)） | 后续仅维护回归测试和文档同步 |

**理由：** 这些命令改变仓库状态，必须告知用户发生了什么。`checkout` 的兼容收口随 `switch` 一起落地，因为 `switch` 的 `ensure_clean_status()` 签名变更强制要求 `checkout` 同步适配。

### 第三批：历史查询命令（P1 结构化输出）

使用频率高，AI Agent 场景依赖结构化输出。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **13** | `log` | ✅ 第三批用户契约已落地：JSON / machine、稳定错误码、`run_log()`、`--help` EXAMPLES、decorate refs best-effort、历史 blob 损坏显式失败 | 后续仅维护回归测试和文档同步；完整 `LogError` / human render split 归入后续跨命令 error/render 收口，不再阻塞第三批验收 |
| **14** | `diff` | ✅ 主改造已落地：`DiffError`、`run_diff()` / render split、JSON / machine、`--name-only` / `--name-status` / `--numstat` / `--stat`、`--quiet` exit code、`--help` EXAMPLES | 后续仅维护大 diff 性能回归和 pager / TTY 细节 |
| **15** | `show` | ✅ 第三批用户契约已落地：JSON / machine、稳定错误码、`run_show()`、`--quiet` 契约、refs best-effort、历史 blob 损坏显式失败 | 后续仅维护回归测试和文档同步；完整 `ShowError` / human render split 归入后续跨命令 error/render 收口，不再阻塞第三批验收 |
| **16** | `blame` | ✅ 主改造已落地：`BlameError`、`run_blame()`、JSON / machine、`-L` 结构化输出、`--help` EXAMPLES | 后续仅维护 blame 归属正确性、范围过滤和边界回归 |

**理由：** Agent 需要从历史/差异中提取结构化信息来决策。log --json 是 MCP 维度最关键的改进。

**第三批基于 Review 的计划修订：**

- `diff` 负责 hunk / patch 级结构化输出；`log` / `show` 的 JSON 只保留提交元数据和文件变更摘要，避免 schema 重叠、重复计算和用户认知冲突。
- `log` / `show` 中的 refs / decoration 元数据属于辅助信息，按用户习惯改为 best-effort；commit / tree / blob 主体对象读取保持 strict，历史对象损坏必须显式失败，禁止回退到工作区内容。
- `--quiet` 对历史查询命令统一解释为“只抑制 human stdout，不跳过校验和退出语义”；`diff --quiet` 仍以 exit `1` 表示存在差异，即使同时写入 `--output` 文件。
- 第三批验收以“对外契约完整、测试覆盖到 review 回归、命令文档与实现一致”为准；`LogError` / `ShowError` 这类内部统一重构保留为后续跨命令 error/render 收口项，不再与第三批用户契约绑定。

### 第四批：暂存与撤销命令（P1 一致性修复）

审计报告指出的跨命令一致性问题集中在这些命令。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **17** | `stash` | ✅ 已落地 | `StashError` typed enum、`run_stash()` / render split、JSON / machine、`--help` EXAMPLES、显式 `StableErrorCode`（详见 [stash.md](stash.md)） |
| **18** | `restore` | ✅ 已落地 | `RestoreError` → `StableErrorCode` 映射、`run_restore()` / render split、JSON / machine、确认消息、`--help` EXAMPLES（详见 [restore.md](restore.md)） |
| **19** | `revert` | ✅ 已落地 | `RevertError` typed enum、`run_revert()` / render split、JSON / machine、`--help` EXAMPLES、显式 `StableErrorCode`（详见 [revert.md](revert.md)） |
| **20** | `cherry-pick` | ✅ 已落地 | `CherryPickError` typed enum、`run_cherry_pick()` / render split、JSON / machine、`--help` EXAMPLES、显式 `StableErrorCode`（详见 [cherry-pick.md](cherry-pick.md)） |

**理由：** 撤销操作的错误反馈尤为重要，用户需要知道操作是否成功。

### 第五批：远程管理（P1 对齐）

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **21** | `remote` | ✅ 已落地 | `run_remote()` / render split；JSON / machine；显式 `StableErrorCode`；`remote -v` 多 URL 展示修复；prune 结构化输出（详见 [remote.md](remote.md)） |
| **22** | `fetch` | ✅ 已落地 | `FetchOutput` 顶层结果；JSON / machine；显式 `StableErrorCode`；JSON progress 事件；human 摘要输出（详见 [fetch.md](fetch.md)） |

**第五批基于 Review 的计划修订：**

- `remote` 沿用当前 CLI 形态，只收口输出、错误码和 `remote -v` 多 URL 行为；`remote show` 的 Git 全量兼容语义单列为后续独立收口项，避免本批引入 breaking CLI 变更。
- `fetch` 保持底层传输和 refs 更新 helper 不变，只在顶层命令补结构化结果、显式错误码和进度契约，避免影响 `pull` / `clone` / `convert` 对 `fetch_repository_with_result()` 的复用。
- 本轮 Review 同步修正了第四批子计划文档的陈旧状态：`stash` / `restore` / `revert` / `cherry-pick` 改为记录“已落地基线 + 后续维护点”，不再与当前代码状态冲突。

### 第一到第五批 Review 总结

- 已落地命令的核心对外契约整体可用，没有发现需要回滚的跨命令冲突。
- 主要问题集中在文档状态陈旧、个别兼容层（如 `checkout`）被误写成“完整现代化已完成”，以及部分已存在 JSON 能力的底层命令缺少命令文档与契约回归测试。
- 本轮同步修正了这些计划/文档漂移问题，并把后续批次按风险重新拆分，避免继续把低风险只读命令和高风险状态机命令混在一个批次推进。

### 第六批：只读辅助命令收口（P2） ✅ 已落地

本轮 Review 发现，原第六批把低风险只读命令和高风险状态变更命令混在一起，批次过大且验收维度不一致。第六批改为先收口用户最常见、实现风险最低、最容易形成稳定 JSON 契约的只读辅助命令。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **23** | `describe` | ✅ 已落地 | 补齐 `--always`；支持 commit-ish；JSON / machine；显式错误码 |
| **24** | `shortlog` | ✅ 已落地 | 补齐 revision 位置参数；JSON / machine；日期 / revision 错误显式化 |
| **25** | `clean` | ✅ 已落地 | `-n` / `-f` 共用结构化结果；JSON / machine；成功确认消息；显式错误码 |

### 第七批：轻量交互与底层桥接命令（P2） ✅ 已落地

第七批聚焦体量小、依赖面窄但经常被用户和脚本直接调用的命令，优先解决“成功后不可确认”“脚本无法稳定消费”的问题。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **26** | `open` | ✅ 已落地 | 补齐 `remote` / `remote_url` / `web_url` 结果模型；JSON / machine；显式错误码与 hint |
| **27** | `show-ref` | ✅ 已落地（本轮补契约） | 命令文档补齐；JSON 回归测试补齐；README 状态同步 |
| **28** | `index-pack` | ✅ 已落地 | JSON / machine；显式错误码；输出收口为稳定结果模型 |

### 第八批：底层命令契约收口（P2） ✅ 已落地

第八批不再盲目扩张到所有底层和云端命令，而是先把已经“半现代化”的底层命令收口为清晰、可测试、可文档化的对外契约。

| 顺序 | 命令 | 当前状态 | 改进重点 |
|------|------|--------|--------|
| **29** | `cat-file` | ✅ 已完成契约收口 | 保持现有 JSON 实现不变，补命令文档与 JSON 回归测试，明确 `-e` 仍不支持 JSON |

### 后续批次（基于本轮 Review 重排）

原第六到第八批中剩余命令仍然需要改进，但被 Review 证明不适合继续和低风险命令混批推进，故后移：

| 顺序 | 命令 | 当前状态 | 后续重点 |
|------|------|--------|--------|
| **30** | `reflog` / `checkout` | ✅ 已落地（checkout 完整收口于 v0.17.372）：`reflog show/delete/exists` 已有 JSON/machine 成功路径、裸分支解析与显式错误码；`checkout` 已有 `CheckoutOutput`、JSON/machine 成功路径、render split、checkout-owned stable code，并补齐完整 `CheckoutError` typed enum（含 `SwitchingToBranchBlocked` / `BranchNotFound` / `PathSpecNotMatched` / `RemoteHeadMissing` / `RemoteSyncFailed { stage, source }` 五个新变体）与 `get_remote()` 内 `set_upstream` / `pull` 代理调用的 `RemoteSyncFailed` 细分层 | 后续仅维护回归测试和文档同步 |
| **31** | `mv` / `rm` / `worktree` | ✅ 已落地：`mv` / `rm` / worktree 成功路径、非 FUSE worktree 错误路径、FUSE `umount` 成功路径均已有 JSON/machine；后续仅保留 FUSE mount 管理的更细错误码扩展 | destructive 路径的结果模型、显式错误码、确认消息 |
| **32** | `merge` / `rebase` | ✅ 已落地：[`merge`](merge.md) 已有 `run_merge()` / `render_merge_output()`、fast-forward / already-up-to-date / remote ref JSON/machine、non-fast-forward JSON error envelope 和显式错误码；[`rebase`](rebase.md) 已同步当前 human 输出文档、移除路径处理生产 `unwrap()`，显式化 no-state JSON 错误码，并落地 start / `--abort` / `--continue` / `--skip` 成功 JSON/machine、human start 共享 runner、unresolved-conflict typed error；v0.17.190 引入 `ReplayErrorKind`（14 变体，4 个独立稳定错误码），把 replay 内部失败与真实合并冲突分离；`execute()` 已回收 legacy 内部路径并与 `execute_safe()` 共享同一 runner | 后续仅维护回归测试与兼容 additive 扩展 |
| **33** | `lfs` / `cloud` | 部分落地：[`lfs`](lfs.md) 已有 `run_lfs()` / `render_lfs_output()`、成功 JSON/machine、本地路径无 panic 输出收口，`get_locks()` typed network/protocol error 映射，client + CLI 两层 mock 测试（v0.17.190 / v0.17.191）覆盖 locks/lock/unlock 成功 + 403/409 错误码，`ls-files --json` 增加 `full_oid` 字段（v0.17.207），batch protocol 合约回归覆盖 `push_object` 错误对象数（v0.17.232）/ `download_object` 缺 `download` action（v0.17.233）/ `upload_object` 缺 `upload` action（v0.17.234），均返回 typed 错误并携带 oid，v0.17.237 / v0.17.254-258 把 `lfs_client.rs` 全部生产 `.unwrap()` 迁移到 INVARIANT-documented `.expect("...")`（`from_url` trait 实现、`lock`/`unlock`/`verify_locks`/`get_locks`/`upload_object` retry loop 与 URL join 均覆盖，panic 消息指明被破坏契约）；[`cloud`](cloud.md) 已有 `cloud status` JSON/machine、本地查询错误码，`cloud sync --json/--machine/--quiet` silent runner + 成功 schema + `--progress=json` 事件流，`cloud restore --json/--machine/--quiet` 成功 schema，v0.17.208 引入 `CloudError` typed enum 集中 String→StableErrorCode 分类，底层 helper 已全部切到 `CloudResult<T>` 返回（[`src/command/cloud.rs`](cloud.md) `pub(crate) enum CloudError` 与 `type CloudResult<T>`），`cloud status` / `cloud sync` / `cloud restore` 执行路径通过 `cloud_cli_error_typed` 收口稳定错误码 | 后续仅维护回归测试与 schema additive 扩展 |

**不纳入命令级批次改进的模块：**
- `web_assets.rs`（11 行）：纯资源嵌入模块，无命令逻辑
- `code.rs` 及 AI 子系统（`src/internal/ai/`、`src/internal/tui/`）**不在上面的命令批次表中推进**，但已由 [agent.md](agent.md) 作为 AI Agent 子系统专项计划统一跟踪；该文档现在是 Agent runtime / `libra code` Phase Workflow & Implementation Phase 0-5 / Local TUI Automation Control 的唯一权威来源（2026-05-02 已合并原 `code.md` 与 `tui.md` 的全部内容；`claudecode/` 已在 Wave 1C 完成硬删除）

### 全局层面改进（贯穿所有命令）

这些改进不针对单个命令，而是全局性的：

| 顺序 | 改进项 | 优先级 |
|------|--------|--------|
| **A** | 退出码三级模型统一对齐（0/128/129） | 与各命令改进同步进行 |
| **B** | 每个子命令 --help 添加 EXAMPLES 段 | ✅ 已落地（v0.17.812..v0.17.836 完成全量 rollout；v0.17.877..v0.17.882 完成 invocation-shape 审计修复）：`init` / `add` / `clone` / `status` / `commit` / `branch` / `tag` / `switch` / `checkout` / `reset` / `restore` / `revert` / `cherry-pick` / `stash` / `bisect` / `clean` / `describe` / `shortlog` / `pull` / `fetch` / `push` / `merge` / `rebase` / `reflog` / `log` / `show` / `diff` / `blame` / `show-ref` / `cat-file` / `index-pack` / `hash-object` / `verify-pack` / `ls-remote` / `rev-parse` / `rev-list` / `symbolic-ref` / `remote` / `worktree` / `mv` / `rm` / `grep` / `open` / `cloud` / `lfs` / `usage` / `publish` / `sandbox` / `db` / `automation` / `code` / `code-control` / `graph` / `hooks` / `agent` 均渲染 EXAMPLES；`fsck` / `config` 使用 `after_help` 输出 EXAMPLES 等价段。回归由 `tests/command/*_test.rs` 中各自 `test_<cmd>_help_lists_examples_banner` 与 `*_help_test.rs` 全量覆盖，外加 `tests/compat/help_examples_banner.rs`（v0.17.841）做 cross-cutting 守卫。**后审计已知问题（v0.17.877..v0.17.882 修复）**：5 处 invocation 在原 rollout 中是无效的 — `libra code --control automation`（应为 `write`，v0.17.877）、`libra agent rpc <name> <method>`（应为 `rpc list` / `rpc invoke <slug> <method>`，v0.17.879）、`libra bisect start <bad> <good>`（应为 `start <bad> --good <good>`，v0.17.880）、`libra publish unpublish --yes`（应为 `unpublish --site-id <uuid> --yes`，v0.17.881）、`MergeNonFastForward` ghost 错误码引用（应为 `ConflictOperationBlocked` / `LBR-CONFLICT-002`，v0.17.882）；现已全部修复并由对应 `test_<cmd>_help_lists_examples_banner` 测试 pin 住正确形态，外加 `test_code_control_value_enum_rejects_invalid_and_accepts_write`（v0.17.878）binary-spawned guard。**Round-2 layout convergence（v0.17.900..v0.17.921）**：补齐 EXAMPLES per-line inline descriptions 与跨命令 canonical layout，并新增 `tests/compat/help_flag_descriptions.rs`（v0.17.900）+ `tests/compat/help_no_impl_meta_leak.rs`（v0.17.894，扩展到 6 个 forbidden phrase）作为继续守护 — 覆盖 110 个 help surface（42 个 root + 26+ 个 subcommand），任何后续 EXAMPLES 出现 markdown leak / 大写 `EXAMPLES:` 缺失 / 字段名占位符 / 描述行缺失都会被 CI 捕获 |
| **C** | `NO_COLOR` / `TERM=dumb` / `--no-color` 颜色控制 | ✅ 已落地：`--no-color` 等价 `--color=never`，`TERM=dumb` 在 auto 模式禁色，显式 `--color=always` 可覆盖环境禁色 |
| **D** | log/diff/blame/show TTY 下使用 pager | ✅ 已落地：`log` / `diff` / `blame` / `show` human 输出均走共享 `Pager::with_config()`；`--json` / `--machine` / `--quiet` / `--no-pager` 保持不初始化 pager |
| **E** | 顶层 help 按场景分组 | ✅ 已落地：根 `libra --help` 按 Repository Setup / Working Tree / History Inspection / Commit And Branching / Remote And Cloud / AI And Automation / Maintenance And Plumbing 分组 |
| **F** | 拼写纠错建议（确认 clap suggest 已启用） | ✅ 已落地：clap fuzzy suggestion 已启用，并由 `cli::tests::clap_fuzzy_suggests_similar_command` 覆盖 `initt -> init` |
| **G** | 意外错误时输出 GitHub Issues URL | ✅ 已落地：reportable internal errors 的 human / JSON 渲染统一附带 GitHub Issues URL；CLI thread spawn / panic 旁路也输出同一报告 hint |
| **H** | **In-process SSH Client**：使用 Rust SSH 库（`russh`）替换外部 `ssh` 进程调用，实现 SSH 私钥纯内存传递（不落盘），消除临时文件泄漏风险和文件系统依赖。解除 Agent blocker | 后续批次优先 |
| **I** | **Git surface 兼容性补齐** → 见 [compatibility/README.md](compatibility/README.md)：4-tier `COMPATIBILITY.md` / 仓库治理 / CI 兼容矩阵 / stash・bisect 子命令面 / worktree 与 checkout 行为差异 | 与各命令批次并行 |

### 跨命令契约约定（所有命令文档共用）

为避免命令子计划之间出现命名漂移、字段冲突和职责重叠，下面是被所有 ✅ 已落地命令隐式遵守、并要求所有后续新命令显式遵守的跨命令约定。任何命令子计划都不应在自己的"设计原则"中重复声明这些规则；冲突时以本节为准。

#### 1. 函数命名（执行/渲染层）

- 顶层入口：`pub async fn execute_safe(args, output: &OutputConfig) -> CliResult<()>`
- 纯执行层：`async fn run_<cmd>(args) -> Result<<Cmd>Output, <Cmd>Error>`（命名前缀必须是 `run_`，不接受 `execute_impl_` / `do_` / `internal_` 等变体）
- 渲染层：`fn render_<cmd>_output(result: &<Cmd>Output, output: &OutputConfig) -> CliResult<()>`（命名后缀必须是 `_output`）
- 委托型命令（如 `pull` 调 `fetch` + `merge`）：内部 helper 命名为 `run_<cmd>_for_<delegator>()`，例如 `run_fetch_for_pull()`，明确"被谁复用"。
- 已落地基线：`init` / `add` / `status` / `commit` / `branch` / `tag` / `reset` / `restore` / `revert` / `cherry_pick` / `stash` / `remote` / `clean` / `describe` / `shortlog` 都遵守该命名。
- 不要求改名的历史例外：暂无；任何不一致都按本节修正。

#### 2. typed error enum 字段风格

- 单字符串场景（来自外部 io / parse / 字符串透传）使用 **元组变体**：`InvalidObjectName(String)`
- 多字段或带 `detail` 的场景使用 **结构体变体**：`InvalidRevision { revision: String, detail: String }`
- 当一个变体只承载一个有语义的字段（不是错误透传）但未来可能扩展时，建议使用结构体变体而非元组变体，例如 `BadRevision { revision: String }` 而非 `BadRevision(String)`，便于未来加 `detail` / `hint` 字段
- 跨命令对外契约（`StableErrorCode`、退出码）必须一致；内部变体形状只是实现细节，不强制完全统一

#### 3. JSON schema 演化规则

- **向后兼容是绝对约束**：已发布字段名、类型和语义不可变；新增字段必须是 additive
- 字段命名必须 `snake_case`；嵌套对象使用平铺式（不引入 envelope 包装层），example：直接 `data.head` 不要 `data.commit.head`
- 跨命令重复出现的字段必须使用同一名字（详见 §5）
- 底层命令（`show-ref` / `index-pack` / `cat-file`）扩展时也遵守同样规则；不允许各自演化出不一致的字段命名
- 如需 breaking change：新增独立字段 + 标记旧字段 deprecated，至少跨一个 release 后才能删旧字段

#### 4. JSON schema 的所有权与重叠

- 同一概念的 schema **只能由一个命令拥有定义权**；其他命令引用而非重复定义。
- 当前 schema 所有权：
  - **commit 元数据**（hash / author / committer / subject / body / parents / refs / files）：由 `log.md` 拥有；`show.md` 复用并允许追加 type-specific 字段（如 tag / tree / blob 子类）
  - **diff hunk / patch**：由 `diff.md` 拥有；`log` / `show` 不重复 hunk 级输出，只输出文件变更摘要
  - **fetch result**：由 `fetch.md` 拥有；`pull` / `clone` 通过内部 helper 复用
  - **restore result**：由 `restore.md` 拥有；`checkout` 兼容路径复用 `RestoreError` typed API
- 同一字段在多个命令的 JSON 中出现时（如 `branch` / `commit` / `head`），其类型与含义必须保持一致

#### 5. 跨命令字段命名（含 URL 字段）

- **URL 字段**：远端 fetch/push URL 用 `remote_url`；浏览器 deep link 用 `web_url`；不要使用 `website_url` / `forge_url` / `homepage_url` 等同义词
- **commit 引用**：完整 hash 用 `commit`；7 字符短形式用 `short_id`（`log` / `commit` / `cherry-pick` 已用）或 `short_<role>`（`revert` 用 `short_reverted` / `short_new`，因为同一 envelope 含两个 commit）
- **分支字段**：当前分支名用 `branch`；HEAD 标签（branch name 或 "detached"）用 `head`
- **文件变更**：列表字段用 `files`（条目含 `path` + `status`）；统计字段用 `files_changed`（含 `total` / `new` / `modified` / `deleted`）
- **路径字段**：相对仓库根目录的路径用 `path`；绝对路径仅在 init / clone 等创建仓库的命令使用
- 任何新命令引入新字段前，先在本表查找是否已有同义字段；必须复用而非新造

#### 6. `DelegatedCli` passthrough 兼容债

`switch` / `branch` 等命令在过渡期通过 `<Cmd>Error::DelegatedCli(#[from] CliError)` 变体透传未 typed 化的下游 helper（如 `branch::create_branch_safe()` / `restore::execute_safe()`）。这是**有意识的技术债**，不是最终态。

- **偿还路径**：随 `branch` / `restore` / `checkout` 各自的 typed sub-error 改造逐步消除；当被委托命令的 helper 全部返回 typed error 时，`DelegatedCli` 变体即可删除
- **偿还时间表**：本债务不阻断任何当前命令验收；偿还跟随 README 第 170-172 行后续批次（reflog / mv / rm / worktree / merge / rebase）演进，**不晚于** merge / rebase 批次完成
- **新代码约束**：在债务清偿前，新加的命令一律不允许引入 `DelegatedCli` 模式——只有 `switch` / `branch` 这种已存在的兼容点可继续保留

#### 7. AI 子系统单文档结构（agent.md）

**2026-05-02 合并**：原 `code.md` 与 `tui.md` 已合入 [agent.md](agent.md)，分别作为 Part B（`libra code` 实现规格）和 Part C（Local TUI Automation Control）。AI 子系统（Agent runtime / `libra code` / TUI automation）从此只有 agent.md 一份权威计划，避免跨文档协调成本。

agent.md 内部分工：

- **Part A（Step 1.x / Step 2.x）**：Agent 子系统两步演进——单 Agent 基线补齐 + 三层 sub-agent 架构。Part A 的 `--resume` 章节聚焦 JSONL session 字节层（header + tail N 快速恢复 / append-only 崩溃恢复）
- **Part B（Implementation Phase 0-5 + Wave 1A/1B/1C）**：`libra code` 的 Phase Workflow 状态机（Phase 0 Intent / Phase 1 Plan / Phase 2 Execution / Phase 3 Validation / Phase 4 Decision）、`Runtime` formal write 层、Snapshot / Event / Projection 对象模型、provider TaskExecutor、`--resume <thread_id>` 的 phase-aware 恢复合同。**Wave 1C claudecode 硬删除已完成（2026-05-02 baseline 验证：`src/internal/ai/claudecode/` 不存在，CLI 仅保留迁移提示）**
- **Part C（Phase 0-6）**：Local TUI Automation Control——`--control` CLI / token + lease 鉴权 / HTTP control endpoints / redaction / audit。Part C 与 Part B 的 Implementation Phase 3 协同：Part C 的 `Automation` lease 是 Phase 3 真相源统一的子集
- **`from_env() → resolve_env()` 改造**：归属 agent.md Part B Implementation Phase 5（Security / Permission / Diagnostics / Testing Hardening）。`libra code` TUI 与 Ollama headless web-only bootstrap 已切到 `ProviderFactory` + `--env-file` / process-env 解析；provider crate 的 legacy `from_env()` 构造器已保持兼容并接入 vault-aware sync lookup；v0.17.1048 已在 `src/internal/ai/providers/mod.rs` 与 `CHANGELOG.md` 明确公开策略：0.17 保留兼容 helper，新运行时调用继续使用 `from_resolved_env(LocalIdentityTarget)`，v0.18 release notes 宣告 deprecated/migration path。
- agent.md 内部 Part A / Part B / Part C 的冲突，以 Part A Changelog 中最近一次明确修订为准

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
- [Checkout 命令改进详细计划（第二批兼容收口）](checkout.md) ✅ 已落地（完整现代化留第 30 批）
- [Reset 命令改进详细计划](reset.md) ✅ 已落地
- [Tag 命令改进详细计划](tag.md) ✅ 已落地
- [Branch 命令改进详细计划](branch.md) ✅ 已落地（v0.17.217 已删除 8 个 lossy wrapper，所有 `tests/command/*` 已迁到 `*_result` API）
- [Log 命令改进详细计划](log.md) ✅ 已落地（内部统一重构留后续全局收口）
- [Diff 命令改进详细计划](diff.md) ✅ 已落地
- [Show 命令改进详细计划](show.md) ✅ 已落地（内部统一重构留后续全局收口）
- [Blame 命令改进详细计划](blame.md) ✅ 已落地
- [Stash 命令改进详细计划](stash.md) ✅ 已落地
- [Restore 命令改进详细计划](restore.md) ✅ 已落地
- [Revert 命令改进详细计划](revert.md) ✅ 已落地
- [Cherry-Pick 命令改进详细计划](cherry-pick.md) ✅ 已落地
- [Remote 命令改进详细计划](remote.md) ✅ 已落地
- [Fetch 命令改进详细计划](fetch.md) ✅ 已落地
- [Describe 命令改进详细计划](describe.md) ✅ 已落地
- [Shortlog 命令改进详细计划](shortlog.md) ✅ 已落地
- [Clean 命令改进详细计划](clean.md) ✅ 已落地
- [Open 命令改进详细计划](open.md) ✅ 已落地
- [Show-Ref 命令改进详细计划](show-ref.md) ✅ 已落地
- [Index-Pack 命令改进详细计划](index-pack.md) ✅ 已落地
- [Cat-File 命令改进详细计划](cat-file.md) ✅ 已落地

## AI Agent 子系统专项计划

- [Agent 子系统改进详细计划（Agent + libra code + TUI Automation 三合一）](agent.md) — 进行中；2026-05-02 合并自原 `code.md` + `tui.md` + `agent.md`，覆盖：
  - **Part A**：Step 1.0 - 1.11（单 Agent 基线补齐）+ Step 2.1 - 2.8（三层 sub-agent 架构）
  - **Part B**：`libra code` Phase Workflow（Phase 0-4）+ Implementation Phase 0-5（含 Wave 1A/1B/1C，**claudecode 已硬删除**）+ Snapshot / Event / Projection 对象模型 + dagrs 0.8.1（**已升级**）
  - **Part C**：Local TUI Automation Control（Phase 0-6，Phase 1 已落地：`ControlMode` / `--control` / `code_control_files.rs` / `TuiControlCommand` / TuiCodeUiAdapter）

## 命令改进实施记录

- [Clone 命令改进实施记录](../commands/clone.md)

---

## 收尾工作（命令批次完成后的遗留清理）

以下工作依赖所有命令批次全部完成，作为改进计划的最终收口：

### Legacy Config 清理

| 清理项 | 说明 | 来源 |
|--------|------|------|
| 删除旧 `config` 表 schema | 从 `sql/sqlite_20260309_init.sql` 中移除 `CREATE TABLE config` 定义 | config.md 特性 1 |
| 删除旧 `Config` API | 移除 `src/internal/config.rs` 中所有标记 `#[deprecated]` 的旧公共 API（`get`/`get_all`/`insert`/`update`/`remove`/`remove_config`/`list_all`/`remote_config`/`all_remote_configs`/`get_remote`/`get_remote_url`/`branch_config` 等） | config.md 验证方式 |
| 删除旧 SeaORM entity | 移除 `src/internal/model/config.rs` 中标记 `#[deprecated]` 的 `Model`/`Entity`/`Column`/`ActiveModel` | config.md 验证方式 |
| 原始 SQL 最终清扫 | `rg -i '(FROM\|INTO\|UPDATE\|DELETE\s+FROM)\s+["\x60]?config["\x60]?\b' src/ --type rust` 确认零结果（deprecated 定义文件一并删除后不再有例外） | config.md 验证方式 |

### 验收标准

- `cargo clippy --all-targets --all-features -- -D warnings` 通过（旧 API 删除后不再有 deprecated 引用）
- 原始 SQL 检查零结果
- `cargo test --all` 全部通过
- 旧 `config` 表在新建仓库中不再被创建

## 跨子系统后续事项

### AI Provider `from_env()` → `resolve_env()` 改造

`config.md` 设计原则 #5 只说明这项改造**不属于 config 批次本身**；该 follow-up 现已并入 [agent.md](agent.md) Part B（原 `code.md`）的 Implementation Phase 5（Security / Permission / Diagnostics / Testing Hardening），不再作为"命令批次全部结束后再处理"的独立尾项。

**2026-05-12 复核；2026-05-24 更新；2026-05-26 收口**：`libra code` TUI 的 direct provider bootstrap 已经通过 `ProviderFactory` 统一解析 API key、base URL 和 `OLLAMA_COMPACT_TOOLS`，并由 `build_helper_missing_api_key_errors_name_canonical_env_vars` / `build_helper_honors_cli_api_base_for_deepseek` 锁定缺 key 与 `--api-base` 行为。`--web-only --provider ollama` headless v0 也复用同一 ProviderFactory bootstrap。provider crate 中保留 `Client::from_env()` 作为程序化构造器或 legacy helper，并通过 sync lookup 读取 process env、repo/global vault fallback；Zhipu 与其他 provider 的 fallback 语义已对齐。v0.17.1048 已把公开策略写入 provider Rustdoc 与 `CHANGELOG.md`：0.17 继续保留 `from_env()` 兼容 surface，v0.18 release notes 标注其 deprecated 迁移路径，新运行时调用必须继续走 `from_resolved_env(LocalIdentityTarget)`。

---

## 每次改进质量验收
1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. **测试覆盖规则**：凡纳入迁移范围的命令、内部模块和转发路径，都必须有对应的集成测试覆盖新 config_kv 读写链路。不维护固定测试列表，以迁移范围清单为准
5. **计划复核规则**：若本次改动更新 `docs/improvement/**`，必须按“计划可信度与事实复核协议”重跑链接、状态、placeholder、验收命令和生产 `unwrap()` / `expect()` 扫描；发现漂移先修计划，再实现代码
