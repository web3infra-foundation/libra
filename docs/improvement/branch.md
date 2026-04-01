## Branch 命令改进详细计划

> 最后编写时间：2026-04-01

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

> 当前工作区实现已按本文范围落地一部分改动；以下内容改为记录已落地能力、剩余遗漏和后续收口项。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`branch` 是第二批（状态变更确认命令）中管理分支的命令，JSON 已覆盖主要操作，但错误建模和 human 输出一致性仍未完全现代化。

**已确认落地的基线：**

- `config_kv` 后端已落地；`branch` 已通过 `ConfigKv` 管理 upstream tracking 配置
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在
- `BranchOutput` + `run_branch_json()` 已覆盖 list / create / delete / rename / set-upstream / show-current 的 JSON 输出
- `--list` / `--delete` / `--delete-force` / `--set-upstream-to` / `--show-current` / `--move` / `--remotes` / `--all` / `--contains` / `--no-contains` 已实现
- create / delete / rename / set-upstream / show-current 路径都已补上命令层 `StableErrorCode`
- `is_valid_git_branch_name()` 分支名验证已实现（`branch.rs:694-725`）
- `delete_branch_safe()` 已有 merge 检查和 `.with_hint()`（`branch.rs:342-349`）
- human 路径已至少覆盖 delete-safe、rename、set-upstream、show-current 的确认输出
- `after_help` 已有 compatibility notes，但尚未补 EXAMPLES

**基于当前代码的 Review 结论（已改进部分 vs 仍需改进部分）：**

已改进（当前代码已具备）：

- **JSON 已覆盖主要操作**：`BranchOutput` + `run_branch_json()` 已支持 list / create / delete / rename / set-upstream / show-current，list schema 也已保持向后兼容
- **大部分命令层错误已带显式 `StableErrorCode`**：invalid name、already exists、invalid commit、branch not found、detached HEAD、I/O 写失败等主要路径已显式映射
- **退出码对齐已部分落地**：删除不存在分支已走 `CliInvalidTarget`（exit `129`），删除当前分支走 `RepoStateInvalid`（exit `128`）
- **部分成功确认消息已落地**：safe delete、rename、set-upstream、show-current 均已有 human 输出
- **现有测试已验证关键契约**：`branch_test.rs` 已覆盖 invalid start point error code、detached HEAD set-upstream 和 JSON create schema

仍需改进：

- **无 `BranchError` typed enum**：错误仍散落在 create/delete/rename/list 各函数中
- **无统一 `run_branch()` / `render_branch_output()` 分层**：当前只有 JSON 路径走 `run_branch_json()`，human 路径仍按分支直接执行
- **create / force-delete 仍缺确认消息**：`create_branch_safe()` 和 `delete_branch()` 成功后仍然沉默，不符合第二批“状态变更必须确认”的目标
- **缺少 fuzzy suggestion**：分支不存在时还没有 Levenshtein 类 `did you mean ...` 提示
- **`--help` 仍缺 EXAMPLES**：当前 `after_help` 只有 compatibility notes
- **仍有零散错误未显式赋码**：例如 `cannot get HEAD commit` 等路径还依赖默认推断
- **`delete_branch()` / `delete_branch_safe()` 仍有重复前置检查**：locked/current/not-found 检查可继续抽取复用
- **`internal::branch` 仍吞掉底层失败**：`list_branches_with_conn()` 里存在 `unwrap()`，`find_branch_with_conn()` / `delete_branch_with_conn()` 仍用 `eprintln!()` 吞掉查询/删除失败；不先把这些 API 改成 fallible，命令层无法真正收口为 `BranchError`

### 目标与非目标

