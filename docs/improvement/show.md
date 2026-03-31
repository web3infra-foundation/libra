## Show 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`show` 是第三批（历史查询命令）中展示任意 Git 对象的通用命令。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, _output)` 双入口已存在（`show.rs:57/66`）
- `--no-patch` / `--oneline` / `--name-only` / `--stat` / pathspec 已实现
- show 支持 commit、tag（annotated + lightweight）、tree、blob 四种对象类型
- 复用 `log.rs` 的 `generate_diff()` 和 `get_changed_files_for_commit()` 函数
- `show_bad_revision_error()` 已有 `.with_hint()` 但**缺少** `.with_stable_code()`（`show.rs:333`）

**基于当前代码的 Review 结论（show 仍需改进的部分）：**

- **零 JSON / machine 输出**：`OutputConfig` 参数标记为 `_output` 完全未使用（`show.rs:66`）
- **零 `StableErrorCode`**：所有错误使用 `CliError::fatal()` 无显式错误码；`show_bad_revision_error()` 有 hint 但无 stable code
- **无 `ShowError` typed enum**：错误散落在多个 `show_*` 函数内部
- **测试期望 `LBR-CLI-003` 但代码未赋值**：`show_test.rs:140` 和 `show_test.rs:352` 期望 stable code `LBR-CLI-003`（`CliInvalidTarget`），但 `show_bad_revision_error()` 未调用 `.with_stable_code()`

### 目标与非目标

**本批目标：**
- 引入 `ShowError` typed error enum，覆盖所有对象类型的展示错误
- 所有 `ShowError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_show(args) -> Result<ShowOutput, ShowError>` 纯执行入口
- 实现 JSON 输出，根据对象类型返回不同结构（commit、tag、tree、blob）
- 修复 `show_bad_revision_error()` 缺失的 `.with_stable_code(StableErrorCode::CliInvalidTarget)`
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不引入 `--pretty` 格式支持**。log 的 `--pretty` 自定义模板在 show 中不适用（show 面向多种对象类型）
- **不引入 `--decorate` 支持**。show 本身已在 commit 输出中包含 refs 信息
- **不改变 diff 生成逻辑**。show 复用 `log.rs` 的 `generate_diff()`

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_show()` 收集结构化结果，再根据 `OutputConfig` 渲染
2. **JSON 根据对象类型返回不同 schema**：使用 `type` 字段区分 commit/tag/tree/blob
3. **错误码显式映射**：每个 `ShowError` 变体都有确定的 `StableErrorCode`
4. **JSON 模式下 `--oneline` / `--no-patch` / `--stat` 是 no-op**：JSON 始终包含完整信息

### 特性 1：ShowError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ShowError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("bad revision '{revision}'")]
    BadRevision { revision: String },

    #[error("path '{path}' does not exist in '{revision}'")]
    PathNotFound { path: String, revision: String },

    #[error("failed to load object '{object_id}': {detail}")]
    ObjectLoad { object_id: String, detail: String },

    #[error("unsupported object type for display: {object_type}")]
    UnsupportedObjectType { object_type: String },
}
```

**`ShowError → CliError` 显式映射：**

| ShowError 变体 | StableErrorCode | 退出码 | hint |
|---------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `BadRevision` | `CliInvalidTarget` | 129 | `use 'libra log --oneline' to see available commits` + `use 'libra tag -l' to see available tags` |
| `PathNotFound` | `CliInvalidTarget` | 129 | `check the path and revision; use 'libra show <rev>:' to list the tree` |
| `ObjectLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `UnsupportedObjectType` | `CliInvalidTarget` | 129 | 无 |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ShowOutput {
    #[serde(rename = "commit")]
    Commit(ShowCommitData),
    #[serde(rename = "tag")]
    Tag(ShowTagData),
    #[serde(rename = "tree")]
    Tree(ShowTreeData),
    #[serde(rename = "blob")]
    Blob(ShowBlobData),
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowCommitData {
    pub hash: String,
    pub short_hash: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub files: Vec<ShowFileChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowFileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTagData {
    pub tag_name: String,
    pub tagger_name: Option<String>,
    pub tagger_email: Option<String>,
    pub tagger_date: Option<String>,
    pub message: String,
    pub target_hash: String,
    pub target_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTreeData {
    pub entries: Vec<ShowTreeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowTreeEntry {
    pub mode: String,
    pub object_type: String,
    pub hash: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowBlobData {
    pub hash: String,
    pub size: usize,
    pub is_binary: bool,
    /// UTF-8 content for text blobs; null for binary blobs
    pub content: Option<String>,
}
```

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 现有格式（commit/tag/tree/blob 各自的 human 渲染） | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope（含 `type` 字段区分对象类型） | 无 |

