# `libra prune`

## 命令实现目标

`libra prune` 公开低层对象清理入口，用于删除从 refs 和额外 head 参数都不可达、且符合过期策略的 loose object。

## 当前状态

- 兼容级别：`partial`。
- 入口：`src/cli.rs::Commands::Prune`。
- 实现：`src/command/prune.rs`。
- 用户文档：[`docs/commands/prune.md`](../../commands/prune.md)。
- 详细设计资料：[`docs/development/internal/prune.md`](../internal/prune.md)。

## 已实现

- `-n` / `--dry-run`。
- `-v` / `--verbose`。
- `--expire <TIME>`。
- `[HEAD]...` 额外保留根。
- JSON / machine 输出。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 并发 writer 保护 | 直接 prune 仍保留 Git-like 并发 writer 风险。 | 用户文档提示优先 dry-run 与 `--expire`；日常维护优先 `libra gc`。 |
