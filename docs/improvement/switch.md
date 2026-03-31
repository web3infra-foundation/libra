## Switch 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`switch` 是第二批（状态变更确认命令）中最常用的命令——切换分支后用户必须知道发生了什么。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`switch.rs:51/60`）
- `--create` / `--detach` / `--track` 已实现
- `ensure_clean_status()` 检测工作树脏状态已实现（`switch.rs:114-146`）
- `switch_to_branch()` / `switch_to_commit()` / `switch_to_tracked_remote_branch()` 三条主路径已实现
- `restore_to_commit()` 委托给 `restore::execute_safe()` 更新工作树
- reflog 记录已集成（`ReflogContext` + `with_reflog()`）
- `output.is_json()` 已用于抑制 `status::execute()` 输出（`switch.rs:124, 138`），但 `--json` 模式下不产生结构化 stdout

**基于当前代码的 Review 结论（switch 仍需改进的部分）：**

- **无结构化 JSON 输出**：虽然 `output.is_json()` 已用于抑制 human 输出，但 `--json` 模式下 stdout 为空——不产生任何结构化输出，Agent 无法获知切换结果
- **零 `StableErrorCode`**：所有 17 处错误使用 `CliError::fatal()` / `CliError::command_usage()` 无显式错误码
- **无 `SwitchError` typed enum**：错误散落在 `execute_safe()`、`switch_to_branch()`、`switch_to_commit()`、`switch_to_tracked_remote_branch()` 多个函数中
- **成功时确认消息不一致**：分支切换后调用 `status::execute()` 显示状态，但 `--detach` 和 `--create` 路径的成功反馈格式不同
- **`--create` 不存在分支时无 `did you mean -c` 提示**：审计报告指出的改进点

### 目标与非目标

**本批目标：**
- 引入 `SwitchError` typed error enum，覆盖 switch 层面的错误场景
- 所有 `SwitchError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_switch(args) -> Result<SwitchOutput, SwitchError>` 纯执行入口
- 实现 JSON 输出（切换结果结构化），替代当前的"silent JSON"
- 切换不存在分支时提示 `did you mean 'libra switch -c <branch>'?`
- 统一成功确认消息格式
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不改变 `restore_to_commit()` 逻辑**。工作树恢复行为不变
- **不改变 `ensure_clean_status()` 检测逻辑**。脏状态检测行为不变
- **不引入 `--merge` / `--force` 选项**。强制切换或合并切换留后续
- **不引入 stash 集成**（如 `switch --stash`）

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_switch()` 收集结构化结果，再根据 `OutputConfig` 渲染
2. **JSON 包含切换前后状态**：previous branch/commit、new branch/commit、是否新创建、是否 detached
3. **错误码显式映射**：每个 `SwitchError` 变体都有确定的 `StableErrorCode`
4. **不存在分支的 fuzzy 提示**：当目标分支不存在时，检测是否可能需要 `--create`，提供 hint

### 特性 1：SwitchError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("branch name is required")]
    MissingBranchName,

    #[error("branch name is required when using --detach")]
    MissingDetachTarget,

    #[error("branch '{0}' not found")]
    BranchNotFound(String),

    #[error("remote branch '{0}' not found")]
    RemoteBranchNotFound(String),

    #[error("invalid remote branch '{0}'")]
    InvalidRemoteBranch(String),

    #[error("a branch named '{0}' already exists")]
    BranchAlreadyExists(String),

    #[error("creating/switching to '{0}' branch is not allowed")]
    InternalBranchBlocked(String),

    #[error("unstaged changes, can't switch branch")]
    DirtyUnstaged,

    #[error("uncommitted changes, can't switch branch")]
    DirtyUncommitted,

    #[error("failed to determine working tree status: {0}")]
    StatusCheck(String),

    #[error("failed to resolve commit: {0}")]
    CommitResolve(String),

    #[error("failed to create branch '{branch}': {detail}")]
    BranchCreate { branch: String, detail: String },

    #[error("failed to update HEAD: {0}")]
    HeadUpdate(String),

    #[error("failed to restore working tree: {0}")]
    Restore(String),
}
```

**`SwitchError → CliError` 显式映射：**

