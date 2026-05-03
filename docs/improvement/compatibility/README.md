# Libra Git 兼容命令补齐计划（compatibility 子计划）

## Context

本计划基于 2026-05-02 的第三方静态审计报告 *Libra 仓库与常用 Git 命令兼容性评估及改进计划*，与 [`docs/improvement/README.md`](../README.md) 已完成的 8 批 CLIG 现代化工作并行推进。两份计划维度正交：

- **上层 README（第 1–8 批 + 后续 30–33 批）**：CLI UX / CLIG 现代化——`run_<cmd>()` 与 `render_<cmd>_output()` 拆分、typed `<Cmd>Error`、JSON / machine schema、`StableErrorCode`、退出码三级模型。
- **本计划（C1–C6）**：Git **surface** 兼容——子命令面是否齐全、CLI flag 是否公开、仓库治理文件是否到位、CI 兼容矩阵是否产品化、行为差异是否对外显式化。

PDF 报告核心结论：Libra 已经接近一个对象格式与远程协议层兼容的 Git 客户端，但命令面、运维面与治理层尚未把"承诺到哪一层"写清楚。本计划目标是把 P0 / P1 / P2 的可观测缺口转化为可执行批次，并明确哪些审计项被显式延后或拒绝。

## 2026-05-02 完整性复核结论

本目录的 C1-C6 子计划已经覆盖审计报告指出的核心兼容缺口：治理基线、CI required-checks、浅克隆、stash / bisect 子命令面、worktree / checkout 行为差异，以及拒绝/延后项登记。复核后判断：**总体结构完整，但原稿存在若干会影响实施的事实漂移，必须在落地前修正**。

已补齐/修正的缺口：

- **顶层命令矩阵不完整**：C1 原骨架未覆盖 `src/cli.rs` 中的 `rm` / `mv` / `grep` / `rev-parse` / `rev-list` / `lfs` / `cloud` / `code` / `code-control` / `graph` 等实际命令。已要求 `COMPATIBILITY.md` 以 `src/cli.rs::Commands` 为枚举来源，并显式列出所有顶层命令（含 hidden 命令）。
- **LFS 表述冲突**：仓库已有 `libra lfs` 命令与 `docs/commands/lfs.md`，不能把 LFS 总体写成 `unsupported`。已区分"Libra 内置 LFS 命令面"与"Git LFS `.gitattributes` 过滤器兼容层"：前者列入命令矩阵，后者作为 Git-compat 差异单独说明。
- **CI 基线陈旧**：当前 `base.yml` 已有 `format` / `clippy` / `redundancy` / `test` 多 job，并非单 job。C2 改为"job 名唯一化 + required-checks 拆分 + live/secret 测试隔离"，避免后续按错误基线重写 workflow。
- **命令文档状态漂移**：`docs/commands/bisect.md` / `docs/commands/checkout.md` / `docs/commands/worktree.md` 已存在；相关子计划改为"修改/同步"而非"新建"。`docs/commands/README.md` 还缺少现有 `code-control` 命令，并把 bisect / checkout 标成 hidden；C1 / C4 / C5 需要分别同步这些索引偏移。

仍需在实施时动态确认的事项：

- `clone --sparse` 需在 C3 实施当天重新检索 `src/internal/`，按 plumbing 是否存在选择 `unsupported` 或 `partial`。
- C2 的 required-checks 名称必须与 GitHub UI 中的实际 branch protection 配置同步；文档只能给 checklist，不能替代平台操作。
- CODEOWNERS 团队 handle 仍是占位符，C1 落地时必须替换为真实 GitHub team 或 maintainer handle。

## 2026-05-03 最终 Review 收口

经多轮编辑后做最后一次跨文档一致性扫描，结论：**计划可发布，无未决冲突**。本轮收尾只做了三处装订级别修正：

- [shallow.md](shallow.md) §目标：把 `COMPATIBILITY.md` 行更新指令中的 em-dash 笔误改回 `--depth` code 标记，与 [governance.md](governance.md) roadmap 表中的 `--depth public flag` 字面对齐。
- [declined.md](declined.md) §D10：把"`clone --sparse`"扩为"`clone --sparse` 与顶层 `sparse-checkout` 命令"，并加 **覆盖范围** 段，避免顶层 `sparse-checkout` 在 [governance.md](governance.md) 矩阵骨架与本表里被重复登记。
- [governance.md](governance.md) §"Git commands intentionally absent from `src/cli.rs`"：`sparse-checkout` 行 notes 末尾追加 `(see compatibility/declined.md#d10-...)`，与 `submodule` 行的链接风格统一。

