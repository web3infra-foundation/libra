# C5：Checkout 命令处置（取消 hide + 兼容别名）

## 所属批次

C5（Audit P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线（2026-05-11 复核）
- [`src/cli.rs`](../../../src/cli.rs) 中 `Checkout` 已作为顶层可见命令暴露，about 文案明确为 branch compatibility surface，并推荐 `switch` / `restore`。
- [`src/command/checkout.rs`](../../../src/command/checkout.rs) 已实现 checkout 的分支类基础语义（显示当前分支、切换本地分支、`-b` 新建并切换、远端同名分支 auto-track + pull）。内部使用 `restore` 将工作树 materialize 到目标 commit；C9 后又补齐显式 `--` path mode：`libra checkout -- <path>` 从 index 恢复 worktree，`libra checkout <tree-ish> -- <path>` 从指定 source 恢复 index + worktree。
- 第 30 批已完整落地（v0.17.372）：`CheckoutOutput`、JSON/machine 成功输出、执行/渲染拆分、checkout-owned stable code 全部就绪，并已补齐完整 `CheckoutError` typed enum（[`src/command/checkout.rs:75`](../../../src/command/checkout.rs)）含 `CheckingOutBranchBlocked` / `CreatingBranchBlocked` / `SwitchingToBranchBlocked` / `BranchNotFound` / `PathSpecNotMatched` / `DirtyUnstaged` / `DirtyUncommitted` / `UntrackedOverwrite` / `BranchStoreRead` / `BranchStoreCorrupt` / `RemoteHeadMissing` / `RemoteSyncFailed { stage, source }` / `DelegatedCli` 13 个变体；`get_remote()` 内 `set_upstream` / `pull` 代理调用已通过 `RemoteSyncFailed` 细分层透传底层 `StableErrorCode`。
- [docs/improvement/checkout.md](../checkout.md) 已记录"第二批兼容收口已落地，完整现代化留第 30 批"。
- [`docs/commands/checkout.md`](../../commands/checkout.md) 已说明 checkout 是兼容 surface；顶层索引不再标 hidden。
- [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) 已断言顶层 `--help` 包含 checkout，且 `checkout --help` 推荐 `switch` / `restore`。
- 用户决策已落地：**取消隐藏，正式作为分支类兼容入口**；文件恢复继续通过 `libra restore` 暴露。

### 基于当前代码的 Review 结论
- C5 的可见性与 help banner 已落地；checkout 不再处于"存在但不可发现"状态。
- 第 30 批的结构化输出目标与本批已落地的"取消 hide + 文案调整"互不冲突：v0.17.372 完整 typed `CheckoutError` 与 `RemoteSyncFailed { stage, source }` 细分层落地时未重新隐藏 checkout。
- 当前 help 和 docs 已明确 checkout 是分支类兼容入口；新流程推荐 switch，文件恢复推荐 restore。

## 目标与非目标

**已落地目标：**
- [`src/cli.rs`](../../../src/cli.rs) 已让 `Checkout` 在 `libra --help` 顶层列表显示。
- [`src/command/checkout.rs`](../../../src/command/checkout.rs) 的 `CHECKOUT_EXAMPLES` 常量已加入 "branch compatibility surface; prefer switch / restore" 提示。
- [`docs/commands/checkout.md`](../../commands/checkout.md) 已说明何时使用 checkout、何时推荐 switch / restore。
- `COMPATIBILITY.md` 中 checkout 行已更新为 `partial`，notes "branch compatibility surface; use restore for file restoration; full modernization pending"。
- [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) 已断言：`libra --help` 顶层文本包含 "checkout"。

**非目标：**
- C5 本身不改 checkout 内部实现；第 30 批已完整落地 JSON / render split 与 typed `CheckoutError`（v0.17.372 含 13 个变体与 `RemoteSyncFailed` 细分层），无遗留范围。
- 不 deprecate checkout——它是有意保留的兼容入口，不是过渡性废弃。
- 不引入 `--no-hint` 或动态隐藏机制；取消 hide 是无条件的。
- 不修改 switch / restore 的 `--help` 文案——它们已在第二批稳定，本批不动。

## 设计要点

### `src/cli.rs` 现状

```rust
#[command(
    about = "Branch compatibility surface; prefer 'switch' for branches and 'restore' for files"
)]
Checkout(command::checkout::CheckoutArgs),
```

`Checkout` 现在是顶层可见命令；help 文案保持兼容定位，不承诺文件恢复重载。

