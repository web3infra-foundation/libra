# Describe 命令改进详细计划

## 所属批次

第六批：只读辅助命令收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `describe` 已补齐 `--always`
- 已新增 `DescribeOutput`，human / JSON 复用同一份执行结果
- 已为 revision 解析、无 tag、对象读取失败补齐显式 `StableErrorCode`
- `docs/commands/describe.md` 已记录输出契约
- `tests/command/describe_test.rs` 已覆盖 annotated exact tag、`--tags` 下的 lightweight tag、`--always` 和错误路径

### 基于当前代码的 Review 结论
- 旧实现只能接受原始 hash，和 Git 用户对 commit-ish 的习惯不一致；本轮已改为走统一 revision 解析
- 成功路径原先只有 human 输出；本轮已补 JSON，便于脚本和 agent 消费

## 目标与非目标

**已完成目标：**
- `--always`
- JSON / machine 输出
- 显式错误码
- `--help` EXAMPLES

**后续维护目标：**
- 继续观察多 tag 指向同一提交时的优先级是否需要进一步细化

**本批非目标：**
- 不引入 `--contains` / `--dirty`
- 不重写 tag 选择算法

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test describe_test`
4. `docs/commands/describe.md` 与实际输出保持一致
