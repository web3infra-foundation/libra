## Status 命令改进详细计划

> 最后编写时间：2026-03-26

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#第七批全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

`config`、`init`、`clone` 的主改造已在当前代码库落地。`status` 已经具备基础的 JSON 输出和 porcelain 格式。

**已确认落地的基线：**

- `config_kv` 后端已落地；`status` 已通过 `ConfigKv::get("core.bare")` 读取配置
- `OutputConfig` + `emit_json_data()` 输出框架已可用
- `execute_safe(args, output)` 入口已存在（`status.rs:837`），已区分 JSON / quiet / human 三种路径
- **JSON 输出已实现**（`status.rs:850-938`）：`collect_status_json()` 构建 `serde_json::Value`，`emit_status_json()` 通过 `emit_json_data("status", ...)` 输出
- 当前 JSON schema 包含：`head`、`has_commits`、`staged`（`new`/`modified`/`deleted`）、`unstaged`（`modified`/`deleted`）、`untracked`、`ignored`、`is_clean`、可选 `stash_entries`
- `--porcelain` v1/v2 已实现，与 Git 格式兼容
- `--short` 格式已实现，含颜色支持
- `--branch` 标志已实现（porcelain / short 模式）
- `--show-stash` 标志已实现（标准模式）
- `--ignored` 标志已实现（所有模式）
- `--untracked-files` 标志已实现（`normal`/`all`/`no`）
- `StatusError` 枚举已定义 5 个变体（`status.rs:101-113`）
- human 模式已有"下一步命令建议"（`use "libra add ..."` / `use "libra restore ..."` 等）
- `StatusError` 未实现 `From<StatusError> for CliError`——`execute_to()` 内部通过闭包 `status_error()` 手动包装为 `CliError::fatal()`

**基于当前代码的 Review 结论（status 仍需改进的部分）：**

- **无 `StableErrorCode`**：所有 `StatusError` 都被 `status_error()` 闭包包装为通用 `CliError::fatal()`，无显式错误码
- **bare repository 检测无错误码**：`is_bare_repository()` 判定后返回通用 `CliError::fatal("this operation must be run in a work tree")`
- **human 模式缺少 upstream tracking 信息**：标准模式 `On branch main` 后不显示 `Your branch is up to date with 'origin/main'`（审计报告要求补齐 ahead/behind 计数）
- **JSON schema 缺少 upstream 信息**：没有 `upstream` / `ahead` / `behind` 字段
- **porcelain v2 `--branch` 缺少 upstream 信息**：当前只输出 `branch.head` 和 `branch.oid`，没有 `branch.upstream` / `branch.ab`
- **`execute_to()` 和 `collect_status_json()` 有大量重复逻辑**：两个函数各自独立调用 `changes_to_be_committed_safe()`、`changes_to_be_staged()`、`collapse_untracked_directories()` 等，违反 DRY
- **共享数据层设计尚未覆盖 porcelain v2 元数据**：`output_porcelain_v2()` 目前仍会自己读取 index / HEAD tree 并拼装 mode/hash；如果 `StatusData` 只承载基础 `Changes`，那只是把重复从 human/JSON 移走，仍没解决 v2 的分叉逻辑
- **各种 `unwrap()` 残留**（如 `status.rs:967`、`977`、`983` 等）：`Index::load(path::index()).unwrap()` 在索引损坏时会 panic；`head_commit.unwrap()` 以及 `item_path.to_str().unwrap()` 也有 panic 风险。必须改成带错误处理的安全版本（已有的 `_safe` 变体中）。
- **颜色控制未统一**：`should_use_colors()` 函数独立读取 `color.status.short` / `color.ui` 配置，未与全局 `--color` / `--no-color` / `NO_COLOR` 标志协调
- **`--quiet` 模式语义不完整**：当前 `--quiet` 调用 `collect_status_json()` 后丢弃结果（`let _ = ...`），但不设置退出码——clean 和 dirty 都返回 exit `0`。Git 的 `git status --quiet` 在工作树 dirty 时返回 exit `1`

### 目标与非目标

**本批目标：**
- 为所有 `StatusError` 补齐显式 `StableErrorCode` 映射
- 补齐 upstream tracking 信息（human / JSON / porcelain v2）：ahead/behind 计数
- 消除 `execute_to()` 与 `collect_status_json()` 的逻辑重复
- 修复 `--quiet` 模式的退出码语义（dirty → exit `1`）
- 统一颜色控制与全局 `--color` / `NO_COLOR` 标志

