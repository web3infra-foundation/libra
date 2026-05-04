# Stash 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `StashError` typed enum 已落地（stash.rs:51），含 `NotInRepo` / `NoInitialCommit` / `NoStashFound` / `InvalidStashRef` / `StashNotExist` / `MergeConflict` / `ReadObject` / `WriteObject` / `IndexSave` / `ResetFailed` / `Other` 共 11 变体，每个变体在 stash.rs:89-99 有显式 `StableErrorCode` 映射
- `run_stash()` + `render_stash_output()` 已完成执行层/渲染层拆分（stash.rs:182 / stash.rs:356），human / JSON / machine 共用一套结果模型
- `StashOutput` 是 `#[serde(tag = "action")]` enum（stash.rs:134），已覆盖 `push` / `pop` / `apply` / `drop` / `list` / `noop`；list 项使用 `StashListEntry` 结构体（index / message / stash_id）
- `STASH_EXAMPLES` 常量已定义（stash.rs:3）并通过 cli.rs:201-205 的 `#[command(after_help = command::stash::STASH_EXAMPLES)]` 接入，`libra stash --help` 末尾会显示 7 条示例
- `docs/commands/stash.md` 已记录 JSON schema、错误码和常用示例
- `tests/command/stash_test.rs` 已覆盖 push/pop/list/apply/drop、JSON 输出、错误码和仓库外调用（12 个 `#[test]` / `#[tokio::test]`）

### 基于当前代码的 Review 结论
- 第四批对外契约已落地，代码、测试和命令文档已对齐
- 成功路径不再沉默：human 模式下会输出明确确认信息，JSON / machine 模式返回稳定 envelope
- 本轮 Review 主要修订点不在 stash 代码，而在把本计划文档从“改造前草案”收口为“已落地现状”，避免后续批次误判基线

## 目标与非目标

**已完成目标：**
- typed error、显式错误码、run/render 分层、JSON / machine、`--help` EXAMPLES 和集成测试已全部落地

**后续维护目标：**
- 继续维护冲突、空 stash、list schema 和 no-op 场景的回归测试
- 如后续要补 `stash branch` 等扩展能力，应以新增 action 的方式向后兼容扩展现有 JSON schema

**本批非目标：**
- 不改变 stash object/reflog 存储模型
- 不引入交互式 stash 选择器或 TUI
- 不调整现有 apply/pop 合并算法

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test stash_test`
4. `docs/commands/stash.md` 与命令实际输出保持一致
