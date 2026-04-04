## Blame 命令改进详细计划

> 最后编写时间：2026-04-04

同时落地 [Cross-Cutting Improvements A/B/F/G](README.md#全局层面改进贯穿所有命令)。

### 已完成前置条件与当前代码状态

第一批全部 8 个命令的主改造已在当前代码库落地。`blame` 是第三批（历史查询命令）中用于追溯代码行归属的命令，当前工作区已经按本计划完成主改造；本文保留为对外契约和后续维护基线。

**已确认落地的基线：**

- `config_kv` 后端已落地
- `OutputConfig` + `emit_json_data()` + `info_println!()` 输出框架已可用
- `StableErrorCode` 体系已有 18 个错误码
- `CliError` 支持 `.with_hint()`、`.with_stable_code()`、`.with_detail()`
- `execute()` / `execute_safe(args, out_config)` 双入口已存在（`blame.rs:45/54`）
- `Pager::with_config(out_config)` 集成已实现（`blame.rs:197`）
- `--quiet` 已实现（`blame.rs:74, 196`）
- `-L, --line-range` 已实现，支持 "10"、"10,20"、"10,+5" 格式
- 基于内容相等性的 blame 算法已实现（BFS 历史回溯，`blame.rs:94-146`）
- SHA-1 和 SHA-256 双格式支持已测试
- `run_blame()` + `BlameOutput` 已落地，`--json` / `--machine` 可返回逐行结构化结果
- `BlameError` typed enum 已落地，主要错误路径已显式映射到 `StableErrorCode`
- JSON `date` 已统一为 RFC3339；human / JSON 都复用同一份逐行归属数据
- `--help` EXAMPLES 已落地；`tests/command/blame_test.rs` 已补 blame 归属、`-L` 过滤和错误路径回归

**基于当前代码的 Review 结论（已改进部分 vs 后续维护重点）：**

已改进（当前代码已具备）：

- `run_blame()` / `BlameOutput` 已把 blame 算法和 human / JSON 渲染拆开
- `BlameError` 已统一 revision、object、missing file、line range 等错误，并接入稳定错误码
- `--json` / `--machine` 已可返回逐行归属结果，`-L` 会在执行层生效
- SHA-1 / SHA-256、长作者名、逐行归属、范围过滤和错误路径都已有回归测试

后续维护重点：

- 继续观察 blame 算法在更长提交链和重复内容场景下的归属正确性
- 如后续要对齐 Git porcelain blame，再在本计划之外新增独立 schema，而不是修改当前 JSON 契约

### 目标与非目标

**已完成目标：**
- `BlameError`、`run_blame()` / `BlameOutput`、JSON / machine、`-L` 结构化输出、回归测试和 `--help` EXAMPLES 已全部落地

**后续维护目标：**
- 继续维护 blame 归属正确性、范围过滤和格式化稳定性回归

**本批非目标：**
- **不改变 blame 归属算法**。基于内容相等性的 BFS 回溯逻辑不变
- **不引入 `--porcelain` 格式**。Git 的 porcelain blame 格式留后续需要时实现
- **不引入 `--show-email` / `--show-name` 选项**。保持现有输出字段
- **不引入增量 blame 或 blame 缓存**。性能优化留后续
- **不支持 blame 范围的函数名语法**（如 `git blame -L :funcName`）

### 设计原则

1. **执行层与渲染层拆分**：`execute_safe()` 调用 `run_blame()` 收集结构化结果，再根据 `OutputConfig` 渲染
2. **JSON 按行返回归属信息**：每行包含 commit hash、author、date、line number、content
3. **错误码显式映射**：每个 `BlameError` 变体都有确定的 `StableErrorCode`
4. **JSON 模式下 `-L` 仍然生效**：行范围过滤在执行层完成

### 特性 1：BlameError typed error enum

**方案：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum BlameError {
    #[error("not a libra repository")]
    NotInRepo,

    #[error("failed to resolve revision '{revision}': {detail}")]
    InvalidRevision { revision: String, detail: String },

    #[error("failed to load commit '{commit_id}': {detail}")]
    CommitLoad { commit_id: String, detail: String },

    #[error("file '{path}' not found in revision '{revision}'")]
    FileNotFound { path: String, revision: String },

    #[error("invalid line range: {detail}")]
    InvalidLineRange { detail: String },

    #[error("file '{path}' is empty")]
    EmptyFile { path: String },
}
```

**`BlameError → CliError` 显式映射：**

| BlameError 变体 | StableErrorCode | 退出码 | hint |
|----------------|-----------------|--------|------|
| `NotInRepo` | `RepoNotFound` | 128 | `run 'libra init' to create a repository` |
| `InvalidRevision` | `CliInvalidTarget` | 129 | `check the revision name and try again` |
| `CommitLoad` | `RepoCorrupt` | 128 | `the object store may be corrupted` |
| `FileNotFound` | `CliInvalidTarget` | 129 | `check the file path; use 'libra show <rev>:' to list available files` |
| `InvalidLineRange` | `CliInvalidArguments` | 129 | `supported formats: "10", "10,20", "10,+5"` |
| `EmptyFile` | `RepoStateInvalid` | 128 | 无 |

### 特性 2：执行层与渲染层拆分

**方案：**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct BlameLine {
    /// Line number (1-indexed)
    pub line_number: usize,
    /// Abbreviated commit hash (8 chars)
    pub short_hash: String,
    /// Full commit hash
    pub hash: String,
    /// Author name
    pub author: String,
    /// Author date (ISO 8601)
    pub date: String,
    /// Line content
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlameOutput {
    /// File path being blamed
    pub file: String,
    /// Revision used (commit hash or "HEAD")
    pub revision: String,
    /// Blame lines (filtered by -L if specified)
    pub lines: Vec<BlameLine>,
}
```

改造后的调用链：
- `execute_safe(args, out_config)` → `run_blame(args)` → 返回 `BlameOutput`
- `run_blame()` 执行 blame 算法 + 行范围过滤 + 收集结构化结果
- `execute_safe()` 根据 `OutputConfig` 选择渲染：human / JSON / machine
- human 模式使用现有格式（8-char hash, 15-char author, date, line number, content）
- JSON/machine 模式使用 `emit_json_data("blame", &output, config)`

**渲染规则：**

| 模式 | stdout | stderr |
|------|--------|--------|
| human（默认） | 现有 blame 格式（hash + author + date + lineno + content） | 无 |
| human + `--quiet` | 无 | 无 |
| `--json` / `--machine` | JSON envelope | 无 |

### 特性 3：JSON 输出设计

**成功输出：**

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "src/main.rs",
    "revision": "abc1234567890abcdef1234567890abcdef123456",
    "lines": [
      {
        "line_number": 1,
        "short_hash": "abc12345",
        "hash": "abc1234567890abcdef1234567890abcdef123456",
        "author": "Alice",
        "date": "2026-03-30T10:00:00+08:00",
        "content": "fn main() {"
      },
      {
        "line_number": 2,
        "short_hash": "def56789",
        "hash": "def5678901234abcdef5678901234abcdef567890",
        "author": "Bob",
        "date": "2026-03-29T14:00:00+08:00",
        "content": "    println!(\"Hello\");"
      }
    ]
  }
}
```

**`-L 10,20 --json`（行范围过滤）：**

```json
{
  "ok": true,
  "command": "blame",
  "data": {
    "file": "src/main.rs",
    "revision": "abc1234...",
    "lines": [
      { "line_number": 10, "..." : "..." },
      { "line_number": 11, "..." : "..." }
    ]
  }
}
```

**错误 JSON：file not found**

```json
{
  "ok": false,
  "error_code": "LBR-CLI-003",
  "category": "cli",
  "exit_code": 129,
  "message": "file 'nonexistent.rs' not found in revision 'HEAD'",
  "hints": [
    "check the file path; use 'libra show <rev>:' to list available files"
  ]
}
```

### 特性 4：Cross-Cutting Improvements 在 blame 中的具体落地

| ID | 改进 | blame 中的具体落地 |
|----|------|-----------------|
| **A** | 退出码 `0/128/129` | 参数错误（无效 revision、file not found、无效行范围）→ exit `129`；运行时错误（object 损坏）→ exit `128`；成功 → exit `0` |
| **B** | `--help` EXAMPLES | 见下方 EXAMPLES 段 |
| **F** | 拼写纠错 | **不适用**——blame 的参数是文件路径和 revision，无 enum 值可做 fuzzy match |
| **G** | Issues URL | 仅在 `CommitLoad` 错误时输出 Issues URL |

### `--help` EXAMPLES 段

```text
EXAMPLES:
    libra blame src/main.rs                Blame a file at HEAD
    libra blame src/main.rs abc1234        Blame at a specific commit
    libra blame -L 10,20 src/main.rs       Blame lines 10-20
    libra blame -L 10,+5 src/main.rs       Blame 5 lines starting at line 10
    libra blame --json src/main.rs         Structured JSON output for agents
