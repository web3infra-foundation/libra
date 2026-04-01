## Tag 命令改进详细计划

> 最后编写时间：2026-04-01

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

> 当前工作区实现已按本文范围落地一部分改动；以下内容改为记录已落地能力、剩余遗漏和后续收口项。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`tag` 是第二批（状态变更确认命令）中管理版本标记的命令。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, output)` 双入口已存在
- `run_tag_json()` + `TagOutput` 已实现 list / create / delete 的 JSON / machine 输出
- `-l` / `-d` / `-m` / `-f` / `-n` 短标志已实现
- `create_tag_safe()` 已有 `.with_hint()` 提供重复创建时的 hint（`tag.rs:87-96`）
- `render_tags()` 支持 `-n` 控制注释行数显示
- `internal::tag` 模块提供底层 tag API
- create / delete / find major error path 已在命令层映射显式 `StableErrorCode`
- quiet delete、malformed ref delete 和 JSON schema 已有回归测试覆盖

**基于当前代码的 Review 结论（已改进部分 vs 仍需改进部分）：**

已改进（当前代码已具备）：

- **结构化输出已落地**：`run_tag_json()` + `TagOutput` 已覆盖 list / create / delete 三类操作，`--json` / `--machine` 可直接使用
- **主要命令层错误已带显式 `StableErrorCode`**：重复创建、HEAD unborn、tag not found、delete I/O 失败、repo read failure 等路径已有稳定错误码
- **重复创建 hint 已落地**：`map_create_tag_error()` 已保留删除旧 tag 或更换 tag 名的提示
- **quiet / malformed ref delete 回归已覆盖**：当前测试已覆盖 quiet delete、删除损坏 tag ref、JSON delete `hash = null` 等边界

仍需改进：

- **无 `TagError` typed enum**：错误仍散落在 `execute_safe()`、`create_tag_safe()`、`delete_tag_safe()`、`show_tag_safe()` 中
- **无统一 `run_tag()` / `render_tag_output()` 分层**：human 路径仍在 `execute_safe()` 内分支拼装，JSON 路径单独走 `run_tag_json()`
- **list / show 路径仍有隐式错误码**：`render_tags()` 失败在 human 路径仍通过 `CliError::fatal(e.to_string())` 返回，缺少显式 `StableErrorCode`
- **human 成功反馈仍不完全一致**：lightweight create 复用 `show_tag_safe()` 输出对象详情，annotated create 没有单独确认消息，delete 也未回显被删 tag 的 hash
- **create 失败来源尚未结构化区分**：`CheckExisting` / `SerializeTag` / `StoreObject` / `PersistReference` 需要映射到不同稳定错误码，不能继续折叠成一个泛化的 create 失败
- **缺少 `--help` EXAMPLES 段**
- **测试注释仍有全角括号**：`tag_test.rs` 中 `（lightweight tag）` 等注释尚未清理

### 目标与非目标

**本批目标：**
- 引入 `TagError` typed error enum，覆盖 tag 层面的错误场景
- 所有 `TagError → CliError` 映射使用显式 `StableErrorCode`
- 在保留既有 `TagOutput` JSON schema 的前提下，补齐统一的 `run_tag()` / `render_tag_output()` 分层
- 补齐 list / show 路径的显式 `StableErrorCode`
- 统一 human create / delete 成功反馈为简短确认消息，不再沿用当前 lightweight/annotated 两套不同输出习惯
- 让 create 失败来源在 `TagError` 中显式编码，避免同一变体对应多个 `StableErrorCode`
- 修复测试注释中的全角括号
- 补齐 `--help` EXAMPLES 段

**本批非目标：**
- **不重写 `internal::tag` 底层业务语义**。允许做类型收紧和错误建模调整（例如 `create()` 返回 `CreateTagError`），但不改变 tag 创建/删除/查询的语义行为
- **不引入 tag 签名（GPG 签名 tag）**。这是独立特性
- **不引入 `--sort` 选项**。tag 排序留后续
- **不引入 `--verify` 选项**

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_tag()` 收集结构化 `TagOutput` 结果，再根据 `OutputConfig` 通过 `render_tag_output()` 渲染 human / JSON / machine，消除当前 human 路径与 JSON 路径分治的架构
2. **JSON 覆盖 list、create、delete 三种操作**：通过 `action` 字段区分
3. **错误码显式映射**：每个 `TagError` 变体都有确定的 `StableErrorCode`
4. **保留现有 hint**：重复创建时的 hint 保持一致
5. **typed enum 自身携带错误分类信息**：不能依赖 `TagError` 变体外的来源注释再决定 `StableErrorCode`

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

    #[error("cannot create tag: HEAD does not point to a commit")]
    HeadUnborn,

    #[error("failed to read existing tags before creating '{name}': {detail}")]
    CheckExistingFailed { name: String, detail: String },

    #[error("failed to serialize annotated tag object: {0}")]
    SerializeAnnotatedTag(String),

    #[error("failed to store annotated tag object: {0}")]
    StoreObjectFailed(String),

    #[error("failed to persist tag reference '{name}': {detail}")]
    PersistReferenceFailed { name: String, detail: String },

    #[error("failed to delete tag '{name}': {detail}")]
    DeleteFailed { name: String, detail: String },

    #[error("failed to load tag '{name}': {detail}")]
    LoadFailed { name: String, detail: String },

    #[error("failed to list tags: {0}")]
    ListFailed(String),
}
```

> **与 `internal::tag::CreateTagError` 的关系**：`CreateTagError` 是底层业务模块定义的错误类型（含 `AlreadyExists`、`HeadUnborn`、`CheckExisting`、`SerializeTag`、`StoreObject`、`PersistReference`）。`TagError` 是命令层 typed enum，通过 `impl From<CreateTagError> for TagError` 收口映射（现有 `map_create_tag_error()` 将被替代）：`CheckExisting` → `CheckExistingFailed`，`SerializeTag` → `SerializeAnnotatedTag`，`StoreObject` → `StoreObjectFailed`，`PersistReference` → `PersistReferenceFailed`。

**`TagError → CliError` 显式映射：**

| TagError 变体 | StableErrorCode | 退出码 | hint |
|--------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `AlreadyExists` | `ConflictOperationBlocked` | 128 | `delete it first with 'libra tag -d {name}'` + `or choose a different tag name` |
| `NotFound` | `CliInvalidTarget` | 129 | `use 'libra tag -l' to list available tags` |
| `MissingName` | `CliInvalidArguments` | 129 | `provide a tag name` |
| `HeadUnborn` | `RepoStateInvalid` | 128 | `create a commit first before tagging HEAD` |
| `CheckExistingFailed` | `RepoCorrupt` | 128 | 无 |
| `SerializeAnnotatedTag` | `InternalInvariant` | 128 | 附带 Issues URL |
| `StoreObjectFailed` | `IoWriteFailed` | 128 | 无 |
| `PersistReferenceFailed` | `IoWriteFailed` | 128 | 无 |
| `DeleteFailed` | `IoWriteFailed` | 128 | 无 |
| `LoadFailed` | `RepoCorrupt` | 128 | 无 |
| `ListFailed` | `RepoCorrupt` | 128 | 无 |

**与当前代码中 inline 错误的对应关系：**

| 当前代码位置 | 当前 inline 错误 | 对应 TagError 变体 |
|-------------|-----------------|---------------------|
| `execute_safe:75` | `validate_named_tag_action()` | `MissingName`（delete/force 缺少 tag 名） |
| `execute_safe:89` | `CliError::fatal(e.to_string())` render_tags 失败 | `ListFailed` |
| `create_tag_safe:148-151` | `map_create_tag_error()` → `AlreadyExists` | `AlreadyExists` |
| `map_create_tag_error:162-165` | `CreateTagError::HeadUnborn` | `HeadUnborn` |
| `map_create_tag_error:167-171` | `CreateTagError::CheckExisting` | `CheckExistingFailed` |
| `map_create_tag_error:172-175` | `CreateTagError::SerializeTag` | `SerializeAnnotatedTag` |
| `map_create_tag_error:176-178` | `CreateTagError::StoreObject` | `StoreObjectFailed` |
| `map_create_tag_error:180-184` | `CreateTagError::PersistReference` | `PersistReferenceFailed` |
| `delete_tag_safe:242-246` | `tag::delete().map_err(...)` | `DeleteFailed` |
| `show_tag_safe:274-276` | `Ok(None)` tag not found | `NotFound` |
| `show_tag_safe:277-279` | `Err(e)` repo corrupt | `LoadFailed` |
| `run_tag_json:315-317` | `tag::list().map_err(...)` | `ListFailed` |
| `lookup_tag:332-334` | `Ok(None)` tag not found | `NotFound` |
| `lookup_tag:335-337` | `Err(e)` repo corrupt | `LoadFailed` |

### 特性 2：执行层与渲染层拆分

**已落地部分（保持不变）：** `TagOutput` enum（含 `List`/`Create`/`Delete` 三变体）、`TagListEntry` 结构体均已存在于 `tag.rs:41-63`，JSON schema 已稳定。

**本批变更：统一 `run_tag()` / `render_tag_output()` 分层**

当前架构问题：human 路径在 `execute_safe()` 内分支拼装（list 走 `render_tags()`，create 走 `create_tag_safe()` + `show_tag_safe()`，delete 走 `delete_tag_safe()`），JSON 路径单独走 `run_tag_json()`。两条路径各自拼装逻辑，违反执行/渲染分离原则。

目标架构：

```rust
/// 纯执行入口——收集结构化结果，不输出
async fn run_tag(args: &TagArgs) -> Result<TagOutput, TagError>