**本批非目标：**
- **不改变 JSON schema 的已有字段**。已有 JSON 输出已面向用户，本批只做**向后兼容的增量扩展**（新增字段），不修改/删除现有字段
- **不引入 rename/copy 检测**。当前 `Changes` 结构不区分 rename，这是独立特性
- **不改变 porcelain v1 格式**。v1 是稳定的机器接口，与 Git 保持严格兼容
- **不改变核心 diff 逻辑**。`changes_to_be_committed_safe()` / `changes_to_be_staged()` 的算法保持不变

### 设计原则

1. **共享核心数据层必须覆盖所有渲染器**：`StatusData` 不仅服务 human / JSON，也要覆盖 short / porcelain v1 / porcelain v2；渲染层应只负责格式化，不再自行读取 index / HEAD tree
2. **JSON 向后兼容**：新增字段（`upstream`）是增量扩展，不改变现有字段名称、类型和既有路径语义；现有 path 数组继续保持“相对于当前工作目录”的显示形式
3. **`--quiet` 的 exit `1` 是 Git 兼容特例，不是 warning 机制**：dirty 判定独立于 `--exit-code-on-warning`，且必须保持 stderr 静默
   > **注意**：`--exit-code-on-warning` 标志**不适用**于 `status` 命令。`status --quiet` 使用 exit `1` 表示 dirty，这是 Git 兼容行为；`--exit-code-on-warning`（exit `9`）是 Libra 全局语义，用于 `add` / `clone` 等命令的 warning 场景。
4. **错误码显式映射**：每个 `StatusError` 变体都有确定的 `StableErrorCode`
5. **upstream 信息可选但不能丢失“gone”语义**：未配置 upstream 时 `upstream = null`；已配置但 tracking ref 不存在时，必须显式表达 `gone`
6. **本批只承诺精确 ahead/behind 计数**：不在本批引入 `truncated` / 近似计数契约，避免 human 与 JSON 出现含义不一致的“半精确”状态

### 特性 1：消除逻辑重复——StatusData 共享数据层

**当前问题：** `execute_to()` 和 `collect_status_json()` 各自独立调用相同的状态计算函数。新增任何功能（如 upstream）需要在两处同步维护。

**修正后的方案：**

新增内部结构体封装所有预计算的状态数据：

```rust
struct StatusData {
    head: Head,
    has_commits: bool,
    staged: Changes,
    tracked_unstaged: Changes,      // 仅 modified / deleted；new 保持空
    untracked: Vec<PathBuf>,
    ignored: Vec<PathBuf>,
    entries: Vec<StatusEntry>,      // short / porcelain 共用的合并状态视图
    stash_count: Option<usize>,     // Some 仅当 --show-stash 时
    upstream: Option<UpstreamInfo>, // 新增
}

struct StatusEntry {
    path: PathBuf,
    staged_status: char,
    unstaged_status: char,
    porcelain_v2: Option<PorcelainV2Entry>,
}

struct PorcelainV2Entry {
    submodule: String,
    mode_head: u32,
    mode_index: u32,
    mode_worktree: u32,
    hash_head: String,
    hash_index: String,
}

struct UpstreamInfo {
    remote_ref: String,    // 如 "origin/main"
    ahead: Option<usize>,
    behind: Option<usize>,
    gone: bool,
}
```

新增 `collect_status_data(args) -> CliResult<StatusData>`，将所有计算集中到一处。

改造后的调用链：
- `execute_safe()` → `collect_status_data()` → 根据 `OutputConfig` 选择渲染方式
- human / short / porcelain v1 / porcelain v2 渲染都从 `StatusData` 读取
- JSON 渲染从 `StatusData` 构建 `serde_json::Value`

> **实现边界：** 如果担心 porcelain v2 的 mode/hash 计算成本，可让 `collect_status_data()` 接受一个最小的 render capability 参数（例如 `needs_v2_metadata: bool`），只在 `--porcelain=v2` 时填充 `StatusEntry.porcelain_v2`。但无论是否延迟计算，**读取仓库状态的责任都必须留在数据收集层**，不能继续散落在渲染函数里。

