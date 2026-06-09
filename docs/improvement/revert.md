# Revert 命令改进状态

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 当前代码状态

### 已落地能力

- 支持单提交、多提交和 `A..B` 范围回滚，范围按 newest-first 执行。
- 支持 detached `HEAD` 下创建 revert 提交，并直接前推 detached `HEAD`。
- 支持 merge commit revert 的 `-m` / `--mainline <parent-number>`。
- 支持单提交 `-n` / `--no-commit`；多提交或范围与 `-n` 组合会失败闭合。
- 支持 `-s` / `--signoff` trailer。
- 支持冲突 sequencer 控制：`--continue`、`--skip`、`--abort`、`--quit`。
- 进行中状态存储在 `.libra/libra.db` 的 `revert_sequence` 表，不使用 `.git/sequencer` 或文件式 `REVERT_HEAD`。
- root commit revert 使用 Git empty-tree 对象创建空树，避免空 `Tree::from_tree_items` 失败。
- 英文/中文命令文档、兼容矩阵、迁移 registry 文档已同步。

### 仍保留的 partial 差距

- `-e` / `--edit` 与 `--no-edit` 已接受，但还没有启动编辑器。
- 尚未支持 `--strategy`、`-X`、`-S` / `--gpg-sign`、`--cleanup`、`--commit`、`--rerere-autoupdate`、`--reference` 和 Git 的完整 `--no-*` 别名集合。
- 冲突检测仍是 path-level；不是 Git 的行级 hunk merge。

## 验证记录

已完成：

```bash
cargo +nightly fmt --all
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check
source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- revert --test-threads=1 --nocapture
```

`command_test -- revert` 当前结果：18 passed。

后续随总目标一起执行最终 gate：

```bash
cargo +nightly fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
source .env.test && cargo test --all
```
