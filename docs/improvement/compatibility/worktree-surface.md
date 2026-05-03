# C5：Worktree Remove `--delete-dir` 行为对齐

## 所属批次

C5（Audit P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- [`src/command/worktree.rs`](../../../src/command/worktree.rs) 已实现 `add` / `list` / `lock` / `unlock` / `move` / `prune` / `remove` / `repair`，以及 Unix 下的 `umount` 子命令。
- `worktree remove` 当前默认**不删除磁盘目录**（[`worktree.rs:715-742`](../../../src/command/worktree.rs#L715)）；docstring 明确写"The directory on disk is intentionally left untouched to avoid destructive behavior"。
- `WorktreeSubcommand::Remove { path }` 当前没有 `--delete-dir` 或 `--force` flag。
- 第 31 批"mv / rm / worktree 结构化输出"尚未启动；当前 worktree 没有 `WorktreeOutput` schema 或 `WorktreeError` typed enum。
- [`tests/command/worktree_test.rs`](../../../tests/command/worktree_test.rs) 已存在并覆盖基础 add / list / remove。

### 基于当前代码的 Review 结论
- 当前默认非破坏行为是有意的——避免在执行 `git worktree remove` 直觉下意外丢数据。
- 但用户决策已确认：**保留默认行为，新增 `--delete-dir` 显式开关**对齐 Git 直觉。
- 本批仅在 `WorktreeSubcommand::Remove` 加 flag + `remove` handler 增删盘分支；不动 `WorktreeOutput` / `WorktreeError`（这些归第 31 批）。

## 目标与非目标

**目标：**
- 在 `WorktreeSubcommand::Remove` 加 `--delete-dir` 字段（bool，默认 false）。
- 在 `worktree remove` handler 中：
  - 默认（`--delete-dir=false`）：保持当前行为，仅从 registry 移除工作树记录，不动磁盘目录。
  - 显式 `--delete-dir`：先做 dirty 检查，再删除磁盘目录；只有删盘成功后才从 registry 移除记录，避免"目录仍在但已从 registry 消失"的半完成状态。
  - 删盘前若工作树状态非干净（dirty），拒绝执行；本批不新增 `--force` 越过该保护。
- `WORKTREE_EXAMPLES` 加一条 `worktree remove --delete-dir <path>` 示例。
- `COMPATIBILITY.md` 中 worktree 行更新为 `intentionally-different`，notes "remove keeps disk dir by default; --delete-dir for Git-style behavior"。
- `docs/commands/worktree.md`（若存在）同步说明默认 vs `--delete-dir` 的差异。

**非目标：**
- 不翻转默认（不切到 Git 风格删盘默认）——这会破坏现有脚本，已被用户决策排除。
- 不在本批引入 `WorktreeOutput` / `WorktreeError` typed enum；第 31 批拥有这些。
- 不实现 `worktree remove --force`；当前没有该 flag，本批只为 `--delete-dir` 提供最小 dirty 检查（dirty 时拒绝）。
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
async fn remove_worktree(path: String, delete_dir: bool) -> CliResult<()> {
    let mut state = load_state()
        .map_err(|e| CliError::fatal(format!("failed to load worktree state: {e}")))?;
    let target = canonicalize(path)
        .map_err(|e| CliError::fatal(format!("invalid path: {e}")))?;
    let index = state.worktrees.iter().position(|w| Path::new(&w.path) == target)
        .ok_or_else(|| CliError::fatal("no such worktree"))?;
    let entry = &state.worktrees[index];
    if entry.is_main {
        return Err(CliError::fatal("cannot remove main worktree"));
    }
    if entry.locked {
        return Err(CliError::fatal("cannot remove locked worktree"));
    }

    if delete_dir {
        // Refuse on dirty worktree (no --force in this batch).
        // Scope the status check to the target directory via DirGuard.
        let _guard = DirGuard::change_to(&target)
            .map_err(|e| CliError::fatal(format!("cannot enter worktree: {e}")))?;
        let staged = crate::command::status::changes_to_be_committed_safe().await
            .map_err(|e| CliError::fatal(format!("cannot check worktree status: {e}")))?;
        let unstaged = crate::command::status::changes_to_be_staged()
            .map_err(|e| CliError::fatal(format!("cannot check worktree status: {e}")))?;
        if !staged.is_empty() || !unstaged.is_empty() {
            return Err(CliError::conflict(format!(
                "cannot delete dirty worktree '{}' (uncommitted changes)\n\
                 Hint: commit or stash changes, or remove without --delete-dir to keep the directory",
                target.display()
            )));
        }
        std::fs::remove_dir_all(&target)
            .map_err(|e| CliError::fatal(format!(
                "failed to delete worktree directory '{}': {e}",
                target.display()
            )).with_stable_code(StableErrorCode::IoWriteFailed))?;
    }

    state.worktrees.remove(index);
    save_state(&state)
        .map_err(|e| CliError::fatal(format!("failed to save worktree state: {e}")))?;
    Ok(())
}
```

**注意**：
1. `remove_worktree` 需要从同步 `fn` 改为 `async fn`，并在 `execute_safe` 的 match 分支中追加 `.await`：`WorktreeSubcommand::Remove { path, delete_dir } => remove_worktree(path, delete_dir).await`。
2. dirty 检查复用 `crate::command::status` 的公开函数（`changes_to_be_committed_safe` / `changes_to_be_staged`），通过 `DirGuard` 把检查范围限定到目标 worktree 目录。
3. 本批不引入 `WorktreeError` typed enum；第 31 批引入完整 `WorktreeError` 时再把 dirty / delete-dir 失败吸收到 typed variant。

### 非破坏行为保留

默认调用：

```bash
$ libra worktree remove ../feature-x
Removed worktree '../feature-x' from registry. Directory ../feature-x kept on disk.
```

显式 `--delete-dir`：

```bash
$ libra worktree remove --delete-dir ../feature-x
Removed worktree '../feature-x' from registry and deleted directory ../feature-x.

$ libra worktree remove --delete-dir ../dirty-feature
Error: cannot delete dirty worktree '../dirty-feature' (uncommitted changes)
       Hint: commit or stash changes, or remove without --delete-dir to keep the directory
```

### `COMPATIBILITY.md` 行更新

```markdown
| worktree | intentionally-different | remove keeps disk dir by default (no implicit data loss). Use `--delete-dir` for Git-style behavior. |
```

### 与第 31 批的协同

第 31 批落地 `WorktreeOutput` / `WorktreeError` 完整 typed enum 时：

- C5 的 conflict / IO 错误自然吸收为未来 `WorktreeError::DirtyWorktree` / `DeleteDirFailed` 等 typed variant。
- 本批 handler 的返回类型后续可从 `CliResult<()>` 升为 `Result<WorktreeOutput, WorktreeError>`，本批不预先做这层封装。
- `--delete-dir` 字段在 `WorktreeOutput::Remove` 中体现为 `disk_directory_deleted: bool`，第 31 批落地时一并加上。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/worktree.rs`](../../../src/command/worktree.rs) | 修改 | `WorktreeSubcommand::Remove` 加 `--delete-dir`；`remove` handler 加删盘分支 + dirty 检查 |
| [`src/utils/error.rs`](../../../src/utils/error.rs) | 复核/必要时修改 | 优先复用 `ConflictOperationBlocked` / `IoWriteFailed`；仅在确有跨命令需求时新增更细错误码 |
| [`tests/command/worktree_test.rs`](../../../tests/command/worktree_test.rs) | 修改 | 新增 ≥3 条用例（默认不删盘、`--delete-dir` 删盘、dirty + `--delete-dir` 拒绝） |
| [`tests/compat/worktree_delete_dir.rs`](../../../tests/compat/worktree_delete_dir.rs) | 新建 | `--delete-dir` on/off 行为差异跨场景断言 |
| [`docs/commands/worktree.md`](../../commands/worktree.md) | 修改 | 默认行为 vs `--delete-dir` 的差异说明 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | worktree 行 notes 更新 |

