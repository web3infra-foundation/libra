# `libra db` 开发设计

## 命令实现目标

`libra db` 的目标是检查和升级仓库数据库 schema，帮助用户处理 `.libra/libra.db` 与当前二进制之间的版本契约。该命令需要把 schema 状态、显式升级和迁移错误以可诊断方式呈现，是 Libra 仓库维护扩展。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。Libra repository database schema inspection/upgrade extension, not a Git command

- 该命令或行为属于 Libra 扩展/有意差异；重点是清晰边界、结构化输出和可测试错误，而不是 Git 完全同形。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/db.rs`。参数/子命令类型包括：`DbArgs`、`DbSubcommand`；输出、错误或状态类型包括：源码未暴露独立输出/错误类型，错误通过 `CliResult` 或上层命令错误统一传播；主要执行函数包括：`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；schema 检查与升级仅通过 `internal::db` 的 `inspect_database_schema` / `upgrade_database_schema`（SeaORM + SQLite）持久化元数据，不涉及 D1 客户端。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/db.rs"]
    B --> C["参数模型<br/>DbArgs / DbSubcommand"]
    C --> D["执行路径<br/>execute_safe"]
    D --> E["底层对象<br/>.libra/libra.db"]
    D --> F["输出与错误<br/>CliResult"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：SeaORM / `.libra/libra.db`（仓库 schema 版本表，经 `upgrade_database_schema` / `inspect_database_schema` 访问）；升级时附带 `~/.libra/config.db`（全局配置 SQLite 库）的 soft-failure schema 升级。命令不导入或调用 libvault，也不读写配置键、remote、身份或运行时设置。
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2026-05-23 `79c5bc6b`（`feat(db): wire DB_EXAMPLES into clap after_help (v0.17.830)`）：基础实现节点：wire DB_EXAMPLES into clap after_help (v0.17.830)；当前实现的主要轮廓可追溯到该提交。
- 2026-05-23 `7896f9d8`（`feat(db,model): source_call_log schema + auto-upgrade global config DB (v0.17.801)`）：功能演进：source_call_log schema + auto-upgrade global config DB (v0.17.801)；该节点扩展了当前命令可用的参数或行为。
- 2026-05-12 `37772550`（`feat(db): require explicit schema upgrades`）：功能演进：require explicit schema upgrades；该节点扩展了当前命令可用的参数或行为。
- 2026-05-23 `361a739d`（`docs(db): add Examples section mirroring --help banner (v0.17.846)`）：文档与兼容口径：add Examples section mirroring --help banner (v0.17.846)；当前文档按该节点之后的实现状态校准。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/db.md`。
- Synopsis：`libra db <status|upgrade>`。
- 公开参数/子命令包括：`status`、`upgrade`（均无独立 flag，仅复用全局 `--json` / `--machine` 输出开关）。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容矩阵说明 | Libra repository database schema inspection/upgrade extension, 不是 Git 命令 | 按当前兼容矩阵保留；实现状态变化时同步 `_compatibility.md` 和测试证据。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
