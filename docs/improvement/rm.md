# Rm 命令改进状态

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 当前代码状态

### 已落地能力

- 人类输出显式尊重 `OutputConfig.color`：非 TTY / `--no-color` 输出纯文本 `rm '<path>'`，`--color=always` 才输出 ANSI。
- `--machine` / `--json` 走结构化 envelope，不输出人类 `rm '<path>'` 行。
- 冲突文件列表使用四空格缩进，不使用 tab。
- uncommitted-change 拒绝显式归类为 `LBR-WARN-001`，粗粒度退出码为 9。
- `--dry-run` 不保存 index、不删除工作树文件。
- 真实删除顺序为：内存 index 移除 -> 保存 `.libra/index` -> 删除工作树文件。
- index 保存失败在删除磁盘文件前返回 `LBR-IO-002`。
- 工作树删除失败会聚合为单个 warning，并在结构化 stderr 中提供 `details.failed_paths`。

### 保留差距

- `--sparse` 仍未暴露，继续作为 unsupported/declined 兼容差距。
- Git 的 `-n` 短别名仍未映射到 `--dry-run`。
- 现存 `to_string_or_panic()` 属既有路径，本轮未扩大 panic 面。

## 验证记录（2026-06-08）

本轮新增/更新的重点测试位于 `tests/command/remove_test.rs`，已完成以下切片验证：

```bash
cargo +nightly fmt --all --check
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- remove --test-threads=1 --nocapture
source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan
source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.clean-rm-mv-lfs-basic
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_help_flag_descriptions --test compat_help_examples_banner --test compat_command_docs_examples_section --test compat_matrix_alignment --test compat_all_production_unwrap_guard --test compat_extra_production_unwrap_guard -- --nocapture
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings
```

补充人工检查：

```bash
rg -n "\.(unwrap|expect)\(" src/command/remove.rs tests/command/remove_test.rs
```

该检查未在 `src/command/remove.rs` 命中生产代码新增 `unwrap()` / `expect()`；命中项均位于测试文件。