未触及的事项（已在前几轮 Review 中收尾，本轮不重复修改）：

- C1 矩阵骨架以 `src/cli.rs::Commands` 枚举为事实来源，含 `rm` / `mv` / `grep` / `rev-parse` / `rev-list` / `lfs` / `cloud` / `code` / `code-control` / `graph` / `index-pack` 等顶层命令——已在前一轮直接修正于 [governance.md](governance.md)。
- LFS 已分两层描述（`libra lfs` 命令面 vs Git LFS `.gitattributes` filter / hooks bridge）——`libra lfs` 入命令矩阵，filter / hooks bridge 入 [declined.md](declined.md) D5。
- C2 已对齐当前 `base.yml` 的多 job 结构（不再误写为单 job 拆分），改为 job 名规范化 + required-checks 切换。
- `docs/commands/{bisect,checkout,worktree}.md` / `docs/commands/README.md` 现状偏移在 C1 / C4 / C5 各自子文档的"关键文件与改动"中已显式登记修订对象，不另外抽出全局收尾项。

## 用户已确认的方向决策

| 决策点 | 选择 | 影响批次 |
|------|-----|-----|
| `fetch --depth` 公开形态 | 公开为稳定 flag | C3 |
| `Checkout` 命令处置 | 取消 `hide=true`，正式作为分支类兼容入口；文件恢复继续推荐 `restore` | C5 |
| `worktree remove` 行为对齐 | 加 `--delete-dir` 显式开关，保留当前默认（不删盘） | C5 |
| 文档语言风格 | 中文叙述 + 英文兼容矩阵字段 | C1 / 全部子文档 |

## 跨计划职责边界

> 涉及 `run_<cmd>()` 拆分、`<Cmd>Error` typed enum、JSON / machine schema、`render_<cmd>_output()` 的工作 → **上层 README 的命令批次（30–33）拥有**。
> 涉及 CLI flag 的有/无、子命令的有/无、文档兼容矩阵、CI job 命名、仓库治理文件、Git 行为差异显式化 → **本计划（C1–C6）拥有**。

具体边界：

- **第 30 批 `checkout` 完整现代化** vs **C5 checkout 处置**：第 30 批负责 `CheckoutError` typed enum、JSON、render split；C5 只负责 `src/cli.rs` 的 `hide` 标志和 `--help` EXAMPLES 文案。两批可独立推进，不互相阻塞。
- **第 31 批 `worktree` 结构化输出** vs **C5 worktree `--delete-dir`**：第 31 批负责 `WorktreeOutput` schema 与 `WorktreeError`；C5 只负责在 `WorktreeSubcommand::Remove` 加 flag、补删盘分支与回归测试。先后顺序无强约束。
- **第 5 批 `fetch` 已落地** vs **C3 `fetch --depth`**：`fetch` 的 CLIG 改造已在第 5 批完成；本计划只在 [`fetch.md`](../fetch.md) 追加"审计驱动增量"小节，不重新拥有该文档。
- **第 4 批 `stash` 已落地** vs **C4 stash 子命令面补齐**：第 4 批拥有 `StashError` / `StashOutput`；C4 只扩展 `Stash` enum 的 variant（`Show` / `Branch` / `Clear`），按已落地 scaffolding 复用 typed error 与 render split 模式。
- **`lfs` 命令面** vs **Git LFS 过滤器兼容层**：`libra lfs` 已是现有命令，命令文档由 [`docs/commands/lfs.md`](../../commands/lfs.md) 拥有；本计划只在 `COMPATIBILITY.md` 说明其与 Git LFS `.gitattributes` / filter / hooks 机制的兼容差异，不重新设计 LFS。

## 批次表