### 特性 2：Upstream Tracking 信息

**当前问题：** 审计报告指出 `libra status` 不显示当前分支与远端分支的同步状态。这是 Git `status` 的标准功能：

```text
On branch main
Your branch is ahead of 'origin/main' by 2 commits.
```

**实现方案：**

upstream tracking 基于 `config_kv` 中的 `branch.<name>.remote` 和 `branch.<name>.merge` 配置。实现步骤：

1. 读取当前分支的 upstream 配置：
   - `branch.<name>.remote` → remote name（如 `origin`）
   - `branch.<name>.merge` → remote ref（如 `refs/heads/main`）
   - 组合得到 tracking ref：`refs/remotes/<remote>/<branch>`
2. 解析本地分支和远端 tracking ref 的 commit hash
3. 计算 ahead/behind：从两个 commit 向后遍历到公共祖先，返回**精确**的独有提交数
4. 如果 upstream 已配置但 tracking ref 不存在，保留 `remote_ref`，并将 `gone = true`
5. 如果 upstream 未配置，跳过（不报错）

**human 模式输出：**

```text
On branch main
Your branch is up to date with 'origin/main'.

On branch main
Your branch is ahead of 'origin/main' by 2 commits.
  (use "libra push" to publish your local commits)

On branch main
Your branch is behind 'origin/main' by 3 commits.
  (use "libra pull" to update your local branch)

On branch main
Your branch and 'origin/main' have diverged,
and have 2 and 3 different commits each, respectively.
  (use "libra pull" to merge the remote branch into yours)

On branch main
Your branch is based on 'origin/main', but the upstream is gone.
```

**short / porcelain v1 `--branch` 输出（与 Git 格式兼容）：**

```text
## main...origin/main [ahead 2]
## main...origin/main [behind 3]
## main...origin/main [ahead 2, behind 3]
## main...origin/main [gone]
```

**porcelain v2 `--branch` 输出（新增行）：**

```text
# branch.head main
# branch.oid abc123...
# branch.upstream origin/main
# branch.ab +2 -3
```

> **gone 场景：** `# branch.upstream origin/main` 仍输出，但 `# branch.ab` 省略，避免伪造 `+0 -0` 这类会误导脚本的计数。

**JSON 输出（新增 `upstream` 字段）：**

```json
{
  "head": {"type": "branch", "name": "main"},
  "has_commits": true,
  "upstream": {
    "remote_ref": "origin/main",
    "ahead": 2,
    "behind": 3,
    "gone": false
  },
  "staged": { ... },
  "unstaged": { ... },
  ...
}
```

无 upstream 配置时：

```json
{
  "head": {"type": "branch", "name": "main"},
  "has_commits": true,
  "upstream": null,
  ...
}
```

> **向后兼容**：`upstream` 是新增字段，不影响现有 JSON consumer。

**upstream gone 的 JSON 表达：**

```json
{
  "head": {"type": "branch", "name": "main"},
  "has_commits": true,
  "upstream": {
    "remote_ref": "origin/main",
    "ahead": null,
    "behind": null,
    "gone": true
  },
  ...
}
```

**ahead/behind 计算的实现边界：**

- 本批返回**精确计数**，不引入 `truncated` 或近似统计字段
- 如后续确认大型仓库性能需要上限控制，再以向后兼容方式新增字段；本批不提前承诺半成品 schema

### 特性 3：错误处理与 StableErrorCode

**`StatusError → CliError` 显式映射：**

当前所有 `StatusError` 都通过闭包 `status_error()` 包装为通用 `CliError::fatal()`。改为实现 `From<StatusError> for CliError`：

