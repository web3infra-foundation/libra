# C4：Bisect 子命令面补齐（run / view）

## 所属批次

C4（Audit P2）；同时承担 bisect 模块"首次入计划"职责。

## 已完成前置条件与当前代码状态

### 已确认落地的基线（2026-05-11 复核）
- [`src/command/bisect.rs`](../../../src/command/bisect.rs) 已实现 `Start` / `Bad` / `Good` / `Reset` / `Skip` / `Log` / `Run` / `View` 八个子命令，并暴露 `BISECT_EXAMPLES`。
- [`src/cli.rs`](../../../src/cli.rs) 中 `Bisect` enum 已暴露 `run` / `view`，顶层命令没有隐藏。
- [`tests/command/bisect_test.rs`](../../../tests/command/bisect_test.rs) 已覆盖人工 bisect、`view` 无状态错误、`run` 边界和 help banner。
- [`tests/compat/bisect_subcommand_surface.rs`](../../../tests/compat/bisect_subcommand_surface.rs) 已固定 `bisect --help` 必须列出 `run` / `view` 并包含 EXAMPLES banner。
- [`docs/commands/bisect.md`](../../commands/bisect.md) 已记录 `bisect run` 的 0 / 1..124 / 125 / 128+ 退出码语义和 `bisect view` 契约。
- bisect 仍未进入完整 CLIG 现代化批次：没有 `BisectOutput` JSON schema、没有全量 `BisectError` typed enum、没有 `run_bisect()` / `render_bisect_output()` 完整拆分。这部分已从 C4 surface 补齐中剥离，归 README 后续 `reflog / checkout` 之后的跨命令 error/render 收口处理。

### 基于当前代码的 Review 结论
- C4 的自动化 surface 已落地：`bisect run` 可驱动脚本并按 Git-compatible exit code 自动标记 good/bad/skip，`bisect view` 可查看当前状态。
- `bisect replay` / `bisect terms` 仍按 [declined.md](declined.md) 的显式延后项处理，不属于当前交付缺口。
- 完整 CLIG JSON / machine / typed error modernization 仍是后续内部一致性工作；它不能反向把 C4 的 `run` / `view` surface 重新标成未落地。

## 目标与非目标

**目标：**
- 在 `Bisect` enum 新增 `Run` / `View` 两个 variant。
- `bisect run <cmd> [args...]`：脚本驱动定位，退出码语义对齐 Git：
  - 0 → good
  - 1–124, 126–127 → bad
  - 125 → skip（cannot test this commit）
  - 128+ → 终止 bisect 并向用户报错
- `bisect view`：展示当前 bisect 状态——剩余候选 commit 数、当前 HEAD、good / bad 边界、已 skip 列表。
- 引入最小 `BisectError` 与 `BisectOutput`（不要求 18 变体的完整覆盖；至少覆盖 `NotInBisect` / `RunCommandFailed { exit_code }` / `NoMoreCandidates` / `Other`）。
- 将 `BisectError` 接入 `StableErrorCode`；`bisect run` 的脚本退出码通过 `RunCommandFailed { exit_code }` 透传给上层。
- **新建** `BISECT_EXAMPLES` 常量（当前 `src/command/bisect.rs` 不存在该常量），加入 `bisect run` 与 `bisect view` 示例，并在 `Bisect` enum 的 `#[command(...)]` 中加 `after_help = BISECT_EXAMPLES`。
- `COMPATIBILITY.md` 中 bisect 行更新为 `partial`，notes "run / view added in C4; replay / terms deferred"。

**非目标：**
- 不实现 `bisect replay`（从日志重放历史 bisect）；登记到 [declined.md](declined.md)，重启条件：用户明确请求且 `bisect log` 已稳定。
- 不实现 `bisect terms`（自定义 good/bad 别名，如 fast/slow）；登记到 [declined.md](declined.md)。
- 不在本批做 bisect 完整 CLIG 现代化（`run_bisect()` / `render_bisect_output()` 大改造、JSON 完整 envelope、所有错误路径 typed）。这部分留给 README 后续批次。
- 不引入 `bisect visualize` 调外部 GUI（Git 的 visualize 走 gitk / Tk）；`view` 仅文本输出。

