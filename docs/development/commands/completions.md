# completions 命令开发设计

## 命令实现目标

`libra completions <shell>` 为 `libra` CLI 生成 shell 补全脚本，对齐 Lore
`completions` 命令与 `git completion` contrib 脚本的人体工学（`lore.md` §3.1
Phase 0 编号 0.1）。脚本由 clap 命令树实时生成，始终跟随真实子命令/参数面，无需
手工维护补全表。纯 CLI 元命令，不读写仓库状态、不需要仓库。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。Git 无 `git completions` 子命令（补全脚本
  在 `contrib/completion` 手动 source）；Libra 直接把生成器暴露为一级命令。
- 已支持：`completions <shell>`，`<shell>` ∈ `bash|zsh|fish|powershell|elvish`
  （`clap_complete::Shell`）；`--json`/`--machine` 输出 `{ shell, script }`。
- 未知/缺省 shell → clap usage error（Git 风格退出码 129）。

## 设计方案

- 入口与分发：`src/cli.rs::Commands::Completions` → `command::completions::execute_safe`。
  分发处传入 `Cli::command()`（clap 根命令），使 completions 模块与私有 `Cli`
  结构体解耦。
- preflight：列入 `command_preflight` 的 `CommandPreflight::none()` 组——不做
  hash-kind preflight、不需要仓库。
- 源码分层：`src/command/completions.rs`：`CompletionsArgs`（`shell: Shell`，
  `value_enum`）、`CompletionsOutput`（`shell`/`script`）、`execute_safe`。
- 生成：`clap_complete::generate(shell, &mut cmd, "libra", &mut buf)`；bin name
  恒为 `libra`（与 crate 名无关）；脚本按 UTF-8 lossy 转换。
- 输出契约：默认把脚本原样写 stdout（不加/删尾换行，便于重定向或 `eval`）；
  `--json`/`--machine` 走 `emit_json_data("completions", { shell, script })`。
- 底层操作对象：无。不触碰对象库/index/refs/SQLite/网络。

## 实现历史

- 2026-07-02（`lore.md` Phase 0 / 0.1）：Lore→Libra 能力差距补齐计划首个落地项；
  新增 `clap_complete` 依赖；独立 PR。

## 当前状态

- 公开状态：已公开（`Commands::Completions`）。
- 依赖：新增 `clap_complete = "4.6.6"`（与 `clap 4.6` 对齐）。
- 测试：`tests/command/completions_test.rs`（各 shell 生成非空脚本、bash 脚本
  含 `libra`、`--json` 信封、未知 shell 退出码 129、仓库外可运行）+
  `completions.rs` 单测（bash 脚本含 binary、五种 shell 均非空）。
- 用户文档：`docs/commands/completions.md`。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| shell 覆盖 | nushell 等 clap_complete 生态外 shell | 仅覆盖 clap_complete 内置五种；有需求再评估 `clap_complete_nushell`。 |
| 动态补全 | 运行时值补全（分支名/远端名等） | 延后；当前为静态语法补全，与 Git contrib 脚本一致。 |

## 维护要求

- 改进本命令前先阅读 [docs/development/commands/_general.md](_general.md)。
- 新增/改名任何子命令或参数会自动反映到补全脚本；无需手工同步补全表。
- 分发处必须传入 `Cli::command()`，不得在模块内重建命令树。