| StatusError 变体 | StableErrorCode | 退出码 | hint |
|-----------------|-----------------|--------|------|
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted` |
| `InvalidPathEncoding` | `CliInvalidTarget` | 129 | `path contains non-UTF-8 characters` |
| `FileHash` | `IoReadFailed` | 128 | 无 |
| `ListWorkdirFiles` | `IoReadFailed` | 128 | 无 |
| `Workdir` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |

**bare repository 检测：**

当前返回通用 `CliError::fatal("this operation must be run in a work tree")`。改为：
- `StableErrorCode::RepoStateInvalid`
- hint: `this command requires a working tree; bare repositories do not have one`

**渲染层 I/O 失败归并：**

- `output_porcelain_v2()` 当前单独 `Index::load()` / 读取 HEAD tree 的失败路径，也必须回收到 `collect_status_data()` 阶段，通过 `StatusError` 统一映射
- 改造后渲染层只允许产生“写 stdout/stderr 失败”的 `CliError::io(...)`；不再临时拼装 `"failed to determine working tree status: ..."` 的字符串错误

### 特性 4：`--quiet` 退出码修复

**当前问题：** `--quiet` 模式下，无论工作树是否 dirty 都返回 exit `0`。Git 的 `git status --quiet` 在 dirty 时返回 exit `1`，这被 CI/CD 和脚本广泛依赖。

**修正后的方案：**

```rust
// execute_safe() 中 --quiet 分支
if output.quiet {
    let data = collect_status_data(&args).await?;
    if data.is_dirty_for_requested_view() {
        // dirty working tree → exit 1
        return Err(status_quiet_exit(1));
    }
    return Ok(()); // clean → exit 0
}
```

`status_quiet_exit(1)` 表示一个**仅供 status 使用**的 silent-exit 通道（可实现为局部 helper，或在 `CliError` 上做最小扩展），要求：

- 退出码为 `1`
- stdout / stderr 都不输出任何内容
- **不**复用 `WarningEmitted` 或 `--exit-code-on-warning` 机制；dirty 工作树不是 warning
- dirty 判定基于 `collect_status_data()` 已处理后的视图，因此 `--untracked-files=no` 下“只有 untracked 文件”的仓库应视为 clean 并返回 exit `0`

**JSON 模式不受影响**：`--json` 始终返回完整 status 信息到 stdout，退出码始终 `0`（error 场景除外）。Agent 通过 `is_clean` 字段判断状态。

### 特性 5：颜色控制统一

**当前问题：** `should_use_colors()` 函数独立读取 `color.status.short` / `color.ui` 配置，未与全局 `OutputConfig.color` 标志（`--color` / `--no-color`）协调。

**修正后的方案：**

`should_use_colors()` 的判断优先级：

1. `OutputConfig.color == ColorChoice::Never` → 不着色（`--no-color` / `NO_COLOR`）
2. `OutputConfig.color == ColorChoice::Always` → 始终着色（`--color=always`）
3. `OutputConfig.color == ColorChoice::Auto` → 读取 `color.status.short` → `color.ui` → TTY 检测

需要将 `OutputConfig` 传递给渲染函数（`output_short_format`、`execute_to` 等），替代当前独立读取 config 的方式。

### 特性 6：Cross-Cutting Improvements 在 status 中的具体落地

| ID | 改进 | status 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 运行时错误（索引损坏、I/O 失败）→ exit `128`；`--quiet` dirty → exit `1`（Git 兼容特例）；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | `--untracked-files` 的值是 clap `ValueEnum`（`normal`/`all`/`no`），clap 已提供候选列表；`--porcelain` 同理（`v1`/`v2`）。无额外 fuzzy match 需求 |
| **G** | Issues URL | 仅在 `IndexLoad` 内部原因为非预期的 GitError 时输出 Issues URL。路径编码错误等用户可修复问题不输出 |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra status                       Show working tree status
    libra status -s                    Short format output
    libra status --porcelain           Machine-readable output (v1)
    libra status --porcelain v2        Extended machine-readable output
    libra status --branch              Include branch info in short/porcelain
    libra status --show-stash          Show stash count
    libra status --ignored             Include ignored files
    libra status --untracked-files=no  Hide untracked files
    libra status --json                Structured JSON output for agents
```

### 全部场景结构化 Output 设计（`--json` / `--machine`）

所有结构化输出遵循统一信封格式，通过 `emit_json_data()` 输出到 stdout。错误 JSON 通过 `CliError` 输出到 stderr。`--machine` 与 `--json` 使用同一 schema，仅格式化方式不同（紧凑单行）。

#### 成功 envelope

```json
{
  "ok": true,
  "command": "status",
  "data": { ... }
}
```

