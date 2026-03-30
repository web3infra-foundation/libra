## Log 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令（config、init、clone、add、status、commit、push、pull）的主改造已在当前代码库落地。`log` 是第三批（历史查询命令）中最关键的命令，AI Agent / MCP 场景依赖结构化提交列表。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在（`log.rs:313/322`）
- `Pager` 支持 `Pager::with_config(output)` 自动检测 TTY 与 `--no-pager`
- `internal/log/formatter.rs`（205 行）提供 Full/Oneline/Custom 三种格式
- `internal/log/date_parser.rs`（89 行）支持绝对日期、Unix 时间戳和相对日期
- `CommitFilter` 支持 author、since/until、path 过滤（`log.rs:121-197`）
- `GraphState` 支持 ASCII 图形渲染（`log.rs:805-876`）
- `--stat` / `--name-only` / `--name-status` / `--patch` 输出模式已实现
- `--decorate` 支持 no/short/full/auto（从 `log.decorate` 配置读取默认值）

**基于当前代码的 Review 结论（log 仍需改进的部分）：**

- **JSON 输出被明确拒绝**：`execute_safe()` 在 `output.is_json()` 时返回 `CliError::command_usage("`log` does not yet support --json or --machine output")`（`log.rs:323-326`）。这是 MCP 维度最关键的改进
- **无 `StableErrorCode`**：所有错误使用 `CliError::fatal()` 无显式错误码
- **无 `LogError` typed enum**：错误散落在 `execute_safe()` 内部，无法被调用方结构化匹配
- **`--quiet` 已实现**（`log.rs:385-387`）但无 JSON 模式下的行为定义
- **过滤结果无结构化**：author/date/path 过滤只影响 human 输出，无法让 Agent 获取过滤后的结构化提交列表
- **graph 输出无结构化**：`--graph` 仅影响 human 输出，不适用于 JSON

### 目标与非目标

**本批目标：**
- 引入 `LogError` typed error enum，覆盖 log 层面的错误场景
- 所有 `LogError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_log(args) -> Result<LogOutput, LogError>` 纯执行入口
- 实现 JSON 输出（结构化提交列表），替代当前的 `command_usage` 拒绝
- JSON 输出包含过滤后的完整提交元数据（hash、author、date、subject、body、refs、parents、files_changed）
- 保持 `--oneline` / `--graph` / `--pretty` / `--stat` / `--patch` / `--name-only` / `--name-status` 等 human 模式输出不变
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不改变 commit walking 算法**。`get_reachable_commits()` 保持现有拓扑排序逻辑
- **不改变 `CommitFilter` 过滤逻辑**。author / date / path 过滤行为不变
- **不改变 `GraphState` 渲染算法**。ASCII 图形保持现有布局
- **不在 JSON 中输出 graph 信息**。图形渲染是 human 表示层概念，不进入结构化输出
- **不在 JSON 中输出 patch/diff 内容**。Diff 结构化输出由 `diff` 命令负责；log 的 JSON 只包含提交元数据和文件变更摘要
- **不引入分页控制到 JSON**。JSON 输出总是完整列表（受 `-n` 限制）

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_log()` 收集结构化结果，再根据 `OutputConfig` 渲染 human / JSON / machine
2. **JSON 只包含提交元数据和文件变更摘要**：不包含 patch/diff 内容和 graph 信息
3. **错误码显式映射**：每个 `LogError` 变体都有确定的 `StableErrorCode`
4. **JSON 模式下忽略仅影响 human 显示的标志**：`--oneline` / `--graph` / `--pretty` / `--decorate` / `--abbrev-commit` 在 JSON 模式下是 no-op，因为 JSON 始终包含完整信息
5. **JSON 模式下 `--stat` / `--name-only` / `--name-status` / `--patch` 也是 no-op**：JSON 始终包含 `files_changed` 摘要（名称 + 状态），Agent 如需完整 diff 应使用 `libra diff --json`
6. **`-n` 限制在执行层生效**：JSON 也遵守 `-n` 限制
7. **过滤条件在执行层生效**：JSON 输出的提交列表已经过 author/date/path 过滤

### 特性 1：LogError typed error enum

**当前问题：** 错误散落在 `execute_safe()` 内部，使用 `CliError::fatal()` 无显式错误码。

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("invalid object name: '{0}'")]
    InvalidObjectName(String),

    #[error("failed to load object '{commit_id}': {detail}")]
    ObjectLoad { commit_id: String, detail: String },

    #[error("your current branch '{branch}' does not have any commits yet")]
    EmptyBranch { branch: String },

    #[error("current HEAD does not have any commits yet")]
    EmptyHead,

    #[error("invalid --since date: '{value}'")]
    InvalidSinceDate { value: String },

    #[error("invalid --until date: '{value}'")]
    InvalidUntilDate { value: String },

    #[error("invalid --decorate option: '{value}'")]
    InvalidDecorateOption { value: String },
}
```

