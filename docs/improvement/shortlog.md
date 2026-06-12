# Shortlog 命令改进详细计划

## 所属批次

第六批：只读辅助命令收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- 已补 revision 位置参数，默认仍为 `HEAD`
- 已新增 `ShortlogOutput` / `ShortlogAuthor`
- human / JSON 共用同一份聚合结果
- invalid date、invalid revision、unborn HEAD 都已接入稳定错误码
- signed commit 现在会跳过 `gpgsig` 头并提取真实 subject
- `docs/commands/shortlog.md` 已补命令契约
- `tests/command/shortlog_test.rs` 已补 JSON 输出回归，并覆盖 revision 过滤

### 基于当前代码的 Review 结论
- 旧实现只能固定读取 HEAD，和 `log` / `show` 的 commit-ish 习惯不一致；本轮已收口
- 旧实现虽然有较多逻辑测试，但没有 JSON 契约测试；本轮已补
- 旧实现直接读取 `Commit.message` 第一行，遇到签名提交时会把 `gpgsig` 头误当成 subject；本轮已改为复用统一消息解析

## 目标与非目标

**已完成目标：**
- revision 位置参数
- JSON / machine 输出
- 显式错误码收口
- signed commit subject 解析与 human / JSON 输出对齐

**后续维护目标：**
- 补充 revision 过滤和 `-s` / `-e` 组合的 CLI 级 JSON 回归

**本批非目标：**
- 不引入 pathspec 过滤
- 不补 Git `shortlog` 的全部 trailer/group 模式

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test shortlog_test`
4. `docs/commands/shortlog.md` 与命令输出保持一致
