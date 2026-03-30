## Branch 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`branch` 是第二批（状态变更确认命令）中管理分支的命令，已有部分 JSON 支持。

**已确认落地的基线：**

- `config_kv` 后端已落地；`branch` 已通过 `ConfigKv` 管理 upstream tracking 配置
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`branch.rs:99/108`）
- `--list` / `--delete` / `--delete-force` / `--set-upstream-to` / `--show-current` / `--move` / `--remotes` / `--all` / `--contains` / `--no-contains` 已实现
- **JSON 输出已部分实现**：list 操作使用 `emit_json_data("branch", ...)` 返回 `{ branches: [...] }`（`branch.rs:137-145`），每个分支包含 `name`/`current`/`commit` 字段
- `is_valid_git_branch_name()` 分支名验证已实现（`branch.rs:694-725`）
- `delete_branch_safe()` 已有 merge 检查和 `.with_hint()`（`branch.rs:342-349`）

**基于当前代码的 Review 结论（branch 仍需改进的部分）：**

- **JSON 输出仅覆盖 list**：create / delete / rename / set-upstream / show-current 操作无 JSON 输出
- **零 `StableErrorCode`**：所有 25+ 处错误使用 `CliError::fatal()` / `CliError::failure()` 无显式错误码
- **无 `BranchError` typed enum**：错误散落在多个函数中
- **退出码不对齐**：删除不存在分支时应返回明确的 exit `129`（当前通过 `CliError::fatal()` 返回 128）
- **`delete_branch()` 和 `delete_branch_safe()` 重复代码**：locked/current 检查在两个函数中重复
- **测试期望 `LBR-CLI-003` 但代码未赋值**：`branch_test.rs:24` 期望 error code `LBR-CLI-003`，但代码中未调用 `.with_stable_code()`

### 目标与非目标

**本批目标：**
- 引入 `BranchError` typed error enum，覆盖 branch 层面的错误场景
- 所有 `BranchError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_branch(args) -> Result<BranchOutput, BranchError>` 纯执行入口
- 扩展 JSON 输出到所有操作（create / delete / rename / set-upstream / show-current），不仅仅是 list
- 退出码对齐：删除不存在分支 exit `129`，删除当前分支 exit `128`
- 消除 `delete_branch()` 和 `delete_branch_safe()` 的重复代码
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不改变 `--contains` / `--no-contains` 过滤逻辑**。BFS 可达性检查保持现有算法
- **不改变 merge 检查逻辑**。`delete_branch_safe()` 的 merge 检查行为不变
- **不引入 `--set-upstream-to` 的 JSON 输出中的 remote 验证**。remote 存在性检查留后续
- **不改变 JSON list 现有 schema**。`branches` 数组中的 `name`/`current`/`commit` 字段保持兼容

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_branch()` 收集结构化结果，再根据 `OutputConfig` 渲染
2. **JSON 输出通过 `action` 字段区分操作类型**：list / create / delete / rename / set-upstream / show-current
3. **错误码显式映射**：每个 `BranchError` 变体都有确定的 `StableErrorCode`
4. **JSON list 向后兼容**：现有 `branches` 数组 schema 不变，仅添加 `action` 字段作为 envelope 增量

### 特性 1：BranchError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum BranchError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("'{0}' is not a valid branch name")]
    InvalidName(String),

    #[error("a branch named '{0}' already exists")]
    AlreadyExists(String),

    #[error("branch '{0}' not found")]
    NotFound(String),

    #[error("cannot delete the branch '{0}' which you are currently on")]
    DeleteCurrent(String),

    #[error("the branch '{0}' is not fully merged")]
    NotFullyMerged(String),

    #[error("the '{0}' branch is locked by another process")]
    Locked(String),

    #[error("HEAD is detached")]
    DetachedHead,

    #[error("not a valid object name: '{0}'")]
    InvalidCommit(String),

    #[error("invalid upstream '{0}'")]
    InvalidUpstream(String),

    #[error("failed to create branch '{branch}': {detail}")]
    CreateFailed { branch: String, detail: String },

    #[error("too many arguments for rename")]
    RenameTooManyArgs,
}
```

**`BranchError → CliError` 显式映射：**

| BranchError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidName` | `CliInvalidArguments` | 129 | `branch names cannot contain spaces, '..', '~', '^', ':'` |
| `AlreadyExists` | `ConflictOperationBlocked` | 128 | `delete it first or choose a different name` |
| `NotFound` | `CliInvalidTarget` | 129 | `use 'libra branch -l' to list branches` |
| `DeleteCurrent` | `RepoStateInvalid` | 128 | `switch to another branch first` |
| `NotFullyMerged` | `RepoStateInvalid` | 128 | `if you are sure, run 'libra branch -D {name}'` |
| `Locked` | `ConflictOperationBlocked` | 128 | 无 |
| `DetachedHead` | `RepoStateInvalid` | 128 | `checkout a branch first` |
| `InvalidCommit` | `CliInvalidTarget` | 129 | `use 'libra log --oneline' to see available commits` |
| `InvalidUpstream` | `CliInvalidTarget` | 129 | `expected format: 'remote/branch'` |
| `CreateFailed` | `IoWriteFailed` | 128 | 无 |
| `RenameTooManyArgs` | `CliInvalidArguments` | 129 | `usage: libra branch -m [old-name] new-name` |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum BranchOutput {
    #[serde(rename = "list")]
    List(BranchListOutput),
    #[serde(rename = "create")]
    Create(BranchCreateOutput),
    #[serde(rename = "delete")]
    Delete(BranchDeleteOutput),
    #[serde(rename = "rename")]
    Rename(BranchRenameOutput),
    #[serde(rename = "set-upstream")]
    SetUpstream(BranchSetUpstreamOutput),
    #[serde(rename = "show-current")]
    ShowCurrent(BranchShowCurrentOutput),
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchListOutput {
    pub branches: Vec<BranchListEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchListEntry {
    pub name: String,
    pub current: bool,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchCreateOutput {
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchDeleteOutput {
    pub name: String,
    pub commit: String,
    /// Whether merge check was skipped (-D)
    pub force: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchRenameOutput {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchSetUpstreamOutput {
    pub branch: String,
    pub upstream: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchShowCurrentOutput {
    pub name: Option<String>,
    pub detached: bool,
    pub commit: Option<String>,
}
```

