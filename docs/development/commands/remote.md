# `libra remote` 开发设计

## 命令实现目标

`libra remote` 的目标是管理远端配置，包括 add/remove/rename/get-url/set-url/prune/show/-v 等子命令。实现需要保护 SSH key namespace、复用 fetch prune 逻辑，并把远端状态以用户可读和结构化方式呈现。

## 对比 Git 与兼容性

- 兼容级别：`partial`。`add`/`remove`/`rename`/`-v`/`show`/`get-url`/`set-url`/`prune` 加上 `set-branches [--add]`（重写 `remote.<name>.fetch`）与 `set-head <branch>`/`-d`/`--delete`（写入/删除 `refs/remotes/<name>/HEAD`）已支持；详细 `remote show <name>` 已支持但**仅离线**（报告配置的 fetch/push URL、缓存的 remote HEAD、缓存的远端跟踪分支与本地 pull/push 配置，`queried` 恒为 `false`）。尚未公开：`remote show <name>` 的在线发现、`remote update`、`set-head --auto`。

- 当前矩阵承诺常用 Git 行为已支持；新增语义必须同步矩阵、用户文档和测试。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/remote.rs`。参数/子命令类型包括：`RemoteCmds`；输出、错误或状态类型包括：`RemoteListEntry`、`RemotePruneEntry`、`RemoteBranchStatus`、`RemotePullConfig`、`SetHeadMode`、`RemoteOutput`；主要执行函数包括：`execute`、`execute_safe`、`run_remote`、`run_show_remote`、`run_set_branches`、`run_set_head`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；引用路径会读取或更新 SQLite refs、HEAD 与 reflog；网络路径会解析 remote 配置、协商协议并处理 pack/idx 数据。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/remote.rs"]
    B --> C["参数模型<br/>RemoteCmds"]
    C --> D["执行路径<br/>execute / execute_safe"]
    D --> E["底层对象<br/>Branch / ConfigKv"]
    D --> F["输出与错误<br/>RemoteListEntry / RemotePruneEntry / RemoteOutput"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Branch` / branch store（SQLite refs 上的分支读写、过滤和上游关系）；SSH transport（SSH remote 连接和认证）；Vault/libvault（身份、密钥或 vault-backed 签名边界）；`ConfigKv`（配置键值持久化行）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2025-10-25 `5703987b`（`feat: add option rename for remote command (#27)`）：基础实现节点：add option rename for remote command (#27)；当前实现的主要轮廓可追溯到该提交。
- 2026-06-09 `b8e6b4f4`（`feat(remote): add detailed `remote show <name>` subcommand (#379)`）：引入带 `<name>` 参数的详细 `remote show <name>`（fetch/push URL、HEAD 分支、远端及本地跟踪分支）。该 Show 详情曾被一次 reconcile 丢失内容，已于 2026-06-18 以**离线**形态恢复到当前代码：`RemoteCmds::Show { name: Option<String>, no_query, verbose }` 分发到 `run_show_remote`（带 `<name>`）或 `run_list_remotes(false)`（无 `<name>`），新增 `RemoteOutput::Show` 变体与 JSON 凭据脱敏层（`redacted_remote_output`）。在线发现（`fetch::discover_remote_with_name`）与 `remote update` 未恢复，故 `queried` 恒为 `false`。
- 2026-06-06 `586231c0`（`feat(remote): add set-branches and set-head subcommands (#1392)`）：引入 set-branches / set-head 子命令。其内容曾被一次 reconcile 丢失，已于 2026-06-18 恢复到当前代码：`set-branches [--add]` 在单个 `ConfigKv` 事务内重写 `remote.<name>.fetch`，`set-head <branch>`/`-d`/`--delete` 写入/删除 `refs/remotes/<name>/HEAD`（`Head` 行），`--auto` 在 `validate_remote_usage` 中按 129 拒绝（deferred），新增 `RemoteError::RemoteTrackingBranchNotFound`。
- 2026-05-29 `a22d3b4b`（`fix(remote): guard ssh key namespace rename`）：实现修正：guard ssh key namespace rename；该节点把边界行为、错误处理或兼容差异纳入当前实现约束。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/remote.md`。
- Synopsis：`libra remote <subcommand> [OPTIONS] [ARGS]`。
- 公开参数/子命令包括：`add <name> <url>`、`remove <name>`、`rename <old> <new>`、`-v`（verbose 列表）、`show [-n/--no-query] [-v/--verbose] [<name>]`、`get-url [--push] [--all] <name>`、`set-url [--add] [--delete] [--push] [--all] <name> <value>`、`prune [--dry-run] <name>`、`set-branches [--add] <name> <branch>...`、`set-head [-a/--auto] [-d/--delete] <name> [<branch>]`。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 在线发现 | `remote show <name>` 的在线发现（通过 `fetch::discover_remote_with_name` 拉取远端 HEAD/ref 并对比本地跟踪分支，给出 `tracked`/`stale`/`new` 等状态）。当前 `run_show_remote` 仅离线，`queried` 恒为 `false`。 | 恢复在线路径需复用 fetch 的 discovery API，并补 `queried=true` 下的分支分类与测试。 |
| 子命令 | `remote update`（按 remote group 批量 fetch）。`RemoteCmds` 与 `RemoteOutput` 均无 `Update` 变体。 | 后续以新增测试、兼容矩阵或用户命令文档变更为准。 |
| Git flag | `set-head --auto`（自动探测远端 HEAD）。 | 在 `validate_remote_usage` 中按 129 拒绝并提示显式指定分支；恢复需远端 HEAD discovery。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
