# Open 命令改进详细计划

## 所属批次

第七批：轻量命令与底层契约收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- 已新增 `OpenOutput`，返回 `remote` / `remote_url` / `web_url`
- 已为无 remote、unsafe URL、浏览器启动失败补齐稳定错误码
- `docs/commands/open.md` 已补命令契约
- `tests/command/open_test.rs` 已补 JSON 和错误提示回归

### 基于当前代码的 Review 结论
- 旧实现只能输出 `"Opening ..."`，用户和脚本都看不到最终解析链路；本轮已显式暴露
- 无 remote 时旧实现缺少明确指导；本轮已补 `remote add origin` hint

## 目标与非目标

**已完成目标：**
- JSON / machine 输出
- 显式错误码
- remote 缺失 hint

**后续维护目标：**
- 如后续支持 forge-specific deep link，可在现有 `web_url` 上向后兼容扩展字段

**本批非目标：**
- 不检测浏览器是否真正完成打开
- 不引入 PR / issue deep-link 子模式

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test open_test`
4. `docs/commands/open.md` 与命令输出保持一致
