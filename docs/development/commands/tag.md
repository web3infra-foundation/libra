# `libra tag` 开发设计

## 命令实现目标

`libra tag` 的目标是创建、列出、过滤和删除标签。实现需要支持 force、`-n` 展示、annotated tag message 和轻量标签路径，同时把 Git-style `-a` / `--annotate`、`--points-at`、签名与验证等后续能力列为缺口。

## 对比 Git 与兼容性

- 兼容级别：`supported`。

- 当前矩阵承诺常用 Git 行为已支持；新增语义必须同步矩阵、用户文档和测试。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/tag.rs`。参数/子命令类型包括：`TagArgs`；输出、错误或状态类型包括：`TagOutput`、`TagListEntry`、`TagError`（crate-private 错误枚举）；主要执行函数包括：`execute`、`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；创建路径经 `tag::create` 解析 HEAD 提交并写入轻量或附注标签对象；引用路径会读取或更新 SQLite refs（创建/删除标签 ref，不写 reflog，不解析 remote/网络）；数据库路径会通过 SeaORM/SQLite 持久化标签引用。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/tag.rs"]
    B --> C["参数模型<br/>TagArgs"]
    C --> D["执行路径<br/>execute / execute_safe"]
    D --> E["底层对象<br/>Blob / Commit / Tree / Branch"]
    D --> F["输出与错误<br/>TagOutput / TagListEntry / TagError"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Blob`（标签指向的 blob 对象，列表展示时读取其 ID）；`Commit`（标签指向的提交对象及提交消息载荷）；`Tree`（标签指向的目录树对象）；`Branch` / branch store（`branch::BranchStoreError`：解析 HEAD 提交时的错误来源）；SeaORM / `.libra/libra.db`（`DbErr`：读写 refs 等 SQLite 表的错误来源）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：tag 仅写入对象库（附注标签对象）和 SQLite refs（标签引用），不触及索引、reflog、D1、工作树或远端；写入前必须先完成参数校验（如删除/创建前的 name 校验），再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2025-10-02 `3879a44a`（`feat: add argument -f/--force for tag command`）：基础实现节点：add argument -f/--force for tag command；当前实现的主要轮廓可追溯到该提交。
- 2026-06-07 `8fecc10d`（`feat(tag): add -a/--annotate flag requiring a message (v0.17.1409)`）：历史资料中曾记录 `-a/--annotate`，但当前 `TagArgs` 未公开该 flag；当前事实以源码为准。
- 2026-06-06 `58b0cc16`（`feat(tag): add --points-at list filter (v0.17.1406)`）：历史资料中曾记录 `--points-at`，但当前 `TagArgs` 未公开该 flag；当前事实以源码为准。
- 2026-05-18 `b534c401`（`fix(commit,stash,index-pack,tag): restore Issues URL on internal-invariant paths`）：实现修正：restore Issues URL on internal-invariant paths；该节点把边界行为、错误处理或兼容差异纳入当前实现约束。
- 2026-05-16 `fff9cbb0`（`test(tag): pin Display for 5 static-message TagError variants (v0.17.292)`）：测试契约：pin Display for 5 static-message TagError variants (v0.17.292)；相关行为已有回归守卫，后续变更需要继续满足。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/tag.md`。
- Synopsis：`libra tag [OPTIONS] [-l | -d | -f] [-m <MESSAGE>] [-n <N_LINES>] [NAME]`。
- 公开参数/子命令包括：`-l, --list`、`-d, --delete`、`-m, --message <MESSAGE>`、`-f, --force`、`-n, --n-lines <N_LINES>`、`[NAME]`。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容差异项 | Git-style 显式附注标签 flag | 原始对照：Git 的 annotate flag 加消息创建路径；相关参数/替代：当前 message-based 创建路径已创建 annotated tag，但 `-a` / `--annotate` 未公开。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 按对象过滤标签 | 原始对照：git tag --points-at <object>；相关参数/替代：不支持；当前说明：不适用。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 签名标签 | 原始对照：git tag -s <name>；相关参数/替代：不支持 (vault-based planned)；当前说明：不适用。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 验证标签 | 原始对照：git tag -v <name>；相关参数/替代：不支持 (vault-based planned)；当前说明：不适用。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 按包含提交过滤 | 原始对照：git tag --contains <commit>；相关参数/替代：当前 `TagArgs` 未公开 `--contains`；当前说明：不支持。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 按合并状态过滤 | 原始对照：git tag --merged / --no-merged；相关参数/替代：当前 `TagArgs` 未公开 `--merged` / `--no-merged`；当前说明：不支持。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 排序输出 | 原始对照：git tag --sort=<key>；相关参数/替代：当前 `TagArgs` 未公开 `--sort`；当前说明：不支持。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 多列输出 | 原始对照：git tag --column；相关参数/替代：当前 `TagArgs` 未公开 `--column`；当前说明：不支持。 后续实现时需要补对应回归测试并同步兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
