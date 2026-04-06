# Index-Pack 命令改进详细计划

## 所属批次

第七批：轻量命令与底层契约收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- 已新增 `IndexPackOutput`
- 已补 `--json` / `--machine`
- `.pack` 校验、相同输入输出路径、缺失 pack、非法版本、pack 损坏均已绑定稳定错误码
- `docs/commands/index-pack.md` 已补命令契约
- `tests/command/index_pack_test.rs` 已补 JSON 输出回归

### 基于当前代码的 Review 结论
- 旧实现更像内部工具，成功和失败都不利于脚本消费；本轮已统一到标准 envelope

## 目标与非目标

**已完成目标：**
- JSON / machine 输出
- 显式错误码
- `--help` EXAMPLES

**后续维护目标：**
- 如后续需要 bytes / object count 指标，可在现有 schema 上追加字段

**本批非目标：**
- 不增加 pack 校验或 `verify-pack` 语义
- 不改动 v1/v2 索引生成算法

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test index_pack_test`
4. `docs/commands/index-pack.md` 与命令输出保持一致
