# `libra ls-files` 开发设计

## 命令实现目标

`libra ls-files` 的目标是列出索引中的文件条目，服务于 Git index listing 兼容面。当前实现资料存在但顶层 CLI 尚未公开接入，后续需要决定公开入口、pathspec、ignored mode、EOL 和 sparse checkout 等能力的落地范围。

## 对比 Git 与兼容性

- 兼容级别：`unpublished`。未进入 COMPATIBILITY.md；以代码接入状态为准。

- 该资料未对应公开 CLI 命令；用户可见状态按未发布处理。


## 设计方案

- 入口与分发：源码文件存在但尚未公开接入 `src/cli.rs::Commands`；`src/command/ls_files.rs` 在 `src/command/mod.rs`（及 `src/` 任何位置）都没有 `mod ls_files` 声明，因此该文件未被编译进二进制。CLI 层在 `src/cli.rs` 把解析后的参数交给命令模块，命令模块负责把领域错误转换为 `CliError` / `CliResult`。
- 源码分层：主要实现文件为 `src/command/ls_files.rs`。参数/子命令类型包括：`LsFilesArgs`；输出、错误或状态类型包括：`FileEntry`；主要执行函数包括：`execute`、`execute_safe`。
- 执行路径：`execute_safe` 负责 CLI 安全包装、错误映射和输出配置；索引路径只读地加载 `.libra/index`（`Index::load`）并按筛选项收集条目；判断 modified 时对工作树文件内容计算 `Blob` id 与索引哈希比较，全程不解析 revision，也不读写 blob/tree/commit/tag 对象库或刷新/保存索引。

- 流程图：以下流程图按当前源码分层展示主路径和底层对象边界，便于维护者把代码入口、执行函数和副作用范围对应起来。

```mermaid
flowchart TD
    A["入口与分发<br/>未公开 CLI / 设计资料"] --> B["源码分层<br/>src/command/ls_files.rs"]
    B --> C["参数模型<br/>LsFilesArgs"]
    C --> D["执行路径<br/>execute / execute_safe"]
    D --> E["底层对象<br/>Index / .libra/index / Blob"]
    D --> F["输出与错误<br/>FileEntry"]
    E --> G["副作用边界<br/>写入分支需先预检"]
```

- 底层操作对象：`Index` / `.libra/index`（暂存区状态、路径条目和刷新/保存边界）；`Blob`（文件内容或 LFS pointer 写入对象库后的 blob 对象）
- 输出与错误契约：人类输出、`--json` / `--machine` 输出和 quiet/verbose 分支必须继续走现有 `OutputConfig` / `emit_json_data` / `CliError` 路径；新增失败模式要补稳定错误码、用户提示和回归测试。
- 副作用边界：凡是写入索引、对象库、refs/HEAD、reflog、SQLite/D1、工作树或远端的路径，都必须先完成参数校验和 dry-run/预检分支，再执行持久化，避免部分写入后静默成功。

## 实现历史

- 本节依据本地 main 分支提交历史重写，筛选与该命令实现、测试或文档路径直接相关的提交；以下是归纳后的实现脉络。
- 2026-06-13 `8d4fb969`（`Implement ref and index listing commands`）：基础实现节点：Implement ref and index listing commands；当前实现的主要轮廓可追溯到该提交。
- 历史结论：`src/command/ls_files.rs` 或配套测试/文档已有历史节点，但当前 `src/cli.rs::Commands` 未公开 `ls-files` 入口；实现历史不改变当前状态章节中的未接入结论。

## 当前状态

- 公开状态：未公开；模块状态：`src/command/ls_files.rs` 源码文件存在，但 `src/` 中没有任何 `mod ls_files` 声明，因此未被编译进二进制。
- 用户文档：`docs/commands/ls-files.md`。
- 公开参数/子命令以用户文档和 CLI help 为准；当前未抽取到独立 Options/Subcommands 小节。


## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| 兼容矩阵 | `COMPATIBILITY.md` 尚未登记该命令行。 | 需要决定是否纳入用户可见兼容矩阵和矩阵守卫。 |
| CLI 接入 | `src/cli.rs::Commands` 尚未公开该顶层命令。 | 需要决定接入 CLI、降级为内部设计资料，或移出用户命令文档。 |
| 功能缺口 | ls-files is 部分支持. Deferred Git flags include pathspecs, -z, --error-unmatch, ignored-mode variants, explicit exclude sources, --eol, resolve-undo, killed/debug output, and sparse-checkout integration. | 后续实现时需要同步源码、测试和兼容矩阵。 |

## 维护要求

- 改进本命令前，必须先阅读并遵循 [docs/development/commands/_general.md](_general.md)；这是命令设计、实现、测试和文档同步的强制要求。
- 任何行为变更都要先核对实现源码，再同步 `COMPATIBILITY.md`、`docs/commands/<cmd>.md` 和相关测试。
- 新增 Git 兼容参数时必须明确 tier、错误码、JSON/机器输出契约和回归测试。
- 若决定发布该命令，最小闭环是：CLI 变体、`src/command/mod.rs` 导出、dispatch、用户文档、兼容矩阵和测试。
