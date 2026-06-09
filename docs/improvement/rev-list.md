# Rev-List 命令改进详细计划

## 所属批次

对象与历史读取命令（plumbing / history inspection）

## 落地状态（2026-06-08）

`rev-list` 的高频 Git 兼容子集已落地：多 spec、排除引用、`A..B` / `A...B`、限制/过滤/格式化参数和 `--objects` 对象枚举均已支持。`COMPATIBILITY.md` 保持 `rev-list | supported`，低频 Git surface 继续记录在命令文档对照表中。

### 已完成能力

- `RevListArgs` 支持 `[SPEC]...`、`^<rev>`、`A..B`、`A...B`、`-n` / `--max-count`、`--skip`、`--count`、`--merges`、`--no-merges`、`--min-parents`、`--max-parents`、`--parents`、`--timestamp`、`--objects`。
- 提交遍历改为 `utils::graph::CommitWalker`，以 commit timestamp frontier heap 拉取 reachable commits，不再从 `rev-list` 主流程调用 `log::get_reachable_commits`。
- `--objects` 使用 `utils::graph::TreeWalker` 栈式 DFS，输出 commit 后的 root tree / child tree / blob 路径，gitlink entry 跳过，路径分隔符固定为 `/`。
- JSON envelope 保持 `input` / `commits` / `total`，仅在 `--objects` 时追加可选 `objects[]`，其中 commit 仍只在 `commits[]` 中。
- `docs/commands/rev-list.md`、`docs/development/integration-scenarios.yaml`、`docs/development/integration-scenarios/cli.object-readback.md` 和 `tools/integration-runner/src/scenarios/object_readback.rs` 已同步。

### 仍 deferred

- `--topo-order`
- `--since` / `--until`
- `--children`
- `--header`
- pathspec-limited history walking

## 已验证命令

- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo check`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- rev_list --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --lib rev_list -- --test-threads=1 --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --lib graph -- --test-threads=1 --nocapture`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`
- `source .env.test && cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.object-readback`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_help_flag_descriptions --test compat_help_examples_banner --test compat_command_docs_examples_section --test compat_matrix_alignment --test compat_all_production_unwrap_guard --test compat_extra_production_unwrap_guard -- --nocapture`
- `source .env.test && LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`
- `cargo +nightly fmt --all --check`
