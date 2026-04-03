## Diff 命令改进详细计划

> 最后编写时间：2026-04-04

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`diff` 是第三批（历史查询命令）中 Agent 依赖最强的命令之一，当前工作区已经按本计划完成主改造；本文保留为对外契约和后续维护基线。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, _output)` 双入口已存在（`diff.rs`）
- `Pager` 支持 `Pager::with_config(output)` 自动检测 TTY 与 `--no-pager`
- `git_internal::Diff::diff()` 提供核心 diff 算法（histogram、myers、myersMinimal）
- `--old` / `--new` / `--staged` / `--algorithm` / `--output` / pathspec 已实现
- 彩色输出已实现（TTY 检测，bold/cyan/red/green）
- `run_diff()` + `DiffOutput` 已落地，`--json` / `--machine` 可返回 hunk 级结构化 diff
- `DiffError` typed enum 已落地，主要错误路径已显式映射到 `StableErrorCode`
- `--name-only` / `--name-status` / `--numstat` / `--stat` 已落地，并与 JSON 契约解耦
- `--quiet` 已收口为仅抑制 stdout；存在差异时返回 exit `1`，即使同时写入 `--output`
- `--help` EXAMPLES 已落地；生产路径 `unwrap()` 和死代码 `similar_diff_result()` 已移除

**基于当前代码的 Review 结论（已改进部分 vs 后续维护重点）：**

已改进（当前代码已具备）：

- `run_diff()` / `DiffOutput` 已把执行层与渲染层拆开，human / JSON / machine 共用一套数据层
- `DiffError` 已替代散落的 `fatal()` / `eprintln!()`，主要错误路径均有显式 `StableErrorCode`
- `--name-only` / `--name-status` / `--numstat` / `--stat` 已补齐，JSON 恒定返回完整结构，不受 human 视图标志影响
- `--quiet` 契约已与用户习惯对齐：仅抑制 stdout，通过退出码表达“有无差异”
- `--output` 在 JSON 模式下被忽略；human 模式下即使搭配 `--quiet` 也会保留文件写入副作用

后续维护重点：

- 继续用回归测试锁住 `--quiet`、`--output`、`--staged` 和 pathspec 组合行为
- 大 diff / 二进制 diff 的性能与 pager 体验继续观察，但不阻塞第三批验收

### 目标与非目标

**已完成目标：**
- `DiffError`、`run_diff()` / `DiffOutput`、JSON / machine、human 多视图输出、`--quiet` exit code 和 `--help` EXAMPLES 已全部落地

**后续维护目标：**
- 继续把 review 回归（quiet / output / staged / invalid revision）锁进集成测试

**本批非目标：**
- **不改变 `git_internal::Diff::diff()` 核心算法**。diff 生成逻辑不变
- **不引入 word-level diff**。这是独立特性，不在本批范围
- **不引入 diff 缓存**。每次执行重新计算
- **不改变 `--algorithm` 选项**。histogram/myers/myersMinimal 保持现有行为
- **不在 JSON 中输出颜色信息**。颜色是 human 表示层概念

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_diff()` 收集结构化结果，再根据 `OutputConfig` 渲染 human / JSON / machine
2. **JSON 提供 hunk 级别结构化**：每个文件的变更包含 hunks 数组，每个 hunk 包含行范围和内容
3. **错误码显式映射**：每个 `DiffError` 变体都有确定的 `StableErrorCode`
4. **消除 `unwrap()`**：所有 fallible 操作改为 `?` + `map_err()` 返回 `DiffError`
5. **JSON 模式下 `--stat` / `--name-only` 等是 no-op**：JSON 始终包含完整信息
6. **`--output` 标志在 JSON 模式下被忽略**：JSON 输出只到 stdout

