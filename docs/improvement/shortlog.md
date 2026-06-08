# Shortlog 命令改进详细计划

## 所属批次

第六批：只读辅助命令收口（P2）

## 已完成前置条件与当前代码状态

### 2026-06-08 追加落地
- 已支持 `A..B` 双点范围差集，含 `A..` / `..B` 隐式 `HEAD`
- 已支持 `-c` / `--committer` 与 `--no-merges`
- 已支持 `-w` human subject 换行；自定义值使用 `-w=<spec>`
- 已支持受限 `--format`：`%s`、`%h`、`%H`、`%an`、`%ae`、`%cn`、`%ce`、`%%`
- 已支持仓库根 `.mailmap` 映射，坏行/过长行 warning，symlink `.mailmap` 拒绝读取
- 已将实现拆为 `shortlog/format.rs`、`mailmap.rs`、`range.rs`、`render.rs`、`wrap.rs`
- 已同步 `docs/commands/shortlog.md`、`docs/commands/zh-CN/shortlog.md`、`COMPATIBILITY.md`

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
- `A..B` 范围
- JSON / machine 输出
- 显式错误码收口
- signed commit subject 解析与 human / JSON 输出对齐
- `-c` / `--committer`、`--no-merges`、`-w`、`--format`、根 `.mailmap`

**本批非目标：**
- 不引入 pathspec 过滤
- 不补 Git `shortlog` 的全部 trailer/group 模式
- 不支持 stdin log 解析、`--all` / `--branches` / `--tags`、三点范围、`^ref`、`mailmap.file` / `mailmap.blob`

## 验证记录（2026-06-08）

```bash
cargo +nightly fmt --all --check
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- shortlog --test-threads=1 --nocapture
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --lib shortlog -- --test-threads=1 --nocapture
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_help_flag_descriptions --test compat_help_examples_banner --test compat_command_docs_examples_section --test compat_matrix_alignment --test compat_all_production_unwrap_guard --test compat_extra_production_unwrap_guard -- --nocapture
source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan
source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.grep-blame-describe-shortlog
rg -n "\.(unwrap|expect)\(" src/command/shortlog.rs src/command/shortlog
```

`rg` 仅命中 shortlog 测试模块，生产路径没有新增 `unwrap()` / `expect()`。
