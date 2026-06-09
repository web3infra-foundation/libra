# Restore 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 落地状态（2026-06-08）

`restore` 的 Git 兼容核心能力已落地，当前命令整体在 `COMPATIBILITY.md` 中保持 `partial`：常用恢复、冲突阶段恢复、overlay 和 pathspec 文件输入已支持；`-p`/`--patch`、`--conflict=zdiff3`、非 NUL pathspec 文件的 Git C-quoting / `core.quotePath` 解码，以及 restore-time `core.autocrlf` renormalization 仍列为 deferred / intentional difference。

### 已完成能力

- `RestoreArgs` 增加 `--ours` / `-2`、`--theirs` / `-3`、`--merge`、`--conflict=<merge|diff3>`、`--ignore-unmerged`、`--overlay`、`--no-overlay`、`--pathspec-from-file`、`--pathspec-file-nul`。
- 默认 restore 遇 unmerged path 会以 `LBR-CONFLICT-001`、exit 128 阻断；`--ignore-unmerged` 跳过该路径。
- `--ours` / `--theirs` 从 index stage 2 / 3 写回工作区，index 保持 unmerged。
- `--merge` / `--conflict=diff3` 复用 `merge.rs` 的冲突样式解析和冲突标记渲染 helper，重建工作区冲突标记，index 保持 unmerged。
- `--merge` / `--conflict` 对二进制或超过 50 MiB 的冲突 blob fail closed，并建议改用 `--ours` / `--theirs`。
- 默认 no-overlay 会删除目标中已追踪但 source 中不存在的文件；`--overlay` 保留这些文件。
- `--pathspec-from-file=<file>` 和 `--pathspec-from-file=-` 支持 newline / NUL 分隔，并以 128 MiB 上限防止无界读取。
- `docs/commands/restore.md`、`COMPATIBILITY.md`、`docs/development/integration-scenarios.yaml`、`docs/development/integration-scenarios/cli.restore-reset-diff.md` 和 integration-runner 场景已同步。

### 关键保护

- 冲突选择参数只写工作区，不改 index，不创建或推进 merge 状态。
- no-overlay 删除前确认路径是 index stage 0 中的 tracked 文件，不删除未追踪文件。
- pathspec 文件解析结果仍走既有 pathspec 归一化和匹配路径。
- 生产 `src/**` 路径没有新增 `unwrap()` / `expect()`。

## 已验证命令

- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- restore --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --lib restore -- --test-threads=1 --nocapture`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.restore-reset-diff`

## 后续维护点

- 如实现 `-p` / `--patch`，需要引入统一交互式 hunk 框架，而不是在 `restore` 内单独实现一套。
- 如扩展 `--conflict=zdiff3`，应先扩展共享的 `MergeConflictStyle`，让 `merge` 与 `restore` 同步使用同一渲染路径。
- 如实现 Git C-quoting / `core.quotePath`，应优先抽取为共享 pathspec-file parser，供 `add` / `remove` / `restore` 等命令复用。
