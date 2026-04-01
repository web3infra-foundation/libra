## Switch 命令改进详细计划

> 最后编写时间：2026-03-31

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

> 当前工作区实现已按本文范围落地核心改动；以下内容继续作为验收边界、契约说明和后续批次分工文档。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`switch` 是第二批（状态变更确认命令）中最常用的命令——切换分支后用户必须知道发生了什么。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`switch.rs:71/80`）
- `--create` / `--detach` / `--track` 已实现
- `ensure_clean_status()` 检测工作树脏状态已实现（`switch.rs:191-229`），且为 `pub`，被 `checkout.rs:69` 调用
- `switch_to_branch()` / `switch_to_commit()` / `switch_to_tracked_remote_branch()` 三条主路径已实现
- `restore_to_commit()` 委托给 `restore::execute_safe()` 更新工作树
- reflog 记录已集成（`ReflogContext` + `with_reflog()`）

**已确认的跨命令调用关系：**

- `checkout.rs:69` 调用 `switch::ensure_clean_status(output)`，并通过 `err.message()` 字符串匹配区分 `DirtyUnstaged` / `DirtyUncommitted`（fragile pattern，需在本批一并改进）
- `run_switch()` 调用 `branch::create_branch_safe()` → 返回 `CliResult<()>`
- `switch_to_tracked_remote_branch()` 调用 `branch::set_upstream_safe_with_output()` → 返回 `CliResult<()>`
- `restore_to_commit()` 调用 `restore::execute_safe()` → 返回 `CliResult<()>`

`checkout` 侧的详细兼容要求单列于 [checkout.md](checkout.md)；本文件只记录 `switch` 需要暴露给 `checkout` 的共享接口变化，避免两份计划互相覆盖。

**基于当前代码的 Review 结论（已改进部分 vs 仍需改进部分）：**

已改进（当前代码已具备）：

- **结构化 JSON 输出已可用**：`render_switch_output()` 已调用 `emit_json_data("switch", result, output)`，`--json` 模式输出完整的 `SwitchOutput` 结构
- **执行层与渲染层已拆分**：`run_switch(args, output) -> CliResult<SwitchOutput>` 纯执行入口已存在（`switch.rs:85`），`render_switch_output()` 负责 human/JSON/quiet 三模式渲染
- **`SwitchOutput` 结构体已定义**：包含 `previous_branch`、`previous_commit`、`branch`、`commit`、`created`、`detached`、`already_on`、`tracking` 字段
- **`SwitchTrackingInfo` 嵌套结构已定义**：`remote` + `remote_branch`
- **成功确认消息已统一**：`render_switch_output()` 覆盖 4 种场景（already on / detached / new branch / existing branch）
- **大部分错误已带 `StableErrorCode`**：~12 处 `.with_stable_code()` 调用覆盖了主要错误路径
- **分支不存在时已有 `-c` 提示**：`switch_to_branch()` 中已实现 `create it with 'libra switch -c {}'` hint（`switch.rs:391`）

仍需改进：

- **无 `SwitchError` typed enum**：错误仍散落在 `run_switch()`、`ensure_clean_status()`、`switch_to_branch()`、`switch_to_commit()`、`switch_to_tracked_remote_branch()` 多个函数中，使用 inline `CliError::fatal()` / `CliError::command_usage()` 构造，而非第一批命令（`CommitError`/`PushError`）的统一 typed enum + `impl From` 映射模式
- **`checkout.rs` 依赖 `err.message()` 字符串匹配**：`checkout.rs:72-76` 用 `matches!(err.message(), "unstaged changes..." | "uncommitted changes...")` 区分脏状态类型，fragile 且无法在 message 文案变化时编译期捕获
- **无 Levenshtein 模糊匹配**：分支不存在时只提示 `-c` 创建，不提供近似分支名建议
- **缺少 `--help` EXAMPLES 段**

### 目标与非目标