**`LogError → CliError` 显式映射：**

| LogError 变体 | StableErrorCode | 退出码 | hint |
|--------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidObjectName` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `ObjectLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted; try 'libra status' to verify` |
| `EmptyBranch` | `RepoStateInvalid` | 128 | `create a commit first with 'libra commit'` |
| `EmptyHead` | `RepoStateInvalid` | 128 | `create a commit first with 'libra commit'` |
| `InvalidSinceDate` | `CliInvalidArguments` | 129 | `supported formats: YYYY-MM-DD, "N days ago", unix timestamp` |
| `InvalidUntilDate` | `CliInvalidArguments` | 129 | `supported formats: YYYY-MM-DD, "N days ago", unix timestamp` |
| `InvalidDecorateOption` | `CliInvalidArguments` | 129 | `valid options: no, short, full, auto` |

### 特性 2：执行层与渲染层拆分

**当前问题：** `execute_safe()` 直接在内部做 commit walking、格式化和输出，约 240 行混合逻辑。

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct LogCommitEntry {
    /// Full commit hash
    pub hash: String,
    /// Short hash (7 chars)
    pub short_hash: String,
    /// Author name
    pub author_name: String,
    /// Author email
    pub author_email: String,
    /// Author timestamp (ISO 8601)
    pub author_date: String,
    /// Committer name
    pub committer_name: String,
    /// Committer email
    pub committer_email: String,
    /// Committer timestamp (ISO 8601)
    pub committer_date: String,
    /// First line of commit message
    pub subject: String,
    /// Full commit message body (excluding subject)
    pub body: String,
    /// Parent commit hashes
    pub parents: Vec<String>,
    /// Reference names pointing to this commit (branches, tags)
    pub refs: Vec<String>,
    /// Changed files with status (always populated; equivalent to --name-status)
    pub files: Vec<LogFileChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogFileChange {
    /// File path
    pub path: String,
    /// Change type: "added", "modified", "deleted"
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogOutput {
    /// List of commits (already filtered and limited by -n)
    pub commits: Vec<LogCommitEntry>,
    /// Total number of commits before -n limit (for pagination context)
    pub total: Option<usize>,
}
```

改造后的调用链：
- `execute_safe(args, output)` → `run_log(args)` → 返回 `LogOutput`
- `run_log()` 内部做 commit walking + filtering + 收集结构化结果
- `execute_safe()` 根据 `OutputConfig` 选择渲染：human / JSON / machine
- human 模式使用现有 `CommitFormatter` + `GraphState` 渲染
- JSON/machine 模式使用 `emit_json_data("log", &output, config)`

> **`total` 字段说明：** 当使用 `-n` 限制时，`total` 为 `None`（不额外扫描全部提交来计数，避免性能开销）。不使用 `-n` 时 `total` 等于 `commits.len()`。

> **`files` 字段说明：** 每个 `LogCommitEntry` 始终包含 `files`（变更文件列表），相当于 `--name-status` 的结构化版本。对于 root commit，files 是该 commit tree 中的所有文件（status 为 "added"）。Agent 如需完整 diff 内容，应使用 `libra diff --old <hash>~ --new <hash> --json`。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 现有格式（Full/Oneline/Custom + 可选 graph/patch/stat/name-only/name-status） | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope | 无 |

### 特性 3：JSON 输出设计

