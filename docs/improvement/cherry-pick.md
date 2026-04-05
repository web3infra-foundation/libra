# Cherry-Pick 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `CherryPickError` typed enum 已落地，detached HEAD / invalid commit / multi-commit + `--no-commit` / conflict / object / index / HEAD 更新失败均有显式 `StableErrorCode`
- `run_cherry_pick()` + `render_cherry_pick_output()` 已完成执行层/渲染层拆分
- `CherryPickOutput` 已覆盖多 commit 结果列表和 `no_commit` 状态
- `docs/commands/cherry-pick.md` 已记录 JSON schema、错误码和常用示例
- `tests/command/cherry_pick_test.rs` 已覆盖单提交、多提交、`--no-commit`、错误路径和 JSON 输出

### 基于当前代码的 Review 结论
- 第四批对外契约已落地，`cherry-pick` 与 `revert` 在成功确认、错误码和 JSON 结构上保持一致
- 当前实现与命令文档、测试保持同步，没有发现跨命令冲突或重复实现带来的用户层面歧义
- 本轮 Review 的修订重点是把这份计划文档更新到当前实现状态，作为后续批次的可信基线

## 目标与非目标

**已完成目标：**
- typed error、显式错误码、JSON / machine、run/render 分层、`--help` EXAMPLES 和集成测试已全部落地

**后续维护目标：**
- 继续维护多 commit 顺序、冲突和 `--no-commit` 的回归测试
- 如未来增加 merge commit cherry-pick 支持，应新增明确参数和向后兼容字段

**本批非目标：**
- 不支持 merge commit cherry-pick
- 不引入交互式提交选择或冲突解决 UI
- 不改变现有三方合并算法

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test cherry_pick_test`
4. `docs/commands/cherry-pick.md` 与命令实际输出保持一致
