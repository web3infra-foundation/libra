# Clean 命令改进详细计划

## 所属批次

第六批：只读辅助命令收口（P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `clean` 已新增 `CleanOutput`
- `-n` / `-f` 共享同一份结构化结果，JSON 输出已落地
- 缺少模式、索引读取失败、删除失败、越界删除保护都已绑定稳定错误码
- `docs/commands/clean.md` 已补命令契约
- `tests/command/clean_test.rs` 已补 `--json` dry-run 回归

### 基于当前代码的 Review 结论
- 旧实现 force 成功时静默，不利于用户确认；本轮补齐明确确认消息
- 旧实现缺少结构化输出，脚本只能解析 stdout 文本；本轮已收口

## 目标与非目标

**已完成目标：**
- JSON / machine 输出
- 显式错误码
- human 成功确认消息

**后续维护目标：**
- 继续维护 symlink / 权限拒绝 / 缺失索引回归

**本批非目标：**
- 不引入 `-d` / `-x` / `-X`
- 不清理目录树和 ignored 文件的 Git 全量兼容选项

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test clean_test`
4. `docs/commands/clean.md` 与命令输出保持一致