## 设计要点

### `Bisect` enum 扩展

```rust
pub enum Bisect {
    Start(BisectStartArgs),
    Bad(BisectBadArgs),
    Good(BisectGoodArgs),
    Reset(BisectResetArgs),
    Skip(BisectSkipArgs),
    Log,
    Run(BisectRunArgs),   // 新增
    View,                  // 新增
}

#[derive(Args, Debug)]
pub struct BisectRunArgs {
    /// Command to run for each commit; first arg is the executable
    #[clap(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
}
```

`trailing_var_arg` 与 `allow_hyphen_values` 是必要契约：`libra bisect run cargo test -- --ignored` 这类命令必须把后续 `--ignored` 传给测试命令，而不是被 clap 当成 `libra bisect run` 自身参数解析。

### `bisect run` 退出码处理

```rust
match output.status.code() {
    Some(0) => mark_good(...),
    Some(125) => mark_skip(...),
    Some(c) if (1..=127).contains(&c) => mark_bad(...),
    Some(c) if c >= 128 => abort_bisect_with_error(c),
    None => abort_bisect_with_signal(),
}
```

每次迭代后自动 advance 到下一个候选；直到收敛到 first-bad commit 或耗尽候选。

### `BisectOutput` 最小集

```rust
#[derive(Serialize)]
#[serde(tag = "action")]
pub enum BisectOutput {
    Started { good: String, bad: String, candidates: usize },
    Marked { mark: String, commit: String, remaining: usize },  // mark = "good" | "bad" | "skip"
    Converged { first_bad: String, steps: usize },
    Reset,
    Run {
        first_bad: Option<String>,
        steps: usize,
        skipped: Vec<String>,
    },
    View {
        head: String,
        good: Option<String>,
        bad: Option<String>,
        remaining: usize,
        skipped: Vec<String>,
    },
    Log { entries: Vec<BisectLogEntry> },
}
```

### `BisectError` 最小集

```rust
#[derive(thiserror::Error, Debug)]
pub enum BisectError {
    #[error("not in an active bisect; run `libra bisect start` first")]
    NotInBisect,
    #[error("bisect run command failed with non-recoverable exit code {exit_code}")]
    RunCommandFailed { exit_code: i32 },
    #[error("no more candidate commits; bisect already converged")]
    NoMoreCandidates,
    #[error("{0}")]
    Other(String),
}
```

`StableErrorCode` 映射：

| 变体 | 错误码 |
|------|------|
| `NotInBisect` | `NOT_IN_BISECT`（新增） |
| `RunCommandFailed` | `BISECT_RUN_FAILED`（新增） |
| `NoMoreCandidates` | `BISECT_NO_CANDIDATES`（新增） |
| `Other(_)` | `INTERNAL` |

新错误码需在 [`src/utils/error.rs`](../../../src/utils/error.rs) 加变体并同步 [`docs/error-codes.md`](../../error-codes.md)。

### Human 输出示例

```
$ libra bisect run cargo test --test foo
Bisecting: 5 candidates remaining
Running cargo test --test foo at abc1234... PASS (good)
Bisecting: 2 candidates remaining
Running cargo test --test foo at def5678... FAIL (bad)
Bisecting: 1 candidate remaining
Running cargo test --test foo at 901abcd... FAIL (bad)
Converged: first bad commit is 901abcd
3 steps, 0 skipped

$ libra bisect view
Bisecting between abc1234 (good) and def5678 (bad)
HEAD: 901abcd
Remaining: 1 candidate
Skipped: (none)
```

### JSON 输出示例

