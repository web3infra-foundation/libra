# fast-export 命令开发设计

## 命令实现目标

`libra fast-export [<rev>]` 把 `<rev>`（默认 HEAD）可达历史导出为 `git fast-import` 流。只读，不写对象/refs。GGT-13 互操作池命令之一（独立增量）。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`fast-export [<rev>]`；blob（带 mark、去重）、commit（mark/author/committer/data/from/merge）、`deleteall` + 整树 `M` 重建；提交在 `<rev>` 解析的分支 ref 下发出。
- **有意差异/延后**：用整树重建（`deleteall`+`M`）而非父 diff（更大但等价）；一次多 ref、附注/签名 tag、`--export-marks`/`--import-marks`、blob/path 过滤未实现。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::FastExport` → `command::fast_export::execute_safe`。
- 源码分层：`src/command/fast_export.rs`：`FastExportArgs`（`rev?`）、`execute`/`execute_safe`、`resolve_ref_name`、`topological_order`、`flatten_tree`、`format_ident`。
- 复用：`util::get_commit_base`（rev→commit）、`Head::current`（HEAD→分支名）、`command::log::get_reachable_commits`（可达提交）、`command::load_object::<Tree/Blob>`（Result-based，无 panic）。
- 流程：解析 tip + ref_name → 取可达提交 → `topological_order`（迭代后序 DFS，父先于子，保证 `from`/`merge` mark 已定义）→ 对每提交：`flatten_tree`（递归，得 (path, TreeItemMode, oid)）→ 未见 blob 发出（mark + `data <len>` + bytes）→ commit 头（mark/author/committer/data/msg）→ `from`/`merge`（按已 mark 的父）→ `deleteall` → 每文件 `M <mode> :<mark> <path>`（gitlink 用 sha 不用 mark）。
- mark：blob 与 commit 共享递增计数器。mode：`TreeItemMode::to_bytes()`（100644/100755/120000/160000）。
- 输出：`BufWriter<StdoutLock>`，流式写（不全量缓冲）。
- 底层操作对象：只读对象库（commit/tree/blob）。无写入。

## 实现历史

- 2026-06-30（GGT-13 / 2，`grit-gap.md` 阶段 6）：互操作池第二个命令；独立增量。

## 当前状态

- 公开状态：已公开（`Commands::FastExport`）。
- 测试：`tests/command/fast_export_test.rs`（流结构：blob/data/commit refs/heads/deleteall/M 100644 :/author/committer；两提交 `from :` 链接；显式 rev；坏 rev 128；非仓库 128）。
- 用户文档：`docs/commands/fast-export.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 输出形式 | 父 diff（更小输出） | 整树重建（正确但更大）；后续可改 diff。 |
| 范围 | 一次多 ref、tag、marks 文件、过滤 | 延后；首版单 rev。 |
| 配套 | `fast-import`（反向） | GGT-13 另一独立增量（后续）。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- 拓扑序必须保证父先于子（`from`/`merge` mark 先定义）；改输出形式时保持流可被 `git fast-import` 解析。
