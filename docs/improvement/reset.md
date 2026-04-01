## Reset 命令改进详细计划

> 最后编写时间：2026-04-01

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

> 当前工作区实现已按本文范围落地一部分改动；以下内容改为记录已落地能力、剩余遗漏和后续收口项。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`reset` 是第二批（状态变更确认命令）中改变仓库状态最激烈的命令——用户必须知道 HEAD 移动到了哪里。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在
- `run_reset()` + `render_reset_output()` 执行层/渲染层拆分已落地
- `ResetOutput` 已定义，`--json` / `--machine` 已通过 `emit_json_data("reset", ...)` 输出结构化结果
- `--soft` / `--mixed` / `--hard` 三种模式已实现
- pathspec 支持已实现（reset 特定文件）
- reflog 记录已集成（`ReflogContext` + `with_reflog()`）
- `reset_index_to_commit()` / `reset_working_directory_to_commit()` 核心逻辑已实现
- `rebuild_index_from_tree()` 和 `restore_working_directory_from_tree()` 已实现
- 空目录清理 `remove_empty_directories()` 已实现
- human 成功确认消息已落地：全量 reset 输出 `HEAD is now at <short-hash> <subject>`；pathspec reset 输出 `Unstaged changes after reset:`
- pathspec 与 `--soft` / `--hard` 的冲突校验已接入显式 `StableErrorCode`

**基于当前代码的 Review 结论（已改进部分 vs 仍需改进部分）：**

已改进（当前代码已具备）：

- **结构化输出已落地**：`ResetOutput` + `render_reset_output()` 已覆盖 human / `--json` / `--machine` / `--quiet`
- **执行层与渲染层已拆分**：`execute_safe()` 调用 `run_reset()` 收集结构化结果，再统一渲染
- **成功确认消息已落地**：全量 reset 会输出 `HEAD is now at ...`，pathspec reset 会输出 unstaged 文件列表
- **主要错误已带显式 `StableErrorCode`**：invalid revision、pathspec/mode 冲突、repo corrupt、I/O 失败等路径都已显式映射
- **JSON 回归测试已存在**：`tests/command/reset_test.rs` 已覆盖 `--json` schema、`--hard HEAD` restore 计数和 pathspec usage error

仍需改进：

- **无 `ResetError` typed enum**：`run_reset()` 仍返回 `CliResult<ResetOutput>`，typed error 收口尚未完成
- **运行时错误仍是 stringly typed**：`perform_reset()` / `remove_empty_directories()` 等内部 helper 仍返回 `Result<T, String>`，`map_reset_runtime_error()` 依赖关键词匹配分类，较脆弱
- **pathspec 错误仍未纳入 typed enum**：如 `path contains invalid UTF-8`、`pathspec ... did not match any file(s) known to libra` 仍是 inline `CliError`
- **非致命 warning 仍直写 stderr**：目录清理失败仍通过 `eprintln!()` 输出，尚未接入共享 warning/output 管线
- **缺少 `--help` EXAMPLES 段**：Cross-Cutting **B** 在 `reset` 上仍未落地
- **Cross-Cutting `G` 尚未接入**：意外内部错误还未统一附带 Issues URL

### 目标与非目标

**本批目标：**
- 引入 `ResetError` typed error enum，收口剩余的 string-based runtime 错误路径
- 将 pathspec 相关的用户输入错误一并纳入 typed enum，避免残留 inline `CliError`
- 将 `perform_reset()` / `remove_empty_directories()` 等 helper 从 `Result<T, String>` 升级到 typed error
- 将非致命 cleanup warning 接入共享 `emit_warning()` / warning tracker，避免直写 stderr，同时不改变现有 `ResetOutput` JSON schema
- 保持已落地的 `ResetOutput` / JSON / human 确认消息契约不回退
- 补齐 `--help` EXAMPLES 段，并为异常内部错误预留 Issues URL 接入点