### `--help` banner 文案

`CHECKOUT_EXAMPLES` 常量修改示意（替换现有常量内容）：

```rust
pub const CHECKOUT_EXAMPLES: &str = "\
EXAMPLES:
  libra checkout main                # switch to branch (prefer: libra switch main)
  libra checkout -b feature/x        # create branch (prefer: libra switch -c feature/x)
";
```

`src/command/checkout.rs` 中 `CheckoutArgs` 已有 `#[command(after_help = CHECKOUT_EXAMPLES)]`（第 42 行），无需改动属性位置；只需替换常量内容。

### 顶层 help 验证

```bash
$ libra --help
USAGE:
    libra <COMMAND>

COMMANDS:
    init        ...
    add         ...
    ...
    switch      Switch branches
    restore     Restore working tree files
    checkout    Branch compatibility surface; prefer switch / restore
    ...
```

### 与第 30 批的协同时序

- C5 已先落地；第 30 批随后补充了 `CheckoutOutput` / JSON / render split，并保持 `--help` 文案稳定。v0.17.372 完整 typed `CheckoutError` 与 `RemoteSyncFailed` 细分层亦未改 checkout 可见性。
- 若第 30 批先落地：C5 启动时，去 hide 是 trivial 改动；CHECKOUT_EXAMPLES 已存在，本批仅改 banner 文案。
- 两批互不阻塞，但建议 C5 先于第 30 批，因为可见性是即时收益，typed error 是渐进收益。

### `COMPATIBILITY.md` 行更新

