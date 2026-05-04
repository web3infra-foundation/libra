# C6：拒绝/延后立场登记

## 所属批次

C6（Audit P3 + 跨批次延后项汇总）

## 已完成前置条件与当前代码状态

C6 是本计划的最终收口批次，依赖 C1–C5 全部落地后才开始；其唯一职责是把审计建议中"不做"或"按需重启"的决定正式登记，并把每项与当前代码证据、重启条件绑定起来。本批不引入任何代码变更。

## 目标与非目标

**目标：**
- 把审计中被本计划显式拒绝或延后的 10 项决定写成正式清单。
- 每项包含：审计原文要点、当前代码证据、拒绝/延后理由、重新评估的触发条件。
- 在 `COMPATIBILITY.md` 对应行 notes 中链接回本文。

**非目标：**
- 不引入代码变更。
- 不预测未来路线图——重启条件只描述"什么外部信号会让我们重新讨论"，不承诺时间。
- 不与 [agent.md](../agent.md) 重复评估 hooks bridge——本文只引用 agent.md 的现状结论。

## 拒绝项（不实施）

### D1：`submodule` 子命令族

- **审计原文要点**：审计认为 submodule 是 Git 标准能力，建议至少给出迁移指引或部分支持。
- **当前代码证据**：[`src/command/push.rs:1318`](../../../src/command/push.rs#L1318) 已有 "submodule is not supported yet" 警告；README 顶部明确声明 Libra 不提供 submodule / subtree。
- **拒绝理由**：Libra 产品方向是单仓库 + trunk-based + 外部大对象交对象存储，submodule 与该方向冲突；引入会显著扩大 surface 且没有实质用户场景。
- **重启条件**：
  - 出现具体的、不可由 monorepo 解决的多仓库依赖工作流。
  - 至少 3 位独立用户提出请求且给出明确的 use case。
  - 同时承诺在产品边界内重新评估 monorepo + 对象存储的替代方案。

### D2：本地 file remote 的 `push`

- **审计原文要点**：审计认为现有 [`UnsupportedLocalFileRemote`](../../../src/command/push.rs#L117) 是缺口，建议评估补齐。
- **当前代码证据**：[`src/command/push.rs:117`](../../../src/command/push.rs#L117) 显式拒绝；[push.rs:195-198](../../../src/command/push.rs#L195) 给出 hint "use fetch/clone for local-path repositories; push currently supports network remotes only"。
- **拒绝理由**：本地 file remote push 在生产环境使用极少；当前拒绝是有意行为（避免本地路径下的未定义并发写入语义）；在 `COMPATIBILITY.md` 重新框定为 `intentionally-different` 而非"缺口"。
- **重启条件**：
  - 出现明确的本地多 worktree 协作场景且 fetch / pull 不能满足。
  - 设计一份描述并发写入语义的 RFC（包含 lock 策略、原子性保证、错误恢复）。

### D3：Git hooks bridge 作为核心特性

- **审计原文要点**：审计建议把 `core.hooksPath` / `.git/hooks` 的兼容层作为核心特性补齐。
- **当前代码证据**：当前文档强调 AI provider hook forwarding（见 [agent.md](../agent.md) Part B）；不提供 stock Git hooks 兼容层。
- **延后理由**：当前 AI provider hook 体系是有意设计；引入 Git hooks bridge 会让两套 hook 系统的优先级与 conflict resolution 复杂化；agent.md Part B Phase 5 已经在评估 vault / env / hook 的统一收口，本计划不并行决策。
- **重启条件**：
  - agent.md Part B Phase 5 完成并明确 hooks 统一收口位置。
  - 在 Phase 5 收口后再决定是否在 `core.hooksPath` 维度增加 stock Git 兼容层。

### D4：`clone --recurse-submodules`

- **审计原文要点**：审计列入 clone 高级 flag 缺口。
- **当前代码证据**：[`src/command/clone.rs`](../../../src/command/clone.rs) 没有该 flag。
- **拒绝理由**：依赖 D1（submodule 不支持）。
- **重启条件**：D1 重启时同步重启。

### D5：Git LFS `.gitattributes` filter / hooks bridge

- **审计原文要点**：审计把仓库级 LFS 治理与 Git LFS 兼容性作为二进制资产管理风险提出，建议明确是否支持 `.gitattributes` 中的 Git LFS filter 规则。
- **当前代码证据**：[`docs/commands/lfs.md`](../../commands/lfs.md) 明确 Libra 使用内置 LFS，并通过 `.libraattributes` 管理 tracking；[`src/command/lfs.rs`](../../../src/command/lfs.rs) 中 `track` / `untrack` 操作也是写 Libra Attributes，而不是调用 `git-lfs install` 或配置 smudge/clean filters。
- **拒绝理由**：这是有意差异。Libra 的 LFS 产品方向是内置 pointer / lock / batch client，避免依赖外部 `git-lfs` 二进制、`.gitattributes` filter 配置和 hooks bridge。仓库根 `.gitattributes` 只负责文本归一化与 binary diff 策略，不承载 Git LFS filter 安装。
- **重启条件**：
  - 出现必须与 stock Git + `git-lfs` 双向共享同一工作树的生产场景。
  - 给出 filter 优先级、`.libraattributes` 与 `.gitattributes` 冲突处理、hooks bridge 安全边界的独立 RFC。

## 延后项（暂不实施，记录条件）

### D6：`bisect replay`

- **审计原文要点**：审计列入 bisect 自动化能力。
- **当前状态**：C4 落地后 [bisect.md](bisect.md) 已记录延后；属于小众工作流。
- **延后理由**：`bisect replay` 用于从 `bisect log` 输出重放历史 bisect；在 CI 自动化中价值有限，多用于事后复盘；优先级低于 `run` / `view`。
- **重启条件**：
  - `bisect log` 输出格式稳定 ≥1 个 release。
  - 至少 2 位用户报告需要 replay 能力。

### D7：`bisect terms`

- **审计原文要点**：审计列入 bisect 自定义能力。
- **当前状态**：C4 落地后 [bisect.md](bisect.md) 已记录延后。
- **延后理由**：自定义 good/bad 别名（如 fast/slow）属于工作流个性化；不影响核心定位能力。
- **重启条件**：
  - 用户明确请求且 `bisect run` 已稳定。

### D8：`stash create`

- **审计原文要点**：审计列入 stash 完整子命令面。
- **当前状态**：C4 落地后 [stash-surface.md](stash-surface.md) 已记录延后。
- **延后理由**：`stash create` 仅返回 stash object hash 不存 ref，属于内部 plumbing；非用户日常路径；与 `stash store` 配合才有意义。
- **重启条件**：
  - 出现明确的 plumbing 调用方场景（如 git-extras 风格的脚本工具链）。

### D9：`stash store`

- **审计原文要点**：审计列入 stash 完整子命令面。
- **当前状态**：C4 落地后 [stash-surface.md](stash-surface.md) 已记录延后。
- **延后理由**：与 D8 配套，单独存在无意义。
- **重启条件**：与 D8 同步。

### D10：`clone --sparse` 与顶层 `sparse-checkout` 命令

- **审计原文要点**：审计列入 P1 公开建议（`clone --sparse` 高级 flag）；同时 [shallow.md](shallow.md) C3 调研明确顶层 `sparse-checkout` 命令也未实现。
- **当前代码证据**：[`src/command/clone.rs`](../../../src/command/clone.rs) 没有 `--sparse` flag；`src/cli.rs::Commands` 没有 `SparseCheckout` 变体；`src/internal/` 中没有 sparse-checkout reader 实现。
- **延后理由**：sparse-checkout 高度依赖 Git 管理元数据与 worktree config；Libra 已把 config / HEAD / refs 迁到 SQLite，桥接代价高。本计划把 `clone --sparse` flag 与顶层 `sparse-checkout` 命令合并登记为同一项延后——它们共享同一个底层 plumbing 缺失，不应拆分讨论。
- **重启条件**：
  - 出现明确的大型 monorepo 子树检出需求。
  - 评估对象存储 + 部分检出的工程复杂度，给出独立 RFC。
- **覆盖范围**：本项同时收口 `clone --sparse` flag 和 `git sparse-checkout init/set/list/disable/...` 子命令族；任一重启都触发整体重新评估。

## `COMPATIBILITY.md` 行链接策略

每个 `unsupported` / `intentionally-different` 行的 notes 末尾追加 `(see compatibility/declined.md#Dn)`：

```markdown
| submodule | unsupported | intentional product boundary (see compatibility/declined.md#d1-submodule-子命令族) |
| push | partial | local file remote intentionally rejected (see compatibility/declined.md#d2-本地-file-remote-的-push) |
| lfs | partial | built-in `.libraattributes` LFS; Git LFS filters intentionally different (see compatibility/declined.md#d5-git-lfs-gitattributes-filter--hooks-bridge) |
```

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`docs/improvement/compatibility/declined.md`](declined.md) | 新建 | 本文件 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | 各 unsupported / intentionally-different 行 notes 加链接 |

## 测试与验收

- [ ] 本文档的每个拒绝/延后项都链接到当前代码或 README 的具体证据点（不允许"参见 README"这种泛指）。
- [ ] 每项都有"重启条件"段，且条件是外部可观测信号（不能是"看心情"或"有空再说"）。
- [ ] `COMPATIBILITY.md` 中所有 `unsupported` / `intentionally-different` 行 notes 已链接回本文对应小节。

## 风险与缓解

1. **延后项随时间被遗忘** → 缓解：每项的重启条件是可观测的；当条件触发时，本文与 `COMPATIBILITY.md` 同步更新；不需要主动监控。
2. **拒绝项被外部用户反复请求** → 缓解：`COMPATIBILITY.md` 与本文都明确列出"为什么不做"和"什么时候重新讨论"；维护者可以直接引用而不需要重复辩论。
3. **D3（hooks bridge）与 [agent.md](../agent.md) 演化错位** → 缓解：D3 的重启条件直接绑定 agent.md Part B Phase 5；agent.md 更新时本文需同步检视。
4. **D10（sparse）与 D1（submodule）的边界混淆** → 缓解：本文清晰区分——sparse 是"延后"（取决于工程复杂度评估），submodule 是"拒绝"（产品边界已定）。