> **向后兼容说明：** 现有 `--json -l` 返回 `{ "branches": [...] }` 的 schema 通过 `BranchListOutput.branches` 保留。新增的 `action` 字段由 `#[serde(tag = "action")]` 自动添加到 JSON envelope 的 `data` 层。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human list | 分支列表（带颜色、当前分支标记） | 无 |
| human create | `Created branch '{name}' at {short_hash}` | 无 |
| human delete | `Deleted branch {name} (was {short_hash})` | 无 |
| human rename | `Branch '{old}' renamed to '{new}'` | 无 |
| human set-upstream | `branch '{name}' set up to track '{upstream}'` | 无 |
| human show-current | 分支名或 `(HEAD detached at {hash})` | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope（含 `action` 字段） | 无 |

### 特性 3：JSON 输出设计

**list `--json`（向后兼容）：**

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "list",
    "branches": [
      { "name": "main", "current": true, "commit": "abc1234..." },
      { "name": "feature-x", "current": false, "commit": "def5678..." }
    ]
  }
}
```

**create `--json`：**

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "create",
    "name": "feature-new",
    "commit": "abc1234..."
  }
}
```

**delete `--json`：**

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "delete",
    "name": "old-branch",
    "commit": "abc1234...",
    "force": false
  }
}
```

**rename `--json`：**

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "rename",
    "old_name": "old-name",
    "new_name": "new-name"
  }
}
```

**show-current `--json`：**

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "show-current",
    "name": "main",
    "detached": false,
    "commit": "abc1234..."
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
    "use 'libra branch -l' to list branches"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 branch 中的具体落地

| ID | 改进 | branch 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效分支名、不存在的分支、无效 commit、rename 参数过多）→ exit `129`；运行时错误（locked、merge 检查失败、detached HEAD、already exists）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | 分支不存在时基于现有分支列表做 Levenshtein 距离 ≤ 2 fuzzy match |
| **G** | Issues URL | 仅在 `CreateFailed` 错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra branch                           List local branches
    libra branch feature-x                 Create a new branch
    libra branch feature-x abc1234         Create branch from specific commit
    libra branch -d feature-x              Delete a merged branch
    libra branch -D feature-x              Force delete a branch
    libra branch -m old-name new-name      Rename a branch
    libra branch -u origin/main            Set upstream tracking
    libra branch --show-current            Show current branch name
    libra branch -r                        List remote branches
    libra branch -a                        List all branches
    libra branch --contains HEAD~5         Branches containing a commit
    libra branch --json -l                 List branches as JSON
```

### 测试要求

#### `tests/command/branch_test.rs`（核心执行路径扩展）

- **（已有）** invalid start point 错误码、create/list/show_current、create from remote、invalid name、rename、rename current、rename to existing、list all、delete safe、contains filter、error propagation
- **（新增）`BranchError` 变体覆盖**：
  - `NotFound`：删除不存在分支返回 exit `129` + `LBR-CLI-003`
  - `DeleteCurrent`：删除当前分支返回 exit `128`
  - `InvalidName`：无效分支名返回 exit `129`
  - `DetachedHead`：detached HEAD 下 set-upstream 返回 exit `128`
- **（新增）成功确认消息**：human 模式下 create/delete/rename 各自输出确认消息
- **（新增）fuzzy match**：删除名为 `mian` 的分支时提示 `did you mean 'main'?`

#### `tests/command/branch_json_test.rs`（JSON schema 稳定性，新增文件）

- **list `--json` 向后兼容**：`action == "list"`，`branches` 数组存在且 schema 不变
- **create `--json`**：`action == "create"`，`name` 和 `commit` 存在
- **delete `--json`**：`action == "delete"`，`name` 和 `commit` 存在
- **delete -D `--json`**：`force == true`
- **rename `--json`**：`action == "rename"`，`old_name` 和 `new_name` 存在
- **show-current `--json`**：`action == "show-current"`，`name` 非 null
- **detached show-current `--json`**：`detached == true`，`name == null`
- **错误 `--json`**：`ok == false` + 错误码
- **`--machine branch -l`**：stdout 恰好 1 行非空行

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/branch.rs` | **重构** | 新增 `BranchError` typed enum；新增 `BranchOutput` tagged enum 及各操作结构体；新增 `run_branch()` 纯执行入口；消除 `delete_branch()` / `delete_branch_safe()` 重复代码；`BranchError → CliError` 显式 `StableErrorCode` 映射；扩展 JSON 输出到所有操作；添加 create/delete/rename 确认消息；补齐 `--help` EXAMPLES |
| `tests/command/branch_test.rs` | **扩展** | 新增 `BranchError` 变体覆盖、确认消息验证、fuzzy match |
| `tests/command/branch_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证（含 list 向后兼容） |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