### 特性 1：DiffError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("invalid revision: '{0}'")]
    InvalidRevision(String),

    #[error("failed to load object '{commit_id}': {detail}")]
    ObjectLoad { commit_id: String, detail: String },

    #[error("failed to load index: {0}")]
    IndexLoad(String),

    #[error("failed to list working directory files: {0}")]
    WorkdirList(String),

    #[error("failed to read file '{path}': {detail}")]
    FileRead { path: String, detail: String },

    #[error("failed to write output file '{path}': {detail}")]
    OutputWrite { path: String, detail: String },

    #[error("failed to compute diff: {0}")]
    DiffCompute(String),
}
```

**`DiffError → CliError` 显式映射：**

| DiffError 变体 | StableErrorCode | 退出码 | hint |
|---------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidRevision` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `ObjectLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted; try 'libra status' to verify` |
| `IndexLoad` | `RepoCorrupt` | 128 | `the index file may be corrupted` |
| `WorkdirList` | `IoReadFailed` | 128 | 无 |
| `FileRead` | `IoReadFailed` | 128 | 无 |
| `OutputWrite` | `IoWriteFailed` | 128 | 无 |
| `DiffCompute` | `InternalInvariant` | 128 | Issues URL |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct DiffHunk {
    /// Old file start line (1-indexed)
    pub old_start: usize,
    /// Old file line count
    pub old_lines: usize,
    /// New file start line (1-indexed)
    pub new_start: usize,
    /// New file line count
    pub new_lines: usize,
    /// Hunk content lines (with +/-/space prefixes)
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffFileStat {
    /// File path
    pub path: String,
    /// Change status: "added", "modified", "deleted", "renamed"
    pub status: String,
    /// Number of inserted lines
    pub insertions: usize,
    /// Number of deleted lines
    pub deletions: usize,
    /// Hunks for this file
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffOutput {
    /// Source description (e.g., "HEAD", commit hash, "index")
    pub old_ref: String,
    /// Target description (e.g., "working tree", commit hash, "index")
    pub new_ref: String,
    /// Changed files with hunks and statistics
    pub files: Vec<DiffFileStat>,
    /// Summary statistics
    pub total_insertions: usize,
    /// Summary statistics
    pub total_deletions: usize,
    /// Total files changed
    pub files_changed: usize,
}
```

改造后的调用链：
- `execute_safe(args, output)` → `run_diff(args)` → 返回 `DiffOutput`
- `run_diff()` 解析参数、收集 blobs、执行 diff、生成结构化结果
- `execute_safe()` 根据 `OutputConfig` 选择渲染
- human 模式：unified diff / stat / name-only / name-status（使用现有 `colorize_diff()`）
- JSON/machine 模式：`emit_json_data("diff", &output, config)`

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | unified diff（彩色如 TTY） | 无 |
| human + `--stat` | diffstat 统计 | 无 |
| human + `--name-only` | 变更文件名列表 | 无 |
| human + `--name-status` | 变更文件名 + 状态 | 无 |
| human + `--numstat` | TAB 分隔的 insertions/deletions/path | 无 |
| human + `--quiet` | 无（仅 exit code 表示是否有差异） | 无 |
| `--json` / `--machine` | JSON envelope | 无 |

### 特性 3：JSON 输出设计

**成功输出（有差异）：**

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "HEAD",
    "new_ref": "working tree",
    "files": [
      {
        "path": "src/main.rs",
        "status": "modified",
        "insertions": 5,
        "deletions": 3,
        "hunks": [
          {
            "old_start": 10,
            "old_lines": 7,
            "new_start": 10,
            "new_lines": 9,
            "lines": [
              " context line",
              "-old line",
              "+new line",
              " context line"
            ]
          }
        ]
      }
    ],
    "total_insertions": 5,
    "total_deletions": 3,
    "files_changed": 1
  }
}
```

**无差异：**

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "HEAD",
    "new_ref": "working tree",
    "files": [],
    "total_insertions": 0,
    "total_deletions": 0,
    "files_changed": 0
  }
}
```

**`--staged --json`：**

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "HEAD",
    "new_ref": "index",
    "files": [ "..." ],
    "total_insertions": 10,
    "total_deletions": 2,
    "files_changed": 3
  }
}
```

