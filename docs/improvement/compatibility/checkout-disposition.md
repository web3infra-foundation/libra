# C5：Checkout 命令处置（取消 hide + 兼容别名）

## 所属批次

C5（Audit P2）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- [`src/cli.rs:287-291`](../../../src/cli.rs#L287) 中 `Checkout` 标 `hide = true`；`libra --help` 顶层列表当前**不显示** checkout。
- [`src/command/checkout.rs`](../../../src/command/checkout.rs) 已实现 checkout 的分支类基础语义（显示当前分支、切换本地分支、`-b` 新建并切换、远端同名分支 auto-track + pull）。内部使用 `restore` 将工作树 materialize 到目标 commit，但当前 CLI **不支持** `git checkout -- <path>` 形式的文件恢复；文件恢复仍由 `libra restore` 承担。
- 第 30 批（reflog / checkout 完整现代化）尚未启动——`CheckoutError` typed enum / `CheckoutOutput` / JSON / render split 均不存在。
- [docs/improvement/checkout.md](../checkout.md) 已记录"第二批兼容收口已落地，完整现代化留第 30 批"。
- [`docs/commands/checkout.md`](../../commands/checkout.md) 已存在并说明 checkout 是兼容 surface；C5 需要把其中"hidden / 不 prominently featured"等旧表述改成"顶层可见但推荐 switch / restore"。
- [`docs/commands/README.md`](../../commands/README.md) 当前把 checkout 标为 hidden；C5 必须同步更新命令索引。
- 用户决策已确认：**取消 `hide = true`，正式作为分支类兼容入口**；文件恢复继续通过 `libra restore` 暴露。

### 基于当前代码的 Review 结论
- 隐藏 checkout 让该命令处于"存在但不解释"的状态；外部用户用 stock Git 心智进入项目时无法发现命令存在。
- 第 30 批的完整现代化目标与本批的"取消 hide + 文案调整"互不冲突——后者只动 `src/cli.rs` 的标志位与 `--help` 文本，不动内部实现。
- 取消 hide 后必须明确写"checkout 是分支类兼容入口；新流程推荐 switch，文件恢复推荐 restore"，避免新用户误以为 checkout 已完整覆盖 Git checkout 的所有重载语义。

## 目标与非目标

**目标：**
- 在 [`src/cli.rs`](../../../src/cli.rs) 把 `Checkout` 的 `hide = true` 删除（或改 `hide = false`），让 `libra --help` 顶层列表显示 checkout。
- 修改 [`src/command/checkout.rs`](../../../src/command/checkout.rs) 中已存在的 `CHECKOUT_EXAMPLES` 常量内容，加入 "branch compatibility surface; prefer switch / restore" 提示。
- 在 [`docs/commands/checkout.md`](../../commands/checkout.md) 顶部加迁移说明：何时使用 checkout、何时推荐 switch / restore。
- `COMPATIBILITY.md` 中 checkout 行更新为 `partial`，notes "branch compatibility surface; use restore for file restoration; full modernization pending batch 30"。
- 在 [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) 加一条断言：`libra --help` 顶层文本包含 "checkout"。

**非目标：**
- 不改 checkout 内部实现（typed error / JSON / render split 归第 30 批）。
- 不 deprecate checkout——它是有意保留的兼容入口，不是过渡性废弃。
- 不引入 `--no-hint` 或动态隐藏机制；取消 hide 是无条件的。
- 不修改 switch / restore 的 `--help` 文案——它们已在第二批稳定，本批不动。

## 设计要点

### `src/cli.rs` 变更

```rust
// Before
#[command(hide = true, ...)]
Checkout(CheckoutArgs),

// After
#[command(after_help = command::checkout::CHECKOUT_EXAMPLES, ...)]
Checkout(CheckoutArgs),
```

去掉 `hide = true`（或显式写 `hide = false`，但 clap 默认就是 `false`，可直接省略）。

### `--help` banner 文案

`CHECKOUT_EXAMPLES` 常量修改示意（替换现有常量内容）：

```rust
pub const CHECKOUT_EXAMPLES: &str = "\
EXAMPLES:
  libra checkout main                # switch to branch (prefer: libra switch main)
  libra checkout -b feature/x        # create branch (prefer: libra switch -c feature/x)
";
```

`src/command/checkout.rs` 中 `CheckoutArgs` 已有 `#[command(after_help = CHECKOUT_EXAMPLES)]`（第 33 行），无需改动属性位置；只需替换常量内容。

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

- 若 C5 先落地：第 30 批启动时，checkout 已可见且有 examples；只需把内部升级为 typed `CheckoutError` / JSON / render split，不动 `--help` 文案。
- 若第 30 批先落地：C5 启动时，去 hide 是 trivial 改动；CHECKOUT_EXAMPLES 已存在，本批仅改 banner 文案。
- 两批互不阻塞，但建议 C5 先于第 30 批，因为可见性是即时收益，typed error 是渐进收益。

### `COMPATIBILITY.md` 行更新

```markdown
| checkout | partial | branch compatibility surface. Use `switch` for branch workflows and `restore` for file restoration. Full modernization pending. |
```

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/cli.rs`](../../../src/cli.rs) | 修改 | 删除 `Checkout` 上的 `hide = true` |
| [`src/command/checkout.rs`](../../../src/command/checkout.rs) | 修改 | 替换 `CHECKOUT_EXAMPLES` 常量内容为兼容提示文案 |
| [`docs/commands/checkout.md`](../../commands/checkout.md) | 修改 | 顶部迁移说明：何时用 checkout / 何时推荐 switch / restore；删除 hidden 旧表述 |
| [`docs/commands/README.md`](../../commands/README.md) | 修改 | 移除 checkout 的 hidden 标记，确保命令索引与顶层 help 一致 |
| [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) | 新建 | 断言 `libra --help` 包含 "checkout" 与兼容提示 |
| [`tests/command/checkout_test.rs`](../../../tests/command/checkout_test.rs) | 修改 | 新增 1 条 `--help` 文本断言 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | checkout 行更新 |

## 测试与验收

- [ ] `cargo run -- --help` 顶层 COMMANDS 列表包含 `checkout`。
- [ ] `cargo run -- checkout --help` 顶部出现 branch compatibility / prefer switch / restore 文案。
- [ ] EXAMPLES 中分支类示例包含 "prefer: libra switch" 提示；文档明确文件恢复使用 `libra restore`，而不是 `libra checkout -- <path>`。
- [ ] [`tests/compat/checkout_alias_help.rs`](../../../tests/compat/checkout_alias_help.rs) 断言通过。
- [ ] `COMPATIBILITY.md` checkout 行已更新。
- [ ] `cargo test checkout_test` 全部通过。

## 风险与缓解

1. **新用户看到 checkout 误以为它完整覆盖 Git checkout 的分支和文件恢复重载语义** → 缓解：banner 文案明确"branch compatibility surface"，分支 example 标注 "prefer: libra switch"，文件恢复在文档中指向 `libra restore`；`COMPATIBILITY.md` 也写明 `partial` 而非 `supported`。
2. **取消 hide 后 shell 自动补全脚本需要刷新** → 缓解：clap 自动补全在重新生成时会自动包含 checkout，无需手动改；如有用户通过 `libra completions` 生成的旧脚本，建议在 release notes 提示重新生成。
3. **第 30 批完整现代化时与本批 banner 冲突** → 缓解：第 30 批不应动 `--help` 文案；本批的 banner 文案是稳定面向用户的对外契约，不随内部 typed error 升级而改变。
4. **`docs/commands/checkout.md` 与 switch / restore 文档重复维护** → 缓解：checkout 文档主要写"何时用 / 与 switch / restore 的对应关系"，不重复 switch / restore 的语义；交叉引用即可。
