# Rebase 命令改进详细计划

> 最后编写时间：2026-05-15

本文记录第 32 批中 `rebase` 的当前状态。`merge` 已先完成 fast-forward-only 结构化输出收口；`rebase` 仍是更大的 legacy 状态机，需要分批迁移。

## 当前已落地

- CLI/preflight 入口 `execute_safe(args, output)` 已能对仓库缺失、缺少 upstream、无效 upstream、无 rebase in progress 等前置错误返回标准 `CliError`。
- `--abort` / `--continue` / `--skip` 无 rebase state 的 JSON 错误已显式返回 `LBR-REPO-003`，不再依赖字符串推断。
- `--abort` 成功路径已拆出 `RebaseOutput` / `render_rebase_output()`，支持 `--json` / `--machine` 输出恢复的分支和 commit。
- `--continue` / `--skip` 已拆出 typed result，支持成功 `--json` / `--machine` 输出；未解决冲突的 `--continue` 返回 `LBR-CONFLICT-001`，不再混入 human stdout。
- `rebase <upstream>` 结构化路径已覆盖完整 replay、fast-forward、already-up-to-date 和 conflict-stop 错误 envelope；`--json` / `--machine` 不再混入 legacy human stdout。
- `rebase <upstream>` 的 CLI human 路径已改为共享 `run_rebase_start()` / `render_rebase_output()`，成功输出与 JSON/machine 结果来自同一 runner；conflict-stop 通过标准 `CliError` 返回非零退出码。
- 命令文档已同步当前 human 输出，包括 `Found common ancestor`、`Rebasing N commits`、`Applied:`、conflict 提示、abort 恢复消息和 fast-forward 消息。
- 创建树和重置工作区时的路径处理不再使用生产 `unwrap()`；空路径和非 UTF-8 路径会返回带上下文的错误。

## 当前未完成

- replay/conflict-stop 的错误分类仍是初步 typed envelope，尚未细分到所有 replay 内部失败。
- 直接调用 legacy `execute(args).await` 的内部/测试路径仍保留旧打印行为；公开 CLI 入口已走 `execute_safe()`。

## 后续切片建议

1. 细化 replay/conflict-stop 的完整 typed result 和 JSON/machine 错误细节。
2. 收口或下沉 legacy `execute(args).await` 直接调用点，最终让内部调用也返回 `CliResult`。

## 非目标

- 不实现 interactive rebase、`--onto`、autosquash、exec、rebase-merges 或多 source rebase。
- 不改变 SQLite rebase state schema，除非后续输出模型确实需要新增持久字段。
- 不在 structured-output 迁移期间改变现有 conflict-stop 用户流程。
