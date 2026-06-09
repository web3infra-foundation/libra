# C5：Worktree Remove `--delete-dir` 行为对齐

## 所属批次

C5（Audit P2）

## 已完成前置条件与当前代码状态

### 2026-06-08 worktree parity 扩展复核

- `worktree add` 已补齐 `-b` / `-B` / `--detach` / `--no-checkout` / `--lock` / `--reason` / `[commit-ish]`，并在默认实现与 `worktree-fuse` wrapper 的非 FUSE 委托路径同步。
- `worktree list` 已补 `--porcelain` 与 `--verbose`。Porcelain 只输出 `worktree`、共享 `HEAD`、`locked`，不会输出 Git 的 per-worktree `branch` / `detached` 行，因为 Libra linked worktree 共用同一个 `.libra` storage/HEAD。
- `worktree remove` 已补 repeatable `--force`：单个 `--force` 可配合 `--delete-dir` 跳过 dirty 检查；locked worktree 需要 `-f -f` 才能注销；任何 force 都不能越过 main worktree 保护，也不会在缺少 `--delete-dir` 时删除磁盘目录。
- `worktree prune` 已补 `--dry-run` / `--verbose` / `--expire <time>`。`--expire` 只过滤目录缺失的 stale registry entry，仍存在的 worktree 永远不会因 age 被 prune。
- `worktree repair` 已扩展为 symlink-only 修复：重建缺失或 stale 的 linked-worktree `.libra` symlink，跳过 main worktree 和真实 `.libra` 目录，避免误删真实 storage 内容。
- `move` 失败回滚已硬化：先重命名磁盘目录，失败时保持 registry 原样；成功后刷新 linked `.libra` symlink，再写入 registry，写入失败时尝试把目录移回原路径。
- 覆盖测试：`cargo test --test command_test -- worktree_test --test-threads=1 --nocapture`（71 passed）与 `cargo test --test command_test --features worktree-fuse -- worktree_fuse_test --test-threads=1 --nocapture`（9 passed）。

### 已确认落地的基线（2026-05-11 复核）
- [`src/command/worktree.rs`](../../../src/command/worktree.rs) 已实现 `add` / `list` / `lock` / `unlock` / `move` / `prune` / `remove` / `repair`，以及 Unix 下的 `umount` 子命令。
- `worktree remove` 当前默认**不删除磁盘目录**，继续保持非破坏默认。
- `WorktreeSubcommand::Remove { path, delete_dir, force }` 已暴露 `--delete-dir` 与 repeatable `--force`；显式 `--delete-dir` 默认会先检查脏工作树，只有 clean worktree 才删除磁盘目录并从 registry 移除，`--force` 可作为显式越权。
- 第 31 批"mv / rm / worktree 结构化输出"已在 `mv` / `rm` / worktree 常用成功路径上启动；当前 `mv` / `rm` 已有成功 JSON / machine schema，worktree `add` / `list` / `lock` / `unlock` / `move` / `prune` / `remove` / `repair` 已有成功 JSON / machine schema，非 FUSE worktree 错误已通过 `WorktreeError` typed enum 显式映射 `StableErrorCode`，FUSE `umount` 成功路径已有 JSON / machine schema。
- [`tests/command/worktree_test.rs`](../../../tests/command/worktree_test.rs) 已覆盖基础 add / list / remove，包含 `--delete-dir` on/off、dirty 拒绝路径，以及 no-such/main/locked/destination-exists/storage-path/corrupt-state 的 JSON/machine 负向错误契约。
- [`tests/compat/worktree_delete_dir.rs`](../../../tests/compat/worktree_delete_dir.rs) 已固定对外兼容契约：默认保留目录，`--delete-dir` 删除 clean 目录，dirty 时拒绝并保留 registry/目录。

### 基于当前代码的 Review 结论
- C5 的行为对齐已经落地：Libra 默认非破坏，显式 `--delete-dir` 才走 Git-style 删除目录。
- 脏工作树保护已经是当前契约的一部分，不能在后续结构化输出批次中放宽或静默降级。
- 第 31 批的 `mv` / `rm` / worktree destructive success/error 结构化输出已落地；后续只保留 FUSE mount 管理的更细错误码扩展，不再阻塞本批。

## 目标与非目标

**原 C5 目标（已完成并被 2026-06-08 parity 扩展覆盖）：**
- 在 `WorktreeSubcommand::Remove` 加 `--delete-dir` 字段（bool，默认 false）。
- 在 `worktree remove` handler 中：
  - 默认（`--delete-dir=false`）：保持当前行为，仅从 registry 移除工作树记录，不动磁盘目录。
  - 显式 `--delete-dir`：先做 dirty 检查，再删除磁盘目录；只有删盘成功后才从 registry 移除记录，避免"目录仍在但已从 registry 消失"的半完成状态。
  - 删盘前若工作树状态非干净（dirty），拒绝执行；本批不新增 `--force` 越过该保护。