```json
{"action": "run", "first_bad": "901abcd", "steps": 3, "skipped": []}
{"action": "view", "head": "901abcd", "good": "abc1234", "bad": "def5678", "remaining": 1, "skipped": []}
```

## 关键文件与改动

| 文件 | 操作 | 说明 |
|-----|-----|-----|
| [`src/cli.rs`](../../../src/cli.rs) | 修改 | `Bisect` enum 加 `Run` / `View` |
| [`src/command/bisect.rs`](../../../src/command/bisect.rs) | 修改 | `BisectRunArgs`；`Run` / `View` handler；`BisectOutput` / `BisectError` 引入；`BISECT_EXAMPLES` |
| [`src/utils/error.rs`](../../../src/utils/error.rs) | 修改 | 新增三个 `StableErrorCode` 变体 |
| [`docs/error-codes.md`](../../error-codes.md) | 修改 | 同步三个新错误码 |
| [`tests/command/bisect_test.rs`](../../../tests/command/bisect_test.rs) | 修改 | 新增 ≥4 条用例（run 收敛、run 全 skip、view 展示、退出码 125 / 128 处理） |
| [`tests/compat/bisect_subcommand_surface.rs`](../../../tests/compat/bisect_subcommand_surface.rs) | 新建 | `--help` 列出 `run` / `view` 的断言 + JSON schema |
| [`docs/commands/bisect.md`](../../commands/bisect.md) | 修改 | 同步 `run` / `view`，删除 "bisect run not supported" 的旧立场 |
| [`docs/commands/README.md`](../../commands/README.md) | 修改 | 移除 bisect 的 hidden 标记，确保命令索引与 `src/cli.rs` 一致 |
| [`COMPATIBILITY.md`](../../../COMPATIBILITY.md) | 修改 | bisect 行更新 |

## 测试与验收

- [x] `cargo run -- bisect --help` 列出 `run` / `view`。
- [x] `bisect run` 在故意失败的 commit 上正确收敛到 first-bad；human 输出记录步数与 skipped 数。
- [x] `bisect run` 能正确透传被测命令自身的 flags，例如 `libra bisect run cargo test -- --ignored`。
- [x] `bisect run` 中脚本返回 125 时正确 skip 并继续。
- [x] `bisect run` 中脚本返回 128 时终止 bisect 并产生 `BISECT_RUN_FAILED` 错误码。
- [x] `bisect view` 在 active bisect 中显示当前状态；不在 bisect 中时返回 `NOT_IN_BISECT`。
- [x] `docs/commands/bisect.md` 与命令实际输出一致。
- [x] (v0.17.11) 本轮最终回归已运行 `cargo test --test command_test bisect_test`。
- [ ] 完整 JSON / machine schema 仍未交付，归后续 CLIG error/render 收口，不属于 C4 `run` / `view` surface gate。

## 风险与缓解

1. **`bisect run` 长跑命令对 CI 资源冲击** → 缓解：`run` 不引入额外缓存或并行；测试用例使用极短脚本（如 `bash -c 'exit 0'`）。
2. **退出码 125 与 128 边界处理错误** → 缓解：测试用例显式覆盖 0 / 1 / 125 / 128 / 130（SIGINT）五个等价类。
3. **bisect 状态文件破坏后用户无法恢复** → 缓解：`bisect view` 在状态损坏时返回 `NOT_IN_BISECT`，并提示 `bisect reset` 清理；`bisect log` 仍可读取历史（已实现）。
4. **本批最小 `BisectError` 与未来完整 typed enum 冲突** → 缓解：variant 命名留出扩展空间（不用 `Other(String)` 之外的兜底命名）；后续完整批次只新增不破坏。
5. **新增三个错误码与既有 18 个稳定错误码的 churn** → 缓解：错误码命名遵循现有命名规则（`<DOMAIN>_<REASON>`）；在 [`docs/error-codes.md`](../../error-codes.md) 同步 changelog。