## 测试与验收

- [ ] `cargo run -- worktree remove --help` 列出 `--delete-dir`。
- [ ] `worktree remove <path>`（默认）后，`<path>` 目录仍存在；registry 中已移除。
- [ ] `worktree remove --delete-dir <path>` 后，`<path>` 目录已不存在；`ls <path>` 报 not-found。
- [ ] `worktree remove --delete-dir <dirty-path>` 返回 conflict 类稳定错误码（优先 `ConflictOperationBlocked`）并保留目录与 registry 记录。
- [ ] 集成测试覆盖：clean + delete-dir / dirty + delete-dir / clean + 不带 flag。
- [ ] `COMPATIBILITY.md` worktree 行已更新。
- [ ] `cargo test worktree_test` 全部通过。

## 风险与缓解

1. **`std::fs::remove_dir_all` 跨平台行为差异（macOS / Linux / Windows）** → 缓解：测试用例在 `compat-offline-core` job 上运行；macOS / Windows 行为差异在 `docs/commands/worktree.md` 显式注明。
2. **dirty 检查与现有 worktree 状态读取耦合** → 缓解：复用 `crate::command::status::changes_to_be_committed_safe` / `changes_to_be_staged` 公开函数，通过 `DirGuard` 限定检查范围到目标 worktree；不在本批新写状态扫描。
3. **用户脚本依赖现有"不删盘"默认** → 缓解：默认行为不变；新 flag 是 opt-in；`--help` 与 `COMPATIBILITY.md` 显式说明默认。
4. **第 31 批落地时本批结构需重写** → 缓解：本批 handler 的内部签名与第 31 批 `WorktreeOutput::Remove` schema 字段命名预先对齐（`disk_directory_deleted: bool`）。