**本批非目标：**
- **不改变 soft/mixed/hard reset 核心逻辑**。索引重建和工作树恢复行为不变
- **不引入 `--patch` 交互式 reset**
- **不引入 `--keep` 模式**
- **不改变 pathspec reset 行为**

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_reset()` 收集结构化结果，再根据 `OutputConfig` 渲染
2. **成功时必须确认**：human 模式下输出 `HEAD is now at <short-hash> <subject>`
3. **错误码显式映射**：每个 `ResetError` 变体都有确定的 `StableErrorCode`
4. **内部函数错误类型升级**：从 `Result<T, String>` 升级到 `Result<T, ResetError>`
5. **warning 不进入 `ResetOutput` schema**：cleanup warning 通过共享 warning 管线输出并参与 `--exit-code-on-warning`，不新增 JSON 字段污染已稳定的 success schema

### 特性 1：ResetError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ResetError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("invalid revision: '{0}'")]
    InvalidRevision(String),

    #[error("HEAD is unborn — no commits in this repository")]
    HeadUnborn,

    #[error("failed to load commit '{commit_id}': {detail}")]
    CommitLoad { commit_id: String, detail: String },

    #[error("failed to load tree: {0}")]
    TreeLoad(String),

    #[error("failed to load index: {0}")]
    IndexLoad(String),

    #[error("failed to save index: {0}")]
    IndexSave(String),

    #[error("failed to update HEAD: {0}")]
    HeadUpdate(String),

    #[error("failed to restore working tree: {0}")]
    WorktreeRestore(String),

    #[error("path contains invalid UTF-8: {0}")]
    InvalidPathspecEncoding(String),

    #[error("pathspec '{0}' is not compatible with --soft reset")]
    PathspecWithSoft(String),

    #[error("cannot do hard reset with paths")]
    PathspecWithHard,

    #[error("pathspec '{0}' did not match any file(s) known to libra")]
    PathspecNotMatched(String),
}
```

**`ResetError → CliError` 显式映射：**

| ResetError 变体 | StableErrorCode | 退出码 | hint |
|----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidRevision` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `HeadUnborn` | `RepoStateInvalid` | 128 | `create a commit first` |
| `CommitLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `TreeLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted` |
| `IndexSave` | `IoWriteFailed` | 128 | 无 |
| `HeadUpdate` | `IoWriteFailed` | 128 | 无 |
| `WorktreeRestore` | `IoWriteFailed` | 128 | 无 |
| `InvalidPathspecEncoding` | `CliInvalidArguments` | 129 | `rename the path or invoke libra from a path representable as UTF-8` |
| `PathspecWithSoft` | `CliInvalidArguments` | 129 | `--soft only moves HEAD; use --mixed to reset index for specific paths` |
| `PathspecWithHard` | `CliInvalidArguments` | 129 | `--hard updates the working tree; omit pathspecs or use --mixed for specific paths` |
| `PathspecNotMatched` | `CliInvalidTarget` | 129 | `check the path and try again` |

**与当前代码中 inline 错误的对应关系：**

