# Show-Ref 命令改进详细计划

## 所属批次

第七批：轻量命令与底层契约收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `show-ref` 已具备 JSON / machine 输出
- 主要 refs 读取错误已绑定稳定错误码
- `docs/commands/show-ref.md` 已补命令契约
- `tests/command/show_ref_test.rs` 已补 JSON 输出回归

### 基于当前代码的 Review 结论
- 实现层在本轮前已基本完成，缺口主要在命令文档和 JSON 回归测试；本轮已补齐

## 目标与非目标

**已完成目标：**
- 命令文档同步
- JSON 回归测试
- README 计划状态同步

**后续维护目标：**
- 后续如补 remote-tracking refs，应以向后兼容方式扩展 `entries`

**本批非目标：**
- 不引入 Git `show-ref --verify` / `--exclude-existing`

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test show_ref_test`
4. `docs/commands/show-ref.md` 与命令输出保持一致
