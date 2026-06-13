# `libra status` 开发设计

## 命令实现目标

`libra status` 的目标是展示工作区和索引状态，并支持 porcelain v1/v2、NUL 输出、untracked/ignored 模式和结构化输出。实现需要稳定呈现 add/modify/delete/rename 相关状态，同时把 rename detection 与 column display 等能力列为缺口。

## 对比 Git 与兼容性

- 兼容级别：`supported`。

- 当前矩阵承诺常用 Git 行为已支持；新增语义必须同步矩阵、用户文档和测试。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/status.rs`。参数/子命令类型包括：`StatusArgs`；输出、错误或状态类型包括：`StatusError`、`UpstreamInfo`、`MergeStatusInfo`；主要执行函数包括：`execute`、`execute_safe`、`execute_to`、`changes_to_be_committed_safe`、`changes_to_be_staged_split_safe`。
- 源码意图：源码模块注释说明该命令结合 ignore 策略计算 staged/unstaged/untracked 集合，并输出简洁摘要或结构化状态。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；核心领域逻辑集中在 `execute_to`、`changes_to_be_committed_safe`、`changes_to_be_staged_split_safe`；索引路径会加载、比较、刷新或保存 `.libra/index`；对象路径会解析 revision 并读写 blob/tree/commit/tag 等对象；引用路径会读取或更新 SQLite refs、HEAD 与 reflog；数据库路径会通过 SeaORM/SQLite 或 D1 客户端持久化元数据。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/status.rs"]
    B --> C["参数模型<br/>StatusArgs"]
    C --> D["执行路径<br/>execute / execute_safe / execute_to"]
    D --> E["底层对象<br/>Index / .libra/index / Blob / Commit"]
    D --> F["输出与错误<br/>StatusError / UpstreamInfo / MergeStatusInfo"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Index` / `.libra/index`（暂存区状态、路径条目和刷新/保存边界）；`Blob`（文件内容或 LFS pointer 写入对象库后的 blob 对象）；`Commit`（提交对象、父提交关系和提交消息载荷）；`TreeItem` / `TreeItemMode`（tree 中的路径项和 mode）；`Tree`（由索引或对象遍历生成的目录树对象）；`Branch` / branch store（SQLite refs 上的分支读写、过滤和上游关系）；`Head`（SQLite 中的 HEAD 指向、当前分支和 detached 状态）；SeaORM / `.libra/libra.db`（配置、refs、reflog、AI/发布元数据等 SQLite 表）；`ObjectHash`（SHA-1/SHA-256 对象 ID 和 revision 解析结果）；`ConfigKv`（配置键值持久化行）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2025-11-11 `926b2c38`（`Add --ignored arg for libra status (#35)`）：基础实现节点：Add --ignored arg for libra status (#35)；当前实现的主要轮廓可追溯到该提交。
- 2026-06-06 `7d985dec`（`feat(status): add -z NUL-terminated porcelain output (implies v1)`）：功能演进：add -z NUL-terminated porcelain output (implies v1)；该节点扩展了当前命令可用的参数或行为。
- 2025-12-10 `22ecce78`（`feat(status): support --porcelain=v2 and --untracked-files modes (#78) (#82)`）：功能演进：support --porcelain=v2 and --untracked-files modes (#78) (#82)；该节点扩展了当前命令可用的参数或行为。
- 2026-05-17 `f5351224`（`docs(status): correct porcelain-v2 rationale + document stash_entries opt-in`）：文档与兼容口径：correct porcelain-v2 rationale + document stash_entries opt-in；当前文档按该节点之后的实现状态校准。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/status.md`。
- Synopsis：`libra status [OPTIONS]`。
- 公开参数/子命令包括：`-s, --short`、`--porcelain [VERSION]`、`--branch`、`--show-stash`、`--ignored`、`--untracked-files <MODE>`、`--exit-code`。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 功能缺口 | Git's --find-renames / -M is not 支持; rename detection is not yet implemented in Libra's status | 后续实现时需要同步源码、测试和兼容矩阵。 |
| 功能缺口 | column display is not 支持 | 后续实现时需要同步源码、测试和兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
