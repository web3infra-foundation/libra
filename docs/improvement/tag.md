## Tag 命令改进详细计划

> 最后编写时间：2026-03-30

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`tag` 是第二批（状态变更确认命令）中管理版本标记的命令。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, _output)` 双入口已存在（`tag.rs:42/51`）
- `-l` / `-d` / `-m` / `-f` / `-n` 短标志已实现
- `create_tag_safe()` 已有 `.with_hint()` 提供重复创建时的 hint（`tag.rs:87-96`）
- `render_tags()` 支持 `-n` 控制注释行数显示
- `internal::tag` 模块提供底层 tag API
- 内部 tag API 已返回 `LBR-CONFLICT-002` 错误码（从 `tag_test.rs:157` 验证）

**基于当前代码的 Review 结论（tag 仍需改进的部分）：**

- **零 JSON / machine 输出**：`OutputConfig` 参数标记为 `_output` 完全未使用（`tag.rs:51`）
- **零 `StableErrorCode` 在命令层**：虽然内部 tag API 返回 `LBR-CONFLICT-002`，但命令层的 `CliError::fatal()` 无显式错误码
- **无 `TagError` typed enum**：错误散落在 `execute_safe()`、`create_tag_safe()`、`delete_tag_safe()`、`show_tag_safe()` 中
- **退出码不对齐**：重复创建时退出码应为明确的非零值（当前通过 `CliError::fatal()` 返回 128，但无 stable code）
- **删除不存在 tag 时退出码不对齐**：应返回 exit `1` 或 `129`
- **测试注释有全角括号**：`tag_test.rs` 中有 `（lightweight tag）` 等全角括号应改为半角

### 目标与非目标

**本批目标：**
- 引入 `TagError` typed error enum，覆盖 tag 层面的错误场景
- 所有 `TagError → CliError` 映射使用显式 `StableErrorCode`
- 拆分执行层与渲染层：新增 `run_tag(args) -> Result<TagOutput, TagError>` 纯执行入口
- 实现 JSON 输出（tag 操作结果 + tag 列表结构化）
- 重复创建时保留 hint（已有）并补齐 `StableErrorCode`
- 删除不存在 tag 时返回 exit `129` + hint
- 修复测试注释中的全角括号
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不重写 `internal::tag` 底层业务语义**。允许做类型收紧和错误建模调整（例如 `create()` 返回 `CreateTagError`），但不改变 tag 创建/删除/查询的语义行为
- **不引入 tag 签名（GPG 签名 tag）**。这是独立特性
- **不引入 `--sort` 选项**。tag 排序留后续
- **不引入 `--verify` 选项**

### 设计原则

1. **执行路径与渲染职责拆分**：`execute_safe()` 根据 `OutputConfig` 分流 human / JSON 路径，JSON 路径返回结构化 `TagOutput`
2. **JSON 覆盖 list、create、delete 三种操作**：通过 `action` 字段区分
3. **错误码显式映射**：每个 `TagError` 变体都有确定的 `StableErrorCode`
4. **保留现有 hint**：重复创建时的 hint 保持一致

### 特性 1：TagError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("tag '{0}' already exists")]
    AlreadyExists(String),

    #[error("tag '{0}' not found")]
    NotFound(String),

    #[error("tag name is required")]
    MissingName,

    #[error("failed to create tag '{name}': {detail}")]
    CreateFailed { name: String, detail: String },

    #[error("failed to delete tag '{name}': {detail}")]
    DeleteFailed { name: String, detail: String },

    #[error("failed to load tag '{name}': {detail}")]
    LoadFailed { name: String, detail: String },

    #[error("failed to list tags: {0}")]
    ListFailed(String),
}
```

**`TagError → CliError` 显式映射：**

| TagError 变体 | StableErrorCode | 退出码 | hint |
|--------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `AlreadyExists` | `ConflictOperationBlocked` | 128 | `delete it first with 'libra tag -d {name}'` + `or choose a different tag name` |
| `NotFound` | `CliInvalidTarget` | 129 | `use 'libra tag -l' to list available tags` |
| `MissingName` | `CliInvalidArguments` | 129 | `provide a tag name` |
| `CreateFailed` | `IoWriteFailed` | 128 | 无 |
| `DeleteFailed` | `IoWriteFailed` | 128 | 无 |
| `LoadFailed` | `RepoCorrupt` | 128 | 无 |
| `ListFailed` | `IoReadFailed` | 128 | 无 |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum TagOutput {
    #[serde(rename = "list")]
    List(TagListOutput),
    #[serde(rename = "create")]
    Create(TagCreateOutput),
    #[serde(rename = "delete")]
    Delete(TagDeleteOutput),
}

