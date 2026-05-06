# C3：浅克隆契约（fetch / clone / sparse 决策）

## 所属批次

C3（Audit P1）

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- [`src/command/clone.rs`](../../../src/command/clone.rs) 已暴露 `--depth` 和 `--single-branch`（[clone.rs:81-89](../../../src/command/clone.rs#L81)）。
- [`src/command/fetch.rs`](../../../src/command/fetch.rs) 内部 `fetch_repository(..., depth: Option<usize>)` 已支持 depth 参数（fetch.rs:135 附近），但 `FetchArgs` 仅暴露 `repository` / `refspec` / `--all` 三个字段，**未暴露 `--depth`**。
- 第 5 批 [fetch.md](../fetch.md) 与 [`tests/command/fetch_test.rs`](../../../tests/command/fetch_test.rs) 已覆盖 fetch 顶层 JSON / machine / 错误码契约，但浅克隆相关的端到端用例缺失。
- [`docs/commands/clone.md`](../../commands/clone.md) 已包含 `--depth` 示例与参数对比；C3 只需复核它与最终 `COMPATIBILITY.md` 表述一致，不需要重写 clone 文档。
- [`docs/commands/fetch.md`](../../commands/fetch.md) 当前参数对比仍写 "Shallow fetch: Not supported"，C3 公开 `fetch --depth` 时必须同步更新该行。
- `clone --sparse` 和 `clone --recurse-submodules` 都**未暴露**；前者依赖内部 sparse-checkout reader，后者依赖 submodule（README 已声明不在产品边界）。

### 基于当前代码的 Review 结论
- 内部能力（depth）已经存在但没有公开入口，造成"功能似乎存在但没人知道能不能用"的困境，正是审计 P1 的典型示例。
- `clone --depth` 已存在且命令文档已有示例，但根 `COMPATIBILITY.md` 还没有成为事实表；与 `fetch --depth` 同步公开后才是完整对外契约。
- `clone --sparse` 不应在本批强行加 CLI——若内部不支持，宁可在 `COMPATIBILITY.md` 标 `unsupported` 也不发明半成品 flag。

## 目标与非目标

**目标：**
- 在 `FetchArgs` 增 `--depth <N>` 字段，wiring 到现有 `fetch_repository(..., depth)` 路径。
- 在 [`docs/commands/fetch.md`](../../commands/fetch.md) 与 [`docs/improvement/fetch.md`](../fetch.md) 追加"审计驱动增量"小节，说明浅克隆契约。
- 在 [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) 更新三行：fetch（标 `supported`，notes `--depth public flag`）；clone shallow（标 `supported`）；clone sparse（标 `unsupported`）。
- 在 [`tests/command/fetch_test.rs`](../../../tests/command/fetch_test.rs) 新增至少 3 条浅克隆集成测试。

**非目标：**
- 不实现 `clone --sparse`；仅在 `COMPATIBILITY.md` 显式标 `unsupported`。
- 不实现 `clone --recurse-submodules`；submodule 在 [declined.md](declined.md) 登记为产品边界。
- 不改动 `fetch_repository` 内部签名；只接 CLI flag 进现有 wiring。
- 不引入"渐进 deepen"语义（`--shallow-since` / `--shallow-exclude` 等高级 flag）；这些归后续独立评估。

## 设计要点

### `FetchArgs` 扩展

[`src/command/fetch.rs`](../../../src/command/fetch.rs) 现有 `FetchArgs` 增字段：

```rust
#[derive(Args, Debug)]
pub struct FetchArgs {
    pub repository: Option<String>,
    pub refspec: Option<String>,
    #[clap(long)]
    pub all: bool,
    /// Limit fetching to the specified number of commits from the tip of each remote branch
    #[clap(long, value_name = "N")]
    pub depth: Option<usize>,  // ← 新增
}
```

执行层把 `args.depth` 透传给现有 `fetch_repository(..., depth)`；不需要新增 `FetchError` 变体。

### `--help` 文案

```
USAGE:
    libra fetch [OPTIONS] [REPOSITORY] [REFSPEC]

OPTIONS:
        --all           Fetch all remotes
        --depth <N>     Limit fetching to the specified number of commits
                        from the tip of each remote branch
    -h, --help          Print help

EXAMPLES:
    libra fetch
    libra fetch origin
    libra fetch origin main
    libra fetch --all
    libra fetch origin --depth 1   # shallow fetch
```

`--depth` 在 `--help` 中**不带 experimental 标记**（用户决策已确认公开为稳定 flag）。

### `clone --depth` 现状文档化

[`docs/commands/clone.md`](../../commands/clone.md) 在 EXAMPLES 小节确认：

```
libra clone https://github.com/user/repo --depth 1            # shallow clone
libra clone https://github.com/user/repo --depth 1 --single-branch
```

不需要代码变更，只在文档中显式承诺。

### `clone --sparse` 决策

基于内部代码核查，当前 `src/internal/` 不存在 sparse-checkout reader 实现，且根据产品边界定位，sparse-checkout 高度依赖 Git 目录配置（已整体迁移至 SQLite），桥接代价较高。

因此结论为：
- `COMPATIBILITY.md` 标 `unsupported`。
- 本文档指出："未来若有单体仓库必须部分检出的确实需求，将发独立 RFC 评估对象存储 + 虚拟文件系统的工程方案，当前不支持"。

### `clone --recurse-submodules` 决策

`COMPATIBILITY.md` 标 `unsupported`，notes 指向 [declined.md](declined.md)；理由：依赖 submodule，而 submodule 在 README 已声明不在产品边界。

### `COMPATIBILITY.md` 行更新

```markdown
| fetch | supported | --depth public flag (C3) |
| clone | partial | --depth supported; --single-branch supported; --sparse unsupported; --recurse-submodules unsupported |
```

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/command/fetch.rs`](../../../src/command/fetch.rs) | 修改 | `FetchArgs` 加 `--depth`；wiring 到 `fetch_repository` |
| [`docs/commands/fetch.md`](../../commands/fetch.md) | 修改 | EXAMPLES + `--help` schema + 参数对比表同步；删除或重写 "Why no --depth/--shallow?" 设计理由段落 |
| [`docs/improvement/fetch.md`](../fetch.md) | 追加 | "审计驱动增量"小节（仅引用，不重写已有内容） |
| [`docs/commands/clone.md`](../../commands/clone.md) | 复核/必要时修改 | 已有浅克隆示例；确认 `--sparse` / `--recurse-submodules` 说明与 `COMPATIBILITY.md` 一致 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | fetch / clone 两行 |
| [`tests/command/fetch_test.rs`](../../../tests/command/fetch_test.rs) | 修改 | 新增 ≥3 条浅克隆用例 |

## 测试与验收

- [ ] `cargo run -- fetch --help` 输出包含 `--depth <N>`，且不包含 "experimental" 字样。
- [ ] `cargo run -- fetch origin main --depth 1` 执行成功；产生的 `.libra` 元数据反映 shallow 状态。
- [ ] 集成测试覆盖：
  - 单分支仓库 + `--depth 1`。
  - 多分支仓库 + `--depth 3` + `--all`（depth 应作用于全部 remote）。
  - 已经是浅克隆的仓库再次 `fetch --depth 1`（幂等性 / 不应报错）。
- [ ] `COMPATIBILITY.md` 中 fetch / clone 行已更新。
- [ ] `cargo test fetch_test` 全部通过。

## 风险与缓解

1. **过早承诺 stable 后内部 plumbing 回归** → 缓解：本批必须先补足 ≥3 条浅克隆测试再公开 flag；任何回归在 `compat-offline-core` job（或 `compat-network-remotes`）中即时暴露。
2. **用户期望 `clone --sparse` 同步公开** → 缓解：`--help` 与 `COMPATIBILITY.md` 都明确说明 sparse 当前 `unsupported`；shallow.md 写明"非缺口，是有意延后"。
3. **`--depth` 与现有 `--all` 组合时语义不清** → 缓解：测试用例显式覆盖 `--all --depth N`；文档说明 depth 同时作用于所有被 fetch 的 remote。
4. **shallow fetch 与现有 transport（http/ssh/file）兼容性差异** → 缓解：测试用例至少覆盖 http transport；ssh / file 浅克隆作为已知限制写进 `COMPATIBILITY.md` notes（如适用）。