- `WORKTREE_EXAMPLES` 加一条 `worktree remove --delete-dir <path>` 示例。
- `COMPATIBILITY.md` 中 worktree 行更新为 `intentionally-different`，notes "remove keeps disk dir by default; --delete-dir for Git-style behavior"。
- `docs/commands/worktree.md`（若存在）同步说明默认 vs `--delete-dir` 的差异。

**原 C5 非目标（历史记录；2026-06-08 已反转其中 `--force` 项）：**
- 不翻转默认（不切到 Git 风格删盘默认）——这会破坏现有脚本，已被用户决策排除。
- C5 原批次不引入完整 `WorktreeOutput` / `WorktreeError` typed enum；第 31 批已经补齐非 FUSE worktree success schema 与 typed error，并补齐 FUSE `umount` success schema。
- C5 当时不实现 `worktree remove --force`；2026-06-08 parity 扩展已实现 repeatable `--force`，并保留默认不删盘的 intentionally-different 设计。
- 不动 `worktree add` / `list` / `lock` / `move` / `prune` / `repair` / `unlock` 的行为。

## 设计要点

### `WorktreeSubcommand::Remove` 扩展

```rust
Remove {
    /// Filesystem path of the worktree to unregister.
    path: String,
    /// Also delete the worktree directory on disk
    #[clap(long)]
    delete_dir: bool,
},
```

### `worktree remove` handler 修改

```rust
async fn remove_worktree(path: String, delete_dir: bool) -> io::Result<WorktreeRemoveOutput> {
    // load state, resolve canonical target, reject main/locked entries
    // if delete_dir: inspect dirty state inside target, then remove_dir_all(target)
    // remove registry entry last, then return the stable success payload

    Ok(WorktreeRemoveOutput {
        path: target.to_string_lossy().into_owned(),
        registry_removed: true,
        disk_directory_deleted: delete_dir,
    })
}

fn render_remove_worktree(result: &WorktreeRemoveOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        emit_json_data("worktree.remove", result, output)
    } else {
        // human mode prints the same registry/delete-dir distinction.
        Ok(())
    }
}
```

**注意**：
1. `remove_worktree` 已从同步 `fn` 改为 `async fn`，并在 `execute_safe` 的 match 分支中追加 `.await` 与 `render_remove_worktree(...)`。
2. dirty 检查复用 `crate::command::status` 的公开函数（`changes_to_be_committed_safe` / `changes_to_be_staged`），通过 `DirGuard` 把检查范围限定到目标 worktree 目录。
3. v0.17.167 已把 dirty / delete-dir 失败、no-such/main/locked/destination/storage/corrupt-state 等非 FUSE 错误吸收到 `WorktreeError` typed variant，并固定 JSON / machine 负向契约。

### 非破坏行为保留

默认调用：

```bash
$ libra worktree remove ../feature-x
Removed worktree '/repo/feature-x' from registry. Directory kept on disk.
```

显式 `--delete-dir`：

```bash
$ libra worktree remove --delete-dir ../feature-x
Removed worktree '/repo/feature-x' from registry and deleted directory.

$ libra worktree remove --delete-dir ../dirty-feature
Error: cannot delete dirty worktree '../dirty-feature' (uncommitted changes)
       Hint: commit or stash changes, or remove without --delete-dir to keep the directory
```

### `COMPATIBILITY.md` 行更新

```markdown
| worktree | intentionally-different | remove keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior. |
```

### 与第 31 批的协同

第 31 批已落地 worktree success / error 结构化契约：

