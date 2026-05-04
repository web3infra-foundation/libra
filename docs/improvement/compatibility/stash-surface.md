# C4：Stash 子命令面补齐（show / branch / clear）

## 所属批次

C4（Audit P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- 第 4 批 [stash.md](../stash.md) 已落地：`StashError` typed enum（11 变体）、`run_stash()` / `render_stash_output()` 拆分、JSON / machine / human 三套渲染共用同一份 `StashOutput`、`STASH_EXAMPLES` `--help` 接入。
- [`src/command/stash.rs`](../../../src/command/stash.rs) 已实现 `Push` / `Pop` / `List` / `Apply` / `Drop` 五个 variant；`StashOutput` 是 `#[serde(tag = "action")]` enum，扩展时直接加新 variant。
- [`tests/command/stash_test.rs`](../../../tests/command/stash_test.rs) 已覆盖五个现有子命令的 happy path / error path / JSON 输出。

### 基于当前代码的 Review 结论
- 第 4 批的 scaffolding（typed error / render split / JSON enum tag）已经为本批扩展铺好路；C4 只需向 `Stash` enum 加 variant，不需要改架构。
- `show` / `branch` / `clear` 是审计 P2 中"用户视野中已存在但子命令面缺失"的最高优先级三项。
- `create` / `store` 属于 stash 内部 plumbing（构建 stash object、把 object 写入 stash ref），非用户日常路径，本批不做。

## 目标与非目标

**目标：**
- 在 `Stash` enum 新增 `Show` / `Branch` / `Clear` 三个 variant。
- 各自实现：
  - `stash show [<stash>]`：展示 stash 修改内容（默认最新 `stash@{0}`），human 模式输出 diff，JSON 模式输出文件级摘要（与 `diff` schema 协调，复用 `files` / `files_changed` 字段命名约定）。
  - `stash branch <branchname> [<stash>]`：从 stash 创建并 checkout 新分支，apply stash，成功后 drop。
  - `stash clear`：删除所有 stash 条目（带 `--force` / 二次确认警告）。
- 三个新子命令复用现有 `StashError` 与 render split 模式，不新增 `<Cmd>Error` 变体除非有真正新错误类。
- `STASH_EXAMPLES` 增加三条对应示例。
- `COMPATIBILITY.md` 中 stash 行从 `partial` 升为 `partial`（仍 partial，因 create / store 延后），notes 更新为 "show / branch / clear added in C4; create / store deferred"。

**非目标：**
- 不实现 `stash create`：仅返回 stash object hash 不存 ref，属于内部 plumbing。
- 不实现 `stash store`：把 create 的产物存入 stash ref，配合 create 使用，同样内部 plumbing。
- 不实现 `stash export` / `stash import`（Git 也无这些子命令，审计中提到的属于第三方扩展）。
- 不引入交互式 stash 选择器或 TUI。

## 设计要点

### `Stash` enum 扩展

[`src/cli.rs`](../../../src/cli.rs) 与 [`src/command/stash.rs`](../../../src/command/stash.rs) 同步：

```rust
pub enum Stash {
    Push(StashPushArgs),
    Pop(StashPopArgs),
    List,
    Apply(StashApplyArgs),
    Drop(StashDropArgs),
    Show(StashShowArgs),     // 新增
    Branch(StashBranchArgs), // 新增
    Clear(StashClearArgs),   // 新增
}
```

每个新 args 结构：

```rust
#[derive(Args, Debug)]
pub struct StashShowArgs {
    /// Stash reference (default: stash@{0})
    pub stash: Option<String>,
    /// Show only file names
    #[clap(long)]
    pub name_only: bool,
    /// Show only file names with status
    #[clap(long)]
    pub name_status: bool,
}

#[derive(Args, Debug)]
pub struct StashBranchArgs {
    pub branch: String,
    /// Stash reference (default: stash@{0})
    pub stash: Option<String>,
}

#[derive(Args, Debug)]
pub struct StashClearArgs {
    /// Skip confirmation prompt
    #[clap(long)]
    pub force: bool,
}
```

### `StashOutput` 扩展

```rust
#[derive(Serialize)]
#[serde(tag = "action")]
pub enum StashOutput {
    // 既有 variant
    Push { ... },
    Pop { ... },
    Apply { ... },
    Drop { ... },
    List { ... },
    Noop,
    // 新增
    Show {
        stash: String,
        files: Vec<StashFileChange>,
        files_changed: FilesChangedStats,
    },
    Branch {
        branch: String,
        stash: String,
        applied: bool,
        dropped: bool,
    },
    Clear {
        cleared_count: usize,
    },
}
```