/// 渲染层——根据 OutputConfig 决定 human/JSON/machine/quiet 输出
fn render_tag_output(result: &TagOutput, output: &OutputConfig) -> CliResult<()>

/// execute_safe 调用链
pub async fn execute_safe(args: TagArgs, output: &OutputConfig) -> CliResult<()> {
    let result = run_tag(&args).await.map_err(CliError::from)?;
    render_tag_output(&result, output)
}
```

现有 `run_tag_json()` 将被合并入 `run_tag()`，`render_tags()` 的渲染逻辑将移入 `render_tag_output()` 的 human list 分支。

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human list | tag 名称列表（可选 `-n` 注释行数） | 无 |
| human create | 统一确认消息：`Created lightweight tag 'v1.0' at abc1234` 或 `Created annotated tag 'v1.0' at abc1234` | 无 |
| human delete | 确认消息：`Deleted tag 'v1.0' (was abc1234)`；target 丢失时退化为 `Deleted tag 'v1.0'` | 无 |
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

When deleting malformed refs that have no stored target, `hash` is `null`.

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
| **A** | 退出码 `0/128/129` | 参数错误（缺少 tag 名、不存在的 tag 名）→ exit `129`；运行时错误（重复创建、HEAD unborn、I/O 失败）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——tag 名是用户自定义值，无 enum 可做 fuzzy match |
| **G** | Issues URL | 与 switch 保持一致——仅在映射为 `InternalInvariant` 的内部不变式错误时输出。当前仅 `SerializeAnnotatedTag` 属于此类；`RepoCorrupt`/`IoWriteFailed` 是数据或 I/O 问题，不附带 Issues URL |

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
  - `NotFound`：删除不存在 tag 返回 exit `129` + `LBR-CLI-003`
  - `MissingName`：无 tag 名返回 exit `129` + `LBR-CLI-002`
  - `HeadUnborn`：空仓库创建 tag 返回 exit `128` + `LBR-REPO-003`
- **（新增）quiet / delete 输出约束**：`--quiet tag -d` 不应污染 stdout；human delete 保持确认消息
- **（新增）human create 输出统一**：lightweight / annotated create 均输出单行确认消息，不再依赖 `show_tag_safe()` 打印详情
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
| `src/command/tag.rs` | **收口** | 保持已落地的 `TagOutput` / `run_tag_json()` / JSON schema / create hint 不回退；后续补齐 `TagError` typed enum、统一 `run_tag()` / `render_tag_output()`、收口 list/show 路径的显式错误码、统一 human 确认消息、补齐 `--help` EXAMPLES |
| `tests/command/tag_test.rs` | **扩展** | 在现有 JSON / quiet / malformed ref delete 回归基础上，补齐 `TagError` 变体覆盖、human 成功反馈一致性校验和全角括号清理 |