| SwitchError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `MissingBranchName` | `CliInvalidArguments` | 129 | `provide a branch name or use --create` |
| `MissingDetachTarget` | `CliInvalidArguments` | 129 | `provide a commit, tag, or branch to detach at` |
| `BranchNotFound` | `CliInvalidTarget` | 129 | `use 'libra branch -l' to list branches` + `did you mean 'libra switch -c {name}'?` |
| `RemoteBranchNotFound` | `CliInvalidTarget` | 129 | `use 'libra branch -r' to list remote branches` |
| `InvalidRemoteBranch` | `CliInvalidTarget` | 129 | `expected format: 'remote/branch'` |
| `BranchAlreadyExists` | `RepoStateInvalid` | 128 | `delete it first or choose a different name` |
| `InternalBranchBlocked` | `CliInvalidTarget` | 129 | 无 |
| `DirtyUnstaged` | `RepoStateInvalid` | 128 | `commit or stash your changes before switching` |
| `DirtyUncommitted` | `RepoStateInvalid` | 128 | `commit or stash your changes before switching` |
| `StatusCheck` | `IoReadFailed` | 128 | 无 |
| `CommitResolve` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `BranchCreate` | `IoWriteFailed` | 128 | 无 |
| `HeadUpdate` | `IoWriteFailed` | 128 | 无 |
| `Restore` | `IoWriteFailed` | 128 | 无 |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutput {
    /// Previous branch name (None if was detached)
    pub previous_branch: Option<String>,
    /// Previous commit hash
    pub previous_commit: Option<String>,
    /// New branch name (None if detached HEAD)
    pub branch: Option<String>,
    /// New commit hash
    pub commit: String,
    /// Whether the branch was newly created
    pub created: bool,
    /// Whether HEAD is now detached
    pub detached: bool,
    /// Whether tracking was set up (--track)
    pub tracking: Option<SwitchTrackingInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchTrackingInfo {
    pub remote: String,
    pub remote_branch: String,
}
```

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 确认消息（如 `Switched to branch 'main'`） | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope | 无 |

**human 模式确认消息（统一格式）：**

```text
# 切换分支
Switched to branch 'main'

# 创建并切换
Switched to a new branch 'feature-x'

# detach
HEAD is now at abc1234 feat: add feature

# track 远程分支
branch 'main' set up to track 'origin/main'
Switched to a new branch 'main'
```

### 特性 3：JSON 输出设计

**成功输出（切换分支）：**

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234...",
    "branch": "feature-x",
    "commit": "abc1234...",
    "created": false,
    "detached": false,
    "tracking": null
  }
}
```

**创建并切换（`-c`）：**

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234...",
    "branch": "feature-new",
    "commit": "abc1234...",
    "created": true,
    "detached": false,
    "tracking": null
  }
}
```

**detach：**

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234...",
    "branch": null,
    "commit": "def5678...",
    "created": false,
    "detached": true,
    "tracking": null
  }
}
```

**track 远程分支：**

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc1234...",
    "branch": "feature-x",
    "commit": "def5678...",
    "created": true,
    "detached": false,
    "tracking": {
      "remote": "origin",
      "remote_branch": "feature-x"
    }
  }
}
```

**错误 JSON（分支不存在）：**

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "branch 'nonexistent' not found",
  "hints": [
    "use 'libra branch -l' to list branches",
    "did you mean 'libra switch -c nonexistent'?"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 switch 中的具体落地

| ID | 改进 | switch 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（缺少分支名、无效目标）→ exit `129`；运行时错误（脏工作树、HEAD 更新失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | 分支不存在时提示 `did you mean 'libra switch -c <name>'?`；如有 Levenshtein 距离 ≤ 2 的现有分支也一并提示 |
| **G** | Issues URL | 仅在 `HeadUpdate` / `Restore` 等内部错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra switch main                      Switch to an existing branch
    libra switch -c feature-x              Create and switch to a new branch
    libra switch -c fix-123 abc1234        Create branch from specific commit
    libra switch --detach v1.0             Detach HEAD at a tag
    libra switch --track origin/main       Track and switch to remote branch
    libra switch --json main               Structured JSON output for agents
```

### 测试要求

#### `tests/command/switch_test.rs`（核心执行路径扩展）

- **（已有）** CLI 错误码验证、基础功能、track upstream、detach HEAD
- **（新增）`SwitchError` 变体覆盖**：
  - `BranchNotFound`：不存在分支返回 exit `129` + `did you mean -c` hint
  - `DirtyUnstaged`：脏工作树返回 exit `128`
  - `DirtyUncommitted`：未提交变更返回 exit `128`
  - `InternalBranchBlocked`：内部分支拒绝切换
- **（新增）`run_switch()` 结构化结果**：验证 `SwitchOutput` 中 branch/commit/created/detached 正确
- **（新增）成功确认消息**：human 模式下 stdout 包含 `Switched to branch` 或 `Switched to a new branch`

#### `tests/command/switch_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **切换分支 `--json`**：`branch` 为目标分支名，`created == false`，`detached == false`
- **创建分支 `--json`**：`created == true`
- **detach `--json`**：`branch == null`，`detached == true`
- **track `--json`**：`tracking` 对象非 null
- **错误 `--json`**：`ok == false` + 错误码
- **`--machine switch`**：stdout 恰好 1 行非空行

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/switch.rs` | **重构** | 新增 `SwitchError` typed enum；新增 `SwitchOutput` / `SwitchTrackingInfo` 结构体；新增 `run_switch()` 纯执行入口；`SwitchError → CliError` 显式 `StableErrorCode` 映射；JSON 输出替代当前的 silent JSON；统一 human 确认消息格式；补齐 `--help` EXAMPLES |
| `tests/command/switch_test.rs` | **扩展** | 新增 `SwitchError` 变体覆盖、确认消息验证 |
| `tests/command/switch_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