```markdown
| checkout | partial | branch compatibility surface. Use `switch` for branch workflows and `restore` for file restoration. Full modernization pending. |
```

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/cli.rs`](../../../src/cli.rs) | 已修改 | `Checkout` 顶层可见 |
| [`src/command/checkout.rs`](../../../src/command/checkout.rs) | 修改 | 替换 `CHECKOUT_EXAMPLES` 常量内容为兼容提示文案 |
| [`docs/commands/checkout.md`](../../commands/checkout.md) | 修改 | 顶部迁移说明：何时用 checkout / 何时推荐 switch / restore；删除 hidden 旧表述 |
| [`docs/commands/README.md`](../../commands/README.md) | 修改 | 移除 checkout 的 hidden 标记，确保命令索引与顶层 help 一致 |
| [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) | 新建 | 断言 `libra --help` 包含 "checkout" 与兼容提示 |
| [`tests/command/checkout_test.rs`](../../../tests/command/checkout_test.rs) | 修改 | 新增 1 条 `--help` 文本断言 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | checkout 行更新 |

## 测试与验收

- [x] `cargo run -- --help` 顶层 COMMANDS 列表包含 `checkout`。
- [x] `cargo run -- checkout --help` 顶部出现 branch compatibility / prefer switch / restore 文案。
- [x] EXAMPLES 中分支类示例包含 "prefer: libra switch" 提示；文档明确文件恢复使用 `libra restore`，而不是 `libra checkout -- <path>`。
- [x] [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) 断言覆盖顶层可见性和 help banner。
- [x] `COMPATIBILITY.md` checkout 行已更新。
- [x] (v0.17.11) 本轮最终回归已运行 `cargo test --test command_test checkout_test`。

## 风险与缓解

1. **新用户看到 checkout 误以为它完整覆盖 Git checkout 的分支和文件恢复重载语义** → 缓解：banner 文案明确"branch compatibility surface"，分支 example 标注 "prefer: libra switch"，文件恢复在文档中指向 `libra restore`；`COMPATIBILITY.md` 也写明 `partial` 而非 `supported`。
2. **取消 hide 后 shell 自动补全脚本需要刷新** → 缓解：clap 自动补全在重新生成时会自动包含 checkout，无需手动改；如有用户通过 `libra completions` 生成的旧脚本，建议在 release notes 提示重新生成。
3. **第 30 批完整现代化时与本批 banner 冲突** → 缓解：第 30 批不应动 `--help` 文案；本批的 banner 文案是稳定面向用户的对外契约，不随内部 typed error 升级而改变。
4. **`docs/commands/checkout.md` 与 switch / restore 文档重复维护** → 缓解：checkout 文档主要写"何时用 / 与 switch / restore 的对应关系"，不重复 switch / restore 的语义；交叉引用即可。

## 下一阶段：C9 文件恢复兼容入口

### 所属批次

C9（后续 Git surface P2）

### 当前缺口

C5 已让 `checkout` 成为可发现的分支类兼容入口。C9 已在不改变推荐命令的前提下补齐显式兼容入口：只有出现 `--` separator 时才进入 path mode；普通 `checkout <name>` 仍保持分支语义。

### 目标与非目标

**已落地目标：**

- `libra checkout -- <pathspec>...` 等价于从 index 恢复工作树路径，行为对应 `libra restore --worktree <pathspec>...`。
- `libra checkout <tree-ish> -- <pathspec>...` 等价于从指定 tree-ish 恢复路径，并按文档化语义更新 index + worktree。
- `libra switch` / `libra restore` 仍是推荐新工作流；`checkout -- <path>` 仅是 Git 兼容入口。
- `--json` / `--machine` 输出保持 checkout 单一 envelope，不让 delegated restore 输出污染 stdout。
- help、`docs/commands/checkout.md`、`docs/commands/restore.md` 和 `COMPATIBILITY.md` 已明确这是 explicit `--` separator path mode。

**非目标：**

- 不在 C9 引入 patch mode、`--ours` / `--theirs`、`--merge`、interactive restore 等高级 checkout path flags。
- 不把 `libra checkout <commit>` 扩展为 detached HEAD；这仍应走 `switch --detach` 或后续独立计划。
- 不改变 `restore` 的主入口地位；新文档仍应推荐用户直接使用 `libra restore`。

### 设计要点

解析以显式 `--` separator 区分 branch mode 与 path mode，避免把文件名误解析为分支名。checkout 执行结果扩展出 `action: "restore-paths"`，并把 delegated restore 的 `source`、`worktree`、`staged`、`restored_files`、`deleted_files` 放入 checkout data 的 `restore` 对象中。

兼容映射建议：

| Git muscle memory | Libra delegated behavior |
|-------------------|--------------------------|
| `libra checkout -- file.txt` | `libra restore --worktree file.txt` |
| `libra checkout HEAD -- file.txt` | `libra restore --source HEAD --staged --worktree file.txt` 或文档化的等价实现 |
| `libra checkout <tree-ish> -- dir/` | 从 `<tree-ish>` 恢复路径，按 restore 的 pathspec 规则处理 |

### `COMPATIBILITY.md` 行更新

C9 已更新 checkout 行：

```markdown
| checkout | partial | visible branch compatibility surface plus explicit `checkout -- <path>` restoration alias; prefer `switch` / `restore`; detached HEAD and patch modes still partial |
```

### 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/checkout.rs`](../../../src/command/checkout.rs) | 修改 | `--` path mode parser、restore delegation、JSON schema 扩展 |
| [`src/command/restore.rs`](../../../src/command/restore.rs) | 复用/评估 | 确认 source + staged/worktree 映射满足 Git checkout path 语义 |
| [`docs/commands/checkout.md`](../../commands/checkout.md) | 修改 | 增加 path restoration compatibility 示例 |
| [`docs/commands/restore.md`](../../commands/restore.md) | 修改 | 说明 checkout path mode 只是兼容别名 |
| [`tests/command/checkout_test.rs`](../../../tests/command/checkout_test.rs) | 修改 | `checkout -- path`、`checkout HEAD -- path`、JSON/machine |
| [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) | 修改 | help 文案继续推荐 restore，同时展示兼容入口 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | checkout notes 更新 |

### 测试与验收

- [x] `libra checkout -- file.txt` 从 index 恢复工作树文件，不切换分支。
- [x] `libra checkout HEAD -- file.txt` 从 HEAD 恢复文件，并按文档化语义更新 index/worktree。
- [x] 文件名与分支名相同时，只有显式 `--` path mode 触发文件恢复。
- [x] `--json` / `--machine` 输出只有 checkout envelope，且 action 明确为 `restore-paths`。
- [x] help 与命令文档仍把 `libra restore` 标为推荐入口。
- [ ] `cargo +nightly fmt --all --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all` 通过。

### 风险与缓解

1. **重新引入 Git checkout 的语义混乱**：只接受显式 `--` path mode，普通 `checkout <name>` 保持分支语义。
2. **delegated restore 输出破坏 checkout JSON schema**：由 checkout 包装 restore result，保证 stdout 只有一个 command envelope。
3. **source + path 映射与 Git 不一致**：先为 `checkout -- path` 和 `checkout HEAD -- path` 写回归测试，再扩展其他 tree-ish。
