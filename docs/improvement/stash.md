# Stash 命令改进详细计划

## 所属批次

第四批：暂存与撤销命令（P1 一致性修复）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- `StashError` typed enum 已落地，覆盖仓库外、无初始提交、stash 引用语法、stash 不存在、apply 冲突、branch 已存在、clear 缺少 `--force`、对象读写、index 保存、reset 失败等路径，并通过 `From<StashError> for CliError` 映射到稳定错误码和可操作 hint。
- `run_stash()` + `render_stash_output()` 已完成执行层/渲染层拆分，human / JSON / machine 共用一套 `StashOutput` 结果模型。
- `StashOutput` 是 `#[serde(tag = "action")]` enum，已覆盖 `noop` / `push` / `pop` / `apply` / `drop` / `list` / `show` / `branch` / `clear`。`push` 的 JSON envelope 会在相关场景输出 `included_untracked` 和 `kept_index`。
- `stash push` 已支持 `-m`、`-u` / `--include-untracked`、`-a` / `--all` 和 `--keep-index`。默认只保存 tracked index/worktree 修改，并保留 untracked 文件。
- `stash push -u` / `--all` 会把被纳入的 untracked/ignored 文件写入第三个 stash parent；`stash apply` / `pop` 会把这些文件恢复为未跟踪工作区文件，并在会覆盖本地同名文件时拒绝执行。
- `stash push --keep-index` 会保存普通 stash 元数据，然后恢复原 index，并把 worktree 还原到 index 内容；同一文件的 staged 内容保留，unstaged delta 进入 stash。
- `stash show` 已支持 `--stat`、`-p` / `--patch`、`--name-only`、`--name-status`，并按 `-p > --stat > --name-status > --name-only` 解析多 flag；无显式模式时读取 `stash.showPatch` 和 `stash.showStat` 默认。
- `STASH_EXAMPLES` 已通过 clap `after_help` 接入，包含 `-u`、`-a` 和 `--keep-index` 示例。
- `docs/commands/stash.md`、`docs/commands/zh-CN/stash.md`、`COMPATIBILITY.md`、`docs/development/integration-test-plan.md` 和 `docs/development/integration-scenarios/*` 已记录当前公开面。
- `tests/command/stash_test.rs` 已覆盖 push/pop/list/apply/drop/show/branch/clear、JSON 输出、错误码、仓库外调用、默认排除 untracked、`-u`/`--all` 纳入 untracked/ignored、`--keep-index`、untracked apply 恢复和碰撞拒绝。
- `tools/integration-runner` 的 `cli.stash-bisect-worktree` 场景已覆盖 `stash push -u` / `--all` / `--keep-index` 的黑盒 CLI 路径，并由 `check-plan` 维护 yaml、scenario md、矩阵和 runner 注册一致性。

### 基于当前代码的 Review 结论
- 第四批对外契约已落地，代码、测试、命令文档和黑盒 integration scheme 已对齐
- 成功路径不再沉默：human 模式下会输出明确确认信息，JSON / machine 模式返回稳定 envelope
- 当前仍保持 `create` / `store`、pathspec 部分 stash、`apply/pop --index` 为 deferred；这些差异必须继续留在 `COMPATIBILITY.md` 和 declined 文档中

## 目标与非目标

**已完成目标：**
- typed error、显式错误码、run/render 分层、JSON / machine、`--help` EXAMPLES、命令文档和集成测试已全部落地
- Batch 0：`stash push -u/-a/--keep-index`、第三 parent untracked 快照、apply/pop untracked 恢复、碰撞拒绝、black-box runner 覆盖已落地
- Batch 2 show 子集：`stash show --stat`、`-p` / `--patch`、`stash.showPatch` / `stash.showStat` 配置默认和 human-mode 优先级已落地；JSON 继续保持结构化 `files` / `files_changed` schema，human-only mode hint 不外泄

**后续维护目标：**
- 继续维护冲突、空 stash、list schema 和 no-op 场景的回归测试
- 后续如补 `apply/pop --index`，需要保存和恢复 unmerged index stage 1/2/3，而不是只重建 stage 0 工作树
- 后续如补 pathspec stash，需要同步 clap grammar、docs/commands、COMPATIBILITY、integration scenario 和 focused CLI tests

**本批非目标：**
- 不引入交互式 stash 选择器或 TUI
- 不实现 `stash create` / `stash store`
- 不实现 pathspec 部分 stash
- 不实现 `apply/pop --index`

## 验证方式

1. `cargo +nightly fmt --all --check`
2. `LIBRA_SKIP_WEB_BUILD=1 cargo clippy --all-targets --all-features -- -D warnings`
3. `source .env.test; LIBRA_SKIP_WEB_BUILD=1 cargo test --test command_test -- stash --test-threads=1`
4. `LIBRA_SKIP_WEB_BUILD=1 cargo test --test compat_matrix_alignment --test compat_help_examples_banner --test compat_command_docs_examples_section --test compat_all_production_unwrap_guard`
5. `LIBRA_SKIP_WEB_BUILD=1 cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`
6. `LIBRA_SKIP_WEB_BUILD=1 cargo run --manifest-path tools/integration-runner/Cargo.toml -- run --only cli.stash-bisect-worktree`
7. `docs/commands/stash.md`、`docs/commands/zh-CN/stash.md` 与命令实际输出保持一致
