# `libra shortlog` 开发设计

## 命令实现目标

`libra shortlog` 的目标是按作者或提交者汇总提交历史。实现需要支持 committer grouping、`--group=author|committer|trailer:<key>`、no-merges、top 限制、mailmap、范围解析和 JSON 摘要，同时把 format、stdin 和更复杂过滤作为差异项。

## 对比 Git 与兼容性

- 兼容级别：`partial`。基础 author summary、email、count sorting、时间过滤、单 revision、`-c`/`--committer` 分组、`--group=author|committer|trailer:<key>`（按提交消息 trailer 值分组）、`--no-merges`、`--top`/`--min-count`/`--reverse`、`--author` 过滤已支持；`--format`、stdin 输入和 `-w` 换行宽度尚未公开。

- 当前矩阵承诺常用 Git 行为已支持；新增语义必须同步矩阵、用户文档和测试。


## 设计方案

- 入口与分发：已公开接入 `src/cli.rs::Commands`；已由 `src/command/mod.rs` 导出。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/shortlog.rs`。参数/子命令类型包括：`ShortlogArgs`；输出、错误或状态类型包括：源码未暴露独立公开输出/错误类型，错误通过 `CliResult` 统一传播；主要执行函数包括：`execute_to`、`execute`、`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；核心领域逻辑集中在 `execute_to`；对象路径会解析 revision 并读写 blob/tree/commit/tag 等对象；引用路径会读取或更新 SQLite refs、HEAD 与 reflog。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>src/cli.rs::Commands"] --> B["源码分层<br/>src/command/shortlog.rs"]
    B --> C["参数模型<br/>ShortlogArgs"]
    C --> D["执行路径<br/>execute_to / execute / execute_safe"]
    D --> E["底层对象<br/>Commit / Head / ObjectHash"]
    D --> F["输出与错误<br/>CliResult"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Commit`（提交对象、父提交关系和提交消息载荷）；`Head`（SQLite 中的 HEAD 指向、当前分支和 detached 状态）；`ObjectHash`（SHA-1/SHA-256 对象 ID 和 revision 解析结果）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2026-06-06 `cec72108`（`feat(shortlog): add -c/--committer grouping and --no-merges filter`）：新增 `-c`/`--committer`（按 committer 身份分组）与 `--no-merges`（聚合前剔除多父提交）。该内容曾在一次 reconcile 中从工作树丢失，已于 2026-06-18 依据原提交 diff 恢复（含端到端测试与文档）。
- 2026-06-10 `3b170290`（`feat(shortlog): add --top option (#382)`）：新增 `--top`/`--min-count`/`--reverse`（排序后限制/过滤/翻转输出）。同样曾被 reconcile 丢失，已于 2026-06-18 恢复。
- 2026-06-01 `1a7501da`（`test(shortlog): pin json revision summary`）：测试契约：pin json revision summary；相关行为已有回归守卫，后续变更需要继续满足。
- 历史结论：当前文档应以这些提交之后的代码、测试和兼容矩阵为准；更早的迁移式文档只保留为背景，不再作为事实来源。

## 当前状态

- 公开状态：已公开；模块状态：已导出。
- 用户文档：`docs/commands/shortlog.md`。
- Synopsis：`libra shortlog [<revision>] [-n] [-s] [-e] [--since <date>] [--until <date>]`。
- 公开参数/子命令包括：`-n, --numbered`、`-s, --summary`、`-e, --email`、`-c, --committer`、`--group <TYPE>`（`author`/`committer`/`trailer:<key>`）、`--no-merges`、`--top <N>`、`--min-count <N>`、`--reverse`、`--since <DATE>`、`--until <DATE>`、`--author <PATTERN>`、`[<revision>]`。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| ✅ 已实现 | 分组方式 `--group=author\|committer\|trailer:<key>` | `resolve_group_mode` 解析 `--group`（优先于 `-c`）；`trailer:<key>` 经 `extract_trailer_identities` 从消息末段 trailer 块按 key（忽略大小写）提取每个值作为分组（单提交可贡献 0..N 组）。带集成测试（`group_trailer_groups_by_trailer_value`）。 |
| 兼容差异项 | 格式化输出 | 原始对照：不支持；相关参数/替代：--format=<format>；当前说明：不适用。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| 兼容差异项 | 管道输入 | 原始对照：不支持；相关参数/替代：从 stdin 读取管道输入；当前说明：不适用。 后续实现时需要补对应回归测试并同步兼容矩阵。 |
| ✅ 已实现 | 作者过滤 `--author <PATTERN>` | 聚合前按作者 `name <email>` 的大小写不敏感子串过滤（即使配合 `-c` 也按作者过滤）。带 `author_identity_matches` 单元测试。 |
| 兼容差异项 | 换行宽度 | 原始对照：不支持；相关参数/替代：-w[<width>[,<indent1>[,<indent2>]]]；当前说明：`ShortlogArgs` 当前无 `width` 字段。 后续实现时需要补对应回归测试并同步兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