```

### 测试要求

#### `tests/command/blame_test.rs`（核心执行路径，重大扩展）

- **（已有）** 仓库外执行、SHA-1/SHA-256 兼容性、SHA 格式交叉拒绝
- **（新增）blame 归属正确性**：
  - 创建 2 个 commit，第二个修改文件部分行；验证修改行归属到第二个 commit，未修改行归属到第一个 commit
  - 3 个 commit 链：验证每行归属到引入该行的 commit
- **（新增）行范围过滤**：
  - `-L 1` 只返回第 1 行
  - `-L 2,4` 返回第 2-4 行
  - `-L 3,+2` 返回第 3-4 行
  - `-L` 超出范围返回错误
- **（新增）`BlameError` 变体覆盖**：
  - `FileNotFound`：不存在的文件返回 exit `129`
  - `InvalidRevision`：无效 revision 返回 exit `129`
  - `InvalidLineRange`：无效行范围格式返回 exit `129`
- **（新增）empty file**：空文件返回空结果或 "File is empty" 信息

#### `tests/command/blame_json_test.rs`（JSON schema 稳定性，新增文件）

- **schema 完整性**：验证 `--json` 输出中每个字段的类型和存在性
- **blame 归属 `--json`**：验证 `lines` 数组中 hash/author/date/content 正确
- **`-L --json`**：行范围过滤后 `lines` 数组仅包含指定范围
- **file not found `--json`**：`ok == false`，`error_code == "LBR-CLI-003"`
- **`--machine blame`**：stdout 恰好 1 行非空行
- **specific commit `--json`**：`revision` 字段为指定 commit hash

### 质量验收

每次提交前必须通过 [README.md 统一质量验收](README.md#每次改进质量验收)：

1. `cargo +nightly fmt --all --check` 无格式差异
2. `cargo clippy --all-targets --all-features -- -D warnings` 无警告
3. `source .env.test && cargo test --all` 全部通过
4. 凡纳入改进范围的执行路径，都必须有对应的集成测试覆盖

### 涉及文件

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src/command/blame.rs` | **重构** | 新增 `BlameError` typed enum；新增 `BlameOutput` / `BlameLine` 结构体；新增 `run_blame()` 纯执行入口；`BlameError → CliError` 显式 `StableErrorCode` 映射；JSON 输出替代 `command_usage` 拒绝；`LineBlame` 升级为 `BlameLine`（pub + Serialize）；补齐 `--help` EXAMPLES |
| `tests/command/blame_test.rs` | **重大扩展** | 新增归属正确性、行范围过滤、错误变体覆盖 |
| `tests/command/blame_json_test.rs` | **新增** | JSON schema 完整性和稳定性验证 |
| `tests/command/mod.rs` | **修改** | 注册新增的测试文件 |
