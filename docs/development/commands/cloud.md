# `libra cloud` 开发设计

## 命令实现目标

`libra cloud` 的目标是通过 Cloudflare D1/R2 备份、恢复和同步 Libra 仓库元数据与对象数据。它是 Libra 云端备份扩展，重点是结构化进度、JSON/quiet 输出和云配置错误可诊断性，不对应 Git 原生命令。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。Libra cloud backup/restore extension, not a Git command

- 该命令或行为属于 Libra 扩展/有意差异；重点是清晰边界、结构化输出和可测试错误，而不是 Git 完全同形。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/cloud.rs`。参数/子命令类型包括：`CloudArgs`、`CloudCommand`、`SyncArgs`、`RestoreArgs`、`StatusArgs`；输出、错误或状态类型包括：源码未暴露独立输出/错误类型，错误通过 `CliResult` 或上层命令错误统一传播；主要执行函数包括：`execute`、`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；对象路径会解析 revision 并读写 blob/tree/commit/tag 等对象；引用路径会读取或更新 SQLite refs、HEAD 与 reflog；数据库路径会通过 SeaORM/SQLite 或 D1 客户端持久化元数据；AI 路径会读写 session、checkpoint、thread graph 或 agent profile 状态。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/cloud.rs"]
    B --> C["参数模型<br/>CloudArgs / CloudCommand / SyncArgs"]
    C --> D["执行路径<br/>execute / execute_safe"]
    D --> E["底层对象<br/>D1Client / Storage / StorageExt / .libra/libra.db"]
    D --> F["输出与错误<br/>CliResult"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`D1Client`（Cloudflare D1 元数据读写）；`Storage` / `StorageExt`（对象存储抽象，覆盖本地、remote 和 publish 存储）；SeaORM / `.libra/libra.db`（配置、refs、reflog、AI/发布元数据等 SQLite 表）；`ObjectHash`（SHA-1/SHA-256 对象 ID 和 revision 解析结果）；`Blob`（文件内容或 LFS pointer 写入对象库后的 blob 对象）；`Branch` / branch store（SQLite refs 上的分支读写、过滤和上游关系）；`Head`（SQLite 中的 HEAD 指向、当前分支和 detached 状态）；`LocalStorage`（本地对象或发布存储根目录）；`DatabaseConnection`（SeaORM 数据库连接）；`ObjectType`（blob/tree/commit/tag 类型分派）；Vault/libvault（身份、密钥或 vault-backed 签名边界）；agent checkpoint（Agent 运行快照、回放和 transcript 截断输入）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2026-05-15 `11de0c1f`（`feat(cloud): add structured sync output for json and quiet`）：基础实现节点：add structured sync output for json and quiet；当前实现的主要轮廓可追溯到该提交。
- 2026-05-15 `2319b664`（`feat(cloud): add structured restore output for json mode`）：功能演进：add structured restore output for json mode；该节点扩展了当前命令可用的参数或行为。
- 2026-05-15 `bd9f4b99`（`feat(cloud): add json progress events for cloud sync`）：功能演进：add json progress events for cloud sync；该节点扩展了当前命令可用的参数或行为。
- 2026-05-24 `110efec1`（`fix(cloud): semantic value_names + mutual-exclusion hints (v0.17.903)`）：实现修正：semantic value_names + mutual-exclusion hints (v0.17.903)；该节点把边界行为、错误处理或兼容差异纳入当前实现约束。
- 2026-05-17 `f07208d0`（`test(command/cloud): pin Display for 7 CloudError variants (v0.17.365)`）：测试契约：pin Display for 7 CloudError variants (v0.17.365)；相关行为已有回归守卫，后续变更需要继续满足。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/cloud.md`。
- Synopsis：`libra cloud sync [--force] [--batch-size <N>]`。
- 公开参数/子命令包括：`Subcommand: `sync`、`Subcommand: `restore`、`Subcommand: `status`。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容矩阵说明 | Libra 云备份/恢复扩展, 不是 Git 命令 | 按当前兼容矩阵保留；实现状态变化时同步 `_compatibility.md` 和测试证据。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
