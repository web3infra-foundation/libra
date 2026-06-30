# diff-files 命令开发设计

## 命令实现目标

`libra diff-files` 显示 index 与工作树的差异（未暂存改动），作为 `diff` 引擎的底层入口。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`diff-files [-- <path>...]`（= 裸 `diff`，index 对工作树），所有 `diff` 全局标志与 `--json`。
- 未公开（延后）：`-1`/`-2`/`-3` 阶段选择、raw 输出。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::DiffFiles` → `command::diff_plumbing::execute_files_safe`。
- 源码分层：`src/command/diff_plumbing.rs`：`DiffFilesArgs`（`paths`）→ 合成 `["diff","--",paths...]` → `DiffArgs::try_parse_from` → `command::diff::execute_safe`。复用唯一 diff 引擎。
- 底层操作对象：index + 工作树（经 diff 引擎读）。无写入。

## 实现历史

- 2026-06-30（GGT-11 diff-plumbing 部分）：与 `diff-tree`/`diff-index` 同批新增。

## 当前状态

- 公开状态：已公开（`Commands::DiffFiles`）。
- 测试：`tests/command/diff_plumbing_test.rs`（diff-files 显示未暂存改动）。
- 用户文档：`docs/commands/diff-files.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容差异项 | `-1`/`-2`/`-3` 阶段选择、raw 输出 | 延后；更多用 `libra diff`。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- 必须继续复用 `command::diff` 引擎。