**成功输出：**

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [
      {
        "hash": "abc1234567890abcdef1234567890abcdef123456",
        "short_hash": "abc1234",
        "author_name": "Alice",
        "author_email": "alice@example.com",
        "author_date": "2026-03-30T10:00:00+08:00",
        "committer_name": "Alice",
        "committer_email": "alice@example.com",
        "committer_date": "2026-03-30T10:00:00+08:00",
        "subject": "feat: add new feature",
        "body": "Detailed description of the change.\n\nSigned-off-by: Alice <alice@example.com>",
        "parents": ["def5678901234abcdef5678901234abcdef567890"],
        "refs": ["HEAD -> main", "tag: v1.0"],
        "files": [
          { "path": "src/main.rs", "status": "modified" },
          { "path": "src/new_file.rs", "status": "added" }
        ]
      }
    ],
    "total": null
  }
}
```

**空结果（empty branch）：**

```json
{
  "ok": false,
  "error_code": "LBR-REPO-003",
  "category": "repo",
  "exit_code": 128,
  "message": "your current branch 'main' does not have any commits yet",
  "hints": [
    "create a commit first with 'libra commit'"
  ]
}
```

**`-n 1 --json`（单条提交）：**

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [ { "...single commit..." } ],
    "total": null
  }
}
```

### 特性 4：Cross-Cutting Improvements 在 log 中的具体落地

| ID | 改进 | log 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效日期、无效 decorate 选项）→ exit `129`；运行时错误（object 损坏、空分支）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | `--decorate` 值不匹配时提示 `did you mean 'short'?`（Levenshtein 距离 ≤ 2） |
| **G** | Issues URL | 仅在 `ObjectLoad` 错误时输出 Issues URL。日期解析/参数错误是用户可修复问题 |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra log                              Show commit history
    libra log -n 10                        Show last 10 commits
    libra log --oneline                    Compact one-line format
    libra log --graph --oneline            ASCII graph with one-line format
    libra log --author="Alice"             Filter by author
    libra log --since="2 weeks ago"        Commits from the last 2 weeks
    libra log -p src/main.rs               Show patches for a specific file
    libra log --stat                       Show diffstat for each commit
    libra log --name-status                Show changed files with status
    libra log --json                       Structured JSON output for agents
    libra log --json -n 5                  Last 5 commits as JSON
```

### 测试要求

#### `tests/command/log_test.rs`（核心执行路径扩展）

- **（已有）** 基础 log 执行、oneline、abbrev、patch、stat、author 过滤、日期过滤、pathspec、graph、decorate 解析
- **（新增）`LogError` 变体覆盖**：
  - `InvalidObjectName`：无效 revision 返回对应错误
  - `EmptyBranch`：空分支返回对应错误 + hint
  - `InvalidSinceDate`：无效日期格式返回 exit `129`
  - `InvalidDecorateOption`：无效 decorate 值返回 exit `129`
- **（新增）`run_log()` 结构化结果**：验证 `LogOutput.commits` 中 hash/author/subject/files 分类准确

#### `tests/command/log_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **`-n 1 --json`**：`commits` 数组恰好 1 个元素
- **`--author --json`**：过滤后的 commits 仅包含匹配 author
- **`--since --json`**：过滤后的 commits 日期均在指定范围内
- **pathspec `--json`**：`files` 数组仅包含匹配路径的变更
- **empty branch `--json`**：返回 `ok == false` + 错误码
- **root commit `--json`**：`parents` 为空数组，`files` 全部为 "added"
- **`--machine log`**：stdout 恰好 1 行非空行，可被 `serde_json::from_str()` 解析
- **human-only flags in JSON mode**：`--oneline --json`、`--graph --json` 等不影响 JSON 输出内容

#### CLI 错误码验证

- `InvalidObjectName` 返回 `LBR-CLI-003`
- `EmptyBranch` 返回 `LBR-REPO-003`
- `InvalidSinceDate` 返回 `LBR-CLI-002`
- `InvalidDecorateOption` 返回 `LBR-CLI-002`
- 仓库外执行返回 `LBR-REPO-001`

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/log.rs` | **重构** | 新增 `LogError` typed enum；新增 `LogOutput` / `LogCommitEntry` / `LogFileChange` 结构体；新增 `run_log()` 纯执行入口；`LogError → CliError` 显式 `StableErrorCode` 映射；JSON 输出替代 `command_usage` 拒绝；human 渲染逻辑提取到 `render_log_output()`；补齐 `--help` EXAMPLES |
| `src/internal/log/formatter.rs` | **无改动** | `CommitFormatter` 仍由 human 渲染路径使用 |
| `src/internal/log/date_parser.rs` | **无改动** | 日期解析逻辑不变 |
| `tests/command/log_test.rs` | **扩展** | 新增 `LogError` 变体覆盖、`run_log()` 结构化结果验证 |
| `tests/command/log_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