字段命名严格遵守跨命令契约 [`README.md`](../README.md#5-跨命令字段命名含-url-字段) §5：`branch` / `files` / `files_changed`。

### Diff schema 复用

`stash show` 的 JSON `files` / `files_changed` 字段直接复用 `diff.md` 拥有的 schema（通过引用类型，不重复定义）。具体类型：

```rust
#[derive(Serialize)]
pub struct StashFileChange {
    pub path: String,
    pub status: String,  // "modified" / "added" / "deleted" / "renamed"
}
```

如未来用户需要 hunk 级输出，应通过 `libra diff stash@{0}` 或单独 `stash show --patch` flag（本批不实现）。

### Human 输出示例

```
$ libra stash show
On branch feature/x
Files changed:
  modified: src/main.rs
  added:    src/util.rs
2 files changed, 1 insertion(+), 0 deletions(-)

$ libra stash branch hotfix
Switched to a new branch 'hotfix'
Applied stash@{0}
Dropped stash@{0}

$ libra stash clear
Cleared 3 stash entries.
```

### JSON 输出示例

```json
{"action": "show", "stash": "stash@{0}", "files": [{"path": "src/main.rs", "status": "modified"}], "files_changed": {"total": 2, "new": 1, "modified": 1, "deleted": 0}}
{"action": "branch", "branch": "hotfix", "stash": "stash@{0}", "applied": true, "dropped": true}
{"action": "clear", "cleared_count": 3}
```

### Error 复用

复用现有 `StashError` 变体；`stash show` 在 stash ref 不存在时返回 `StashError::StashNotExist`；`stash branch` 在分支已存在时返回 `StashError::Other(...)` 携带分支冲突 detail（或新增 `StashError::BranchExists { name }` 变体——若新增需同步加 `StableErrorCode` 映射）。

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/cli.rs`](../../../src/cli.rs) | 修改 | `Stash` enum 加三个 variant |
| [`src/command/stash.rs`](../../../src/command/stash.rs) | 修改 | 新增 args struct、handler、`StashOutput` 三个 variant、`STASH_EXAMPLES` 三条示例 |
| [`tests/command/stash_test.rs`](../../../tests/command/stash_test.rs) | 修改 | 新增 ≥6 条用例（每个子命令 happy + error 各一条） |
| [`tests/compat/stash_subcommand_surface.rs`](../../../tests/compat/stash_subcommand_surface.rs) | 新建 | `--help` 列出三个新子命令的断言 + 跨子命令 JSON schema 一致性 |
| [`docs/commands/stash.md`](../../commands/stash.md) | 修改 | JSON schema、错误码、EXAMPLES |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | stash 行 notes 更新 |

## 测试与验收

- [ ] `cargo run -- stash --help` 列出 `show` / `branch` / `clear`。
- [ ] `cargo run -- stash show` / `stash show stash@{1}` / `stash show --name-only` 各自通过用例。
- [ ] `cargo run -- stash branch <new-name>` 在新建分支后正确 apply 并 drop。
- [ ] `cargo run -- stash clear` 默认行为：交互模式下要求确认；`--force` 跳过；JSON 模式直接执行不询问。
- [ ] JSON 输出与 [`docs/commands/stash.md`](../../commands/stash.md) schema 一致。
- [ ] `cargo test stash_test` 全部通过。

## 风险与缓解

1. **`stash branch` 跨当前工作树 dirty 状态时行为不一致** → 缓解：实现复用 `switch` 的 `ensure_clean_status()`；测试覆盖 dirty 工作树场景。
2. **`stash clear` 误操作不可逆** → 缓解：human 模式默认要求 `y/N` 确认；`--force` 跳过；JSON / machine 模式按文档约定不询问（脚本场景假设调用方已确认）。
3. **`stash show` 与未来 `stash show --patch` 字段冲突** → 缓解：本批仅文件级 `files` / `files_changed`；`--patch` 引入时新增独立字段（如 `patch` 或 `diff`），保持向后兼容。
4. **新增 variant 破坏既有 JSON 消费方** → 缓解：JSON 是 `#[serde(tag = "action")]` enum，新 tag 值对老消费方表现为未知 action；不破坏现有字段。
