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

**本次已完成（本批）：**
- 完成 `cat-file` JSON/AI 查询路径的稳定错误码收口，覆盖：
  - `LBR-CLI-003`（无效对象/类型）
  - `LBR-CLI-002`（参数冲突）
  - `LBR-IO-001`（对象读取失败）
  - `LBR-REPO-002`（对象类型不一致/对象体异常）
- 新增回归测试：
  - 无效对象名（含 `--json`）
  - `--ai-list` 非法类型（含 `--json`）
  - 损坏对象体的 JSON pretty-print 读取失败

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