#### `libra status --json`（有变更 + upstream tracking）

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {"type": "branch", "name": "main"},
    "has_commits": true,
    "upstream": {
      "remote_ref": "origin/main",
      "ahead": 2,
      "behind": 0,
      "gone": false
    },
    "staged": {
      "new": ["src/new_file.rs"],
      "modified": ["src/main.rs"],
      "deleted": []
    },
    "unstaged": {
      "modified": ["README.md"],
      "deleted": []
    },
    "untracked": ["temp.log"],
    "ignored": [],
    "is_clean": false
  }
}
```

#### `libra status --json`（clean + 无 upstream）

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {"type": "branch", "name": "feature-x"},
    "has_commits": true,
    "upstream": null,
    "staged": {
      "new": [],
      "modified": [],
      "deleted": []
    },
    "unstaged": {
      "modified": [],
      "deleted": []
    },
    "untracked": [],
    "ignored": [],
    "is_clean": true
  }
}
```

#### `libra status --show-stash`

```text
On branch main
Your branch is up to date with 'origin/main'.

nothing to commit, working tree clean

Stash entries: 3
```

#### `libra status --show-stash --json`

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {"type": "branch", "name": "main"},
    "has_commits": true,
    "upstream": {
      "remote_ref": "origin/main",
      "ahead": 0,
      "behind": 0,
      "gone": false
    },
    "staged": {
      "new": [],
      "modified": [],
      "deleted": []
    },
    "unstaged": {
      "modified": [],
      "deleted": []
    },
    "untracked": [],
    "ignored": [],
    "is_clean": true,
    "stash_entries": 3
  }
}
```

#### `libra status --json`（detached HEAD）

```json
{
  "ok": true,
  "command": "status",
  "data": {
    "head": {"type": "detached", "oid": "abc123def456..."},
    "has_commits": true,
    "upstream": null,
    "staged": {
      "new": [],
      "modified": [],
      "deleted": []
    },
    "unstaged": {
      "modified": [],
      "deleted": []
    },
    "untracked": [],
    "ignored": [],
    "is_clean": true
  }
}
```

#### 错误 JSON：索引损坏

```json
{
  "ok": false,
  "error_code": "LBR-REPO-002",
  "category": "repo",
  "exit_code": 128,
  "message": "failed to determine working tree status: failed to open index '.libra/index': <detail>",
  "hints": [
    "the index file may be corrupted"
  ]
}
```

#### 错误 JSON：bare repository

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "this operation must be run in a work tree",
  "hints": [
    "this command requires a working tree; bare repositories do not have one"
  ]
}
```

### Libra vs Git status 对比

| 特性 | Git | Libra（当前） | Libra（本批目标） |
|------|-----|-------------|------------------|
| 基础 human 输出 | 完整 | ✅ 完整 | 保持 |
| `--short` | ✅ | ✅ | 保持 |
| `--porcelain` v1 | ✅ | ✅ | 保持 |
| `--porcelain` v2 | ✅ | ✅ | 补齐 upstream 行 |
| `--branch` | ✅ | ✅（不含 upstream） | **补齐 upstream tracking** |
| `--show-stash` | ✅ | ✅ | 保持 |
| `--ignored` | ✅ | ✅ | 保持 |
| `--untracked-files` | `normal/all/no` | ✅ `normal/all/no` | 保持 |
| `--json` | ❌ | ✅ | **补齐 upstream 字段** |
| Upstream ahead/behind | ✅ | ❌ | **新增** |
| `--quiet` exit code | dirty → `1` | dirty → `0` ❌ | **修复为 `1`** |
| 稳定错误码 | ❌ | ❌ | **新增 `StableErrorCode`** |
| hint 建议 | 有 | 有 | 保持 |

### 测试要求

#### `tests/command/status_test.rs`（核心路径扩展）

- **（已有）** 基础测试：outside repository、ignored outputs、bare repository、porcelain v1/v2、short format、untracked files
- **（新增）`StableErrorCode` 验证**：bare repository 错误返回 `LBR-REPO-003`
- **（新增）`--quiet` 退出码与静默性**：dirty 工作树 → exit `1` 且 stdout/stderr 均为空；clean 工作树 → exit `0`
- **（新增）`--quiet` 与过滤参数联动**：仅有 untracked 文件时，`--untracked-files=no --quiet` 返回 exit `0`
- **（新增）颜色控制验证**：`--color=always` 强制着色，`--color=never` 禁用着色，`NO_COLOR=1` 环境变量禁用着色
- **（新增）upstream tracking human 输出**：覆盖 up-to-date / ahead / behind / diverged / gone 五种文案
- **（新增）short / porcelain v1 `--branch` upstream**：输出包含 `[ahead N]` / `[behind N]` / `[gone]`
- **（新增）upstream ahead/behind 准确性**：创建本地和远端 commit 后，ahead / behind / diverged 计数都准确
- **（新增）porcelain v2 `--branch` upstream**：输出包含 `# branch.upstream`；仅在 `gone == false` 时包含 `# branch.ab`