#[derive(Debug, Clone, Serialize)]
pub struct TagListOutput {
    pub tags: Vec<TagListEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagListEntry {
    pub name: String,
    pub hash: String,
    /// "lightweight" or "annotated"
    pub tag_type: String,
    /// Annotation message (first N lines, None for lightweight)
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagCreateOutput {
    pub name: String,
    pub hash: String,
    pub tag_type: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagDeleteOutput {
    pub name: String,
    pub hash: String,
}
```

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human list | tag 名称列表（可选 `-n` 注释行数） | 无 |
| human create | 保留现有创建路径输出（lightweight create 后展示 tag/commit 信息；annotated create 无额外确认消息） | 无 |
| human delete | 确认消息（如 `Deleted tag 'v1.0' (was abc1234)`） | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope（含 `action` 字段区分操作类型） | 无 |

### 特性 3：JSON 输出设计

**list `--json`：**

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "list",
    "tags": [
      { "name": "v1.0", "hash": "abc123...", "tag_type": "lightweight", "message": null },
      { "name": "v1.1", "hash": "def456...", "tag_type": "annotated", "message": "Release v1.1" }
    ]
  }
}
```

**create `--json`：**

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.0",
    "hash": "abc123...",
    "tag_type": "lightweight",
    "message": null
  }
}
```

**delete `--json`：**

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "delete",
    "name": "v1.0",
    "hash": "abc123..."
  }
}
```

**错误 JSON（重复创建）：**

```json
{
  "ok": false,
  "error_code": "LBR-CONFLICT-002",
  "category": "conflict",
  "exit_code": 128,
  "message": "tag 'v1.0' already exists",
  "hints": [
    "delete it first with 'libra tag -d v1.0'",
    "or choose a different tag name"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 tag 中的具体落地

| ID | 改进 | tag 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（缺少 tag 名、不存在的 tag 名）→ exit `129`；运行时错误（重复创建、I/O 失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——tag 名是用户自定义值，无 enum 可做 fuzzy match |
| **G** | Issues URL | 仅在 `LoadFailed` / `ListFailed` 错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra tag v1.0                         Create a lightweight tag
    libra tag -m "Release v1.0" v1.0       Create an annotated tag
    libra tag -l                           List all tags
    libra tag -l -n 1                      List tags with one line of annotation
    libra tag -d v1.0                      Delete a tag
    libra tag -f v1.0                      Force update a tag
    libra tag --json -l                    List tags as JSON
    libra tag --json v1.0                  Create a lightweight tag and emit JSON
```

### 测试要求

#### `tests/command/tag_test.rs`（核心执行路径扩展）

- **（已有）** 重复 tag 错误码、basic creation、annotated tag、force tag、list、delete、annotation lines
- **（新增）`TagError` 变体覆盖**：
  - `NotFound`：删除不存在 tag 返回 exit `129`
  - `MissingName`：无 tag 名返回 exit `129`
- **（新增）quiet / delete 输出约束**：`--quiet tag -d` 不应污染 stdout；human delete 保持确认消息
- **（新增）force 失败路径回归**：`-f` 遇到对象存储失败时必须保留原有 ref，不得丢 tag
- **（修复）全角括号**：将 `（lightweight tag）` 等改为 `(lightweight tag)`

#### `tests/command/tag_test.rs`（JSON schema 稳定性扩展）

- **list `--json`**：`action == "list"`，`tags` 数组包含 `name`/`hash`/`tag_type`/`message`
- **create `--json`**：`action == "create"`，`name` 和 `hash` 存在
- **annotated create `--json`**：`tag_type == "annotated"`，`message` 非 null
- **delete `--json`**：`action == "delete"`，`name` 和 `hash` 存在
- **重复创建 `--json`**：`ok == false`，`error_code == "LBR-CONFLICT-002"`
- **`--machine tag -l`**：stdout 恰好 1 行非空行

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/tag.rs` | **重构** | 新增 `TagError` typed enum；新增 `TagOutput` / `TagListOutput` / `TagCreateOutput` / `TagDeleteOutput` 结构体；命令层 `TagError → CliError` 显式 `StableErrorCode` 映射；JSON 输出；quiet/delete 输出约束；补齐 `--help` EXAMPLES |
| `tests/command/tag_test.rs` | **扩展** | 新增 `TagError` 变体覆盖、JSON schema 回归、force 失败路径保护、修复全角括号 |