**本批目标：**
- 引入 `SwitchError` typed error enum，覆盖 switch 层面的错误场景
- 所有 `SwitchError → CliError` 映射集中在 `impl From<SwitchError> for CliError` 块中，使用显式 `StableErrorCode`
- 重构 `run_switch()` 及其辅助函数，返回 `SwitchError` 而非 inline `CliError`
- `ensure_clean_status()` 返回 `Result<(), SwitchError>`，`checkout.rs` 同步改为变体匹配，消除字符串匹配
- 切换不存在分支时附加 Levenshtein 模糊匹配建议（距离 ≤ 2 的现有分支一并提示）
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不改变 `restore_to_commit()` 逻辑**。工作树恢复行为不变
- **不改变 `ensure_clean_status()` 检测逻辑**。脏状态检测行为不变，仅改返回类型
- **不改变 `SwitchOutput` 结构体和 JSON 输出 schema**。已有的结构化输出保持向后兼容
- **不改变 `render_switch_output()` 渲染逻辑**。human/JSON/quiet 三模式渲染已正确
- **不改变 `checkout` 的对外行为，也不强行抽象 `switch`/`checkout` 共用执行层**。两者的成功/失败文案与兼容目标暂时独立维护；`checkout` 仅同步适配 `ensure_clean_status()` 新返回类型
- **不引入 `--merge` / `--force` 选项**。强制切换或合并切换留后续
- **不引入 stash 集成**（如 `switch --stash`）

### 设计原则

1. **typed enum 归拢散落错误**：将 5 个函数中的 inline `CliError` 统一为 `SwitchError` 变体，错误处理模式与 `CommitError`（18 变体）、`PushError`（20 变体）对齐
2. **`impl From<SwitchError> for CliError` 集中映射**：每个变体有确定的 `StableErrorCode`、退出码和 hint，不再散落在各个函数中
3. **`run_switch()` 签名变更**：返回 `Result<SwitchOutput, SwitchError>`（当前为 `CliResult<SwitchOutput>`），辅助函数 `ensure_clean_status()`、`switch_to_branch()` 等同步变更
4. **不破坏已有 JSON 和 human 输出**：`SwitchOutput` 结构不变，`render_switch_output()` 不变，仅重构错误路径
5. **优先 typed enum；对未 typed 的委托命令保留 passthrough 例外**：`switch` 自身错误统一收敛为 `SwitchError`；但在 `branch` / `restore` 仍只返回 `CliError` 的现状下，必须保留一个委托透传变体，避免丢失它们现有的 stable code / hint / exit code 契约

### 特性 1：SwitchError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("remote branch name is required")]
    MissingTrackTarget,

    #[error("branch name is required when using --detach")]
    MissingDetachTarget,

    #[error("branch name is required")]
    MissingBranchName,

    #[error("branch '{name}' not found")]
    BranchNotFound {
        name: String,
        /// Local branches with Levenshtein distance <= 2, pre-computed at the error site
        similar: Vec<String>,
    },

    #[error("a branch is expected, got remote branch '{0}'")]
    GotRemoteBranch(String),

    #[error("remote branch '{remote}/{branch}' not found")]
    RemoteBranchNotFound { remote: String, branch: String },

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

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}
```

**关于委托路径的处理原则**：

`branch::create_branch_safe()`、`branch::set_upstream_safe_with_output()`、`restore::execute_safe()` 当前均返回 `CliResult<()>`（即 `Result<(), CliError>`），而不是 typed sub-error。这里不能把它们统一改写成新的 `SwitchError::BranchCreate` / `Restore` 字符串错误，否则会丢失原本已有的 `StableErrorCode`、hint 和 exit code。对这些路径应直接包装为 `SwitchError::DelegatedCli(e)`，在 `impl From<SwitchError> for CliError` 中原样透传：

```rust
branch::create_branch_safe(name.clone(), base)
    .await
    .map_err(SwitchError::DelegatedCli)?;

branch::set_upstream_safe_with_output(branch, upstream, output)
    .await
    .map_err(SwitchError::DelegatedCli)?;

restore::execute_safe(args, output)
    .await
    .map_err(SwitchError::DelegatedCli)?;
