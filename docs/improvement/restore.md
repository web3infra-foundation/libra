# Restore 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `RestoreError` typed enum（restore.rs:52）含 9 变体（`ResolveSource` / `ReferenceNotCommit` / `PathspecNotMatched` / `ReadIndex` / `ReadObject` / `ReadWorktree` / `InvalidPathEncoding` / `WriteWorktree` / `LfsDownload`）；每个变体在 restore.rs:76-84 有显式 `StableErrorCode` 映射，覆盖 source / pathspec / index / object / LFS 失败分类
- `run_restore()`（restore.rs:173）+ `render_restore_output()`（restore.rs:231）已完成执行层/渲染层拆分
- `RestoreOutput`（restore.rs:118）已覆盖 `source`、`worktree`、`staged`、`restored_files`、`deleted_files`
- `checkout` 兼容路径已复用 typed restore API（`execute_checked_typed` at restore.rs:512 返回 `Result<(), RestoreError>`），而不是继续走裸 `io::Error`
- `docs/commands/restore.md` 已记录 JSON schema、错误码和常用示例
- `tests/command/restore_test.rs` 现含 8 个 test，覆盖正向与错误两侧：
  - 正向：`test_restore_worktree_overwrites_modification_with_committed_blob`（worktree restore + 确认消息断言）、`test_restore_staged_resets_index_entry_to_head`（staged restore + status 二次校验）、`test_restore_json_envelope_reports_restored_files`（`--json` envelope schema 与字段断言）、`test_restore_quiet_suppresses_confirmation_but_still_restores`（`--quiet` 静默语义）
  - 错误路径：仓库外调用、unborn HEAD、pathspec 缺失、unborn 分支不回退到 hash 前缀

### 基于当前代码的 Review 结论
- 第四批对外契约已落地，restore 的 human/JSON 行为与命令文档保持一致
- `checkout` 与 `restore` 的 typed 边界已经对齐，减少了跨命令委托时的错误信息丢失
- 本轮 Review 的主要修订是把本计划文档从“待实施”状态更新为“已实施 + 后续维护点”，避免和当前代码冲突

## 目标与非目标

**已完成目标：**
- 显式错误码、JSON / machine、确认消息、run/render 分层和正向 restore 测试已全部落地

**后续维护目标：**
- 继续维护 worktree + staged 组合、pathspec 过滤和 LFS 下载失败的回归测试
- 继续保持 `checkout` 兼容层与 `restore` 真实行为一致

**本批非目标：**
- 不引入交互式 restore
- 不改变底层 tree/index 恢复算法
- 不为 `checkout` 提前承诺完整 JSON 契约（完整现代化仍留第六批）

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test restore_test`
4. `docs/commands/restore.md` 与命令输出、错误码保持一致
