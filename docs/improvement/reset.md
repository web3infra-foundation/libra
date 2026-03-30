## Reset 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`reset` 是第二批（状态变更确认命令）中改变仓库状态最激烈的命令——用户必须知道 HEAD 移动到了哪里。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, _output)` 双入口已存在（`reset.rs:70/79`）
- `--soft` / `--mixed` / `--hard` 三种模式已实现
- pathspec 支持已实现（reset 特定文件）
- reflog 记录已集成（`ReflogContext` + `with_reflog()`）
- `reset_index_to_commit()` / `reset_working_directory_to_commit()` 核心逻辑已实现
- `rebuild_index_from_tree()` 和 `restore_working_directory_from_tree()` 已实现
- 空目录清理 `remove_empty_directories()` 已实现

**基于当前代码的 Review 结论（reset 仍需改进的部分）：**

- **零 JSON / machine 输出**：`OutputConfig` 参数标记为 `_output` 完全未使用（`reset.rs:79`）
- **零 `StableErrorCode`**：所有错误使用 `CliError::fatal()` 无显式错误码
- **无 `ResetError` typed enum**：错误散落在多个函数中，内部函数使用 `Result<T, String>`
- **成功时沉默**：审计报告核心发现——reset 完成后无任何输出告知用户 HEAD 移动到了哪里
- **缺少 `"HEAD is now at <SHA> <msg>"` 输出**：Git 在 hard/mixed reset 后输出此行，libra 不输出
- **内部函数使用 `Result<T, String>`**：`reset_index_to_commit()`、`reset_working_directory_to_commit()` 等返回 `String` 错误，无类型信息
- **`cli_error!` 宏直接打印**：`reset.rs` 中有 `cli_error!()` 直接写 stderr 而非通过 `CliError` 返回

### 目标与非目标

**本批目标：**
- 引入 `ResetError` typed error enum，覆盖 reset 层面的错误场景
- 所有 `ResetError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_reset(args) -> Result<ResetOutput, ResetError>` 纯执行入口
- 实现 JSON 输出（reset 结果结构化）
- 添加 "HEAD is now at \<SHA\> \<msg\>" 成功确认消息
- 补齐 `--help` EXAMPLES 段

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

### 特性 1：ResetError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ResetError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("invalid revision: '{0}'")]
    InvalidRevision(String),

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

    #[error("pathspec '{0}' is not compatible with --soft reset")]
    PathspecWithSoft(String),
}
```

**`ResetError → CliError` 显式映射：**

| ResetError 变体 | StableErrorCode | 退出码 | hint |
|----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidRevision` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `CommitLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `TreeLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted` |
| `IndexSave` | `IoWriteFailed` | 128 | 无 |
| `HeadUpdate` | `IoWriteFailed` | 128 | 无 |
| `WorktreeRestore` | `IoWriteFailed` | 128 | 无 |
| `PathspecWithSoft` | `CliInvalidArguments` | 129 | `--soft only moves HEAD; use --mixed to reset index for specific paths` |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ResetOutput {
    /// Reset mode: "soft", "mixed", "hard"
    pub mode: String,
    /// Target commit hash (full)
    pub commit: String,
    /// Target commit short hash
    pub short_commit: String,
    /// Target commit subject line
    pub subject: String,
    /// Previous HEAD commit hash
    pub previous_commit: Option<String>,
    /// Files unstaged (mixed/hard only)
    pub files_unstaged: usize,
    /// Files restored in working tree (hard only)
    pub files_restored: usize,
    /// Pathspecs that were reset (empty for full reset)
    pub pathspecs: Vec<String>,
}
```

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | `HEAD is now at <short-hash> <subject>` | 无 |
| human + pathspec | `Unstaged changes after reset:` + 文件列表 | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope | 无 |

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
| **A** | 退出码 `0/128/129` | 参数错误（无效 revision、pathspec + soft 冲突）→ exit `129`；运行时错误（object 损坏、I/O 失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——reset 的参数是 revision 和 pathspec，无 enum 值可做 fuzzy match |
| **G** | Issues URL | 仅在 `CommitLoad` / `TreeLoad` / `IndexLoad` 错误时输出 Issues URL |

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
  - `InvalidRevision`：无效 revision 返回 exit `129`
  - `PathspecWithSoft`：`--soft` + pathspec 返回 exit `129`
- **（新增）成功确认消息**：human 模式下 stdout 包含 `HEAD is now at`
- **（新增）pathspec reset 输出**：mixed 模式 + pathspec 后 stdout 包含 unstaged 文件列表

#### `tests/command/reset_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **`--hard --json`**：`mode == "hard"`，`files_restored` 反映实际被恢复的 tracked 文件数；dirty 工作区时 `> 0`，clean repo 上对 `HEAD` 执行时可为 `0`
- **`--mixed --json`**：`mode == "mixed"`
- **`--soft --json`**：`mode == "soft"`，`files_unstaged == 0`，`files_restored == 0`
- **pathspec `--json`**：`pathspecs` 数组非空
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
| `src/command/reset.rs` | **重构** | 新增 `ResetError` typed enum；新增 `ResetOutput` 结构体；新增 `run_reset()` 纯执行入口；内部函数从 `Result<T, String>` 升级到 `Result<T, ResetError>`；`ResetError → CliError` 显式 `StableErrorCode` 映射；JSON 输出；添加 "HEAD is now at" 成功确认消息；消除 `cli_error!()` 直接打印；补齐 `--help` EXAMPLES |
| `tests/command/reset_test.rs` | **扩展** | 新增 `ResetError` 变体覆盖、成功消息验证 |
| `tests/command/reset_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
