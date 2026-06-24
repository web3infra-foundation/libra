# `libra gc`

## 命令实现目标

`libra gc` 公开安全的仓库垃圾回收入口：过期 reflog、追踪可达对象、删除符合策略的不可达 loose object，并清理陈旧 pack 辅助文件。

## 当前状态

- 兼容级别：`partial`。
- 入口：`src/cli.rs::Commands::Gc`。
- 实现：`src/command/gc.rs`。
- 用户文档：[`docs/commands/gc.md`](../../commands/gc.md)。
- 详细设计资料：[`docs/development/internal/gc.md`](../internal/gc.md)。

## 已实现

- `--dry-run` / `--prune=<date>` / `--no-prune` / `--force`。
- `--aggressive` / `--auto` 作为 Git 兼容 no-op 接受。
- JSON / machine 输出。
- 复用 `verify-pack` 校验有效 pack/index 配对。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| pack 重写 | 不 repack、delta-compress、创建 cruft pack 或重写有效 packfile。 | 保持 `partial`；需要独立设计和测试。 |
