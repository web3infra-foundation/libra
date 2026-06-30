# merge-base 命令开发设计

## 命令实现目标

`libra merge-base` 打印两个提交的最佳共同祖先（LCA），并提供 `--all`（全部 LCA）与 `--is-ancestor`（祖先测试）。同一 LCA 实现（`internal/merge_base.rs`）被 `diff A...B` 复用。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`merge-base <a> <b>`（单 base）、`--all`（全部 LCA）、`--is-ancestor`（exit 0/1）、`--json`/`--machine`。
- 退出码：0 找到/祖先成立；1 无共同祖先/祖先不成立（无输出，**对齐 Git**——计划早期写「无共同祖先 → 128」与 Git 不符，Git 此情形 exit 1、128 留给坏 rev，已据此调和）；128 坏 rev / 参数个数错误。
- 未公开（延后）：多于两个提交、`--octopus`/`--independent`/`--fork-point`。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::MergeBase` → `command::merge_base::execute_safe`。
- 核心：`src/internal/merge_base.rs` —— **唯一** LCA 实现：
  - `CommitGraph`（`parents_of` 带缓存，经 `object_ext::CommitExt::try_load`，不依赖 `command::`）+ `ancestors`（BFS，含自身）。
  - `merge_bases(a,b)`：common = anc(a)∩anc(b)；dominated = common 中「是另一 common 的**严格**祖先」者；LCA = common − dominated（按 hex 排序，确定性）。
  - `merge_base(a,b)` = 第一个 LCA；`is_ancestor(anc,desc)` = `anc ∈ ancestors(desc)`（自反，对齐 `--is-ancestor X X`→0）。
  - **修正 first-found**：旧 `log.rs`/`rebase.rs` 的 `find_merge_base` 返回首个命中（非 LCA），交叉合并下可能偏高；本实现返回真 LCA。
- CLI：`src/command/merge_base.rs`：`MergeBaseArgs`（`all`/`is_ancestor`/`commits`）；`--is-ancestor` 与 `--all` 互斥；要求恰好 2 个 commit；`resolve_commit`（`util::get_commit_base`，坏 rev→128）；无共同祖先/祖先不成立→`silent_exit(1)`；`--json` `{ bases }` / `{ is_ancestor }`。
- `diff A...B`：`diff.rs::normalize_diff_range` 在两点解析**之前**先 `split_once("...")`，解析 left/right→`get_commit_base`→`merge_base::merge_base`，把 `args.old` 设为 base、`args.new` 设为 right；无法解析/无 base 时保持 pathspec 回落。保留既有 `A..B` 语义。
- 底层操作对象：对象库（读 commit）。无 refs/网络/index/工作树写入。

## 实现历史

- 2026-06-30（GGT-09 Phase A，`grit-gap.md` 阶段 4）：新建 `internal/merge_base.rs` + `merge-base` CLI + `diff A...B`。

## 当前状态

- 公开状态：已公开（`Commands::MergeBase`）。
- 测试：`tests/command/merge_base_test.rs`（Y 形 merge-base=base、`--is-ancestor` 双向、`--all`、`--json`、坏 rev 128、参数个数 128、`diff A...B` 用 merge-base）。
- 用户文档：`docs/commands/merge-base.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 共享收口（Phase B） | `log.rs`/`rebase.rs` 仍各自 `find_merge_base`（first-found，非 LCA）未迁移到 `internal/merge_base.rs` | **有意延后**：迁移会改变 `log A..B`/`rebase`/输出，需 golden 回归 + `legacy-merge-base` 开关（计划要求）。本次只交付 merge-base CLI + diff A...B（自包含、不动 log/rebase）。 |
| 兼容差异项 | 多提交、`--octopus`/`--independent`/`--fork-point` | 延后。 |
| 性能 | LCA dominated 计算对每个 common 节点做全祖先遍历（O(common×E)） | 正确但非最优；后续可引入 Git 的时间戳 paint 算法。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- LCA 逻辑只允许存在于 `internal/merge_base.rs`；Phase B 迁移 `log`/`rebase` 时必须先有 golden 回归与 legacy 开关。