- C5 的 dirty conflict 已吸收到 `WorktreeError::DirtyWorktree`，delete-dir 失败映射为 `LBR-IO-002`。
- 当前 `worktree.remove` success schema 已固定 `path` / `registry_removed` / `disk_directory_deleted`，并保持字段兼容。
- `--delete-dir` 字段已经在当前 success schema 中体现为 `disk_directory_deleted: bool`；FUSE `umount` 成功 schema 已固定 `mountpoint` / `unmounted` / `cleanup_requested` / `cleanup_root` / `cleanup_root_removed`。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/worktree.rs`](../../../src/command/worktree.rs) | 已修改 | `WorktreeSubcommand::Remove` 加 `--delete-dir`；`remove` handler 加删盘分支 + dirty 检查；非 FUSE `WorktreeError` typed enum 显式映射稳定错误码 |
| [`src/command/worktree-fuse.rs`](../../../src/command/worktree-fuse.rs) | 已修改 | feature-gated `worktree umount` 成功路径输出 `worktree.umount` JSON / machine envelope |
| [`src/utils/error.rs`](../../../src/utils/error.rs) | 复核/必要时修改 | 优先复用 `ConflictOperationBlocked` / `IoWriteFailed`；仅在确有跨命令需求时新增更细错误码 |
| [`tests/command/worktree_test.rs`](../../../tests/command/worktree_test.rs) | 已修改 | 已覆盖默认不删盘、clean `--delete-dir` 删盘、dirty + `--delete-dir` 拒绝、成功 JSON / machine，以及非 FUSE 负向 JSON / machine 错误契约 |
| [`tests/command/worktree_fuse_test.rs`](../../../tests/command/worktree_fuse_test.rs) | 已修改 | feature-gated `worktree umount --cleanup --json` 无仓库成功路径 |
| [`tests/compat/worktree_delete_dir.rs`](../../../tests/compat/worktree_delete_dir.rs) | 已新建 | 固定 help / examples surface、默认保留目录、clean delete 与 dirty 拒绝行为 |
| [`docs/commands/worktree.md`](../../commands/worktree.md) | 修改 | 默认行为 vs `--delete-dir` 的差异说明，以及 `worktree.list` / `worktree.remove` 结构化输出示例 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | worktree 行 notes 更新 |

## 测试与验收

- [x] `cargo run -- worktree remove --help` 列出 `--delete-dir`。
- [x] `worktree remove <path>`（默认）后，`<path>` 目录仍存在；registry 中已移除。
- [x] `worktree remove --delete-dir <path>` 后，`<path>` 目录已不存在；`ls <path>` 报 not-found。
- [x] `worktree remove --delete-dir <dirty-path>` 返回 conflict 类稳定错误码并保留目录与 registry 记录。
- [x] 集成测试覆盖：clean + delete-dir / dirty + delete-dir / clean + 不带 flag。
- [x] `COMPATIBILITY.md` worktree 行已更新为默认保留目录、`--delete-dir` opt-in 删除。
- [x] (v0.17.11) 本轮最终回归已运行 `cargo test --test command_test worktree_test`。
- [x] (v0.17.162) `worktree list` JSON / machine schema 已落地并由 `test_worktree_list_json_outputs_structured_entries`、`test_worktree_list_machine_outputs_single_json_line` 覆盖。
- [x] (v0.17.163) dirty `--delete-dir` 拒绝由 `test_worktree_remove_with_delete_dir_dirty_path_is_rejected` 覆盖，断言 `LBR-CONFLICT-002`、目录保留和 registry 保留。
- [x] (v0.17.164) `worktree remove` 成功路径 JSON / machine schema 已落地并由 `test_worktree_remove_json_reports_kept_directory`、`test_worktree_remove_machine_reports_deleted_directory` 覆盖，断言 canonical `path`、`registry_removed` 与 `disk_directory_deleted`。
- [x] (v0.17.166) worktree `add` / `lock` / `unlock` / `move` / `prune` / `repair` 成功路径 JSON / machine schema 已落地，命令文档已补每个 envelope 示例。
- [x] (v0.17.167) 非 FUSE worktree `WorktreeError` typed enum 已落地；`test_worktree_lock_json_no_such_worktree_reports_invalid_target`、`test_worktree_remove_machine_rejects_main_with_stable_error`、`test_worktree_remove_json_rejects_locked_with_stable_error`、`test_worktree_move_machine_destination_exists_reports_conflict`、`test_worktree_add_json_rejects_storage_path_as_invalid_target`、`test_worktree_list_json_corrupt_state_reports_repo_corrupt` 固定 JSON / machine 负向错误契约。
- [x] (v0.17.168) FUSE `worktree umount` 成功路径 JSON / machine schema 已落地，默认入口由 `test_worktree_umount_json_reports_cleanup` 覆盖，feature-gated 入口由 `test_fuse_worktree_umount_json_reports_cleanup_without_repo` 覆盖。

## 风险与缓解

1. **`std::fs::remove_dir_all` 跨平台行为差异（macOS / Linux / Windows）** → 缓解：测试用例在 `compat-offline-core` job 上运行；macOS / Windows 行为差异在 `docs/commands/worktree.md` 显式注明。
2. **dirty 检查与现有 worktree 状态读取耦合** → 缓解：复用 `crate::command::status::changes_to_be_committed_safe` / `changes_to_be_staged` 公开函数，通过 `DirGuard` 限定检查范围到目标 worktree；不在本批新写状态扫描。
3. **用户脚本依赖现有"不删盘"默认** → 缓解：默认行为不变；新 flag 是 opt-in；`--help` 与 `COMPATIBILITY.md` 显式说明默认。
4. **第 31 批后续继续扩展时字段兼容风险** → 缓解：当前 `worktree.remove` success schema 已固定 `path` / `registry_removed` / `disk_directory_deleted`，FUSE `worktree.umount` success schema 已固定 `mountpoint` / `unmounted` / `cleanup_requested` / `cleanup_root` / `cleanup_root_removed`，非 FUSE typed error 已覆盖负向契约。