### 特性 3：JSON 输出设计

**commit `--json`：**

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "commit",
    "hash": "abc1234...",
    "short_hash": "abc1234",
    "author_name": "Alice",
    "author_email": "alice@example.com",
    "author_date": "2026-03-30T10:00:00+08:00",
    "committer_name": "Alice",
    "committer_email": "alice@example.com",
    "committer_date": "2026-03-30T10:00:00+08:00",
    "subject": "feat: add feature",
    "body": "",
    "parents": ["def5678..."],
    "refs": ["HEAD -> main"],
    "files": [
      { "path": "src/main.rs", "status": "modified" }
    ]
  }
}
```

**annotated tag `--json`：**

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "tag",
    "tag_name": "v1.0",
    "tagger_name": "Alice",
    "tagger_email": "alice@example.com",
    "tagger_date": "2026-03-30T10:00:00+08:00",
    "message": "Release v1.0",
    "target_hash": "abc1234...",
    "target_type": "commit"
  }
}
```

**tree `--json`：**

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "tree",
    "entries": [
      { "mode": "100644", "object_type": "blob", "hash": "abc123...", "name": "README.md" },
      { "mode": "040000", "object_type": "tree", "hash": "def456...", "name": "src" }
    ]
  }
}
```

**blob `--json`：**

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "blob",
    "hash": "abc123...",
    "size": 1024,
    "is_binary": false,
    "content": "file content here..."
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
  "message": "bad revision 'nonexistent'",
  "hints": [
    "use 'libra log --oneline' to see available commits",
    "use 'libra tag -l' to see available tags"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 show 中的具体落地

| ID | 改进 | show 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（bad revision、path not found）→ exit `129`；运行时错误（object 损坏）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——show 的参数是 revision/path，无 enum 值可做 fuzzy match |
| **G** | Issues URL | 仅在 `ObjectLoad` 错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra show                             Show HEAD commit
    libra show abc1234                     Show a specific commit
    libra show v1.0                        Show a tag
    libra show HEAD:src/main.rs            Show a file at HEAD
    libra show HEAD:src/                   Show a directory tree at HEAD
    libra show -s                          Show commit metadata only (no diff)
    libra show --stat                      Show commit with diffstat
    libra show --json                      Structured JSON output for agents
    libra show --json v1.0                 Show tag as JSON
```

### 测试要求

#### `tests/command/show_test.rs`（核心执行路径扩展）

- **（已有）** badref 错误、lightweight tag、annotated tag、multiple tags、nonexistent tag、execute_safe bad ref
- **（新增）`ShowError` 变体覆盖**：
  - `PathNotFound`：`libra show HEAD:nonexistent.rs` 返回 exit `129`
  - `UnsupportedObjectType`：内部不支持的对象类型返回错误
- **（新增）StableErrorCode 验证**：`show_bad_revision_error()` 现在返回 `LBR-CLI-003`（修复已有测试期望）
- **（新增）`run_show()` 结构化结果**：验证 commit/tag/tree/blob 各自的 `ShowOutput` 变体

#### `tests/command/show_json_test.rs`（JSON schema 稳定性，新增文件）

- **commit `--json`**：验证所有 commit 字段类型和存在性
- **annotated tag `--json`**：`type == "tag"`，tagger 字段存在
- **lightweight tag `--json`**：解析到 commit，`type == "commit"`
- **tree `--json`**：`type == "tree"`，`entries` 数组正确
- **blob `--json`**：`type == "blob"`，`content` 为字符串或 null
- **binary blob `--json`**：`is_binary == true`，`content == null`
- **bad revision `--json`**：`ok == false`，`error_code == "LBR-CLI-003"`
- **`--machine show`**：stdout 恰好 1 行非空行

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/show.rs` | **重构** | 新增 `ShowError` typed enum；新增 `ShowOutput` / `ShowCommitData` / `ShowTagData` / `ShowTreeData` / `ShowBlobData` 结构体；新增 `run_show()` 纯执行入口；`ShowError → CliError` 显式 `StableErrorCode` 映射（修复 `show_bad_revision_error()` 缺失的 stable code）；JSON 输出；补齐 `--help` EXAMPLES |
| `tests/command/show_test.rs` | **扩展** | 新增 `ShowError` 变体覆盖、StableErrorCode 验证 |
| `tests/command/show_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