| 当前代码位置 | 当前 inline 错误 | 对应 ResetError 变体 |
|-------------|-----------------|---------------------|
| `run_reset:113` | `util::require_repo().map_err(...)` | `NotInRepo` |
| `run_reset:126-131` | `command_usage("pathspec ... is not compatible with --soft reset")` | `PathspecWithSoft` |
| `run_reset:133-138` | `command_usage("Cannot do hard reset with paths.")` | `PathspecWithHard` |
| `run_reset:141-143` | `resolve_commit().map_err(map_reset_invalid_revision)` | `InvalidRevision` |
| `run_reset:159-161` | `resolve_commit().map_err(map_reset_invalid_revision)` | `InvalidRevision` |
| `run_reset:163-165` | `perform_reset().map_err(map_reset_runtime_error)` | 见下方 `map_reset_runtime_error` 分拆 |
| `reset_pathspecs:206-212` | `path contains invalid UTF-8` | `InvalidPathspecEncoding` |
| `reset_pathspecs:236-240` | `pathspec ... did not match any file(s) known to libra` | `PathspecNotMatched` |
| `map_reset_runtime_error:740-741` | `message.contains("HEAD is unborn")` | `HeadUnborn` |
| `map_reset_runtime_error:742-749` | `message.contains("load commit/tree/index/blob")` | `CommitLoad` / `TreeLoad` / `IndexLoad` |
| `map_reset_runtime_error:734-739` | `message.contains("save index/write file/update HEAD")` | `IndexSave` / `HeadUpdate` / `WorktreeRestore` |
| `remove_empty_directories:600-605,617-621` | `eprintln!("warning: failed to remove empty directory")` | 改为收集 warning 字符串，经 `emit_warning()` / warning tracker 输出（非致命，不映射为 ResetError） |

### 特性 2：执行层与渲染层拆分

**已落地部分（无需变更）：** `ResetOutput` 结构体、`render_reset_output()` 渲染函数、`execute_safe()` → `run_reset()` → `render_reset_output()` 调用链均已存在。

**本批变更：`run_reset()` 返回内部执行结果，显式携带 warning**

为避免把 cleanup warning 塞进已稳定的 `ResetOutput` JSON schema，本批引入一个**仅命令内部使用**的包装结果：

```rust
struct ResetExecution {
    output: ResetOutput,
    warnings: Vec<String>,
}
```

当前签名：
```rust
async fn run_reset(args: ResetArgs) -> CliResult<ResetOutput>
```

目标签名：
```rust
async fn run_reset(args: ResetArgs) -> Result<ResetExecution, ResetError>
```

`execute_safe()` 调用层转换：
```rust
pub async fn execute_safe(args: ResetArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_reset(args).await.map_err(CliError::from)?;
    render_reset_output(&result.output, output)?;
    for warning in &result.warnings {
        emit_warning(warning);
    }
    Ok(())
}
```

辅助函数签名同步变更：
```rust
async fn perform_reset(target: ObjectHash, mode: ResetMode, target_name: &str) -> Result<ResetStats, ResetError>
fn remove_empty_directories(workdir: &Path) -> Result<Vec<String>, ResetError>
// 注：warning 统一经 execute_safe() 调用 emit_warning() 输出，
// quiet 模式不抑制 warning；--exit-code-on-warning 继续生效
```

**渲染规则（已落地，无需变更）：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | `HEAD is now at <short-hash> <subject>` | warning 经 `emit_warning()` 输出 |
| human + pathspec | `Unstaged changes after reset:` + 文件列表 | warning 经 `emit_warning()` 输出 |
| human + `--quiet` | 无 | warning 经 `emit_warning()` 输出 |
| `--json` / `--machine` | JSON envelope | warning 经 `emit_warning()` 输出 |

**human 模式确认消息：**

```text
# --hard / --mixed（全量 reset）
HEAD is now at abc1234 feat: add new feature

# --soft（全量 reset）
HEAD is now at abc1234 feat: add new feature

# pathspec reset（mixed 模式）
Unstaged changes after reset:
M       src/main.rs
M       src/lib.rs
```

### 特性 3：JSON 输出设计

**成功输出（全量 reset）：**

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "hard",
    "commit": "abc1234567890abcdef1234567890abcdef123456",
    "short_commit": "abc1234",
    "subject": "feat: add new feature",
    "previous_commit": "def5678901234abcdef5678901234abcdef567890",
    "files_unstaged": 0,
    "files_restored": 3,
    "pathspecs": []
  }
}
```

**pathspec reset：**

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "mixed",
    "commit": "abc1234...",
    "short_commit": "abc1234",
    "subject": "feat: add new feature",
    "previous_commit": "abc1234...",
    "files_unstaged": 2,
    "files_restored": 0,
    "pathspecs": ["src/main.rs", "src/lib.rs"]
  }
}
```

