# Revert 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `RevertError` typed enum 已落地，detached HEAD / invalid commit / conflict / object / index / HEAD 更新失败均有显式 `StableErrorCode`
- `run_revert()` + `render_revert_output()` 已完成执行层/渲染层拆分
- `RevertOutput` 已覆盖 `reverted_commit`、`short_reverted`、`new_commit`、`short_new`、`no_commit`、`files_changed`（`short_reverted` / `short_new` 是对应 commit hash 的 7 字符短形式，便于 human / agent 直接消费）
- `docs/commands/revert.md` 已记录 JSON schema、错误码和常用示例
- `tests/command/revert_test.rs` 已覆盖基础 revert、`--no-commit`、root commit、JSON 输出和错误码

### 基于当前代码的 Review 结论
- revert 的用户契约已经稳定：human 成功确认、JSON 结果模型、错误码和测试已一致
- 当前实现与命令文档对齐，没有发现与第四批其他命令的语义冲突
- 本轮 Review 的主要修订是清理计划文档陈旧描述，避免把已交付能力误写成“仍缺失”

## 目标与非目标

**已完成目标：**
- typed error、显式错误码、JSON / machine、run/render 分层、`--help` EXAMPLES 和回归测试已全部落地

**后续维护目标：**
- 继续维护 conflict、`--no-commit`、root commit 和 detached HEAD 回归
- 若未来支持 revert merge commit，应以新增参数/新 schema 字段的方式向后兼容扩展

**本批非目标：**
- 不支持 merge commit revert
- 不改变三方合并与反向补丁算法
- 不引入交互式冲突解决工作流

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test revert_test`
4. `docs/commands/revert.md` 与命令实际输出保持一致
