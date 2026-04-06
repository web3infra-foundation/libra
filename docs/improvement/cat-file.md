# Cat-File 命令改进详细计划

## 所属批次

第八批：底层命令契约收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `cat-file` 在本轮前已具备大部分 JSON 输出能力
- `docs/commands/cat-file.md` 已补命令契约与 JSON 说明
- `tests/command/cat_file_test.rs` 已补 JSON type 模式回归

### 基于当前代码的 Review 结论
- 当前主要缺口不在核心实现，而在“契约是否清晰、测试是否覆盖”；本轮已把文档和测试补齐

## 目标与非目标

**已完成目标：**
- 命令文档同步
- JSON 契约测试
- README 计划状态同步

**后续维护目标：**
- 后续继续把剩余 legacy 错误路径收口到更一致的稳定错误码

**本批非目标：**
- 不重写 `cat-file` 内部 object/AI 双通道实现
- 不让 `-e` 支持 JSON

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test cat_file_test`
4. `docs/commands/cat-file.md` 与命令输出保持一致