```

这不是对 typed enum 方向的回退，而是对当前依赖现状的兼容处理：等 `branch` / `restore` 后续批次各自引入 typed error 后，`switch` 再把 `DelegatedCli` 收紧为 typed 子错误包装。

**与当前代码中 inline 错误的对应关系：**

| 当前代码位置 | 当前 inline 错误 | 对应 SwitchError 变体 |
|-------------|-----------------|---------------------|
| `run_switch:99` | `command_usage("remote branch name is required")` | `MissingTrackTarget` |
| `run_switch:123-127` | `fatal("creating/switching to '...' branch is not allowed")` | `InternalBranchBlocked` |
| `run_switch:129` | `branch::create_branch_safe()` 返回 `CliError` | `DelegatedCli` |
| `run_switch:144-147` | `command_usage("branch name is required when using --detach")` | `MissingDetachTarget` |
| `run_switch:148-151` | `fatal(e).with_stable_code(CliInvalidTarget)` | `CommitResolve` |
| `run_switch:165-168` | `command_usage("branch name is required")` | `MissingBranchName` |
| `ensure_clean_status:195-198` | `fatal("failed to determine working tree status")` | `StatusCheck` |
| `ensure_clean_status:205-207` | `fatal("unstaged changes, can't switch branch")` | `DirtyUnstaged` |
| `ensure_clean_status:212-215` | `fatal("failed to determine working tree status")` | `StatusCheck` |
| `ensure_clean_status:222-224` | `fatal("uncommitted changes, can't switch branch")` | `DirtyUncommitted` |
| `switch_to_tracked_remote_branch:249` | `fatal("invalid remote branch")` | `InvalidRemoteBranch` |
| `switch_to_tracked_remote_branch:261-265` | `fatal("switching to '...' branch is not allowed")` | `InternalBranchBlocked` |
| `switch_to_tracked_remote_branch:273-279` | `fatal("remote branch '...' not found")` | `RemoteBranchNotFound` |
| `switch_to_tracked_remote_branch:287-290` | `fatal("a branch named '...' already exists")` | `BranchAlreadyExists` |
| `switch_to_tracked_remote_branch:299-304` | `Branch::update_branch` 失败 | `BranchCreate` |
| `switch_to_tracked_remote_branch:309-314` | `branch::set_upstream_safe_with_output()` 返回 `CliError` | `DelegatedCli` |
| `switch_to_branch:370-375` | `fatal("switching to '...' branch is not allowed")` | `InternalBranchBlocked` |
| `switch_to_branch:383-386` | `fatal("a branch is expected, got remote branch")` | `GotRemoteBranch` |
| `switch_to_branch:388-392` | `fatal("invalid reference").with_hint("create it with -c")` | `BranchNotFound` |
| `switch_to_commit:348-362` | `fatal(e).with_stable_code(IoWriteFailed)` | `HeadUpdate` |
| `switch_to_branch:426-439` | `fatal(e).with_stable_code(IoWriteFailed)` | `HeadUpdate` |
| `restore_to_commit:446-454` | `restore::execute_safe()` 返回 `CliError` | `DelegatedCli` |

**`SwitchError → CliError` 显式映射：**

| SwitchError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `MissingTrackTarget` | `CliInvalidArguments` | 129 | `provide a remote branch name, for example 'origin/main'.` |
| `MissingDetachTarget` | `CliInvalidArguments` | 129 | `provide a commit, tag, or branch to detach at.` |
| `MissingBranchName` | `CliInvalidArguments` | 129 | `provide a branch name.` |
| `BranchNotFound` | `CliInvalidTarget` | 129 | `create it with 'libra switch -c {name}'.` + Levenshtein 建议 |
| `GotRemoteBranch` | `CliInvalidTarget` | 129 | `use 'libra switch --track {name}' to create a local tracking branch.` |
| `RemoteBranchNotFound` | `CliInvalidTarget` | 129 | `Run 'libra fetch {remote}' to update remote-tracking branches.` |
| `InvalidRemoteBranch` | `CliInvalidTarget` | 129 | `expected format: 'remote/branch'.` |
| `BranchAlreadyExists` | `ConflictOperationBlocked` | 128 | `use 'libra switch {name}' if you meant the existing local branch.` |
| `InternalBranchBlocked` | `CliInvalidTarget` | 129 | 无 |
| `DirtyUnstaged` | `RepoStateInvalid` | 128 | `commit or stash your changes before switching.` |
| `DirtyUncommitted` | `RepoStateInvalid` | 128 | `commit or stash your changes before switching.` |
| `StatusCheck` | `IoReadFailed` | 128 | 无 |
| `CommitResolve` | `CliInvalidTarget` | 129 | `check the revision name and try again.` |
| `BranchCreate` | `IoWriteFailed` | 128 | 无 |
| `HeadUpdate` | `IoWriteFailed` | 128 | 无 |
| `DelegatedCli` | 保持被委托命令原有错误码 | 保持原退出码 | 保持原 hints |

**`DirtyUnstaged` / `DirtyUncommitted` 的特殊渲染**：在转换为 `CliError` 之前，`ensure_clean_status()` 需要在 human 非 quiet 模式下先调用 `status::execute()` 显示当前状态。这要求 `ensure_clean_status()` 仍接收 `output: &OutputConfig` 参数，在返回 `SwitchError` 之前执行 side-effect 输出。这是与 `CommitError` 等纯 typed enum 模式的唯一偏差，但保持了用户体验（先看到脏状态详情，再看到错误消息）。

### 特性 2：`run_switch()` 签名变更

**当前签名：**
```rust
async fn run_switch(args: SwitchArgs, output: &OutputConfig) -> CliResult<SwitchOutput>
```

**目标签名：**
```rust
async fn run_switch(args: SwitchArgs, output: &OutputConfig) -> Result<SwitchOutput, SwitchError>
```

**辅助函数签名同步变更：**

```rust
pub async fn ensure_clean_status(output: &OutputConfig) -> Result<(), SwitchError>
async fn switch_to_branch(branch_name: String, output: &OutputConfig) -> Result<ObjectHash, SwitchError>
async fn switch_to_commit(commit_hash: ObjectHash, output: &OutputConfig) -> Result<ObjectHash, SwitchError>
async fn switch_to_tracked_remote_branch(target: String, output: &OutputConfig) -> Result<TrackedSwitchResult, SwitchError>
async fn restore_to_commit(commit_id: ObjectHash, output: &OutputConfig) -> Result<(), SwitchError>
```

**`execute_safe()` 调用层转换：**
```rust
pub async fn execute_safe(args: SwitchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_switch(args, output).await.map_err(CliError::from)?;
    render_switch_output(&result, output)
}
```

**`run_switch()` 保持私有**：当前不把它暴露给 `checkout` 复用，避免把 `switch` 的成功/失败文案、dirty-state 处理和 `checkout` 的兼容行为提前耦合在一起。

### 特性 3：`checkout.rs` 适配 `ensure_clean_status()` 新返回类型

`checkout` 侧的完整边界、非目标、代码变更和测试要求见 [checkout.md](checkout.md)。

`switch` 需要知晓的共享接口变更：`ensure_clean_status()` 返回 `Result<(), SwitchError>` 后，`checkout.rs:69-83` 从 `err.message()` 字符串匹配改为 `SwitchError::DirtyUnstaged | DirtyUncommitted` 变体匹配，其余错误通过 `CliError::from(err)` 转换。`checkout` 现有对外行为不变。

### 特性 4：Levenshtein 模糊匹配

**触发条件**：`switch_to_branch()` 在确认"本地分支不存在，且不是 remote branch"后，查询所有本地分支名并预计算与目标名称的编辑距离；`impl From<SwitchError> for CliError` 只负责渲染 hint，不做仓库查询。

**方案**：在 `SwitchError::BranchNotFound` 变体中携带预计算的近似分支名列表：

```rust
#[error("branch '{name}' not found")]
BranchNotFound {
    name: String,
    /// Branches with Levenshtein distance ≤ 2, pre-computed at error site
    similar: Vec<String>,
},
```

**`From` 映射中的 hint 构造：**
```rust
SwitchError::BranchNotFound { name, similar } => {
    let mut err = CliError::fatal(format!("branch '{}' not found", name))
        .with_stable_code(StableErrorCode::CliInvalidTarget)
        .with_hint(format!("create it with 'libra switch -c {}'.", name));
    for s in similar {
        err = err.with_hint(format!("did you mean '{}'?", s));
    }
    err
}
```

**注意**：Levenshtein 距离计算使用标准库级别的简单实现（~10 行），不引入额外依赖。仅在 `switch_to_branch()` 构造 `BranchNotFound` 时计算，不影响正常路径性能。

### 特性 5：`--help` EXAMPLES 段

```text
EXAMPLES:
    libra switch main                      Switch to an existing branch
    libra switch -c feature-x              Create and switch to a new branch
    libra switch -c fix-123 abc1234        Create branch from specific commit
    libra switch --detach v1.0             Detach HEAD at a tag
    libra switch --track origin/main       Track and switch to remote branch
    libra switch --json main               Structured JSON output for agents