**本批目标：**
- 引入 `BranchError` typed error enum，覆盖 branch 层面的错误场景
- 所有 `BranchError → CliError` 映射使用显式 `StableErrorCode`
- 在保留既有 `BranchOutput` JSON schema 的前提下，补齐统一的 `run_branch()` / `render_branch_output()` 分层
- 先把 `internal::branch` 的 `list/find/delete` 改成 fallible API（去掉 `unwrap()` / `eprintln!()`），让命令层能接住真实失败
- 抽取 delete 共享前置检查，减少 `delete_branch()` / `delete_branch_safe()` 重复逻辑
- 补齐 create / force-delete 的 human 确认消息
- 收口剩余未显式赋码的错误路径
- 消除 `delete_branch()` 和 `delete_branch_safe()` 的重复代码
- 补齐 `--help` EXAMPLES 段
- 为分支不存在路径补齐 fuzzy suggestion

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
5. **先让底层 branch store 变成可失败 API**：命令层 typed error 不能建立在 `unwrap()` / `eprintln!()` / `None` 伪装失败之上
6. **对复用 helper 保留 passthrough 例外**：`resolve_commits()` / `commit_contains()` 等当前仍返回 `CliError` 的路径，可先通过 `DelegatedCli` 透传，避免本批次过度扩散

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

    #[error("branch '{name}' not found")]
    NotFound {
        name: String,
        /// Local branches with Levenshtein distance ≤ 2, pre-computed at the error site
        similar: Vec<String>,
    },

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

    #[error("failed to query branch storage: {0}")]
    StorageQueryFailed(String),

    #[error("stored branch reference is corrupt: {0}")]
    StoredReferenceCorrupt(String),

    #[error("failed to create branch '{branch}': {detail}")]
    CreateFailed { branch: String, detail: String },

    #[error("failed to delete branch '{branch}': {detail}")]
    DeleteFailed { branch: String, detail: String },

    #[error("too many arguments for rename")]
    RenameTooManyArgs,

    #[error(transparent)]
    DelegatedCli(#[from] CliError),
}
```

> **`NotFound` 携带 `similar` 列表**：与 `SwitchError::BranchNotFound` 模式一致，在错误构造点预计算 Levenshtein ≤ 2 近似分支名列表，`impl From<BranchError> for CliError` 只负责渲染 hint。Levenshtein 距离计算复用 switch 批次落地的共享工具函数（~10 行，位于 `src/utils/` 或 `src/command/mod.rs`）。

> **关于底层 branch store 的前置收口**：在引入 `BranchError` 之前，需要先把 `src/internal/branch.rs` 中的 `list_branches_with_conn()`、`find_branch_with_conn()`、`delete_branch_with_conn()` 改成 `Result` 风格，去掉当前的 `unwrap()` / `eprintln!()`。否则命令层根本拿不到真实失败，只能把查询失败误判成 `NotFound` 或直接 panic。

**`BranchError → CliError` 显式映射：**

| BranchError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidName` | `CliInvalidArguments` | 129 | `branch names cannot contain spaces, '..', '~', '^', ':'` |
| `AlreadyExists` | `ConflictOperationBlocked` | 128 | `delete it first or choose a different name` |
| `NotFound` | `CliInvalidTarget` | 129 | `use 'libra branch -l' to list branches` + Levenshtein `did you mean '{name}'?` |
| `DeleteCurrent` | `RepoStateInvalid` | 128 | `switch to another branch first` |
| `NotFullyMerged` | `RepoStateInvalid` | 128 | `if you are sure, run 'libra branch -D {name}'` |
| `Locked` | `ConflictOperationBlocked` | 128 | 无 |
| `DetachedHead` | `RepoStateInvalid` | 128 | `checkout a branch first` |
| `InvalidCommit` | `CliInvalidTarget` | 129 | `use 'libra log --oneline' to see available commits` |
| `InvalidUpstream` | `CliInvalidTarget` | 129 | `expected format: 'remote/branch'` |
| `StorageQueryFailed` | `IoReadFailed` | 128 | 无 |
| `StoredReferenceCorrupt` | `RepoCorrupt` | 128 | 无 |
| `CreateFailed` | `IoWriteFailed` | 128 | 无 |
| `DeleteFailed` | `IoWriteFailed` | 128 | 无 |
| `RenameTooManyArgs` | `CliInvalidArguments` | 129 | `usage: libra branch -m [old-name] new-name` |
| `DelegatedCli` | 保持被委托 helper 原有错误码 | 保持原退出码 | 保持原 hints |

**与当前代码中 inline 错误的对应关系：**

