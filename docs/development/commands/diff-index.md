# diff-index 命令开发设计

## 命令实现目标

`libra diff-index <tree>` 显示一个 tree 与工作树的差异，作为 `diff` 引擎的底层入口。

## 对比 Git 与兼容性

- 兼容级别：`partial`。
- 已支持：`diff-index <tree> [-- <path>...]`（= `diff --old <tree>`，tree 对工作树），所有 `diff` 全局标志与 `--json`。
- **未公开**：`--cached`（tree 对 index）→ 退出 128 并指引用 `diff --staged`（Libra `diff --staged` 仅 HEAD 对 index，不支持任意 tree 对 index）。raw 输出、`-m` 延后。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::DiffIndex` → `command::diff_plumbing::execute_index_safe`。
- 源码分层：`src/command/diff_plumbing.rs`：`DiffIndexArgs`（`cached`/`tree`/`paths`）。`--cached` → `command_usage` + `Unsupported` + 128；否则合成 `["diff","--old",tree,"--",paths...]` → `DiffArgs::try_parse_from` → `command::diff::execute_safe`。复用唯一 diff 引擎。
- 底层操作对象：对象库 + 工作树（经 diff 引擎读）。无写入。

## 实现历史

- 2026-06-30（GGT-11 diff-plumbing 部分）：与 `diff-tree`/`diff-files` 同批新增。

## 当前状态

- 公开状态：已公开（`Commands::DiffIndex`）。
- 测试：`tests/command/diff_plumbing_test.rs`（diff-index tree 对工作树、`--cached` → 128）。
- 用户文档：`docs/commands/diff-index.md`（EN + zh-CN）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容差异项 | `--cached`（任意 tree 对 index）、raw、`-m` | `--cached` → 128 指引 `diff --staged`；Libra diff 引擎暂无「任意 tree 对 index」模式。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- 必须继续复用 `command::diff` 引擎；若未来支持 `--cached`，应在 diff 引擎层新增「tree 对 index」模式，而非在本命令内另建 diff。