| 批次 | 名称 | Audit P-level | 关键交付 | 子文档 | 依赖 |
|------|-----|--------------|----|-----|-----|
| **C1** | 仓库治理基线 | P0 | `COMPATIBILITY.md`（4-tier）+ `.gitattributes` + `.github/CODEOWNERS` + 分支保护建议 | [governance.md](governance.md) | 无 |
| **C2** | CI 兼容矩阵与 job 唯一化 | P1 | `compat-rustfmt` / `compat-clippy` / `compat-offline-core` / `compat-network-remotes` / `security-codeql-*` 命名规范；`tests/compat/` 集结点 | [ci.md](ci.md) | C1 |
| **C3** | 浅克隆契约 | P1 | `libra fetch --depth` 公开为稳定 flag；`clone --depth` 文档化；`clone --sparse` / `--recurse-submodules` 立场登记 | [shallow.md](shallow.md) | C1 |
| **C4** | 子命令面补齐（stash + bisect） | P2 | `stash show / branch / clear`；`bisect run / view`；replay/terms/create/store 显式延后 | [stash-surface.md](stash-surface.md), [bisect.md](bisect.md) | C1 + C2 |
| **C5** | 行为差异显式化（worktree + checkout） | P2 | `worktree remove --delete-dir`；`Checkout` 取消 hide 并标 branch compatibility surface | [worktree-surface.md](worktree-surface.md), [checkout-disposition.md](checkout-disposition.md) | C1 |
| **C6** | 拒绝/延后立场登记 | P3 | `submodule` / 本地 file remote push / Git hooks bridge / Git LFS filter bridge / `clone --recurse-submodules` 等正式不做或按需重启清单 | [declined.md](declined.md) | 全部前置 |

执行顺序：

```
C1 ──┬─→ C2 ──┐
     │        ├─→ C4 ──┐
     ├─→ C3 ──┤        ├─→ C6
     │        │        │
     └─────── C5 ──────┘
```

## 子文档索引

1. [governance.md](governance.md) — C1：`COMPATIBILITY.md` 4-tier 设计；`.gitattributes` 最小集；CODEOWNERS 路由；分支保护建议；未来 LFS 叠加规则伪代码。
2. [ci.md](ci.md) — C2：workflow job 命名规范；`compat-*` 矩阵；`tests/compat/` 目录约定；与 `Cargo.toml` 现有 feature 的映射；required-checks 切换 checklist。
3. [shallow.md](shallow.md) — C3：`fetch --depth` 公开实现要点；`clone --depth` 现状；`clone --sparse` 待评估；`clone --recurse-submodules` 拒绝。
4. [stash-surface.md](stash-surface.md) — C4：`stash show / branch / clear` 语义；与第 4 批 [`stash.md`](../stash.md) 已落地基线的复用关系；`stash create` / `stash store` 延后。
5. [bisect.md](bisect.md) — C4：`bisect run` / `bisect view` 设计；退出码语义对齐 Git；`bisect replay` / `bisect terms` 延后；同时承担 bisect 模块"首次入计划"。
6. [worktree-surface.md](worktree-surface.md) — C5：`--delete-dir` 行为定义；与 `WorktreeSubcommand::Remove` 当前内联参数形态的兼容路径；测试夹具说明。
7. [checkout-disposition.md](checkout-disposition.md) — C5：取消 `hide` 的影响面；`--help` 顶部 banner 文案；与第 30 批的协同时序。
8. [declined.md](declined.md) — C6：拒绝/延后清单及重启条件。

## 与上层 README 的最小集成

仅在 [`docs/improvement/README.md`](../README.md) "全局层面改进" 表（A–H）后追加一行：

```markdown
| **I** | **Git surface 兼容性补齐** → 见 [compatibility/README.md](compatibility/README.md)：4-tier `COMPATIBILITY.md` / 仓库治理 / CI 兼容矩阵 / stash・bisect 子命令面 / worktree 与 checkout 行为差异 | 与各命令批次并行 |
```

不修改现有命令批次表与 30–33 后续批次表。

## 时间线立场

PDF 报告中的 gantt 时间线（2026-05-04 起）**视为参考估算**，不绑定具体日期。批次顺序按 C1 → (C2 ‖ C3 ‖ C5) → C4 → C6 推进；具体排期由维护者按现实带宽决定。

## 验证总则

每个批次落地后必须保证：

1. `cargo +nightly fmt --all --check` 无格式差异。
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告。
3. `cargo test --all` 全部通过。
4. `COMPATIBILITY.md` 中对应行的状态字段已更新为最新结论；不允许出现"代码已落地但矩阵未更新"或反之的偏移。
5. 涉及 CI 改动时，governance.md 已记录需要在 GitHub UI 同步切换的 required-checks 清单。
6. 涉及 `src/cli.rs::Commands` 新增/删除/取消 hidden 的改动时，必须同步检查：
   - 根 `COMPATIBILITY.md` 的命令行是否完整。
   - [`docs/commands/README.md`](../../commands/README.md) 的命令索引是否仍准确。
   - 对应 `docs/commands/<cmd>.md` 是否需要更新 synopsis、参数对比和兼容差异说明。