| 当前代码位置 | 当前 inline 错误 | 对应 BranchError 变体 |
|-------------|-----------------|---------------------|
| `internal::branch::list_branches_with_conn()` | `all(db).await.unwrap()` | `StorageQueryFailed` / `StoredReferenceCorrupt`（先改底层 API） |
| `internal::branch::find_branch_with_conn()` | `eprintln!("fatal: failed to query branch ...")` + `None` | `StorageQueryFailed`（先改底层 API） |
| `internal::branch::delete_branch_with_conn()` | `eprintln!("fatal: failed to delete branch ...")` | `DeleteFailed`（先改底层 API） |
| `create_branch_safe:244-247` | `CliError::fatal("... is not a valid branch name")` | `InvalidName` |
| `create_branch_safe:249-254` | `CliError::fatal("... branch is locked")` | `Locked` |
| `create_branch_safe:262` | `CliError::fatal("... already exists")` | `AlreadyExists` |
| `create_branch_safe:269-271` | `CliError::fatal("not a valid object name")` | `InvalidCommit` |
| `create_branch_safe:304` | `CliError::fatal("failed to create branch")` | `CreateFailed` |
| `delete_branch_safe:322` | branch not found | `NotFound` |
| `delete_branch_safe:334` | `CliError::fatal("cannot delete ... currently on")` | `DeleteCurrent` |
| `delete_branch_safe:349` | `CliError::fatal("... not fully merged")` | `NotFullyMerged` |
| `delete_branch:360` | branch not found | `NotFound` |
| `delete_branch:373` | `CliError::fatal("cannot delete ... currently on")` | `DeleteCurrent` |
| `rename_branch:428-431` | `CliError::command_usage("too many arguments")` | `RenameTooManyArgs` |
| `rename_branch:436-439` | `CliError::fatal("invalid branch name")` | `InvalidName` |
| `rename_branch:442-455` | `CliError::fatal("... is locked")` | `Locked` |
| `rename_branch:459-462` | `CliError::fatal("... not found")` | `NotFound` |
| `rename_branch:467-469` | `CliError::fatal("... already exists")` | `AlreadyExists` |
| `rename_branch:477-480` | `CliError::fatal("failed to create branch")` | `CreateFailed` |
| `set_upstream_safe_with_output:210-213` | `CliError::fatal("invalid upstream")` | `InvalidUpstream` |
| `execute_safe:161` | `detached_head_branch_error()` | `DetachedHead` |
| `collect_branch_names()` / `list_branches()` | `resolve_commits()` / `commit_contains()` 现返 `CliError` | `DelegatedCli`（本批允许透传） |

**跨命令公开 API 边界说明：**

`create_branch_safe()`、`set_upstream_safe()`、`set_upstream_safe_with_output()` 是被 `switch.rs` 通过 `DelegatedCli` 调用的公开 API。本批这些函数**继续返回 `CliResult`**（内部由 `BranchError → CliError` 转换），不要求 `switch` 同步修改 `DelegatedCli` 处理。与此同时，`src/internal/branch.rs` 的底层 `list/find/delete` helper 可以先收紧为 `Result` 风格，再由命令层转换为 `CliError`。等 `switch` 后续收紧 `DelegatedCli` 时，可选择让这些 API 返回 `Result<_, BranchError>` 并在 `switch` 侧包装为 `SwitchError::DelegatedBranch(BranchError)`。

### 特性 2：执行层与渲染层拆分

**已落地部分（保持不变）：** `BranchOutput` enum（含 `List`/`Create`/`Delete`/`Rename`/`SetUpstream`/`ShowCurrent` 六变体）和 `BranchListEntry` 结构体均已存在于 `branch.rs:28-58`，JSON schema 已稳定。

> **向后兼容说明：** 现有 `--json -l` 返回 `{ "branches": [...] }` 的 schema 通过 `BranchOutput::List { branches }` 保留。`action` 字段由 `#[serde(tag = "action")]` 自动添加到 JSON envelope 的 `data` 层。

**本批变更：统一 `run_branch()` / `render_branch_output()` 分层**

当前架构问题：human 路径在 `execute_safe()` 内按分支直接执行（create/delete/rename/set-upstream/list 各自独立），JSON 路径单独走 `run_branch_json()`。两条路径各自拼装逻辑，create 和 force-delete 成功后在 human 路径沉默。