```

与 `init` / `config` / `branch` 保持一致，通过 `const SWITCH_EXAMPLES: &str = ...` + clap `#[command(after_help = SWITCH_EXAMPLES)]` 属性接入。

### 特性 6：Cross-Cutting Improvements 在 switch 中的具体落地

| ID | 改进 | switch 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（缺少分支名、无效目标）→ exit `129`；运行时错误（脏工作树、HEAD 更新失败）→ exit `128`；成功 → exit `0`。**当前代码已正确使用 `command_usage()`（exit 129）和 `fatal()`（exit 128）**，typed enum 映射保持一致 |
| **B** | `--help` EXAMPLES | 见上方 EXAMPLES 段 |
| **F** | 拼写纠错 | 分支不存在时 `BranchNotFound` 携带 Levenshtein ≤ 2 近似分支名列表；`-c` 创建提示已有 |
| **G** | Issues URL | 与 `init` / `push` 保持一致，仅在明确映射为 `InternalInvariant` 的内部不变式错误时输出。当前 `switch` 计划内没有新增明确的 `InternalInvariant` 变体，因此本批不为 `IoWriteFailed` / 委托错误追加 Issues URL，避免把可修复 I/O 问题误报成“请提 bug” |

### 已有 JSON 输出 schema（无需变更）