#### `tests/command/status_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性：
  - `head` 是 object，包含 `type`（`"branch"` 或 `"detached"`）
  - `has_commits` 是 bool
  - `upstream` 是 object 或 null
  - 当 `upstream` 非 null 时包含 `remote_ref`（string）和 `gone`（bool）
  - 当 `upstream.gone == false` 时，`ahead` / `behind` 是 number
  - 当 `upstream.gone == true` 时，`ahead` / `behind` 为 null
  - `staged` / `unstaged` 是 object，内含 `new` / `modified` / `deleted` 数组
  - `untracked` 是 string 数组
  - `ignored` 是 string 数组
  - `is_clean` 是 bool
- **upstream 场景覆盖**：
  - 无 upstream 配置 → `upstream` 为 `null`
  - 配置 upstream 且 up-to-date → `gone: false, ahead: 0, behind: 0`
  - ahead / behind / diverged → 计数准确
  - upstream gone → `gone: true` 且保留 `remote_ref`
  - detached HEAD → `upstream` 为 `null`
- **`--show-stash --json`**：包含 `stash_entries` 字段
- **路径向后兼容验证**：从仓库子目录执行 `libra status --json` 时，已有路径字段继续按“当前工作目录相对路径”返回
- **`--machine status`**：stdout 按 `\n` 分割后恰好 1 行非空行，可被 `serde_json::from_str()` 解析为与 `--json` 相同的 schema
- **向后兼容验证**：已有字段（`head`、`staged`、`unstaged`、`untracked`、`ignored`、`is_clean`）的类型和语义不变
- **（新增）`porcelain=v2` 完整行格式**：验证 XY 字段、模式位、hash 值的格式符合 Git porcelain v2 规范

#### `tests/command/status_error_test.rs`（错误码验证，新增文件）

- bare repository 返回 `LBR-REPO-003`
- 仓库外执行返回 `LBR-REPO-001`
- 索引损坏返回 `LBR-REPO-002`（需构造损坏索引文件）
- `--json` 模式下 bare repository / 索引损坏仍通过 stderr 输出结构化错误 JSON，不污染 stdout

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/status.rs` | **重构** | 新增 `StatusData` / `StatusEntry` / `UpstreamInfo` 共享数据层；`collect_status_data()` 统一 human / JSON / short / porcelain v1/v2 所需计算；`StatusError → CliError` 显式 `StableErrorCode` 映射；upstream tracking（含 gone 语义）；`--quiet` 退出码修复；颜色控制统一到 `OutputConfig`；清理遗留的 `unwrap()` 方法换用 `Result` |
| `src/command/{commit,remove,rebase}.rs`、`src/utils/ignore.rs` | **小改** | 迁移生产代码对 `changes_to_be_committed()` 的依赖，改用 `_safe` 变体或等价的不 panic helper，消除 `unwrap()` 风险。**这些修改是接口兼容的，只改变内部错误处理方式，不改变 API 签名** |
| `src/internal/head.rs` | **小改** | 视需要新增 `resolve_upstream_ref()` helper，读取 `branch.<name>.remote` + `branch.<name>.merge` 配置并解析为 tracking ref |
| `src/utils/error.rs` | **可选小改** | 仅当现有执行链无法承载 status 的 silent exit 时，才新增最小 helper；不要把 dirty→exit `1` 误建模为 `WarningEmitted` |
| `tests/command/status_test.rs` | **扩展** | 新增 `StableErrorCode`、`--quiet` 退出码、upstream tracking 场景 |
| `tests/command/status_json_test.rs` | **新增** | JSON schema 完整性、upstream 场景覆盖、向后兼容验证 |
| `tests/command/status_error_test.rs` | **新增** | 错误码对齐验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