**`--old <A> --new <B> --json`：**

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "abc1234",
    "new_ref": "def5678",
    "files": [ "..." ],
    "total_insertions": 15,
    "total_deletions": 8,
    "files_changed": 4
  }
}
```

### 特性 4：新增输出格式

**`--numstat` 输出格式（新增）：**

```text
5       3       src/main.rs
10      0       src/new_file.rs
0       8       src/deleted_file.rs
```

TAB 分隔的三列：insertions、deletions、path。与 Git `git diff --numstat` 格式一致。

**`--name-only`（新增，对齐 Git）：**

```text
src/main.rs
src/new_file.rs
```

**`--name-status`（新增，对齐 Git）：**

```text
M       src/main.rs
A       src/new_file.rs
D       src/deleted_file.rs
```

**`--stat`（新增，对齐 Git）：**

```text
 src/main.rs     | 8 +++++---
 src/new_file.rs | 10 ++++++++++
 2 files changed, 15 insertions(+), 3 deletions(-)
```

### 特性 5：Cross-Cutting Improvements 在 diff 中的具体落地

| ID | 改进 | diff 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 revision）→ exit `129`；运行时错误（object 损坏、I/O 失败）→ exit `128`；成功（无论是否有差异）→ exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | `--algorithm` 值不匹配时提示 `did you mean 'histogram'?` |
| **G** | Issues URL | 仅在 `DiffCompute` / `ObjectLoad` 错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra diff                             Show working tree changes
    libra diff --staged                    Show staged changes
    libra diff --old HEAD~3 --new HEAD     Compare two commits
    libra diff --stat                      Show diffstat summary
    libra diff --numstat                   Machine-readable stat output
    libra diff --name-only                 Show only changed file names
    libra diff --name-status               Show changed files with status
    libra diff --json                      Structured JSON output for agents
    libra diff src/main.rs                 Diff a specific file
    libra diff --output changes.patch      Write diff to file
```

### 测试要求

#### `tests/command/diff_test.rs`（核心执行路径扩展）

- **（已有）** 基础 diff（working tree、staged、commit-to-commit）、pathspec、algorithm、output file
- **（新增）`DiffError` 变体覆盖**：
  - `InvalidRevision`：无效 `--old` 或 `--new` 返回 exit `129`
  - `IndexLoad`：损坏 index 返回 exit `128`
- **（新增）`run_diff()` 结构化结果**：验证 `DiffOutput.files` 中 path/status/insertions/deletions 准确
- **（新增）`--numstat` 输出**：TAB 分隔格式正确
- **（新增）`--name-only` 输出**：仅文件名
- **（新增）`--name-status` 输出**：状态 + 文件名
- **（新增）`--stat` 输出**：diffstat 格式正确
- **（新增）unwrap 消除**：此前 panic 场景现在返回结构化错误

#### `tests/command/diff_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **`--staged --json`**：`old_ref == "HEAD"`, `new_ref == "index"`
- **`--old A --new B --json`**：`old_ref` / `new_ref` 包含 commit hash
- **无差异 `--json`**：`files` 为空数组，统计值为 0
- **hunk 结构**：`hunks` 数组中每个元素包含 `old_start`/`old_lines`/`new_start`/`new_lines`/`lines`
- **pathspec `--json`**：仅返回匹配路径的文件
- **`--machine diff`**：stdout 恰好 1 行非空行
- **错误 JSON**：无效 revision 返回 `ok == false` + 错误码

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/diff.rs` | **重构** | 新增 `DiffError` typed enum；新增 `DiffOutput` / `DiffFileStat` / `DiffHunk` 结构体；新增 `run_diff()` 纯执行入口；消除全部 7 处 `unwrap()`；删除死代码 `similar_diff_result()`；新增 `--numstat` / `--name-only` / `--name-status` / `--stat` 输出；`DiffError → CliError` 显式 `StableErrorCode` 映射；JSON 输出；补齐 `--help` EXAMPLES |
| `tests/command/diff_test.rs` | **扩展** | 新增 `DiffError` 变体覆盖、新输出格式、unwrap 消除验证 |
| `tests/command/diff_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
