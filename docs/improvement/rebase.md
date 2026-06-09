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
- replay 内部失败分类已细化为 `ReplayErrorKind`（14 个变体），通过 `ReplayResult::Internal { kind, detail }` 与新增的 `RebaseError::ReplayInternal { commit, subject, kind, detail }` 透传，映射到 4 个独立稳定错误码（`RepoCorrupt` 用于对象/parent 加载失败、`IoReadFailed` 用于 index 读取、`IoWriteFailed` 用于 tree/commit/index/workdir 写入、`ConflictOperationBlocked` 用于 untracked overwrite）；不再让真实合并冲突与内部 IO 失败共用 `LBR-CONFLICT-001`。
- `execute()` 已回收 legacy 内部路径；`execute` 与 `execute_safe` 共享同一运行路径，减少历史输出风格差异。
- replay 创建新 tree、重建 index 时会保留普通 blob、可执行 blob、symlink 的 mode；Unix 工作区写回会恢复可执行位与 symlink，非 fast-forward rebase 不再把可执行脚本改写成普通文件，也不会因 mode 丢失留下脏工作区。
- criss-cross 多 best merge-base 现在通过专用 `RebaseError::AmbiguousMergeBase` 返回 `LBR-CONFLICT-002` / 128，不再被降级成 commit-load/repo-corrupt 类错误；命令文档错误表已同步。
- `--onto <newbase> <upstream> [<branch>]` 已接入非交互 rebase：`<upstream>` 只定义 replay range，`<newbase>` 作为 replay 目标；给出 `<branch>` 时会先切到该分支再执行 rebase。
- 高阶非交互 rebase 能力已接入：`--autostash`/`rebase.autoStash`、`--autosquash`/`rebase.autoSquash`、`--reapply-cherry-picks`、`--root`（单根历史）、`--keep-empty`、`--empty=drop|keep|stop`、`--signoff`、vault-backed `-S`/`--gpg-sign`。

## 当前未完成

- 本计划范围内的项目已全部收口；结构化输出、JSON/machine envelope、稳定错误码、replay 内部失败分类、`--abort/--continue/--skip` 状态机均已落地（详见上面"当前已落地"列表）。
- interactive / exec / rebase-merges 等历史 git 功能列在下面"非目标"段，是本计划**显式不实现**的范围，不算未完成项。

## 后续切片建议

1. 已补齐：`rebase --continue` / `--abort` / `--skip` 的 machine/json 边界回归测试，覆盖成功路径与无状态错误路径，且 `--skip`/`--continue`/`--abort` 的 machine/json 输出边界均覆盖。

## 非目标

- 不实现 interactive rebase、exec、rebase-merges 或多 source rebase。
- `--root` 的多根历史仍不支持；`--empty=ask` 不支持；`-S=<keyid>` / `--gpg-sign=<keyid>` 不支持（Libra 仅使用 vault 单一签名密钥）。
- 不改变 SQLite rebase state schema，除非后续输出模型确实需要新增持久字段。
- 不在 structured-output 迁移期间改变现有 conflict-stop 用户流程。
