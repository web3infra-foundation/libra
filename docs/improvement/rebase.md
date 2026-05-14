# Rebase 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 32 批中 `rebase` 的当前状态。`merge` 已先完成 fast-forward-only 结构化输出收口；`rebase` 仍是更大的 legacy 状态机，需要分批迁移。

## 当前已落地

- CLI/preflight 入口 `execute_safe(args, output)` 已能对仓库缺失、缺少 upstream、无效 upstream、无 rebase in progress 等前置错误返回标准 `CliError`。
- `--abort` / `--continue` / `--skip` 无 rebase state 的 JSON 错误已显式返回 `LBR-REPO-003`，不再依赖字符串推断。
- 命令文档已同步当前 human 输出，包括 `Found common ancestor`、`Rebasing N commits`、`Applied:`、conflict 提示、abort 恢复消息和 fast-forward 消息。
- 创建树和重置工作区时的路径处理不再使用生产 `unwrap()`；空路径和非 UTF-8 路径会返回带上下文的错误。

## 当前未完成

- `execute_safe()` 仍委托 legacy `execute(args).await`，深层运行时错误会直接 `println!` / `eprintln!` 后 `return`。
- 成功路径尚无 `RebaseOutput` / `render_rebase_output()`，`--json` / `--machine` 成功输出未落地。
- replay、conflict、continue、abort、skip 的运行时错误尚未统一建模为 `RebaseError`。
- 部分 legacy 运行时失败仍可能只打印错误文本而不把失败状态传回 CLI 边界。

## 后续切片建议

1. 把 `rebase_abort()` 提升为 `run_rebase_abort() -> Result<RebaseOutput, RebaseError>`，先收口 abort 成功和失败路径。
2. 再拆 `rebase_continue()` 与 `rebase_skip()`，把 stopped commit、resolved index、empty todo 等状态纳入 typed result。
3. 最后拆 `start_rebase()` / `continue_replay()`，落地完整 `RebaseOutput`、conflict result 和 JSON/machine 成功输出。

## 非目标

- 不实现 interactive rebase、`--onto`、autosquash、exec、rebase-merges 或多 source rebase。
- 不改变 SQLite rebase state schema，除非后续输出模型确实需要新增持久字段。
- 不在 structured-output 迁移期间改变现有 conflict-stop 用户流程。