**错误 JSON：**

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "invalid revision: 'nonexistent'",
  "hints": [
    "check the revision name and try again"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 reset 中的具体落地

| ID | 改进 | reset 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 revision、pathspec + soft/hard 冲突）→ exit `129`；运行时错误（object 损坏、HEAD unborn、I/O 失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——reset 的参数是 revision 和 pathspec，无 enum 值可做 fuzzy match |
| **G** | Issues URL | 与 switch 保持一致——仅在映射为 `InternalInvariant` 的内部不变式错误时输出。当前 `reset` 计划内没有 `InternalInvariant` 变体，`RepoCorrupt` 是数据问题而非代码 bug，不附带 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra reset                            Reset index to HEAD (mixed)
    libra reset --soft HEAD~1              Soft reset: move HEAD only
    libra reset --mixed HEAD~1             Mixed reset: move HEAD + reset index
    libra reset --hard HEAD~1              Hard reset: move HEAD + reset index + working tree
    libra reset src/main.rs                Unstage a specific file
    libra reset --json                     Structured JSON output for agents
```

### 测试要求

#### `tests/command/reset_test.rs`（核心执行路径扩展）

- **（已有）** 仓库外执行、soft/mixed/hard reset、HEAD~ 引用、分支上 reset
- **（新增）`ResetError` 变体覆盖**：
  - `InvalidRevision`：无效 revision 返回 exit `129` + `LBR-CLI-003`
  - `PathspecWithSoft`：`--soft` + pathspec 返回 exit `129` + `LBR-CLI-002`
  - `PathspecWithHard`：`--hard` + pathspec 返回 exit `129` + `LBR-CLI-002`
  - `HeadUnborn`：空仓库 reset 返回 exit `128` + `LBR-REPO-003`
- **（新增）成功确认消息**：human 模式下 stdout 包含 `HEAD is now at`
- **（新增）pathspec reset 输出**：mixed 模式 + pathspec 后 stdout 包含 unstaged 文件列表
- **（新增）warning 管线**：目录清理 warning 不再直写 stderr，统一经 `emit_warning()` 输出并触发 warning tracker
- **（新增）`--exit-code-on-warning` 回归**：成功 reset 伴随 cleanup warning 时返回 exit `9`，且 JSON schema 不新增 `warnings` 字段

#### `tests/command/reset_json_test.rs`（JSON schema 稳定性，可选拆分文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **`--hard --json`**：`mode == "hard"`，`files_restored` 反映实际被恢复的 tracked 文件数；dirty 工作区时 `> 0`，clean repo 上对 `HEAD` 执行时可为 `0`
- **`--mixed --json`**：`mode == "mixed"`
- **`--soft --json`**：`mode == "soft"`，`files_unstaged == 0`，`files_restored == 0`
- **pathspec `--json`**：仅 mixed pathspec reset 成功，`pathspecs` 数组非空
- **错误 `--json`**：`ok == false` + 错误码
- **`--machine reset`**：stdout 恰好 1 行非空行

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/reset.rs` | **收口** | 保持已落地的 `ResetOutput` / `run_reset()` / `render_reset_output()` / JSON / human 确认消息；后续补齐 `ResetError` typed enum、移除 `map_reset_runtime_error()` 的关键词分类、把目录清理 warning 接入共享输出、补齐 `--help` EXAMPLES |
| `tests/command/reset_test.rs` | **扩展** | 在现有 JSON / human 输出回归基础上，补齐 typed error、warning 路径与 help EXAMPLES 回归 |
| `tests/command/reset_json_test.rs` | **可选拆分** | 若 `reset_test.rs` 中的 JSON 覆盖继续膨胀，可再拆出独立 schema 稳定性文件；当前不是阻断项 |