目标架构：

```rust
/// 纯执行入口——收集结构化结果，不输出
async fn run_branch(args: &BranchArgs) -> Result<BranchOutput, BranchError>

/// 渲染层——根据 OutputConfig 决定 human/JSON/machine/quiet 输出
fn render_branch_output(result: &BranchOutput, output: &OutputConfig) -> CliResult<()>

/// execute_safe 调用链
pub async fn execute_safe(args: BranchArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_branch(&args).await.map_err(CliError::from)?;
    render_branch_output(&result, output)
}
```

现有 `run_branch_json()` 将被合并入 `run_branch()`。`list_branches()`、`display_branches()` 的渲染逻辑将移入 `render_branch_output()` 的 human list 分支。

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
| **A** | 退出码 `0/128/129` | 参数错误（无效分支名、不存在的分支、无效 commit、rename 参数过多、无效 upstream）→ exit `129`；运行时错误（locked、merge 检查失败、detached HEAD、already exists、I/O 失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | 分支不存在时基于现有分支列表做 Levenshtein 距离 ≤ 2 fuzzy match；复用 switch 批次落地的共享 Levenshtein 工具函数 |
| **G** | Issues URL | 与 switch 保持一致——仅在映射为 `InternalInvariant` 的内部不变式错误时输出。当前 `branch` 计划内的 `CreateFailed`/`DeleteFailed` 属于 `IoWriteFailed`，不附带 Issues URL |

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
  - `DeleteCurrent`：删除当前分支返回 exit `128` + `LBR-REPO-003`
  - `InvalidName`：无效分支名返回 exit `129` + `LBR-CLI-002`
  - `DetachedHead`：detached HEAD 下 set-upstream 返回 exit `128` + `LBR-REPO-003`
- **（新增）成功确认消息**：human 模式下 create 输出 `Created branch '{name}' at {hash}`，delete 输出 `Deleted branch {name} (was {hash})`，rename 输出 `Branch '{old}' renamed to '{new}'`
- **（新增）fuzzy match**：删除名为 `mian` 的分支时提示 `did you mean 'main'?`（复用 switch 的 Levenshtein 工具函数）

#### `src/internal/branch.rs`（底层错误面补测）

- `list_branches_with_conn()`：数据库查询失败不再 `unwrap()` panic，改为 `Result` 返回
- `find_branch_with_conn()`：查询失败不再 `eprintln!()` 后伪装成 `None`
- `delete_branch_with_conn()`：删除失败不再仅打印 fatal，而是返回可断言的错误
- malformed stored commit/hash：不再 `unwrap()` panic，改为 `StoredReferenceCorrupt`

#### `tests/command/branch_json_test.rs`（JSON schema 稳定性，可选拆分文件）

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
| `src/internal/branch.rs` | **收口** | 把 `list_branches_with_conn()` / `find_branch_with_conn()` / `delete_branch_with_conn()` 从 `unwrap()` / `eprintln!()` 风格改为 `Result` 风格，暴露真实查询/删除失败与存储损坏，并扩展该文件内现有单元测试覆盖这些失败面 |
| `src/command/branch.rs` | **收口** | 保持已落地的 `BranchOutput` / JSON schema / 主要 `StableErrorCode` 不回退；后续补齐 `BranchError` typed enum（含 `StorageQueryFailed`/`StoredReferenceCorrupt`/`DeleteFailed` 新变体和 `DelegatedCli` 透传）、统一 `run_branch()` / `render_branch_output()`（替代 `run_branch_json()`）、抽取 delete 共享前置检查、补齐 create/force-delete 确认消息、fuzzy suggestion 和 `--help` EXAMPLES |
| `tests/command/branch_test.rs` | **扩展** | 在现有错误码和 JSON create 回归基础上，补齐 `BranchError` 变体覆盖、create/force-delete 确认消息和 fuzzy suggestion |
| `tests/command/branch_json_test.rs` | **可选拆分** | 若 `branch_test.rs` 中的 JSON 覆盖继续膨胀，可再拆出独立 schema 稳定性文件；当前不是阻断项 |
