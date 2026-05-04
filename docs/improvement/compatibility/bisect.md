# C4：Bisect 子命令面补齐（run / view）

## 所属批次

C4（Audit P2）；同时承担 bisect 模块"首次入计划"职责。

## 已完成前置条件与当前代码状态

### 已确认落地的基线
- [`src/command/bisect.rs`](../../../src/command/bisect.rs) 已实现 `Start` / `Bad` / `Good` / `Reset` / `Skip` / `Log` 六个子命令。
- [`src/cli.rs`](../../../src/cli.rs) 中 `Bisect` enum 已暴露上述六个 variant（cli.rs:325-356）。
- [`tests/command/bisect_test.rs`](../../../tests/command/bisect_test.rs) 已存在并覆盖人工 bisect 流程。
- [`docs/commands/bisect.md`](../../commands/bisect.md) 已存在，但当前显式说明 `bisect run` 不支持；C4 落地时必须把该设计理由改为新的 `run` / `view` 契约。
- [`docs/commands/README.md`](../../commands/README.md) 当前把 `libra bisect` 标为 hidden，但 `src/cli.rs` 中 `Bisect` 没有 `hide = true`；C4 需要同步修正文档索引。
- bisect 模块**尚未进入任何已落地的 CLIG 现代化批次**——没有 `BisectError` typed enum、没有 `BisectOutput`、没有 `run_bisect()` / `render_bisect_output()` 拆分、没有 JSON / machine 输出。

### 基于当前代码的 Review 结论
- 当前 bisect 命令的对外契约是"人工 bisect 可用、自动化 bisect 缺失"。审计 P2 把 `bisect run` 列为"自动化定位回归的关键能力"。
- `bisect run` 是脚本驱动定位的核心入口；缺它意味着 CI 中的回归定位必须手写循环。
- `bisect view` / `bisect visualize` 是查看当前 bisect 状态与剩余候选 commit 的便利入口。
- `bisect replay` / `bisect terms` 属于小众工作流，本批不做。
- 因为 bisect 没有完整 CLIG 基线，本批做"surface 补齐 + 最小现代化基础"二合一：补 `run` / `view` 子命令的同时，把模块至少升级到具备 `BisectError` 与 `BisectOutput` 的最小可演化形态；完整 typed error / render split 收口归后续批次（README 后续批次 32 之后）。

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

- [ ] `cargo run -- bisect --help` 列出 `run` / `view`。
- [ ] `bisect run` 在故意失败的 commit 上正确收敛到 first-bad；步数与 skipped 列表与 JSON 输出一致。
- [ ] `bisect run` 能正确透传被测命令自身的 flags，例如 `libra bisect run cargo test -- --ignored`。
- [ ] `bisect run` 中脚本返回 125 时正确 skip 并继续。
- [ ] `bisect run` 中脚本返回 128 时终止 bisect 并产生 `BISECT_RUN_FAILED` 错误码。
- [ ] `bisect view` 在 active bisect 中显示当前状态；不在 bisect 中时返回 `NOT_IN_BISECT`。
- [ ] `cargo test bisect_test` 全部通过。
- [ ] `docs/commands/bisect.md` 与命令实际输出一致。

## 风险与缓解

1. **`bisect run` 长跑命令对 CI 资源冲击** → 缓解：`run` 不引入额外缓存或并行；测试用例使用极短脚本（如 `bash -c 'exit 0'`）。
2. **退出码 125 与 128 边界处理错误** → 缓解：测试用例显式覆盖 0 / 1 / 125 / 128 / 130（SIGINT）五个等价类。
3. **bisect 状态文件破坏后用户无法恢复** → 缓解：`bisect view` 在状态损坏时返回 `NOT_IN_BISECT`，并提示 `bisect reset` 清理；`bisect log` 仍可读取历史（已实现）。
4. **本批最小 `BisectError` 与未来完整 typed enum 冲突** → 缓解：variant 命名留出扩展空间（不用 `Other(String)` 之外的兜底命名）；后续完整批次只新增不破坏。
5. **新增三个错误码与既有 18 个稳定错误码的 churn** → 缓解：错误码命名遵循现有命名规则（`<DOMAIN>_<REASON>`）；在 [`docs/error-codes.md`](../../error-codes.md) 同步 changelog。