当前 `SwitchOutput` 序列化已通过 `emit_json_data("switch", result, output)` 输出，schema 如下：

**切换分支：**

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
    "already_on": false,
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
    "already_on": false,
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
    "already_on": false,
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
    "already_on": false,
    "tracking": {
      "remote": "origin",
      "remote_branch": "feature-x"
    }
  }
}
```

**注意**：与原计划相比，实际 schema 多了 `already_on` 字段（当目标分支即当前分支时为 `true`）。这是当前代码已有的行为，保持不变。

### 测试要求

#### `tests/command/switch_test.rs`（核心行为 / 成功路径，扩展已有测试）

- **（已有）** JSON create/track 输出、human tracking message、基础功能、detach HEAD
- **（新增）成功确认消息**：human 模式下 stdout 包含 `Switched to branch` / `Switched to a new branch` / `HEAD is now at` / `Already on`
- **（新增）track 成功路径**：human 模式下同时保留 upstream message 和 switch confirmation，顺序稳定，不污染 `--json`

#### `tests/command/switch_error_test.rs`（新增，错误码 / hint / 契约验证）

- `BranchNotFound`：不存在分支返回 exit `129` + `LBR-CLI-003` + `create it with` hint + Levenshtein 建议
- `GotRemoteBranch`：传入远程分支名返回 exit `129` + `--track` 提示
- `DirtyUnstaged`：脏工作树返回 exit `128` + `LBR-REPO-003` + hint
- `DirtyUncommitted`：未提交变更返回 exit `128` + `LBR-REPO-003` + hint
- `MissingDetachTarget`：`switch --detach` 缺少目标时返回 exit `129`
- `InternalBranchBlocked`：内部分支拒绝切换
- `BranchAlreadyExists`：`--track` 时本地分支已存在返回 exit `128` + `LBR-CONFLICT-002`
- `DelegatedCli` passthrough：`switch -c` 经过 `branch::create_branch_safe()` 触发的冲突/无效目标错误，稳定错误码和 hints 不得被 `switch` 重写
- `DelegatedCli` passthrough：`restore::execute_safe()` / `set_upstream_safe_with_output()` 的现有 `CliError` 契约不得被 `switch` 重写

#### `tests/command/switch_json_test.rs`（新增，JSON schema 稳定性）

- **schema 完整性**：验证 `--json` 输出中 `previous_branch`、`previous_commit`、`branch`、`commit`、`created`、`detached`、`already_on`、`tracking` 字段的类型和存在性
- **切换分支 `--json`**：`branch` 为目标分支名，`created == false`，`detached == false`，`already_on == false`
- **创建分支 `--json`**：`created == true`
- **detach `--json`**：`branch == null`，`detached == true`
- **track `--json`**：`tracking` 对象非 null，包含 `remote` 和 `remote_branch`
- **already_on `--json`**：切换到当前分支时 `already_on == true`
- **错误 `--json`**：`ok == false` + 错误码 + hints
- **`--machine switch`**：stdout 恰好 1 行非空行

#### `tests/command/output_flags_test.rs`（已有回归测试，保留并验证）

已有测试：
- `machine_switch_dirty_repo_returns_only_json_error()`（line 664）：`--machine switch` 在 dirty repo 上仅向 stderr 输出 JSON error，不泄漏 `status::execute()` human summary
- `quiet_switch_dirty_repo_suppresses_status_summary()`（line 690）：`--quiet switch` 在 dirty repo 上抑制 status summary

这两条测试在 `ensure_clean_status()` 返回类型重构后**必须继续通过**，作为全局输出契约的回归保障。

#### `tests/command/checkout_test.rs`（已有转发路径测试，需继续扩展/保留）

已有测试已覆盖：
- `test_checkout_new_branch_with_dirty_worktree_returns_error()`：dirty worktree 仍应映射为 `local changes would be overwritten by checkout`
- `test_checkout_current_branch_with_dirty_worktree_succeeds()`：checkout 当前分支仍应是 no-op

本批若把 `checkout.rs` 从字符串匹配改为 `SwitchError` 变体匹配，上述两条测试必须继续通过；必要时补一条 `invalid index` / `status failure` 不得被折叠成 dirty-tree 错误的回归（也可继续由 `output_flags_test.rs` 承担）。

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/switch.rs` | **重构** | 新增 `SwitchError` typed enum（含 `DelegatedCli` passthrough 例外）；`impl From<SwitchError> for CliError` 集中映射；`run_switch()` 及辅助函数返回 `SwitchError`；`branch::create_branch_safe` / `set_upstream_safe_with_output` / `restore::execute_safe` 现阶段通过 `DelegatedCli` 保留原有契约；Levenshtein 模糊匹配；补齐 `--help` EXAMPLES。**不变更**：`SwitchOutput`、`SwitchTrackingInfo`、`render_switch_output()`、JSON schema |
| `src/command/checkout.rs` | **适配** | `ensure_clean_status()` 返回 `Result<(), SwitchError>` 后，将 `err.message()` 字符串匹配替换为 `SwitchError::DirtyUnstaged | DirtyUncommitted` 变体匹配；其余错误通过 `CliError::from(err)` 转换。新增 `use super::switch::SwitchError;` 导入 |
| `tests/command/switch_test.rs` | **扩展** | 核心成功路径、tracking message 与确认消息验证 |
| `tests/command/switch_error_test.rs` | **新增** | 错误码 / hint / 全部 `SwitchError` 变体覆盖 |
| `tests/command/switch_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证（含 `already_on` 字段） |
| `tests/command/output_flags_test.rs` | **回归** | 已有 `machine_switch_dirty_repo_*` / `quiet_switch_dirty_repo_*` 测试必须继续通过 |
| `tests/command/checkout_test.rs` | **回归** | dirty worktree / current branch no-op 等 checkout 兼容行为在变体匹配重构后必须继续通过 |
| `tests/command/mod.rs` | **修改** | 注册新增的 `switch_error_test` / `switch_json_test` 测试文件 |
